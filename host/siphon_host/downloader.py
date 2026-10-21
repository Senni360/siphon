"""yt-dlp construction and a real normalization pass (even for native codecs)."""

from pathlib import Path
import os
import subprocess

import yt_dlp
from yt_dlp.postprocessor import get_postprocessor
from yt_dlp.postprocessor.embedthumbnail import EmbedThumbnailPP
from yt_dlp.postprocessor.ffmpeg import FFmpegExtractAudioPP, FFmpegMetadataPP, FFmpegPostProcessor, FFmpegPostProcessorError
from yt_dlp.utils import replace_extension

from .runtime import subprocess_options


def leading_silence(ffmpeg, path, *, start=0., threshold_db=-60., limit=120., rate=44100):
    """Seconds before the first sample louder than threshold_db, measured from start.

    Returns 0 when the audible start is immediate or when nothing audible occurs
    within limit seconds, so an unusual file is never cut to nothing.
    """
    import numpy as np
    threshold = 10 ** (threshold_db / 20)
    command = [ffmpeg or "ffmpeg", "-v", "error", "-nostdin", "-ss", str(start), "-t", str(limit), "-i", str(path),
               "-vn", "-map", "0:a:0", "-ac", "2", "-ar", str(rate), "-f", "f32le", "-"]
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, **subprocess_options())
    frames = 0
    try:
        while chunk := process.stdout.read(rate * 8):
            samples = np.frombuffer(chunk[:len(chunk) // 8 * 8], dtype="<f4").reshape(-1, 2)
            loud = np.flatnonzero(np.abs(samples).max(axis=1) > threshold)
            if loud.size:
                return (frames + int(loud[0])) / rate
            frames += len(samples)
        return 0.
    finally:
        process.kill()
        process.wait()
        process.stdout.close()


def shift_segments(segments, start, end):
    """Move chapter-like segments onto a trimmed timeline, dropping those outside it."""
    shifted = []
    for segment in segments or []:
        left, right = max(start, segment["start_time"]), segment["end_time"]
        if end is not None:
            right = min(end, right)
        if right > left:
            shifted.append(dict(segment, start_time=left - start, end_time=right - start))
    return shifted


def encoder_args(codec, quality):
    """Safe encoding settings for mono/stereo and unusual input sample rates."""
    args = ["-ar", "48000"] if codec in ("libmp3lame", "aac", "libfdk_aac", "libopus", "libvorbis") else []
    numeric = float(quality) if quality is not None and str(quality).replace(".", "", 1).isdigit() else 0
    if codec == "libopus":
        # libopus rejects >256 kbps per channel. Cap all re-encodes at 160k,
        # including quality=best/V0 and normalization of a native Opus stream.
        args += ["-b:a", f"{int(min(numeric if numeric > 10 else 160, 160))}k"]
    elif codec == "libvorbis":
        # Fixed 320k is also invalid for mono Vorbis. VBR adapts to channels.
        level = {320: "9", 256: "8", 192: "6", 128: "4"}.get(numeric, "10")
        args += ["-q:a", level]
    return args


class FFmpegDiagnostics:
    """Preserve stderr that yt-dlp normally reduces to its final line."""
    def write_debug(self, message, *args, **kwargs):
        if not message.startswith("ffmpeg command line:"):
            self._ffmpeg_stderr = message
        return super().write_debug(message, *args, **kwargs)

    def real_run_ffmpeg(self, *args, **kwargs):
        self._ffmpeg_stderr = ""
        try:
            return super().real_run_ffmpeg(*args, **kwargs)
        except (FFmpegPostProcessorError, IndexError) as exc:
            stderr = self._ffmpeg_stderr or str(exc)
            logger = self._downloader.params.get("logger")
            if logger:
                logger.error("FFmpeg processing failed; full stderr:\n" + stderr)
            tail = "\n".join(stderr.strip().splitlines()[-8:])[-1800:]
            raise FFmpegPostProcessorError("FFmpeg audio processing failed. " + tail) from exc


class SiphonExtractAudioPP(FFmpegExtractAudioPP):
    @classmethod
    def pp_key(cls):
        return "ExtractAudio"

    def __init__(self, downloader=None, trim=None, trim_silence=False, **kwargs):
        super().__init__(downloader, **kwargs)
        self.trim = trim
        self.trim_silence = trim_silence

    def _effective_trim(self, info):
        trim = dict(self.trim or {})
        start, end = trim.get("start") or 0, trim.get("end")
        if self.trim_silence:
            # An off-topic intro removed by SponsorBlock would leave the cut
            # point mid-file; start the silence search after it instead.
            for segment in sorted(info.get("sponsorblock_chapters") or [], key=lambda s: s["start_time"]):
                if segment["start_time"] <= start + .05 < segment["end_time"]:
                    start = segment["end_time"]
            try:
                offset = leading_silence(self.executable, info["filepath"], start=start)
            except Exception as exc:
                self.report_warning(f"Could not measure leading silence: {exc}")
                offset = 0
            if (end is None or start + offset < end) and (start + offset) - (trim.get("start") or 0) >= .005:
                trim["start"] = start + offset
        return trim if trim.get("start") or trim.get("end") is not None else None

    def _quality_args(self, codec):
        if codec in ("flac", "pcm_s16le", None):
            return []
        safe = encoder_args(codec, self._preferredquality)
        if codec in ("libopus", "libvorbis"):
            return safe
        return [*super()._quality_args(codec), *safe]

    def run(self, info):
        trim = self._effective_trim(info)
        if not trim:
            return super().run(info)
        # Work on a complete local file. A sample-accurate cut requires decoding
        # even when source and target codecs match; encode directly to target.
        target = self.mapping
        if target == "best":
            native = self.get_audio_codec(info["filepath"])
            target = {"aac": "m4a", "vorbis": "ogg", "pcm_s16le": "wav", "pcm_s24le": "wav"}.get(native, native)
        target = "ogg" if target == "vorbis" else target
        codec = {"mp3": "libmp3lame", "m4a": "aac", "opus": "libopus", "ogg": "libvorbis", "flac": "flac", "wav": "pcm_s16le"}.get(target)
        if not codec:
            target, codec = "mp3", "libmp3lame"
        path = info["filepath"]
        output = replace_extension(path, target, info["ext"])
        temporary = str(Path(output).with_name(Path(output).stem + ".siphon-trim." + target))
        start, end = trim.get("start") or 0, trim.get("end")
        args = ["-vn", "-map", "0:a:0", "-ss", str(start)]
        if end is not None:
            args += ["-t", str(end - start)]
        args += ["-c:a", codec, *self._quality_args(codec)]
        try:
            FFmpegPostProcessor.run_ffmpeg(self, path, temporary, args)
            os.replace(temporary, output)
        finally:
            Path(temporary).unlink(missing_ok=True)
        info.update(filepath=output, ext=target)
        info["duration"] = end - start if end is not None else max(0, (info.get("duration") or start) - start)
        # Keep chapters and SponsorBlock cuts on the new timeline.
        for key in ("chapters", "sponsorblock_chapters"):
            if key in info:
                info[key] = shift_segments(info[key], start, end)
        return ([path] if path != output else []), info


class SiphonMetadataPP(FFmpegMetadataPP):
    @classmethod
    def pp_key(cls):
        return "Metadata"

    def run(self, info):
        if info.get("ext") != "ogg":
            return super().run(info)
        # FFmpeg 8.1 stream-copying short resampled Vorbis can wrap a negative
        # granule timestamp to 2**32, reporting a ~25-hour duration. Updating
        # Vorbis comments in place preserves the original packets/timestamps.
        from mutagen.oggvorbis import OggVorbis
        audio = OggVorbis(info["filepath"])
        if self._add_metadata:
            for option in self._get_metadata_opts(info):
                if option[0] == "-metadata":
                    key, value = option[1].split("=", 1)
                    audio[key] = value
        if self._add_chapters:
            self._fixup_chapters(info)
            for index, chapter in enumerate(info.get("chapters") or [], 1):
                milliseconds = round(chapter["start_time"] * 1000)
                seconds, ms = divmod(milliseconds, 1000)
                minutes, second = divmod(seconds, 60)
                hour, minute = divmod(minutes, 60)
                audio[f"CHAPTER{index:03}"] = f"{hour:02}:{minute:02}:{second:02}.{ms:03}"
                if chapter.get("title"):
                    audio[f"CHAPTER{index:03}NAME"] = chapter["title"]
        audio.save()
        return [], info


class SiphonEmbedThumbnailPP(EmbedThumbnailPP):
    @classmethod
    def pp_key(cls):
        return "EmbedThumbnail"

    def run(self, info):
        # "best" can preserve an unsupported native container such as WAV.
        # Artwork should never turn an otherwise successful download into failure.
        if info.get("ext") not in {"mp3", "mkv", "mka", "ogg", "opus", "flac", "m4a", "mp4", "m4v", "mov"}:
            self.report_warning(f"Skipping artwork: {info.get('ext', 'this format')} does not support embedded thumbnails.")
            return [t["filepath"] for t in info.get("thumbnails", []) if t.get("filepath")], info
        return super().run(info)


class SiphonNormalizePP(FFmpegPostProcessor):
    def __init__(self, downloader=None, quality="320"):
        super().__init__(downloader)
        self.quality = quality

    def run(self, info):
        path = info["filepath"]
        ext = info["ext"]
        codec = {"mp3": "libmp3lame", "m4a": "aac", "aac": "aac", "opus": "libopus", "ogg": "libvorbis", "flac": "flac", "wav": "pcm_s16le", "webm": "libopus"}.get(ext)
        if not codec:
            raise ValueError(f"Normalization is unavailable for {ext} audio")
        temporary = str(Path(path).with_name(Path(path).stem + ".siphon-normalize." + ext))
        # loudnorm's dynamic mode internally upsamples to 192 kHz. Explicitly
        # resample its output: Vorbis/MP3 cannot encode that at normal bitrates.
        rate = info.get("asr") if codec in ("flac", "pcm_s16le") else 48000
        args = ["-vn", "-c:a", codec, "-ar", str(int(rate or 48000)), *encoder_args(codec, self.quality)]
        if codec in ("libmp3lame", "aac", "libopus", "libvorbis"):
            if codec not in ("libopus", "libvorbis"):
                args += ["-q:a", "0"] if self.quality == "V0" and codec == "libmp3lame" else ["-b:a", (self.quality if self.quality.isdigit() else "320") + "k"]
        try:
            # FFmpegPostProcessor injects siphonnormalize+ffmpeg_o here.
            self.run_ffmpeg(path, temporary, args)
            os.replace(temporary, path)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
        return [], info


def create_downloader(params: dict):
    params = params.copy()
    processors = params.pop("postprocessors", [])
    ydl = yt_dlp.YoutubeDL(params)
    for spec in processors:
        spec = spec.copy()
        key, when = spec.pop("key"), spec.pop("when", "post_process")
        cls = {"FFmpegExtractAudio": SiphonExtractAudioPP, "FFmpegMetadata": SiphonMetadataPP, "SiphonNormalize": SiphonNormalizePP, "EmbedThumbnail": SiphonEmbedThumbnailPP}.get(key) or get_postprocessor(key)
        if issubclass(cls, FFmpegPostProcessor):
            cls = type(cls.__name__, (FFmpegDiagnostics, cls), {})
        ydl.add_post_processor(cls(ydl, **spec), when=when)
    return ydl
