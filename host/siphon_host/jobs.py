"""Resizable download workers, cooperative cancellation, and bounded progress events."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import datetime, timezone
import logging
from pathlib import Path
import queue
import threading
import time
import tempfile

from .downloader import create_downloader
from .analysis import format_suffix, write_acid, write_tags
from .analysis_service import analyze, rename_with_suffix
from .errors import Cancelled, HostError, error_details
from .media import number, string, thumbnail
from .options import build_download_params, cookie_params
from .runtime import YTDLPLogger, default_output_dir
from .youtube import extract_youtube, is_youtube_url

TERMINAL = {"done", "error", "cancelled"}


def timestamp():
    return datetime.now(timezone.utc).isoformat()


@dataclass
class Job:
    job_id: str
    url: str
    options: dict
    emit: object
    source: str = ""
    clock: object = time.monotonic
    cancel: threading.Event = field(default_factory=threading.Event)
    lock: threading.RLock = field(default_factory=threading.RLock)
    last_emit: float = float("-inf")
    pending: threading.Timer | None = None
    partials: set = field(default_factory=set)
    completed_files: list = field(default_factory=list)
    processed_files: set = field(default_factory=set)
    # Set for clients that accept `analysis` events: the job finishes as soon as
    # its audio exists, and BPM/key arrive afterwards on this executor.
    analysis_pool: ThreadPoolExecutor | None = None

    def __post_init__(self):
        self.data = {"type": "job", "jobId": self.job_id, "url": self.url, "source": self.source,
                     "options": self.options.copy(), "createdAt": timestamp(), "updatedAt": timestamp(),
                     "state": "queued", "stage": "downloading", "progress": None,
                     "speed": None, "eta": None, "downloadedBytes": 0, "totalBytes": None,
                     "item": None, "file": None, "files": None, "error": None,
                     "title": "", "thumbnail": None, "analysis": None, "analyses": [], "analysisError": None}

    def snapshot(self):
        with self.lock:
            return self.data.copy()

    def update(self, **changes):
        with self.lock:
            if self.data["state"] in TERMINAL:
                return
            self.data.update(changes, updatedAt=timestamp())
            now = self.clock()
            terminal = self.data["state"] in TERMINAL
            if terminal or changes.get("state") == "analyzing" or now - self.last_emit >= .2:
                if self.pending:
                    self.pending.cancel()
                    self.pending = None
                self.last_emit = now
                self.emit(self.data.copy())
            elif self.pending is None:
                self.pending = threading.Timer(max(.001, .2 - (now - self.last_emit)), self._flush)
                self.pending.daemon = True
                self.pending.start()

    def _flush(self):
        with self.lock:
            self.pending = None
            if self.data["state"] in TERMINAL:
                return
            self.last_emit = self.clock()
            self.emit(self.data.copy())

    def check_cancel(self):
        if self.cancel.is_set():
            raise Cancelled()

    def describe(self, info):
        changes = {"title": string(info.get("title")), "thumbnail": thumbnail(info)}
        index = number(info.get("playlist_index"))
        if index is not None:
            changes["item"] = {"index": index, "count": number(info.get("playlist_count") or info.get("n_entries")), "title": changes["title"]}
        return changes

    def progress_hook(self, progress):
        info = progress.get("info_dict") or {}
        for key in ("tmpfilename", "filename"):
            if name := progress.get(key):
                with self.lock:
                    self.partials.add(str(name))
        # Record even the first hook's partial before observing cancellation.
        self.check_cancel()
        downloaded = number(progress.get("downloaded_bytes")) or 0
        total = number(progress.get("total_bytes") or progress.get("total_bytes_estimate"))
        finished = progress.get("status") == "finished"
        self.update(state="converting" if finished else "downloading", stage="converting" if finished else "downloading",
                    progress=1 if finished else progress.get("_siphon_progress", min(1, downloaded / total) if total else None),
                    speed=number(progress.get("speed")), eta=number(progress.get("eta")),
                    downloadedBytes=downloaded, totalBytes=total, **self.describe(info))

    def postprocessor_hook(self, progress):
        self.check_cancel()
        name = progress.get("postprocessor", "")
        tagging = any(word in name for word in ("Metadata", "EmbedThumbnail"))
        state = "tagging" if tagging else "converting"
        self.update(state=state, stage=state, speed=None, eta=None,
                    **self.describe(progress.get("info_dict") or {}))

    def match_filter(self, info, *, incomplete=False):
        self.check_cancel()
        if info.get("is_live") or info.get("live_status") in ("is_live", "is_upcoming"):
            raise HostError("LIVE_NOT_SUPPORTED", "Live and upcoming streams cannot be downloaded. Try again after the stream ends.")
        self.update(state="probing", **self.describe(info))
        return None

    def finished_file(self, path):
        self.check_cancel()
        path = str(Path(path).absolute())
        with self.lock:
            if path in self.processed_files:
                return
            self.processed_files.add(path)
            self.completed_files.append(path)
        if not self.options.get("analyze", True):
            return
        if self.analysis_pool is not None:
            with self.lock:
                self.data["analysisPending"] = self.data.get("analysisPending", 0) + 1
            self.analysis_pool.submit(self._deferred_analysis, path)
            return
        self.update(state="analyzing", stage="analyzing", file=path, speed=None, eta=None)
        entry = self._analyze_file(path)
        self.update(file=entry["file"], files=self.completed_files.copy(), analysis=entry["analysis"],
                    analyses=[*self.data["analyses"], entry],
                    analysisError=entry.get("analysisError") or self.data["analysisError"])

    def _analyze_file(self, path):
        """Estimate, tag and rename one file; returns its `analyses` entry."""
        result, failure = None, None
        try:
            result = analyze(path, bpm_range=tuple(self.options.get("bpmRange") or (70, 180)), check_cancel=self.check_cancel)
        except Cancelled:
            raise
        except Exception as exc:
            failure = error_details(exc)
            logging.getLogger("siphon").warning("Analysis failed for %s: %s", path, failure["message"])
        self.check_cancel()
        if result is not None:
            # Tagging and renaming fail independently; keep the successful
            # estimate and the completed audio even if a container rejects tags.
            if self.options.get("writeBpmKeyTags", True):
                try:
                    write_tags(path, result)
                    if path.lower().endswith(".wav"):
                        write_acid(path, result)
                except Exception as exc:
                    failure = error_details(exc)
            self.check_cancel()
            if self.options.get("appendBpmKey", True):
                try:
                    suffix = format_suffix(result, self.options.get("bpmKeySuffix", " [{bpm} BPM {key}]"), self.options.get("keyNotation", "standard"))
                    renamed = rename_with_suffix(path, suffix, overwrite=self.options.get("overwrite", False))
                    with self.lock:
                        self.completed_files[self.completed_files.index(path)] = renamed
                        if self.data["file"] == path:
                            self.data["file"] = renamed
                        if self.data["files"]:
                            self.data["files"] = [renamed if f == path else f for f in self.data["files"]]
                    path = renamed
                except Exception as exc:
                    failure = error_details(exc)
        self.check_cancel()
        entry = {"file": path, "analysis": result}
        if failure:
            entry["analysisError"] = failure
        return entry

    def _deferred_analysis(self, path):
        entry = None
        try:
            entry = self._analyze_file(path)
        except Cancelled:
            pass
        except Exception as exc:
            logging.getLogger("siphon").exception("Deferred analysis failed for %s", path)
            entry = {"file": path, "analysis": None, "analysisError": error_details(exc)}
        with self.lock:
            self.data["analysisPending"] = max(0, self.data.get("analysisPending", 1) - 1)
            if entry is None:
                return
            # Applied even after the job is terminal: this is the point of deferral.
            self.data.update(analyses=[*self.data["analyses"], entry], analysis=entry["analysis"] or self.data["analysis"],
                             analysisError=entry.get("analysisError") or self.data["analysisError"], updatedAt=timestamp())
            event = {"type": "analysis", "jobId": self.job_id, "source": path, **entry,
                     "files": self.completed_files.copy(), "pending": self.data["analysisPending"]}
        self.emit(event)

    def cleanup_parts(self):
        # Only touch exact filenames observed in this job's yt-dlp hooks.
        with self.lock:
            paths = self.partials.copy()
        for name in paths:
            path = Path(name)
            candidates = [path] if ".part" in path.name else [Path(name + ".part"), Path(name + ".ytdl")]
            # Fragment downloads use <filename>.part-Frag<N> alongside .part.
            candidates += list(path.parent.glob(path.name.replace("[", "[[]") + "-Frag*")) if path.parent.exists() and path.name.endswith(".part") else []
            for candidate in candidates:
                try:
                    candidate.unlink(missing_ok=True)
                except OSError:
                    logging.getLogger("siphon").debug("Could not remove partial %s", candidate)

    def dispose(self):
        with self.lock:
            if self.pending:
                self.pending.cancel()
                self.pending = None


class JobManager:
    def __init__(self, emit, logger, ffmpeg, node, factory=create_downloader, concurrency=3):
        self.emit, self.logger, self.ffmpeg, self.node = emit, logger, ffmpeg, node
        self.factory = factory
        self.jobs = {}
        self.queue = queue.Queue()
        self.condition = threading.Condition(threading.RLock())
        self.workers = []
        self.active = 0
        self.stopped = False
        self.defer_analysis = False
        # analysis_service bounds numerical work globally; this only queues it.
        self.analysis_pool = ThreadPoolExecutor(max_workers=2, thread_name_prefix="analysis")
        self.resize(concurrency)

    def resize(self, concurrency):
        if isinstance(concurrency, bool) or not isinstance(concurrency, int) or not 1 <= concurrency <= 16:
            raise HostError("BAD_REQUEST", "maxConcurrent must be an integer from 1 to 16.")
        with self.condition:
            self.limit = concurrency
            while len(self.workers) < concurrency:
                worker = threading.Thread(target=self._worker, name=f"download-{len(self.workers) + 1}", daemon=True)
                self.workers.append(worker)
                worker.start()
            self.condition.notify_all()

    def create(self, job_id, url, options, source=""):
        with self.condition:
            if job_id in self.jobs:
                raise HostError("BAD_REQUEST", "jobId already exists; use a new jobId to retry.")
            # Retain a bounded terminal tail for duplicate-ID protection, but
            # listJobs returns only active jobs. Never retain cookie text here.
            if len(self.jobs) >= 1000:
                for key, job in list(self.jobs.items()):
                    if job.snapshot()["state"] in TERMINAL:
                        del self.jobs[key]
                    if len(self.jobs) < 800:
                        break
            if len(self.jobs) >= 1000:
                raise HostError("BAD_REQUEST", "The download queue is full.")
            job = Job(job_id, url, options, self.emit, source=source,
                      analysis_pool=self.analysis_pool if self.defer_analysis else None)
            self.jobs[job_id] = job
            return job

    def enqueue(self, job, cookies=None):
        job.update()
        self.queue.put((job, cookies))
        with self.condition:
            self.condition.notify_all()

    def cancel_job(self, job_id):
        with self.condition:
            job = self.jobs.get(job_id)
            if not job or job.snapshot()["state"] in TERMINAL:
                return False
            job.cancel.set()
            if job.snapshot()["state"] == "queued":
                job.update(state="cancelled", error=None, speed=None, eta=None)
            self.condition.notify_all()
            return True

    def list_jobs(self):
        with self.condition:
            return [snapshot for job in self.jobs.values() if (snapshot := job.snapshot())["state"] not in TERMINAL]

    def _worker(self):
        while True:
            with self.condition:
                self.condition.wait_for(lambda: self.stopped or self.active < self.limit and not self.queue.empty())
                if self.stopped:
                    return
                job, cookies = self.queue.get_nowait()
                self.active += 1
            try:
                if job.snapshot()["state"] != "cancelled":
                    self._download(job, cookies)
            finally:
                cookies = None
                self.queue.task_done()
                with self.condition:
                    self.active -= 1
                    self.condition.notify_all()

    def _download(self, job, cookies):
        try:
            job.check_cancel()
            if not self.ffmpeg["found"]:
                raise HostError("FFMPEG_MISSING", "FFmpeg is required to extract and tag audio. Install FFmpeg and reconnect Siphon.")
            job.update(state="probing")
            Path(job.options["outputDir"] or default_output_dir()).mkdir(parents=True, exist_ok=True)
            params = build_download_params(job.options, YTDLPLogger(self.logger, self.log_event), self.ffmpeg, self.node)
            params.update(progress_hooks=[job.progress_hook], postprocessor_hooks=[job.postprocessor_hook],
                          post_hooks=[job.finished_file], match_filter=job.match_filter)
            # Each job owns this directory exclusively. yt-dlp moves finished
            # items to paths.home; all fragments, thumbnails and FFmpeg temps
            # remaining here are removed on success, error or cancellation.
            with tempfile.TemporaryDirectory(prefix=".siphon-", dir=params["paths"]["home"]) as staging, cookie_params({"cookies": cookies, "cookiesFromBrowser": job.options["cookiesFromBrowser"]} if cookies is not None else {"cookiesFromBrowser": job.options["cookiesFromBrowser"]}) as cookie_options:
                params["paths"]["temp"] = staging
                params.update(cookie_options)
                for attempt in range(2):
                    job.check_cancel()
                    try:
                        if is_youtube_url(job.url):
                            info = extract_youtube(job.url, params, self.factory, check_cancel=job.check_cancel,
                                                   can_retry=lambda: not job.completed_files)
                        else:
                            with self.factory(params) as ydl:
                                info = ydl.extract_info(job.url, download=True)
                        break
                    except Exception as exc:
                        # A freshly resolved signed media URL can already be
                        # rejected by a CDN. Retry extraction once, only before
                        # any audio bytes/items exist so progress cannot regress
                        # and playlist items cannot be duplicated.
                        if (is_youtube_url(job.url) or attempt or job.cancel.is_set() or job.completed_files
                                or job.snapshot()["downloadedBytes"]
                                or "http error 403" not in str(exc).lower()
                                or error_details(exc)["code"] != "NETWORK"):
                            raise
                        self.logger.warning("Download %s: media HTTP 403 before transfer; refreshing URL once", job.job_id)
            job.check_cancel()
            if not job.completed_files:
                # Some mocked/alternate yt-dlp implementations don't invoke post_hooks.
                self._collect_files(job, info)
            if not job.completed_files:
                raise HostError("INTERNAL", "The extractor completed without producing an audio file.")
            files = job.completed_files.copy()
            job.update(state="done", stage="done", progress=1, speed=None, eta=None,
                       file=files[-1], files=files, error=None)
        except Exception as exc:
            job.cleanup_parts()
            if job.cancel.is_set() or isinstance(exc, Cancelled):
                job.update(state="cancelled", error=None, speed=None, eta=None)
            else:
                self.logger.exception("Download %s failed", job.job_id)
                job.update(state="error", error=error_details(exc), speed=None, eta=None)

    def _collect_files(self, job, info):
        if not isinstance(info, dict):
            return
        if "entries" in info:
            for entry in info["entries"] or []:
                self._collect_files(job, entry)
        elif info.get("filepath"):
            job.finished_file(info["filepath"])

    def log_event(self, level, message):
        self.emit({"type": "log", "level": level, "message": message[:4000]})

    def shutdown(self):
        with self.condition:
            self.stopped = True
            for job in self.jobs.values():
                job.dispose()
            self.condition.notify_all()
        self.analysis_pool.shutdown(wait=False, cancel_futures=True)
