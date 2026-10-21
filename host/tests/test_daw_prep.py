"""Leading-silence trim, ACID tempo metadata, and the drag tray request."""

import json
import os
import logging
from pathlib import Path
import shutil
import struct
import subprocess

import pytest

from siphon_host.analysis import write_acid, write_tags
from siphon_host.downloader import create_downloader, leading_silence, shift_segments
from siphon_host.errors import HostError
from siphon_host.options import build_download_params, validate_options
from siphon_host.runtime import subprocess_options

FFMPEG = shutil.which("ffmpeg")
needs_ffmpeg = pytest.mark.skipif(not FFMPEG, reason="FFmpeg is not available")


def tone(path, *, silence, duration=1.0, rate=44100):
    """A 440 Hz tone preceded by exact digital silence."""
    subprocess.run([FFMPEG, "-v", "error", "-f", "lavfi", "-i", f"anullsrc=r={rate}:cl=stereo:d={silence}",
                    "-f", "lavfi", "-i", f"sine=frequency=440:sample_rate={rate}:duration={duration}",
                    "-filter_complex", "[1]aformat=channel_layouts=stereo[b];[0][b]concat=n=2:v=0:a=1", str(path)],
                   check=True, capture_output=True, **subprocess_options())
    return path


def duration(path):
    result = subprocess.run([shutil.which("ffprobe"), "-v", "error", "-show_entries", "format=duration", "-of", "json", str(path)],
                            check=True, capture_output=True, text=True, **subprocess_options())
    return float(json.loads(result.stdout)["format"]["duration"])


def run_postprocessors(source, options, **info):
    params = build_download_params({"embedThumbnail": False, "sponsorblockRemove": False, **options},
                                   logging.getLogger("test"), {"path": FFMPEG}, None)
    info = {"filepath": str(source), "ext": "wav", "title": "Generated tone", "vcodec": "none", "acodec": "pcm_s16le", **info}
    with create_downloader(params) as ydl:
        for pp in ydl._pps["post_process"]:
            deletions, info = pp.run(info)
            for deletion in deletions:
                Path(deletion).unlink(missing_ok=True)
    return info


@needs_ffmpeg
def test_leading_silence_is_sample_accurate(tmp_path):
    source = tone(tmp_path / "source.wav", silence=0.75)
    assert leading_silence(FFMPEG, source) == pytest.approx(0.75, abs=3 / 44100)
    assert leading_silence(FFMPEG, source, start=0.5) == pytest.approx(0.25, abs=3 / 44100)


@needs_ffmpeg
def test_all_silent_audio_is_never_cut(tmp_path):
    source = tmp_path / "silent.wav"
    subprocess.run([FFMPEG, "-v", "error", "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo:d=0.5", str(source)],
                   check=True, capture_output=True, **subprocess_options())
    assert leading_silence(FFMPEG, source) == 0


@needs_ffmpeg
@pytest.mark.parametrize("format", ["wav", "flac", "mp3"])
def test_download_starts_on_first_sound(tmp_path, format):
    source = tone(tmp_path / "source.wav", silence=1.25)
    info = run_postprocessors(source, {"format": format, "trimSilence": True}, duration=2.25,
                              chapters=[{"start_time": 0, "end_time": 2.25, "title": "Track"}])
    assert info["duration"] == pytest.approx(1.0, abs=.001)
    assert info["chapters"][0]["end_time"] == pytest.approx(1.0, abs=.001)
    # MP3 framing adds encoder padding; lossless formats are exact.
    assert duration(info["filepath"]) == pytest.approx(1.0, abs=.06 if format == "mp3" else .002)


@needs_ffmpeg
def test_trim_disabled_keeps_original_bytes(tmp_path):
    source = tone(tmp_path / "source.wav", silence=0.5)
    original = source.read_bytes()
    info = run_postprocessors(source, {"format": "best", "trimSilence": False, "embedMetadata": False})
    assert Path(info["filepath"]).read_bytes() == original


@needs_ffmpeg
def test_silence_search_starts_after_removed_offtopic_intro(tmp_path):
    source = tone(tmp_path / "source.wav", silence=1.5)
    info = run_postprocessors(source, {"format": "wav", "trimSilence": True}, duration=2.5,
                              sponsorblock_chapters=[{"start_time": 0, "end_time": 1.0, "category": "music_offtopic"}])
    assert info["duration"] == pytest.approx(1.0, abs=.001)
    assert info["sponsorblock_chapters"] == []


def test_shift_segments_clips_to_new_timeline():
    segments = [{"start_time": 0, "end_time": 1}, {"start_time": 1, "end_time": 5}, {"start_time": 6, "end_time": 8}]
    assert shift_segments(segments, 2, 7) == [{"start_time": 0, "end_time": 3}, {"start_time": 4, "end_time": 5}]


def test_trim_silence_option_is_validated_and_passed():
    assert validate_options({})["trimSilence"] is True
    with pytest.raises(HostError):
        validate_options({"trimSilence": "yes"})
    params = build_download_params({"trimSilence": False}, logging.getLogger("test"), {"path": None}, None)
    audio = next(pp for pp in params["postprocessors"] if pp["key"] == "FFmpegExtractAudio")
    assert audio["trim_silence"] is False


def read_acid(path):
    data = Path(path).read_bytes()
    assert struct.unpack("<I", data[4:8])[0] + 8 == len(data)
    index = data.index(b"acid")
    assert data.count(b"acid") == 1
    return dict(zip(("flags", "root", "unknown1", "unknown2", "beats", "denominator", "numerator", "tempo"),
                    struct.unpack("<IHHfIHHf", data[index + 8:index + 32])))


@needs_ffmpeg
def test_acid_chunk_carries_tempo_beats_and_root(tmp_path):
    source = tone(tmp_path / "loop.wav", silence=0, duration=4)
    analysis = {"bpm": 120.0, "key": "F# minor", "keyShort": "F#m"}
    write_tags(source, analysis)
    write_acid(source, analysis)
    acid = read_acid(source)
    assert acid["tempo"] == 120 and acid["beats"] == 8
    assert (acid["numerator"], acid["denominator"]) == (4, 4)
    assert acid["root"] == 66 and acid["flags"] == 0x06
    # Rewriting replaces the chunk in place, and the file stays readable.
    write_acid(source, {"bpm": 90.0, "key": "C major", "keyShort": "C"})
    assert read_acid(source)["tempo"] == 90 and read_acid(source)["root"] == 60
    assert duration(source) == pytest.approx(4, abs=.01)
    import mutagen
    assert str(mutagen.File(source).tags["TBPM"]) == "120"


def test_acid_rejects_non_wav(tmp_path):
    path = tmp_path / "x.wav"
    path.write_bytes(b"not a riff file at all")
    with pytest.raises(ValueError):
        write_acid(path, {"bpm": 120, "key": "A minor"})


def test_show_drop_validates_and_launches(tmp_path, monkeypatch):
    from siphon_host import host as host_module
    from test_host import RecordingWriter, host
    audio = tmp_path / "song.wav"
    audio.write_bytes(b"RIFF")
    launched = []

    class FakeProcess:
        def __init__(self, command, **kwargs):
            launched.append((command, kwargs))
            self.stdin = self
            self.data = b""
        def write(self, data):
            self.data += data
            launched.append(json.loads(data))
        def close(self):
            pass

    monkeypatch.setattr(host_module, "find_spec", lambda name: object())
    monkeypatch.setattr(host_module, "tray_pipe_name", lambda: "siphon-test-no-tray")
    monkeypatch.setattr(host_module.subprocess, "Popen", FakeProcess)
    writer = RecordingWriter()
    instance = host(writer)
    try:
        instance.dispatch({"id": "a", "type": "showDrop", "items": [{"files": [str(audio)], "title": "Song", "subtitle": "120 BPM", "extra": 1}], "pinned": True})
        assert writer.messages[-1]["data"] == {"shown": 1}
        command, kwargs = launched[0]
        assert command[-2:] == ["-m", "siphon_host.dropzone"]
        assert launched[1] == {"items": [{"files": [str(audio)], "title": "Song", "subtitle": "120 BPM"}], "pinned": True, "ffmpeg": None,
                               "resident": True, "hostPid": os.getpid()}
        for bad in ({"items": []}, {"items": [{"files": ["relative.wav"]}]}, {"items": [{"files": [str(tmp_path / "gone.wav")]}]}):
            instance.dispatch({"id": "b", "type": "showDrop", **bad})
            assert writer.messages[-1]["ok"] is False
    finally:
        instance.shutdown()


def test_grab_never_cold_starts_a_drag(tmp_path, monkeypatch):
    from siphon_host import host as host_module
    from test_host import RecordingWriter, host
    audio = tmp_path / "song.wav"
    audio.write_bytes(b"RIFF")
    warmed = []
    monkeypatch.setattr(host_module, "find_spec", lambda name: object())
    monkeypatch.setattr(host_module, "tray_pipe_name", lambda: "siphon-test-no-tray")
    writer = RecordingWriter()
    instance = host(writer)
    monkeypatch.setattr(instance, "_warm_tray", lambda: warmed.append(True))
    try:
        instance.dispatch({"id": "g", "type": "grab", "item": {"files": [str(audio)], "title": "Song"}})
        # The gesture is long over by the time a cold tray starts; just warm it for next time.
        assert writer.messages[-1]["data"] == {"started": False}
        assert warmed == [True]
        instance.dispatch({"id": "h", "type": "grab", "item": {"files": ["relative.wav"]}})
        assert writer.messages[-1]["ok"] is False
    finally:
        instance.shutdown()
