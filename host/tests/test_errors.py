import errno
import pytest

from siphon_host.errors import Cancelled, HostError, error_details


@pytest.mark.parametrize("message,code", [
    ("Sign in to confirm your age", "AGE_RESTRICTED"), ("Private video", "LOGIN_REQUIRED"),
    ("This video is not available in your country", "GEO_BLOCKED"), ("DRM protected", "DRM"),
    ("Unsupported URL: https://example.com", "UNSUPPORTED_URL"), ("HTTP Error 403: Forbidden", "NETWORK"),
    ("ffmpeg not found", "FFMPEG_MISSING"), ("ffprobe and ffmpeg not found", "FFMPEG_MISSING"),
    ("Failed to decrypt with DPAPI", "LOGIN_REQUIRED"), ("No space left on device", "DISK"),
    ("connection timed out", "NETWORK"), ("unrecognized error", "INTERNAL"),
    ("ffmpeg exited with code 3436169992", "NETWORK"),
    ("ffmpeg exited with code -858797304", "NETWORK"),
])
def test_error_mapping(message, code):
    assert error_details(Exception(message))["code"] == code


def test_typed_errors_and_os_disk_failure():
    assert error_details(Cancelled())["code"] == "CANCELLED"
    assert error_details(HostError("LIVE_NOT_SUPPORTED", "Live"))["code"] == "LIVE_NOT_SUPPORTED"
    assert error_details(OSError(errno.ENOSPC, "full"))["code"] == "DISK"


def test_ffmpeg_http_code_has_useful_diagnostic():
    result = error_details(Exception("ffmpeg exited with code 3436169992"))
    assert "HTTP Error 403 Forbidden" in result["message"]
    assert "refresh the media URL" in result["message"]
