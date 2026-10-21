"""Write DJ-readable tempo/key tags without replacing unrelated metadata.

TKEY/INITIALKEY always contain the standard short name (Am, F#m, Bb), never
Camelot. MP4 tmpo is an integer; other formats retain the 0.1 BPM precision.
"""

import math

from .notation import key_names, parse_key


def write_tags(path, analysis: dict) -> None:
    """Update MP3/WAV ID3, MP4, FLAC, Ogg Opus or Ogg Vorbis in place.

    Unsupported/corrupt media and invalid analyses raise ValueError (or Mutagen's
    error). The download pipeline can catch these independently of analysis.
    """
    import mutagen
    from mutagen.flac import FLAC
    from mutagen.id3 import TBPM, TKEY
    from mutagen.mp3 import MP3
    from mutagen.mp4 import MP4, MP4FreeForm
    from mutagen.oggopus import OggOpus
    from mutagen.oggvorbis import OggVorbis
    from mutagen.wave import WAVE

    bpm = float(analysis["bpm"])
    if not math.isfinite(bpm) or not 0 < bpm <= 65535:
        raise ValueError("bpm must be finite, positive, and representable in MP4 tmpo")
    key = key_names(*parse_key(analysis.get("keyShort") or analysis["key"]))["keyShort"]
    text_bpm = f"{bpm:.1f}".removesuffix(".0")
    audio = mutagen.File(path)
    if not isinstance(audio, (MP3, WAVE, MP4, FLAC, OggOpus, OggVorbis)):
        raise ValueError("Unsupported audio container for BPM/key tags")
    if audio.tags is None:
        audio.add_tags()
    if isinstance(audio, (MP3, WAVE)):
        audio.tags.setall("TBPM", [TBPM(encoding=3, text=[text_bpm])])
        audio.tags.setall("TKEY", [TKEY(encoding=3, text=[key])])
    elif isinstance(audio, MP4):
        audio.tags["tmpo"] = [max(1, round(bpm))]
        audio.tags["----:com.apple.iTunes:initialkey"] = [MP4FreeForm(key.encode("utf-8"))]
    else:
        audio.tags["BPM"] = [text_bpm]
        audio.tags["INITIALKEY"] = [key]
    audio.save()


ACID_ROOT_SET, ACID_STRETCH = 0x02, 0x04


def write_acid(path, analysis: dict, *, numerator=4, denominator=4) -> None:
    """Add or replace a WAV 'acid' chunk so DAWs can tempo-sync the file.

    ACID metadata (read by FL Studio, Ableton Live, Acid, Reaper and others)
    stores tempo, a beat count for the whole file, meter, and the root note.
    The root note is the analysed tonic as a MIDI note from C4 (60) to B4.
    """
    import struct

    bpm = float(analysis["bpm"])
    if not math.isfinite(bpm) or not 0 < bpm <= 999:
        raise ValueError("bpm must be finite and positive")
    try:
        tonic, _ = parse_key(analysis.get("keyShort") or analysis["key"])
    except (KeyError, ValueError):
        tonic = None
    with open(path, "r+b") as stream:
        header = stream.read(12)
        if len(header) < 12 or header[:4] != b"RIFF" or header[8:] != b"WAVE":
            raise ValueError("ACID metadata requires a RIFF/WAVE file")
        size = stream.seek(0, 2)
        end = min(size, 8 + struct.unpack("<I", header[4:8])[0])
        position, acid, rate, block, frames = 12, None, None, None, None
        while position + 8 <= end:
            stream.seek(position)
            identifier, length = struct.unpack("<4sI", stream.read(8))
            if identifier == b"fmt ":
                _, _, rate, _, block = struct.unpack("<HHIIH", stream.read(14))
            elif identifier == b"data":
                frames = length
            elif identifier == b"acid" and length == 24:
                acid = position
            position += 8 + length + (length & 1)
        if not rate or not block or frames is None:
            raise ValueError("WAV file is missing its format or audio data")
        seconds = frames // block / rate
        flags = ACID_STRETCH | (ACID_ROOT_SET if tonic is not None else 0)
        chunk = struct.pack("<4sIIHHfIHHf", b"acid", 24, flags, 60 + (tonic or 0), 0x8000, 0.,
                            max(1, round(seconds * bpm / 60)), denominator, numerator, bpm)
        if acid is None:
            # Append after the last complete chunk and grow the RIFF size.
            acid = position
            if acid + len(chunk) - 8 > 0xFFFFFFFF:
                raise ValueError("WAV file is too large for RIFF metadata")
            stream.seek(acid)
            stream.write(chunk)
            stream.truncate()
            stream.seek(4)
            stream.write(struct.pack("<I", acid + len(chunk) - 8))
        else:
            stream.seek(acid)
            stream.write(chunk)
