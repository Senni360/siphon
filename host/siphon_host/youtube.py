"""YouTube-only recovery using fresh extractions, clients, and transfer files.

Client policies below were checked against yt-dlp 2026.8.19, not older recipes:
default = visionos/web (different when authenticated); android_vr has known 403s,
web/Safari may expose only SABR, and missing_pot explicitly exposes unsafe URLs.
Let yt-dlp enforce its PO-token policy and discover installed provider plugins.
"""

from dataclasses import dataclass
import re
import tempfile
from urllib.parse import parse_qs, urlsplit

from yt_dlp.postprocessor.common import PostProcessor

from .errors import HostError, error_details


@dataclass(frozen=True)
class Strategy:
    name: str
    clients: tuple[str, ...]
    format: str


STRATEGIES = (
    Strategy("default", ("default",), "bestaudio/best[height<=360]"),
    Strategy("embedded", ("web_embedded",), "bestaudio/best[height<=360]"),
    # Change both client selection and source codec after a rejected Opus URL.
    # Keep cookies: yt-dlp skips visionos itself for authenticated sessions.
    Strategy("visionos-aac", ("visionos",), "bestaudio[ext=m4a]/bestaudio"),
    Strategy("safari-hls", ("web_safari",), "bestaudio[protocol^=m3u8]/best[protocol^=m3u8][height<=360]"),
    # With a provider this offers token-bearing audio; without one, yt-dlp
    # excludes missing-POT audio and itag 18 is a small, token-free last resort.
    Strategy("mweb", ("mweb",), "bestaudio/best[format_id=18]"),
)


def is_youtube_url(url):
    try:
        host = (urlsplit(url).hostname or "").lower().rstrip(".")
    except ValueError:
        return False
    return any(host == domain or host.endswith("." + domain)
               for domain in ("youtube.com", "youtu.be", "youtube-nocookie.com", "youtubekids.com"))


def _retryable(exc):
    if isinstance(exc, (HostError, OSError)):
        return False
    details = error_details(exc)
    message = details["message"].lower()
    # A different client cannot grant permission to private/member-only content.
    if any(word in message for word in ("private video", "members-only", "video has been removed", "video is unavailable")):
        return False
    if details["code"] in {"AGE_RESTRICTED", "GEO_BLOCKED", "DISK", "FFMPEG_MISSING", "UNSUPPORTED_URL"}:
        return False
    return any(word in message for word in (
        "http error 403", "http error 429", "http error 500", "http error 502", "http error 503",
        "http error 504", "timed out", "timeout", "connection reset", "connection aborted",
        "requested format is not available", "no video formats", "only images are available",
        "only drm formats", "page needs to be reloaded", "not available on this app",
        "sign in", "not a bot", "bot check", "this content isn't available, try again later",
    ))


def _log(logger, level, message):
    if logger:
        getattr(logger, level)("[siphon:youtube] " + message)


class _SelectedFormatPP(PostProcessor):
    def __init__(self, downloader, strategy, logger):
        super().__init__(downloader)
        self.strategy, self.logger = strategy, logger

    def run(self, info):
        for selected in info.get("requested_downloads") or [info]:
            url = urlsplit(selected.get("url") or "")
            client = parse_qs(url.query).get("c", [None])[0]
            if not client and (match := re.search(r"/c/([^/]+)", url.path)):
                client = match[1]
            # Never log signed media URLs, cookie values, visitor IDs or tokens.
            _log(self.logger, "info", f"strategy={self.strategy.name} selected client={client or 'unknown'} "
                 f"format={selected.get('format_id', 'unknown')} protocol={selected.get('protocol', 'unknown')}")
        return [], info


def extract_youtube(url, params, factory, *, check_cancel, can_retry, audio_only=False):
    """Run one job; never replay a playlist after a completed item.

    Each attempt has a new YoutubeDL, cookie jar, extraction, and private transfer
    directory. Even a mid-transfer 403 can recover without resuming incompatible
    bytes into another client's format. The caller owns cookies and job state.
    """
    logger = params.get("logger")
    progress_max = {"item": None, "fraction": 0}

    def progress_hook(progress):
        # Keep the UI monotonic during recovery; do not modify yt-dlp's info.
        event = progress.copy()
        if event.get("status") in ("downloading", "finished"):
            info = event.get("info_dict") or {}
            item = (info.get("id"), info.get("playlist_index"))
            if item != progress_max["item"]:
                progress_max.update(item=item, fraction=0)
            downloaded = event.get("downloaded_bytes") or 0
            total = event.get("total_bytes") or event.get("total_bytes_estimate")
            if event.get("status") == "finished":
                progress_max["fraction"] = 1
            elif total:
                progress_max["fraction"] = max(progress_max["fraction"], min(1, downloaded / total))
            # Keep actual byte counts/totals intact when the source format changes.
            event["_siphon_progress"] = progress_max["fraction"] or None
        for hook in params.get("progress_hooks", ()):
            hook(event)

    for index, strategy in enumerate(STRATEGIES, 1):
        check_cancel()
        attempt = params.copy()
        attempt["extractor_args"] = {key: value.copy() for key, value in (params.get("extractor_args") or {}).items()}
        youtube_args = attempt["extractor_args"].setdefault("youtube", {})
        youtube_args["player_client"] = list(strategy.clients)
        # Do not opt into formats that yt-dlp knows need an unavailable PO token.
        youtube_args["formats"] = [value for value in youtube_args.get("formats", ()) if value != "missing_pot"]
        attempt.update(format=strategy.format, http_chunk_size=10 * 1024 * 1024,
                       retries=4, fragment_retries=4, extractor_retries=3,
                       skip_unavailable_fragments=False, progress_hooks=[progress_hook])
        if audio_only:
            # Standalone analysis never fetches a merged video fallback. Keep
            # the same client chain while preferring its lowest-bitrate audio.
            attempt["format"] = ("worstaudio[protocol^=m3u8]" if strategy.name == "safari-hls"
                                 else "worstaudio[ext=m4a]/worstaudio" if strategy.name == "visionos-aac"
                                 else "worstaudio")
            for key in ("retries", "fragment_retries", "extractor_retries"):
                attempt[key] = params.get(key, 1)
        attempt["paths"] = params.get("paths", {}).copy()
        parent = attempt["paths"].get("temp") or attempt["paths"].get("home")
        _log(logger, "info", f"attempt={index}/{len(STRATEGIES)} strategy={strategy.name} "
             f"clients={','.join(strategy.clients)} cookies={'yes' if params.get('cookiefile') or params.get('cookiesfrombrowser') else 'no'}")
        try:
            with tempfile.TemporaryDirectory(prefix="youtube-", dir=parent) as staging:
                attempt["paths"]["temp"] = staging
                with factory(attempt) as ydl:
                    ydl.add_post_processor(_SelectedFormatPP(ydl, strategy, logger), when="before_dl")
                    info = ydl.extract_info(url, download=True)
            _log(logger, "info", f"strategy={strategy.name} succeeded attempt={index}")
            return info
        except Exception as exc:
            check_cancel()
            if not _retryable(exc) or not can_retry():
                raise
            _log(logger, "warning", f"strategy={strategy.name} failed ({error_details(exc)['code']}); "
                 + ("trying a fresh client/format" if index < len(STRATEGIES) else "strategies exhausted"))
            if index == len(STRATEGIES):
                message = error_details(exc)["message"].lower()
                if any(word in message for word in ("403", "429", "sign in", "bot", "requested format", "no video formats", "only images", "reloaded", "this content isn't available")):
                    raise HostError("LOGIN_REQUIRED", "YouTube's bot check or playback restrictions blocked every audio strategy. "
                                    "Open the video in Helium, complete any sign-in/bot check, enable site-cookie export in Siphon, "
                                    "and retry. If it persists, update yt-dlp from Siphon and reconnect.") from exc
                raise
