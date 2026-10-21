"""Request routing. Slow native operations never block the protocol reader."""

from importlib.metadata import version, PackageNotFoundError
from importlib.util import find_spec
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading

from . import __version__
from .analysis_service import warm_up, shutdown as shutdown_analysis
from .analyze_request import analyze_request
from .downloader import create_downloader
from .errors import HostError, error_details
from .framing import BadRequestFrame, FrameError, read_message
from .jobs import JobManager
from .media import media_info
from .options import base_params, cookie_params, validate_options, validate_url
from .runtime import YTDLPLogger, default_output_dir, discover_ffmpeg, discover_node, subprocess_options, tray_pipe_name


def package_version(name):
    try:
        return version(name)
    except PackageNotFoundError:
        return None


class Host:
    def __init__(self, writer, logger, *, factory=create_downloader, ffmpeg=None, node=None):
        self.writer, self.logger, self.factory = writer, logger, factory
        self.ffmpeg = ffmpeg if ffmpeg is not None else discover_ffmpeg()
        self.node = node if node is not None else discover_node()
        self.jobs = JobManager(writer.write, logger, self.ffmpeg, self.node, factory=factory)
        self.requests = queue.Queue(maxsize=64)
        self.stop = threading.Event()
        self.update_lock = threading.Lock()
        for index in range(4):
            threading.Thread(target=self._request_worker, name=f"request-{index + 1}", daemon=True).start()

    def result(self, request, data=None, exc=None):
        identifier = request.get("id")
        if not isinstance(identifier, str) or len(identifier) > 256:
            identifier = None
        message = {"id": identifier, "type": "result", "ok": exc is None}
        message["error" if exc else "data"] = error_details(exc) if exc else (data if data is not None else {})
        try:
            self.writer.write(message)
        except FrameError:
            self.writer.write({"id": identifier, "type": "result", "ok": False,
                               "error": {"code": "BAD_REQUEST", "message": "The result is too large for native messaging. Try a smaller playlist."}})

    def dispatch(self, request):
        try:
            if not isinstance(request.get("id"), str) or not request["id"] or len(request["id"]) > 256:
                raise HostError("BAD_REQUEST", "Every request needs a nonempty string id.")
            kind = request.get("type")
            if kind == "hello":
                if "maxConcurrent" in request:
                    self.jobs.resize(request["maxConcurrent"])
                self.result(request, {"hostVersion": __version__, "ytdlpVersion": package_version("yt-dlp"),
                                      "ffmpeg": self.ffmpeg, "jsRuntime": {k: v for k, v in self.node.items() if k != "path"} if self.node else None,
                                      "defaultOutputDir": default_output_dir(), "platform": sys.platform,
                                      "dropzone": find_spec("PySide6") is not None})
                warm_up(self.logger)
                self._warm_tray()
            elif kind == "listJobs":
                self.result(request, {"jobs": self.jobs.list_jobs()})
            elif kind == "cancel":
                job_id = self._job_id(request)
                self.result(request, {"jobId": job_id, "cancelled": self.jobs.cancel_job(job_id)})
            elif kind == "download":
                url = validate_url(request.get("url"))
                options = validate_options(request.get("options"))
                cookies = request.get("cookies")
                if cookies is not None and (not isinstance(cookies, str) or len(cookies.encode("utf-8")) > 8 * 1024 * 1024):
                    raise HostError("BAD_REQUEST", "cookies must be Netscape text smaller than 8 MiB.")
                job = self.jobs.create(self._job_id(request), url, options, str(request.get("source") or "")[:256])
                # The acknowledgement is guaranteed to precede every job event.
                self.result(request, {"jobId": job.job_id, "accepted": True})
                self.jobs.enqueue(job, cookies)
            elif kind == "showDrop":
                self.result(request, self._show_drop(request))
            elif kind == "grab":
                self.result(request, self._grab(request))
            elif kind in ("probe", "analyze", "pickFile", "pickFolder", "reveal", "open", "update"):
                try:
                    self.requests.put_nowait(request)
                except queue.Full:
                    raise HostError("BAD_REQUEST", "Too many pending requests; try again shortly.")
            else:
                raise HostError("BAD_REQUEST", "Unknown native request type.")
        except Exception as exc:
            self.result(request, exc=exc)

    @staticmethod
    def _job_id(request):
        value = request.get("jobId")
        if not isinstance(value, str) or not value or len(value) > 256:
            raise HostError("BAD_REQUEST", "jobId must be a nonempty string.")
        return value

    def _request_worker(self):
        while not self.stop.is_set():
            try:
                request = self.requests.get(timeout=.2)
            except queue.Empty:
                continue
            try:
                data = self._slow_request(request)
                self.result(request, data)
            except Exception as exc:
                self.logger.exception("Request %s failed", request.get("type"))
                self.result(request, exc=exc)
            finally:
                self.requests.task_done()

    def _slow_request(self, request):
        kind = request["type"]
        if kind == "analyze":
            return analyze_request(request, factory=self.factory, ffmpeg=self.ffmpeg, node=self.node,
                                   logger=self.logger, log_event=self.jobs.log_event)
        if kind == "pickFile":
            return {"path": self._pick_file()}
        if kind == "probe":
            url = validate_url(request.get("url"))
            params = base_params(YTDLPLogger(self.logger, self.jobs.log_event), self.ffmpeg, self.node)
            params.update(extract_flat="in_playlist", lazy_playlist=True, playlistend=201,
                          skip_download=True, noplaylist=not bool(request.get("flatPlaylist")))
            with cookie_params(request) as cookies:
                params.update(cookies)
                with self.factory(params) as ydl:
                    info = ydl.extract_info(url, download=False, process=False)
                    # process=False intentionally avoids resolving playlists. Resolve a
                    # single transparent URL once, e.g. Bandcamp/embedded players.
                    if info and info.get("_type") in ("url", "url_transparent"):
                        info = ydl.process_ie_result(info, download=False)
                    if not isinstance(info, dict):
                        raise HostError("UNSUPPORTED_URL", "No supported media was found on this page.")
                    return media_info(info, url)
        if kind == "pickFolder":
            return {"path": self._pick_folder(request.get("initial"))}
        if kind in ("reveal", "open"):
            path = self._path(request.get("path"))
            if kind == "reveal":
                subprocess.Popen(["explorer.exe", "/select,", str(path)], stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **subprocess_options())
            else:
                os.startfile(str(path))
            return {}
        if kind == "update":
            if not self.update_lock.acquire(blocking=False):
                raise HostError("BAD_REQUEST", "An update is already running.")
            try:
                before = package_version("yt-dlp")
                command = [sys.executable, "-m", "pip", "install", "-U", "yt-dlp[default]", "yt-dlp-ejs", "--disable-pip-version-check", "--no-input"]
                result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=300, **subprocess_options())
                self.logger.info("yt-dlp update: %s %s", result.stdout, result.stderr)
                if result.returncode:
                    raise HostError("NETWORK", "yt-dlp update failed. Check the host log and your internet connection.")
                # Metadata is read afresh; imported Python modules stay at the old
                # version until the extension reconnects and starts a new host.
                return {"before": before, "after": package_version("yt-dlp"), "ejsVersion": package_version("yt-dlp-ejs"), "restartRequired": True}
            finally:
                self.update_lock.release()
        raise HostError("BAD_REQUEST", "Unknown native request type.")

    def _show_drop(self, request):
        """Hand finished files to the floating drag tray (a separate GUI process)."""
        if find_spec("PySide6") is None:
            raise HostError("BAD_REQUEST", "The drag tray needs PySide6. Rerun the Siphon installer to add it.")
        items = request.get("items")
        if not isinstance(items, list) or not 1 <= len(items) <= 20:
            raise HostError("BAD_REQUEST", "items must list 1 to 20 downloads.")
        clean = []
        for item in items:
            if not isinstance(item, dict) or not isinstance(item.get("files"), list) or not 1 <= len(item["files"]) <= 500:
                raise HostError("BAD_REQUEST", "Each item needs a files list.")
            files = [str(path) for value in item["files"] if (path := self._path(value)).is_file()]
            if not files:
                raise HostError("DISK", "The selected file no longer exists.")
            clean.append({"files": files, **{key: item[key][:300] for key in ("title", "subtitle", "service") if isinstance(item.get(key), str)}})
        self._tray({"items": clean, "pinned": bool(request.get("pinned")), "ffmpeg": self.ffmpeg.get("path")})
        return {"shown": len(clean)}

    def _grab(self, request):
        """Turn a drag gesture in the extension into a native file drag (see dropzone.Grabber)."""
        if find_spec("PySide6") is None:
            raise HostError("BAD_REQUEST", "Dragging needs PySide6. Rerun the Siphon installer to add it.")
        item = request.get("item")
        if not isinstance(item, dict) or not isinstance(item.get("files"), list) or not 1 <= len(item["files"]) <= 500:
            raise HostError("BAD_REQUEST", "item needs a files list.")
        files = [str(path) for value in item["files"] if (path := self._path(value)).is_file()]
        if not files:
            raise HostError("DISK", "The selected file no longer exists.")
        grab = {"files": files, **{key: item[key][:300] for key in ("title", "subtitle", "service") if isinstance(item.get(key), str)}}
        # A cold tray needs ~1 s to start; by then the gesture is usually over.
        return {"started": self._tray({"grab": grab}, spawn=False)}

    def _warm_tray(self):
        if find_spec("PySide6") is not None:
            try:
                self._tray({"resident": True, "hostPid": os.getpid(), "ffmpeg": self.ffmpeg.get("path")})
            except OSError:
                self.logger.exception("Could not start the drag tray")

    def _tray(self, payload, *, spawn=True):
        """Deliver payload to the running tray over its pipe, else start one with it."""
        message = json.dumps(payload).encode("utf-8") + b"\n"
        if os.name == "nt":
            for _ in range(3):
                try:
                    with open("//./pipe/" + tray_pipe_name(), "wb", buffering=0) as pipe:
                        pipe.write(message)
                    return True
                except FileNotFoundError:
                    break
                except OSError:  # every pipe instance busy; the server makes a new one shortly
                    threading.Event().wait(.02)
        if not spawn:
            if payload.get("grab"):
                self._warm_tray()
            return False
        if payload.get("resident") is None:
            payload = {**payload, "resident": True, "hostPid": os.getpid()}
            message = json.dumps(payload).encode("utf-8")
        executable = Path(sys.executable)
        windowless = executable.with_name("pythonw.exe")
        command = [str(windowless if windowless.exists() else executable), "-m", "siphon_host.dropzone"]
        flags = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
        options = {"stdin": subprocess.PIPE, "stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL,
                   "cwd": str(Path(__file__).resolve().parent.parent)}
        try:
            # Outlive the browser's job object, so the tray survives a host restart.
            process = subprocess.Popen(command, creationflags=flags | (0x01000000 if os.name == "nt" else 0), **options)  # CREATE_BREAKAWAY_FROM_JOB
        except OSError:
            process = subprocess.Popen(command, creationflags=flags, **options)
        process.stdin.write(message)
        process.stdin.close()
        return True

    @staticmethod
    def _path(value):
        if not isinstance(value, str) or not value or "\0" in value or not Path(value).is_absolute():
            raise HostError("BAD_REQUEST", "An absolute file path is required.")
        path = Path(value)
        if not path.exists():
            raise HostError("DISK", "The selected file no longer exists.")
        return path

    def _pick_folder(self, initial):
        if initial is not None and (not isinstance(initial, str) or "\0" in initial):
            raise HostError("BAD_REQUEST", "initial must be a directory path.")
        script = """Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = 'Choose a folder for Siphon audio'
$dialog.UseDescriptionForTitle = $true
$dialog.SelectedPath = $env:SIPHON_FOLDER_INITIAL
try {
  if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    ConvertTo-Json -Compress -InputObject $dialog.SelectedPath
  } else { 'null' }
} finally { $dialog.Dispose() }
"""
        environment = os.environ.copy()
        environment["SIPHON_FOLDER_INITIAL"] = initial or default_output_dir()
        # The subprocess is hidden but its folder picker is intentionally visible.
        result = subprocess.run(["powershell.exe", "-NoProfile", "-STA", "-NonInteractive", "-Command", script], env=environment, capture_output=True, text=True, encoding="utf-8-sig", errors="replace", **subprocess_options())
        if result.returncode:
            raise HostError("INTERNAL", "The folder picker could not be opened. " + result.stderr[-1000:])
        return json.loads(result.stdout.strip() or "null")

    def _pick_file(self):
        script = """Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$dialog = New-Object System.Windows.Forms.OpenFileDialog
$dialog.Title = 'Open audio file'
$dialog.Filter = 'Audio files|*.mp3;*.m4a;*.aac;*.flac;*.wav;*.ogg;*.opus;*.webm;*.aiff|All files|*.*'
$dialog.CheckFileExists = $true
$dialog.Multiselect = $false
try {
  if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    ConvertTo-Json -Compress -InputObject $dialog.FileName
  } else { 'null' }
} finally { $dialog.Dispose() }
"""
        result = subprocess.run(["powershell.exe", "-NoProfile", "-STA", "-NonInteractive", "-Command", script],
                                capture_output=True, text=True, encoding="utf-8-sig", errors="replace", **subprocess_options())
        if result.returncode:
            raise HostError("INTERNAL", "The audio file picker could not be opened. " + result.stderr[-1000:])
        return json.loads(result.stdout.strip() or "null")

    def read_loop(self, stream):
        try:
            while not self.stop.is_set() and not self.writer.closed:
                try:
                    request = read_message(stream)
                except BadRequestFrame as exc:
                    self.logger.warning("Rejected native messaging frame: %s", exc)
                    self.result({}, exc=HostError("BAD_REQUEST", str(exc)))
                    continue
                if request is None:
                    break
                try:
                    self.dispatch(request)
                except Exception as exc:
                    self.logger.exception("Native request handler failed")
                    self.result(request, exc=exc)
        except (FrameError, OSError):
            self.logger.exception("Native messaging input closed or malformed")
        finally:
            self.shutdown()

    def shutdown(self):
        self.stop.set()
        self.writer.close()
        self.jobs.shutdown()
        shutdown_analysis()
