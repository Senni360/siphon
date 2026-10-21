"""Network-free analysis tests; fixtures are constructed independently here."""

import json
import math
from pathlib import Path
import re
import subprocess
import sys

import numpy as np
import pytest
from scipy.io import wavfile

from siphon_host.analysis import (AnalysisError, analyze_file, format_suffix, from_camelot,
                                  from_open_key, key_names, parse_key, to_camelot, to_open_key)
from siphon_host.runtime import discover_ffmpeg, subprocess_options

SR = 22050


@pytest.fixture(scope="module")
def ffmpeg():
    found = discover_ffmpeg()
    if not found["found"]:
        pytest.skip("FFmpeg is needed for the analysis integration tests")
    return found["path"]


@pytest.mark.parametrize("mode", ["major", "minor"])
@pytest.mark.parametrize("tonic", range(12))
def test_notation_round_trip(tonic, mode):
    names = key_names(tonic, mode)
    for value in (names["key"], names["keyShort"], names["camelot"], names["openKey"]):
        assert parse_key(value) == (tonic, mode)
        assert to_camelot(value) == names["camelot"]
        assert to_open_key(value) == names["openKey"]
    assert from_camelot(names["camelot"]) == names["key"]
    assert from_open_key(names["openKey"], short=True) == names["keyShort"]


def test_known_wheel_anchors_and_enharmonics():
    assert to_camelot("A minor") == "8A"
    assert to_open_key("C major") == "1d"
    assert from_open_key("6m") == "G# minor"
    assert parse_key("F♯ minor") == parse_key("Gbm") == (6, "minor")
    assert parse_key("A♭ major") == (8, "major")
    assert key_names(6, "major")["key"] == "Gb major"


@pytest.mark.parametrize("value", ["H minor", "13A", "0d", "8C", "Cm7", "C.__class__", ""])
def test_invalid_notation(value):
    with pytest.raises(ValueError):
        parse_key(value)


def test_suffix_safety_and_formatting():
    analysis = {"bpm": 128., **key_names(6, "minor")}
    assert format_suffix(analysis) == " [128 BPM F#m]"
    assert format_suffix(analysis, notation="camelot") == " [128 BPM 11A]"
    assert format_suffix(analysis, notation="openkey") == " [128 BPM 4m]"
    assert format_suffix({**analysis, "bpm": 128.4}, " {bpmExact} {keyLong} {camelot} {openkey} {keyShort}") == " 128.4 F# minor 11A 4m F#m"
    unsafe = format_suffix(analysis, ' /<>:"\\|?*\x00{key}. ')
    assert not re.search(r'[<>:"/\\|?*\x00-\x1f]', unsafe)
    assert not unsafe.endswith((".", " "))
    assert len(format_suffix(analysis, "x" * 300)) == 160
    for template in ("{bad}", "{key.__class__}", "{key[0]}", "{bpm:.1f}", "{key!r}"):
        with pytest.raises(ValueError):
            format_suffix(analysis, template)
    with pytest.raises(ValueError):
        format_suffix(analysis, notation="invalid")


@pytest.mark.parametrize("bpm", [70, 83, 96, 111, 128, 149, 167, 180])
def test_click_tempo(tmp_path, ffmpeg, bpm):
    y = np.zeros(SR * 16, np.float32)
    t = np.arange(500) / SR
    click = np.sin(2 * np.pi * 1800 * t) * np.exp(-t * 200)
    for when in np.arange(.2, 15.8, 60 / bpm):
        index = round(when * SR)
        y[index:index + len(click)] += click
    path = tmp_path / "click.wav"
    wavfile.write(path, SR, y)
    result = analyze_file(path)
    assert abs(result["bpm"] - bpm) <= 1, result
    assert result["durationAnalyzed"] <= 16
    assert result["engine"] == "siphon-analysis/1"
    assert 0 <= result["bpmConfidence"] <= 1
    assert 1 <= len(result["bpmCandidates"]) <= 3
    json.dumps(result, allow_nan=False)


@pytest.mark.parametrize("tonic,mode,cents", [(0, "major", 0), (9, "minor", 27), (6, "minor", -30), (10, "major", 15)])
def test_chord_progression_key(tmp_path, ffmpeg, tonic, mode, cents):
    # I-IV-V-I or i-iv-V-i, with tonic bass and realistic harmonic partials.
    chords = [(0, 4, 7), (5, 9, 12), (7, 11, 14), (0, 4, 7)] if mode == "major" else [(0, 3, 7), (5, 8, 12), (7, 11, 14), (0, 3, 7)]
    audio = []
    for chord in chords * 2:
        t = np.arange(SR * 2, dtype=np.float32) / SR
        segment = np.zeros_like(t)
        for note in (*[48 + tonic + n for n in chord], 36 + tonic + chord[0]):
            frequency = 440 * 2 ** ((note - 69 + cents / 100) / 12)
            for h in (1, 2, 3, 4):
                segment += np.sin(2 * np.pi * frequency * h * t) / (8 * h ** 2)
        segment *= np.minimum(t * 30, 1) * np.minimum((2 - t) * 20, 1)
        audio.append(segment)
    path = tmp_path / "chords.wav"
    wavfile.write(path, SR, np.concatenate(audio))
    result = analyze_file(path)
    assert parse_key(result["key"]) == (tonic, mode), result
    assert 0 <= result["keyConfidence"] <= 1


def test_bounded_middle_and_silence(tmp_path, ffmpeg):
    # Intro/outro deliberately have different pitches; selected center is A minor.
    t = np.arange(SR * 12, dtype=np.float32) / SR
    y = sum(np.sin(2 * np.pi * f * t) for f in (220, 261.625565, 329.627557)) / 4
    path = tmp_path / "center.wav"
    wavfile.write(path, SR, np.concatenate([np.zeros(SR * 8), y, np.zeros(SR * 8)]).astype(np.float32))
    result = analyze_file(path, max_seconds=8)
    assert result["durationAnalyzed"] == 8
    assert result["keyShort"] == "Am"
    wavfile.write(path, SR, np.zeros(SR * 3, np.float32))
    with pytest.raises(AnalysisError, match="silent"):
        analyze_file(path)


@pytest.mark.parametrize("kwargs", [{"max_seconds": 0}, {"max_seconds": math.inf}, {"bpm_range": (180, 70)}, {"bpm_range": (1, 400)}, {"bpm_range": (70,)}])
def test_invalid_analysis_options(kwargs):
    with pytest.raises(ValueError):
        analyze_file("does-not-exist", **kwargs)


def test_package_import_is_lazy():
    result = subprocess.run([sys.executable, "-c", "import sys; import siphon_host.analysis; assert not any(n in sys.modules for n in ('numpy','scipy','librosa','mutagen'))"], capture_output=True, **subprocess_options())
    assert result.returncode == 0, result.stderr


def test_decimal_tempo_and_range_folding(tmp_path, ffmpeg):
    y = np.zeros(SR * 24, np.float32)
    t = np.arange(700) / SR
    click = np.sin(2 * np.pi * 1000 * t) * np.exp(-t * 180)
    for when in np.arange(.2, 23.8, 60 / 127.4):
        i = round(when * SR)
        y[i:i + len(click)] += click
    path = tmp_path / "fractional.wav"
    wavfile.write(path, SR, y)
    result = analyze_file(path)
    assert abs(result["bpm"] - 127.4) <= .2, result
    assert result["bpm"] != round(result["bpm"])
    folded = analyze_file(path, bpm_range=(40, 70))
    assert 40 <= folded["bpm"] <= 70
    assert abs(folded["bpm"] - 63.7) <= .3, folded


@pytest.mark.parametrize("mode", ["minor", "dorian", "mixolydian"])
def test_sub_bass_and_modal_tonic(tmp_path, ffmpeg, mode):
    # Strong fundamentals below the old A1 cutoff must not be mistaken for
    # their major-third overtones. Modal tonic is represented by API minor/major.
    chords = {
        "minor": [(0, 3, 7), (5, 8, 12), (7, 11, 14), (0, 3, 7)],
        "dorian": [(0, 3, 7), (5, 9, 12), (10, 14, 17), (0, 3, 7)],
        "mixolydian": [(0, 4, 7), (10, 14, 17), (5, 9, 12), (0, 4, 7)],
    }[mode]
    tonic = 5 if mode == "minor" else 2
    segments = []
    for chord in chords * 2:
        t = np.arange(SR * 2) / SR
        v = np.zeros_like(t)
        for midi in [24 + tonic + chord[0]] + [48 + tonic + interval for interval in chord]:
            frequency = 440 * 2 ** ((midi - 69) / 12)
            for partial in range(1, 9):
                v += np.sin(2 * np.pi * frequency * partial * t) / partial ** 1.5 * (.3 if midi < 48 else .07)
        segments.append(v * np.minimum(t * 30, 1) * np.minimum((2 - t) * 20, 1))
    path = tmp_path / "modal.wav"
    wavfile.write(path, SR, np.concatenate(segments).astype(np.float32))
    result = analyze_file(path)
    assert parse_key(result["key"]) == (tonic, "major" if mode == "mixolydian" else "minor"), result
    if mode == "mixolydian":
        assert result["keyConfidence"] < .6


def test_modulation_lowers_confidence():
    from siphon_host.analysis.engine import _key
    def triad(tonic):
        t = np.arange(SR * 24) / SR
        return sum(np.sin(2 * np.pi * 440 * 2 ** ((48 + tonic + n - 69) / 12) * t) for n in (0, 4, 7)) / 4
    c, fs = triad(0).astype(np.float32), triad(6).astype(np.float32)
    stable, stable_confidence = _key(np.concatenate([c, c, c]))
    modulating, changed_confidence = _key(np.concatenate([c, fs, c]))
    assert stable["key"] == modulating["key"] == "C major"
    assert changed_confidence < stable_confidence


def test_variable_tempo_lowers_confidence():
    from siphon_host.analysis.engine import _tempo_from_onsets, TEMPO_HOP
    rate = SR / TEMPO_HOP
    def envelope(variable):
        y = np.zeros(round(72 * rate))
        position = .2
        while position < 71.8:
            y[round(position * rate)] = 8
            bpm = 120 + (6 * np.sin(position / 9) if variable else 0)
            position += 60 / bpm
        return np.tile(y, (4, 1))
    stable = _tempo_from_onsets(envelope(False), (70, 180))
    varying = _tempo_from_onsets(envelope(True), (70, 180))
    assert abs(stable[0] - 120) <= .2
    assert varying[1] < stable[1]
    assert varying[1] < .6


def test_confidence_calibration_is_monotone_and_zero_safe():
    from siphon_host.analysis.confidence import calibrate
    for kind in ("bpm", "key"):
        scores = [calibrate(kind, n / 100) for n in range(101)]
        assert scores == sorted(scores)
        assert scores[0] == 0
        assert 0 < scores[-1] < 1
