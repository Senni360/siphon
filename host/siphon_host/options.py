"""Validate browser options and translate them to the installed yt-dlp API."""

from contextlib import contextmanager
import math
import os
from pathlib import Path
import re
import tempfile
from urllib.parse import urlsplit

from .errors import HostError
from .runtime import default_output_dir, local_app_data

DEFAULTS = {
    "format": "mp3", "quality": "320", "outputDir": None,
    "filenameTemplate": "%(artist,uploader)s - %(title)s.%(ext)s",
    "embedMetadata": True, "embedThumbnail": True, "embedChapters": False,
    "playlist": "single", "playlistSubfolder": True, "cookiesFromBrowser": False,
    "sponsorblockRemove": True, "normalize": False, "trim": None, "trimSilence": True,
    "overwrite": False, "rateLimit": None,
    "analyze": True, "appendBpmKey": True, "writeBpmKeyTags": True,
    "keyNotation": "standard", "bpmKeySuffix": " [{bpm} BPM {key}]", "bpmRange": [70, 180],
}


def validate_bpm_range(value):
    if (not isinstance(value, (list, tuple)) or len(value) != 2
            or any(isinstance(x, bool) or not isinstance(x, (int, float)) or not math.isfinite(x) for x in value)
            or not 30 <= value[0] < value[1] <= 360):
        raise HostError("BAD_REQUEST", "bpmRange must contain two increasing bounds within 30–360 BPM.")
    return list(value)


def validate_url(value) -> str:
    if not isinstance(value, str) or len(value) > 16384:
        raise HostError("BAD_REQUEST", "A valid HTTP(S) media URL is required.")
    try:
        parts = urlsplit(value)
        valid = parts.scheme in ("http", "https") and parts.hostname and not parts.username and not parts.password
    except ValueError:
        valid = False
    if not valid:
        raise HostError("BAD_REQUEST", "A valid HTTP(S) media URL is required.")
    return value


def parse_rate_limit(value) -> int | None:
    if value is None or value == "":
        return None
    if not isinstance(value, str):
        raise HostError("BAD_REQUEST", "rateLimit must be a size such as 2M.")
    match = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*([kKmMgGtT]?)(?:i?[bB])?\s*", value)
    if not match:
        raise HostError("BAD_REQUEST", "rateLimit must be a size such as 2M.")
    result = float(match[1]) * 1024 ** ("kmgt".index(match[2].lower()) + 1 if match[2] else 0)
    if not math.isfinite(result) or result < 1 or result > 2**63 - 1:
        raise HostError("BAD_REQUEST", "rateLimit must be a positive, finite size.")
    return int(result)


def validate_options(value) -> dict:
    if value is None:
        value = {}
    if not isinstance(value, dict):
        raise HostError("BAD_REQUEST", "options must be an object.")
    options = DEFAULTS | value
    if options["format"] not in ("mp3", "m4a", "opus", "flac", "wav", "ogg", "best"):
        raise HostError("BAD_REQUEST", "Unsupported audio format.")
    if options["quality"] not in ("320", "256", "192", "128", "V0", "best"):
        raise HostError("BAD_REQUEST", "Unsupported audio quality.")
    if options["playlist"] not in ("single", "all"):
        raise HostError("BAD_REQUEST", "playlist must be single or all.")
    for key in ("embedMetadata", "embedThumbnail", "embedChapters", "playlistSubfolder", "cookiesFromBrowser", "sponsorblockRemove", "normalize", "overwrite", "trimSilence", "analyze", "appendBpmKey", "writeBpmKeyTags"):
        if not isinstance(options[key], bool):
            raise HostError("BAD_REQUEST", f"{key} must be a boolean.")
    options["bpmRange"] = validate_bpm_range(options["bpmRange"])
    if options["keyNotation"] not in ("standard", "camelot", "openkey"):
        raise HostError("BAD_REQUEST", "keyNotation must be standard, camelot, or openkey.")
    if not isinstance(options["bpmKeySuffix"], str) or len(options["bpmKeySuffix"]) > 2048:
        raise HostError("BAD_REQUEST", "bpmKeySuffix must be a suffix template of at most 2048 characters.")
    from .analysis import format_suffix
    try:
        format_suffix({"bpm": 120, "keyShort": "Am", "key": "A minor", "camelot": "8A", "openKey": "1m"}, options["bpmKeySuffix"], options["keyNotation"])
    except (ValueError, KeyError) as exc:
        raise HostError("BAD_REQUEST", str(exc)) from exc
    output = options["outputDir"]
    if output is not None and (not isinstance(output, str) or not output.strip() or "\0" in output or not Path(output).is_absolute()):
        raise HostError("BAD_REQUEST", "outputDir must be an absolute directory path.")
    template = options["filenameTemplate"]
    if not isinstance(template, str) or not template or len(template) > 2048 or any(c in template for c in ("/", "\\", "\0", "\r", "\n")) or template in (".", ".."):
        raise HostError("BAD_REQUEST", "filenameTemplate must be a filename without directory separators.")
    parse_rate_limit(options["rateLimit"])
    trim = options["trim"]
    if trim is not None:
        if not isinstance(trim, dict):
            raise HostError("BAD_REQUEST", "trim must contain start/end seconds.")
        for key in ("start", "end"):
            number = trim.get(key)
            if number is not None and (isinstance(number, bool) or not isinstance(number, (int, float)) or not math.isfinite(number) or number < 0):
                raise HostError("BAD_REQUEST", "Trim times must be finite nonnegative seconds.")
        if trim.get("end") is not None and trim["end"] <= (trim.get("start") or 0):
            raise HostError("BAD_REQUEST", "Trim end must follow start.")
    # Never retain unrecognized fields (in particular cookies) in listJobs/history.
    return {key: options[key] for key in DEFAULTS}


def base_params(logger, ffmpeg: dict, node: dict | None) -> dict:
    return {
        "logger": logger, "quiet": True, "no_warnings": False, "noprogress": True,
        "socket_timeout": 20, "retries": 3, "fragment_retries": 3,
        "extractor_retries": 2, "windowsfilenames": True, "restrictfilenames": False,
        "ffmpeg_location": ffmpeg.get("path"),
        # Verified in yt-dlp 2026.08.19 YoutubeDL.py: {runtime: {path: executable}}.
        # yt-dlp[default] supplies matching yt-dlp-ejs; no remote code fetch needed.
        "js_runtimes": {"node": {"path": node["path"]}} if node else {},
    }


def build_download_params(options: dict, logger, ffmpeg: dict, node: dict | None) -> dict:
    options = validate_options(options)
    params = base_params(logger, ffmpeg, node)
    directory = Path(options["outputDir"] or default_output_dir())
    template = options["filenameTemplate"]
    if options["playlistSubfolder"] and options["playlist"] == "all":
        template = "%(playlist_title|Playlist)s/" + template
    params.update({
        "format": "bestaudio/best", "outtmpl": {"default": template, "pl_thumbnail": ""},
        "paths": {"home": str(directory)},
        "noplaylist": options["playlist"] == "single", "overwrites": options["overwrite"],
        "ratelimit": parse_rate_limit(options["rateLimit"]),
        "writethumbnail": options["embedThumbnail"] and options["format"] not in ("wav", "ogg"),
        "clean_infojson": True,
    })
    pp = []
    if options["sponsorblockRemove"]:
        pp.append({"key": "SponsorBlock", "categories": ["music_offtopic"], "when": "after_filter"})
    if params["writethumbnail"]:
        pp.append({"key": "FFmpegThumbnailsConvertor", "format": "jpg", "when": "before_dl"})
    # The protocol calls the container "ogg"; yt-dlp calls its encoder "vorbis".
    pp.append({"key": "FFmpegExtractAudio", "preferredcodec": "vorbis" if options["format"] == "ogg" else options["format"],
               "preferredquality": "0" if options["quality"] in ("V0", "best") else options["quality"],
               "nopostoverwrites": not options["overwrite"], "trim": options["trim"],
               "trim_silence": options["trimSilence"]})
    if options["sponsorblockRemove"]:
        pp.append({"key": "ModifyChapters", "remove_sponsor_segments": ["music_offtopic"], "force_keyframes": True})
    if options["normalize"]:
        # Resolved to our PP by create_downloader, inserted before tagging/artwork.
        pp.append({"key": "SiphonNormalize", "quality": options["quality"]})
        params["postprocessor_args"] = {"siphonnormalize+ffmpeg_o": ["-af", "loudnorm=I=-16:LRA=11:TP=-1.5"]}
    if options["embedMetadata"] or options["embedChapters"]:
        pp.append({"key": "FFmpegMetadata", "add_metadata": options["embedMetadata"], "add_chapters": options["embedChapters"], "add_infojson": False})
    if params["writethumbnail"]:
        pp.append({"key": "EmbedThumbnail", "already_have_thumbnail": False})
    params["postprocessors"] = pp
    return params


@contextmanager
def cookie_params(request: dict):
    """One private temporary file per operation; never log or publish its contents."""
    cookiefile = None
    try:
        text = request.get("cookies")
        if text is not None:
            if not isinstance(text, str) or len(text.encode("utf-8")) > 8 * 1024 * 1024:
                raise HostError("BAD_REQUEST", "cookies must be Netscape cookie text smaller than 8 MiB.")
            if text and not text.lstrip().startswith(("# Netscape HTTP Cookie File", "# HTTP Cookie File")):
                raise HostError("BAD_REQUEST", "cookies must use the Netscape cookie file format.")
            with tempfile.NamedTemporaryFile(mode="w", suffix=".cookies.txt", prefix="siphon-", encoding="utf-8", newline="\n", delete=False) as stream:
                cookiefile = stream.name
                stream.write(text)
            yield {"cookiefile": cookiefile}
        elif request.get("cookiesFromBrowser"):
            yield {"cookiesfrombrowser": ("chromium", str(local_app_data() / "imput" / "Helium" / "User Data"), None, None)}
        else:
            yield {}
    finally:
        if cookiefile:
            try:
                os.unlink(cookiefile)
            except FileNotFoundError:
                pass
