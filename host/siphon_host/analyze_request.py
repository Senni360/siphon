"""Ephemeral URL excerpts and read-only local-file analysis requests."""

import math
from pathlib import Path
import tempfile
import threading
import time

from yt_dlp.utils import download_range_func

from .analysis_service import analyze
from .errors import HostError
from .options import base_params, cookie_params, validate_bpm_range, validate_url
from .runtime import YTDLPLogger
from .youtube import extract_youtube, is_youtube_url

REQUEST_TIMEOUT = 120
_requests = threading.BoundedSemaphore(4)


def _timeout():
    return HostError("NETWORK", "Audio analysis timed out. Try a local file or a shorter clip.")


def analyze_request(request, *, factory, ffmpeg, node, logger, log_event):
    # Socket/extractor timeouts alone do not bound DLL initialization, DNS,
    # third-party plugins, or FFmpeg startup. Bound the protocol response too.
    # The operation retains ownership of its temporary files until its decoder
    # has stopped, including when the waiter has already timed out.
    deadline = time.monotonic() + REQUEST_TIMEOUT
    if not _requests.acquire(timeout=REQUEST_TIMEOUT):
        raise _timeout()
    finished, outcome = threading.Event(), {}

    def run():
        try:
            outcome["data"] = _analyze_request(request, factory=factory, ffmpeg=ffmpeg, node=node,
                                                logger=logger, log_event=log_event, deadline=deadline)
        except Exception as exc:
            outcome["error"] = exc
        finally:
            _requests.release()
            finished.set()

    try:
        threading.Thread(target=run, name="analyze-request", daemon=True).start()
    except BaseException:
        _requests.release()
        raise
    if not finished.wait(max(0, deadline - time.monotonic())):
        raise _timeout()
    if "error" in outcome:
        raise outcome["error"]
    return outcome["data"]


def _analyze_request(request, *, factory, ffmpeg, node, logger, log_event, deadline):
    if bool(request.get("url")) == bool(request.get("path")):
        raise HostError("BAD_REQUEST", "Supply exactly one audio URL or absolute file path.")
    seconds = request.get("maxSeconds", 150)
    if (isinstance(seconds, bool) or not isinstance(seconds, (int, float))
            or not math.isfinite(seconds) or not 2 <= seconds <= 150):
        raise HostError("BAD_REQUEST", "maxSeconds must be between 2 and 150 seconds.")
    bpm_range = validate_bpm_range(request.get("bpmRange", [70, 180]))
    if not ffmpeg["found"]:
        raise HostError("FFMPEG_MISSING", "FFmpeg is required to analyze audio. Install FFmpeg and reconnect Siphon.")
    def check(*args, **kwargs):
        if time.monotonic() >= deadline:
            raise _timeout()

    if request.get("path"):
        value = request["path"]
        if not isinstance(value, str) or "\0" in value or not Path(value).is_absolute():
            raise HostError("BAD_REQUEST", "An absolute audio file path is required.")
        path = Path(value)
        if not path.exists() or not path.is_file():
            raise HostError("DISK", "The selected audio file does not exist or is not a file.")
        return {"analysis": analyze(path, max_seconds=seconds, bpm_range=bpm_range, deadline=deadline), "source": "file"}

    url = validate_url(request["url"])
    params = base_params(YTDLPLogger(logger, log_event), ffmpeg, node)
    params.update(format="worstaudio", noplaylist=True, socket_timeout=10,
                  retries=1, fragment_retries=1, extractor_retries=1,
                  download_ranges=download_range_func(None, [(0, seconds)]),
                  # FFmpeg's output duration bound also covers HLS and formats
                  # whose source duration is missing or inaccurate.
                  external_downloader_args={"ffmpeg_i": ["-rw_timeout", "10000000"], "ffmpeg_o": ["-t", str(seconds)]},
                  progress_hooks=[check], postprocessor_hooks=[check], match_filter=lambda info, **kw: _filter(info, check))
    with tempfile.TemporaryDirectory(prefix="siphon-analyze-") as directory, cookie_params(request) as cookies:
        params.update(cookies, paths={"home": directory}, outtmpl={"default": "excerpt.%(ext)s"})
        files = []
        params["post_hooks"] = [lambda path: files.append(Path(path))]
        if is_youtube_url(url):
            info = extract_youtube(url, params, factory, check_cancel=check, can_retry=lambda: True, audio_only=True)
        else:
            with factory(params) as ydl:
                info = ydl.extract_info(url, download=True)
        check()
        if not isinstance(info, dict):
            raise HostError("UNSUPPORTED_URL", "No supported audio was found on this page.")
        if not files and info.get("filepath"):
            files.append(Path(info["filepath"]))
        if not files:
            files = [p for p in Path(directory).glob("excerpt.*") if p.is_file() and p.suffix not in (".part", ".ytdl")]
        if not files:
            raise HostError("INTERNAL", "The extractor completed without producing an audio excerpt.")
        # Let an in-flight decoder finish before deleting its input on Windows.
        # The surrounding waiter still enforces the 120-second response limit.
        result = analyze(files[-1], max_seconds=seconds, bpm_range=bpm_range)
        check()
        return {"analysis": result, "source": "url", "title": str(info.get("title") or "")}


def _filter(info, check):
    check()
    if info.get("is_live") or info.get("live_status") in ("is_live", "is_upcoming"):
        raise HostError("LIVE_NOT_SUPPORTED", "Live and upcoming streams cannot be analyzed. Try again after the stream ends.")
