"""Real container round trips exercise the tags DJ software will read."""

import subprocess

import mutagen
import pytest

from siphon_host.analysis import write_tags
from siphon_host.runtime import discover_ffmpeg, subprocess_options


@pytest.mark.parametrize("extension,codec", [("mp3", "libmp3lame"), ("m4a", "aac"), ("flac", "flac"), ("opus", "libopus"), ("ogg", "libvorbis"), ("wav", "pcm_s16le")])
def test_tag_round_trip(tmp_path, extension, codec):
    executable = discover_ffmpeg()
    if not executable["found"]:
        pytest.skip("FFmpeg required")
    path = tmp_path / ("tags." + extension)
    result = subprocess.run([executable["path"], "-nostdin", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.3",
                             "-metadata", "title=Keep this title", "-c:a", codec, str(path)], capture_output=True, **subprocess_options())
    assert result.returncode == 0, result.stderr
    if extension == "wav":
        # FFmpeg's WAV title goes into RIFF INFO, which Mutagen does not expose.
        # Seed a separate existing ID3 frame to verify non-analysis preservation.
        from mutagen.id3 import TIT2
        audio = mutagen.File(path)
        audio.add_tags()
        audio.tags.add(TIT2(encoding=3, text=["Keep this title"]))
        audio.save()
    write_tags(path, {"bpm": 127.4, "keyShort": "F#m"})
    write_tags(path, {"bpm": 128.6, "keyShort": "Am"})
    tags = mutagen.File(path).tags
    if extension in ("mp3", "wav"):
        assert str(tags["TBPM"]) == "128.6"
        assert str(tags["TKEY"]) == "Am"
        assert len(tags.getall("TKEY")) == 1
        assert str(tags["TIT2"]) == "Keep this title"
    elif extension == "m4a":
        assert tags["tmpo"] == [129]
        assert bytes(tags["----:com.apple.iTunes:initialkey"][0]) == b"Am"
        assert tags["\xa9nam"] == ["Keep this title"]
    else:
        assert tags["BPM"] == ["128.6"]
        assert tags["INITIALKEY"] == ["Am"]
        assert tags["title"] == ["Keep this title"]
