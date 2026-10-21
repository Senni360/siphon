"""Map extractor/OS failures to the shared extension contract."""

import errno
import re


class HostError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


class Cancelled(HostError):
    def __init__(self):
        super().__init__("CANCELLED", "Download cancelled.")


def error_details(exc: BaseException) -> dict:
    if isinstance(exc, HostError):
        return {"code": exc.code, "message": str(exc)}
    message = str(exc).removeprefix("ERROR: ").strip() or type(exc).__name__
    exit_code = re.search(r"ffmpeg exited with code\s+(-?\d+)", message, re.IGNORECASE)
    if exit_code:
        # FFmpeg returns negative AVERROR tags. Windows exposes their unsigned
        # 32-bit representation, e.g. 3436169992 == -858797304 == HTTP 403.
        tag = ((-int(exit_code[1])) & 0xFFFFFFFF).to_bytes(4, "little")
        http = {b"400": "400 Bad Request", b"401": "401 Unauthorized", b"403": "403 Forbidden",
                b"404": "404 Not Found", b"429": "429 Too Many Requests", b"4XX": "4xx client error", b"5XX": "5xx server error"}
        if tag[:1] == b"\xf8" and tag[1:] in http:
            message = "HTTP Error " + http[tag[1:]] + " while FFmpeg fetched media. Retry to refresh the media URL. " + message
    lower = message.lower()
    code = "INTERNAL"
    if isinstance(exc, OSError) and exc.errno in (errno.ENOSPC, errno.EACCES, errno.EPERM, errno.EROFS, errno.ENOENT):
        code = "DISK"
    elif any(s in lower for s in ("decrypt", "dpapi", "cookie database", "could not copy chrome cookie", "app-bound")):
        return {"code": "LOGIN_REQUIRED", "message": "Helium cookies could not be read or decrypted. Enable site-cookie export in Siphon, sign in to the site, and retry. " + message[:1000]}
    elif any(s in lower for s in ("confirm your age", "age-restricted", "age restricted", "inappropriate for some users")):
        code = "AGE_RESTRICTED"
    elif any(s in lower for s in ("private video", "sign in", "login required", "log in", "members-only", "authentication", "http error 401")):
        code = "LOGIN_REQUIRED"
    elif any(s in lower for s in ("not available in your country", "geo-restrict", "geographic", "geo blocked", "not available from your location")):
        code = "GEO_BLOCKED"
    elif "drm" in lower:
        code = "DRM"
    elif "unsupported url" in lower or "no suitable extractor" in lower:
        code = "UNSUPPORTED_URL"
    elif "ffmpeg" in lower and any(s in lower for s in ("not found", "not installed", "could not find", "unable to find")):
        code = "FFMPEG_MISSING"
    elif any(s in lower for s in ("no space left", "disk full", "permission denied", "cannot find the path", "filename or extension is too long")):
        code = "DISK"
    elif any(s in lower for s in ("http error", "timed out", "timeout", "unable to download", "connection", "urlopen", "name resolution", "network", "certificate verify")):
        code = "NETWORK"
    if code == "INTERNAL" and any(s in lower for s in ("ffmpeg", "audio conversion failed")):
        message = "Audio processing failed. Retry the download; if it repeats, update FFmpeg and check the host log. " + message
    return {"code": code, "message": message[:4000]}
