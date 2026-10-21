"""Offline functional checks using a generated tone, never copyrighted media."""

import logging
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from siphon_host.downloader import create_downloader
from siphon_host.options import build_download_params
from siphon_host.runtime import subprocess_options

pytestmark = pytest.mark.skipif(not shutil.which("ffmpeg"), reason="FFmpeg is not available")


@pytest.mark.parametrize("format", ["mp3", "m4a", "opus", "flac", "wav", "ogg", "best"])
def test_all_formats_convert_and_tag_real_generated_audio(tmp_path, format):
    # Start with a common real download container/codec, not the target format.
    source = tmp_path / "source.webm"
    subprocess.run([shutil.which("ffmpeg"), "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                    "sine=frequency=440:duration=0.3", "-ac", "2", "-c:a", "libopus", str(source)],
                   check=True, capture_output=True, **subprocess_options())
    params = build_download_params({"format": format, "normalize": True, "embedThumbnail": False, "sponsorblockRemove": False},
                                   logging.getLogger("test"), {"path": shutil.which("ffmpeg")}, None)
    info = {"filepath": str(source), "ext": "webm", "title": "Siphon generated test tone", "vcodec": "none", "acodec": "opus"}
    with create_downloader(params) as ydl:
        for pp in ydl._pps["post_process"]:
            deletions, info = pp.run(info)
            for deletion in deletions:
                Path(deletion).unlink(missing_ok=True)
    expected = "opus" if format == "best" else format
    assert Path(info["filepath"]).suffix == "." + expected
    assert Path(info["filepath"]).stat().st_size > 100


def test_best_native_wav_skips_unsupported_artwork(tmp_path):
    source = tmp_path / "source.wav"
    subprocess.run([shutil.which("ffmpeg"), "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                    "sine=frequency=440:duration=0.1", str(source)], check=True, capture_output=True, **subprocess_options())
    original = source.read_bytes()
    artwork = tmp_path / "artwork.jpg"
    artwork.write_bytes(b"not needed for an unsupported container")
    params = build_download_params({"format": "best", "embedMetadata": False, "sponsorblockRemove": False},
                                   logging.getLogger("test"), {"path": shutil.which("ffmpeg")}, None)
    info = {"filepath": str(source), "ext": "wav", "thumbnails": [{"filepath": str(artwork)}]}
    with create_downloader(params) as ydl:
        for pp in ydl._pps["post_process"]:
            deletions, info = pp.run(info)
            for deletion in deletions:
                Path(deletion).unlink(missing_ok=True)
    assert source.read_bytes() == original
    assert not artwork.exists()


@pytest.mark.parametrize("rate", [22050, 44100, 96000])
@pytest.mark.parametrize("format", ["mp3", "m4a", "opus", "flac", "wav", "ogg", "best"])
@pytest.mark.parametrize("normalize_and_trim", [False, True])
def test_mono_sample_rates_all_formats_and_local_trim(tmp_path, rate, format, normalize_and_trim):
    source = tmp_path / "source.wav"
    subprocess.run([shutil.which("ffmpeg"), "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                    f"sine=frequency=440:duration=0.7:sample_rate={rate}", str(source)],
                   check=True, capture_output=True, **subprocess_options())
    params = build_download_params({"format": format, "quality": "320", "normalize": normalize_and_trim,
                                   "trim": {"start": 0.1, "end": 0.4} if normalize_and_trim else None,
                                   "embedThumbnail": False, "sponsorblockRemove": False},
                                  logging.getLogger("test"), {"path": shutil.which("ffmpeg")}, None)
    info = {"filepath": str(source), "ext": "wav", "title": "Generated mono tone", "asr": rate,
            "vcodec": "none", "acodec": "pcm_s16le", "duration": 0.7,
            "chapters": [{"start_time": 0, "end_time": 0.2}, {"start_time": 0.2, "end_time": 0.7}]}
    with create_downloader(params) as ydl:
        for pp in ydl._pps["post_process"]:
            deletions, info = pp.run(info)
            for deletion in deletions:
                Path(deletion).unlink(missing_ok=True)
    measured = json.loads(subprocess.run([shutil.which("ffprobe"), "-v", "error", "-show_streams", "-show_format", "-of", "json", info["filepath"]],
                                        check=True, capture_output=True, text=True, **subprocess_options()).stdout)
    audio = next(stream for stream in measured["streams"] if stream["codec_type"] == "audio")
    assert audio["channels"] == 1
    assert abs(float(measured["format"]["duration"]) - (0.3 if normalize_and_trim else 0.7)) < 0.08
    if format == "mp3":
        assert audio["bit_rate"] == "320000"
    if format in ("opus", "ogg", "mp3", "m4a"):
        assert audio["sample_rate"] == "48000"
    if normalize_and_trim:
        assert info["chapters"] == [{"start_time": 0, "end_time": 0.1}, {"start_time": 0.1, "end_time": pytest.approx(0.3)}]


def test_ffmpeg_failure_logs_full_stderr_and_includes_tail(tmp_path, caplog):
    from siphon_host.errors import error_details
    source = tmp_path / "corrupt.wav"
    source.write_bytes(b"invalid media")
    params = build_download_params({"format": "mp3", "trim": {"start": 2, "end": 7}, "sponsorblockRemove": False},
                                  logging.getLogger("test"), {"path": shutil.which("ffmpeg")}, None)
    with create_downloader(params) as ydl:
        processor = ydl._pps["post_process"][0]
        with pytest.raises(Exception) as caught:
            processor.run({"filepath": str(source), "ext": "wav"})
    assert "full stderr:" in caplog.text
    assert "Invalid data found" in caplog.text
    error = error_details(caught.value)
    assert error["code"] == "INTERNAL"
    assert "host log" in error["message"] and "Invalid data found" in error["message"]
    assert not list(tmp_path.glob("*.siphon-trim.*"))
