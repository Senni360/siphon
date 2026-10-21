"""FFT onset periodicity and tuned harmonic pitch-class profile analysis.

Only imported when analysis is requested. No models, JIT, network, or subprocess
shells. All scientific operations use prebuilt NumPy/SciPy CPU kernels.
"""

from functools import lru_cache
import json
import math
from pathlib import Path
import subprocess

import numpy as np
from scipy import fft, ndimage, signal, sparse

from ..runtime import discover_ffmpeg, subprocess_options
from . import AnalysisError
from .notation import key_names
from .confidence import calibrate as _calibrate

SR = 22050
TEMPO_HOP = 128


@lru_cache(maxsize=1)
def _ffmpeg():
    found = discover_ffmpeg()
    if not found.get("found"):
        raise AnalysisError("FFmpeg was not found")
    return found["path"]


def _decode(path, limit):
    path = Path(path).resolve(strict=True)
    if not path.is_file():
        raise AnalysisError("Expected a local audio file")
    executable = _ffmpeg()
    probe = Path(executable).with_name("ffprobe.exe" if Path(executable).suffix.lower() == ".exe" else "ffprobe")
    duration = None
    if probe.is_file():
        try:
            result = subprocess.run([str(probe), "-v", "error", "-show_entries", "format=duration",
                                     "-of", "json", str(path)], capture_output=True, timeout=12,
                                    **subprocess_options())
            value = float(json.loads(result.stdout).get("format", {}).get("duration", "nan"))
            if math.isfinite(value) and value > 0:
                duration = value
        except (ValueError, OSError, subprocess.TimeoutExpired):
            pass  # Unknown duration: still decode a bounded excerpt from the start.
    start = max(0, (duration - limit) / 2) if duration else 0
    args = [executable, "-nostdin", "-hide_banner", "-loglevel", "error", "-ss", f"{start:.6f}",
            "-i", str(path), "-t", f"{limit:.6f}", "-map", "0:a:0", "-vn", "-sn", "-dn",
            "-ac", "1", "-ar", str(SR), "-f", "f32le", "-acodec", "pcm_f32le", "pipe:1"]
    try:
        result = subprocess.run(args, capture_output=True, timeout=max(30, limit / 2), **subprocess_options())
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise AnalysisError(f"Audio decoding failed: {exc}") from exc
    if result.returncode:
        raise AnalysisError("FFmpeg could not decode audio: " + result.stderr.decode("utf-8", "replace")[-800:])
    y = np.frombuffer(result.stdout, dtype="<f4").copy()
    if y.size < SR * 2 or not np.isfinite(y).all():
        raise AnalysisError("At least two seconds of finite audio are required")
    # Remove edge silence, not quiet passages inside the selected section.
    block = SR // 4
    energy = np.sqrt(np.mean(y[:len(y) // block * block].reshape(-1, block) ** 2, axis=1))
    active = np.flatnonzero(energy > max(1e-6, float(energy.max()) * .015))
    if not len(active):
        raise AnalysisError("Audio is silent")
    y = y[max(0, (active[0] - 1) * block):min(len(y), (active[-1] + 2) * block)]
    if y.size < 2 * SR:
        raise AnalysisError("Too little non-silent audio to analyze")
    y /= max(float(np.max(np.abs(y))), 1e-8)
    return y


def _stft(y, n_fft, hop):
    frames = np.lib.stride_tricks.sliding_window_view(np.pad(y, (n_fft // 2, n_fft // 2)), n_fft)[::hop]
    return np.abs(fft.rfft(frames * signal.windows.hann(n_fft, sym=False).astype(np.float32), axis=1, workers=1)).T


@lru_cache(maxsize=1)
def _mel_bank():
    frequencies = fft.rfftfreq(1024, 1 / SR)
    points = 700 * (np.expm1(np.linspace(np.log1p(35 / 700), np.log1p(10000 / 700), 50)))
    bank = np.maximum(0, np.minimum((frequencies[None, :] - points[:-2, None]) / np.diff(points)[:-1, None],
                                   (points[2:, None] - frequencies[None, :]) / np.diff(points)[1:, None]))
    bank /= np.maximum(bank.sum(axis=1, keepdims=True), 1e-8)
    return sparse.csr_matrix(bank.astype(np.float32))


def _onsets(y):
    spectrum = _stft(y, 1024, TEMPO_HOP)
    bands = _mel_bank() @ spectrum
    bands = np.log1p(bands / np.maximum(np.mean(bands, axis=1, keepdims=True), 1e-5) * 10)
    # Frequency maximum suppresses vibrato-induced flux (SuperFlux principle).
    previous = ndimage.maximum_filter1d(bands, size=3, axis=0)
    flux = np.maximum(0, bands[:, 1:] - previous[:, :-1])
    envelopes = np.stack([flux.mean(axis=0), flux[:16].mean(axis=0), flux[16:32].mean(axis=0), flux[32:].mean(axis=0)])
    envelopes -= ndimage.uniform_filter1d(envelopes, size=round(.5 * SR / TEMPO_HOP), axis=1)
    envelopes = np.maximum(envelopes, 0)
    envelopes /= np.maximum(np.std(envelopes, axis=1, keepdims=True), 1e-6)
    return envelopes


def _tempo(y, bpm_range):
    return _tempo_from_onsets(_onsets(y), bpm_range)


def _tempo_from_onsets(envelopes, bpm_range, *, calibrated=True):
    frame_rate = SR / TEMPO_HOP
    count = envelopes.shape[1]
    size = fft.next_fast_len(count * 2)
    transform = fft.rfft(envelopes, n=size, axis=1, workers=1)
    acfs = fft.irfft(np.abs(transform) ** 2, n=size, axis=1, workers=1)[:, :count]
    acfs /= np.maximum(np.arange(count, 0, -1), 1)[None, :]
    acfs /= np.maximum(acfs[:, :1], 1e-9)
    acfs = ndimage.gaussian_filter1d(acfs, 1.3, axis=1)
    grid = np.arange(max(30., bpm_range[0] / 2), min(360., bpm_range[1] * 2) + .05, .1)
    lags = 60 * frame_rate / grid
    band_scores = np.zeros((4, len(grid)))
    # A four-tooth comb rewards stable periodic pulse across several beats.
    for multiple, weight in ((1, 1.), (2, .65), (3, .4), (4, .25)):
        # Cubic interpolation avoids snapping a 150 BPM beat to integer lag 34
        # (152 BPM at this hop length); linear interpolation cannot fix that.
        for band in range(4):
            band_scores[band] += weight * ndimage.map_coordinates(acfs[band], [lags * multiple], order=3, mode="constant", cval=0.)
    band_scores /= 2.3
    periodicity = band_scores.copy()
    # Autocorrelation alone rewards long periods and triplet-level aliases.
    # Phase-coherent pulse energy supplies independent support for the faster
    # level (notably DnB), without a hard-coded genre or 160+ BPM preference.
    pulse = np.abs(transform) / np.maximum(envelopes.sum(axis=1, keepdims=True), 1e-8)
    for band in range(4):
        band_scores[band] += .5 * ndimage.map_coordinates(pulse[band], [grid / 60 * size / frame_rate], order=1, mode="constant", cval=0.)
    prior = np.exp(-.5 * (np.log2(grid / 120) / .65) ** 2)
    band_scores *= .84 + .16 * prior
    # Low-band pulse evidence keeps regular hi-hat subdivisions from becoming
    # the beat. Broadband evidence breaks triplet aliases, which otherwise make
    # e.g. a 140 beat appear as 93.3, or a 110 beat as 146.7.
    scores = band_scores[1].copy()
    eligible = (grid >= bpm_range[0] - .001) & (grid <= bpm_range[1] + .001)
    best = np.flatnonzero(eligible)[np.argmax(scores[eligible])]
    broad_scores = np.average(band_scores, axis=0, weights=(.5, .25, 0, .25))
    broad = np.flatnonzero(eligible)[np.argmax(broad_scores[eligible])]
    ratio = grid[best] / grid[broad]
    if max(scores[eligible]) < .12 or any(abs(ratio / r - 1) < .015 for r in (2 / 3, 3 / 2, 3 / 4, 4 / 3)):
        scores = broad_scores
        best = broad
    # A pulse train has equally good ACF peaks at every multiple of its period.
    # Prefer its shortest supported beat period when the double is nearly tied;
    # do not do this for a genuine half-time groove with weak intervening beats.
    double = int(np.argmin(np.abs(grid - grid[best] * 2)))
    if eligible[double] and abs(grid[double] - grid[best] * 2) < .15 and scores[double] >= .94 * scores[best]:
        best = double
    peaks = signal.find_peaks(scores)[0].tolist() + [best]
    # Always expose the chosen pulse first, then distinct alternate peaks.
    choices = [best]
    for index in sorted(set(peaks), key=lambda n: scores[n], reverse=True):
        if all(abs(math.log2(grid[index] / grid[n])) > .045 for n in choices):
            choices.append(index)
        if len(choices) == 3:
            break
    top = max(float(scores[best]), 1e-8)
    candidates = [{"bpm": round(float(grid[n]), 1), "score": round(float(np.clip(scores[n] / max(top, float(scores.max()), 1e-8), 0, 1)), 3)} for n in choices]
    runner = max((scores[n] for n in choices[1:] if bpm_range[0] <= grid[n] <= bpm_range[1]), default=0.)
    margin = max(0, (top - runner) / top)
    refined, variation, coverage, jitter = _beat_refine(envelopes, float(grid[best]))
    confidence = float(np.clip(top * 1.6, 0, 1) * (.35 + .65 * min(1, margin * 5)))
    confidence *= min(1., coverage / .65) * max(.15, 1 - jitter * 2)
    confidence *= max(.2, 1 - variation / 2)
    if abs(math.log2(grid[best] / grid[broad])) > .04:
        confidence = min(confidence, .45)
    for factor in (.5, 2):
        alternate = int(np.argmin(np.abs(grid - grid[best] * factor)))
        if eligible[alternate] and abs(grid[alternate] / grid[best] - factor) < .005:
            # A phase-locked subdivision does not prove its metrical level.
            # Keep this ambiguity visible even when the added Fourier evidence
            # gives one octave a large numerical margin.
            if periodicity[1, alternate] >= .82 * periodicity[1, best]:
                confidence = min(confidence, .55)
    if float(envelopes.max()) < 1e-5:
        confidence = 0.
    # Only snap a stable grid. Rubato and live performances keep their decimal.
    if variation < .15 and abs(refined - round(refined)) < .08:
        refined = float(round(refined))
    refined = float(np.clip(refined, *bpm_range))
    candidates[0]["bpm"] = round(refined, 1)
    return round(refined, 1), _calibrate("bpm", confidence) if calibrated else round(confidence, 3), candidates


def _beat_refine(envelopes, bpm):
    """Robust beat-grid regression in local passages, without a Python DP loop.

    Peak-to-grid assignment allows missing beats and swung subdivisions. Local
    slopes refine ACF lag quantization, while their spread measures rubato or
    tempo changes instead of forcing one precise global metronome onto them.
    """
    rate = SR / TEMPO_HOP
    period = 60 * rate / bpm
    onset = np.average(envelopes, axis=0, weights=(.5, .3, .1, .1))
    width = min(len(onset), round(24 * rate))
    starts = np.arange(0, max(1, len(onset) - width // 2), width)
    estimates, coverages, jitters = [], [], []
    for start in starts:
        part = onset[start:start + width]
        if len(part) < 4 * period:
            continue
        peaks = signal.find_peaks(part, distance=max(1, int(.35 * period)), prominence=.25)[0]
        if len(peaks) < 5:
            continue
        weights = part[peaks]
        # Parabolic interpolation of onset times, with finite edge protection.
        delta = .5 * (part[peaks - 1] - part[peaks + 1]) / np.minimum(part[peaks - 1] - 2 * part[peaks] + part[peaks + 1], -1e-8)
        times = peaks + np.clip(delta, -.5, .5)
        phases = times % period / period
        histogram = np.bincount(np.floor(phases * 48).astype(int), weights=weights, minlength=48)
        histogram = ndimage.gaussian_filter1d(histogram, 1.5, mode="wrap")
        offset = (np.argmax(histogram) + .5) / 48 * period
        beats = np.round((times - offset) / period)
        residual = times - (offset + beats * period)
        keep = np.abs(residual) < .18 * period
        if keep.sum() < 5:
            continue
        x, t, w = beats[keep], times[keep], weights[keep]
        slope, intercept = np.polyfit(x, t, 1, w=np.sqrt(w))
        residual = t - (intercept + slope * x)
        if abs(slope / period - 1) > .04:
            continue
        estimates.append(60 * rate / slope)
        coverages.append(min(1., len(np.unique(x)) / max(1, len(part) / period)))
        jitters.append(float(np.sqrt(np.average(residual ** 2, weights=w)) / period))
    if not estimates:
        return bpm, 3., 0., .5
    center = float(np.median(estimates))
    variation = float(np.quantile(np.abs(np.array(estimates) - center), .8))
    return center, variation, float(np.median(coverages)), float(np.median(jitters))


# Published Krumhansl-Kessler, Temperley, and Shaath profiles, major/minor.
_PROFILES = (
    ((6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88),
     (6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17)),
    ((5., 2., 3.5, 2., 4.5, 4., 2., 4.5, 2., 3.5, 1.5, 4.),
     (5., 2., 3.5, 4.5, 2., 4., 2., 4.5, 3.5, 2., 1.5, 4.)),
    ((6.6, 2., 3.5, 2.3, 4.6, 4., 2.5, 5.2, 2.4, 3.7, 2.3, 3.4),
     (6.5, 2.7, 3.5, 5.4, 2.6, 3.5, 2.5, 5.2, 4., 2.7, 4.3, 3.2)),
    # Faraldo et al. (2017) median EDM profiles, published numerical data.
    # Balanced major/minor profiles; the minor-only EDMM family is unsuitable
    # for a downloader whose repertoire also includes pop and rock.
    ((1., .1573, .42, .157, .5296, .3669, .1632, .7711, .1676, .3827, .2113, .2965),
     (1., .233, .3615, .3905, .2925, .3777, .1961, .7425, .2701, .2161, .4228, .2272)),
)


def _fundamental_chroma(spec, tuning):
    """Nonnegative harmonic-template fit suppresses octave/third overtones.

    Sparse multiplicative least-squares updates keep this small and predictable;
    no learned model or external NNLS implementation is needed.
    """
    # Include sub-bass fundamentals: omitting F1, for example, assigns its fifth
    # harmonic to A and can turn an F-minor recording into a confident F major.
    midi = np.arange(24, 96)
    bins = np.arange(spec.shape[0])
    templates = np.zeros((spec.shape[0], len(midi)), dtype=np.float32)
    for i, note in enumerate(midi):
        frequency = 440 * 2 ** ((note - 69 + tuning) / 12)
        for partial in range(1, 13):
            center = frequency * partial * 4096 / (SR / 2)
            if center >= spec.shape[0] - 2:
                break
            # Hann main-lobe approximation at the precisely tuned frequency.
            distance = np.abs(bins - center)
            templates[:, i] += np.where(distance < 2, .5 + .5 * np.cos(np.minimum(distance, 2) * np.pi / 2), 0) / partial ** 1.25
    templates /= np.maximum(np.linalg.norm(templates, axis=0), 1e-8)
    basis = sparse.csr_matrix(templates)
    gram = (basis.T @ basis).tocsr()
    target = np.maximum(basis.T @ spec, 1e-9)
    activation = target.copy()
    for _ in range(18):
        activation *= target / np.maximum(gram @ activation + .015 * target.max(axis=0), 1e-9)
    chroma = np.zeros((12, spec.shape[1]))
    bass = np.zeros_like(chroma)
    for i, note in enumerate(midi):
        chroma[note % 12] += activation[i]
        if note < 60:
            bass[note % 12] += activation[i]
    chroma /= np.maximum(np.linalg.norm(chroma, axis=0), 1e-8)
    bass /= np.maximum(np.linalg.norm(bass, axis=0), 1e-8)
    return chroma, bass


def _chroma(y):
    # Sample three distributed 24-second passages for bounded key-analysis cost.
    width = min(len(y), 24 * SR)
    starts = np.unique(np.linspace(0, max(0, len(y) - width), 3).astype(int))
    chromas, basses, tuning_votes = [], [], []
    for start in starts:
        # Resample with anti-aliasing, rather than aliasing high percussion down.
        reduced = signal.resample_poly(y[start:start + width], 1, 2)
        spec = _stft(reduced, 4096, 1024)
        harmonic = ndimage.median_filter(spec, size=(1, 13), mode="nearest")
        percussive = ndimage.median_filter(spec, size=(13, 1), mode="nearest")
        spec *= harmonic ** 2 / np.maximum(harmonic ** 2 + percussive ** 2, 1e-12)
        # HPCP: interpolate only local sinusoidal peaks, reducing leakage/noise.
        rows, columns = np.nonzero((spec[1:-1] > spec[:-2]) & (spec[1:-1] >= spec[2:]))
        rows += 1
        valid = (rows * (SR / 2 / 4096) >= 55) & (rows * (SR / 2 / 4096) <= 4500)
        rows, columns = rows[valid], columns[valid]
        left = np.log(np.maximum(spec[rows - 1, columns], 1e-10))
        middle = np.log(np.maximum(spec[rows, columns], 1e-10))
        right = np.log(np.maximum(spec[rows + 1, columns], 1e-10))
        delta = np.clip(.5 * (left - right) / np.minimum(left - 2 * middle + right, -1e-8), -.5, .5)
        frequencies = (rows + delta) * (SR / 2 / 4096)
        midi = 69 + 12 * np.log2(frequencies / 440)
        # Partial spectral whitening preserves salience without bass domination.
        envelope = ndimage.uniform_filter1d(spec.mean(axis=1), size=31)
        weights = spec[rows, columns] / np.maximum(envelope[rows], .01 * envelope.max()) ** .55
        weights = np.maximum(weights, 0) ** .8
        residual = midi - np.round(midi)
        histogram, edges = np.histogram(residual, bins=100, range=(-.5, .5), weights=weights)
        histogram = ndimage.gaussian_filter1d(histogram, 2, mode="wrap")
        tuning = float((edges[np.argmax(histogram)] + edges[np.argmax(histogram) + 1]) / 2)
        tuning_votes.append(tuning)
        pitches = midi - tuning
        nearest = np.round(pitches).astype(int)
        weights *= np.cos(np.clip(pitches - nearest, -.5, .5) * np.pi) ** 2
        chroma = np.zeros((12, spec.shape[1]))
        bass = np.zeros_like(chroma)
        np.add.at(chroma, (nearest % 12, columns), weights)
        low = frequencies < 250
        np.add.at(bass, (nearest[low] % 12, columns[low]), weights[low])
        # Per-frame normalization prevents loud sections overwhelming the song.
        chroma /= np.maximum(np.linalg.norm(chroma, axis=0), 1e-8)
        bass /= np.maximum(np.linalg.norm(bass, axis=0), 1e-8)
        fundamental, fundamental_bass = _fundamental_chroma(spec, tuning)
        chroma = .3 * chroma + .7 * fundamental
        bass = .3 * bass + .7 * fundamental_bass
        chromas.append(chroma)
        basses.append(bass)
    return np.concatenate(chromas, axis=1), np.concatenate(basses, axis=1), float(np.median(tuning_votes))


def _key(y):
    chroma, bass, _ = _chroma(y)
    return _key_from_chroma(chroma, bass)


def _key_from_chroma(chroma, bass, *, calibrated=True):
    vector = chroma.mean(axis=1)
    centered = vector - vector.mean()
    norm = np.linalg.norm(centered)
    if norm < 1e-8:
        return key_names(0, "major"), 0.
    scores = np.zeros(24)
    profile_votes = []
    for profiles, weight in zip(_PROFILES, (0., 0., 0., 1.)):
        correlations = []
        for profile in profiles:
            p = np.asarray(profile)
            p = (p - p.mean()) / np.linalg.norm(p - p.mean())
            correlations.extend(float(centered @ np.roll(p, tonic) / norm) for tonic in range(12))
        scores += weight * np.array(correlations)
        profile_votes.append(int(np.argmax(correlations)))
    # Bass-root evidence helps separate relative keys with identical scales.
    low = bass.mean(axis=1)
    low = (low - low.mean()) / max(float(np.linalg.norm(low - low.mean())), 1e-8)
    scores += .045 * np.tile(low, 2)
    # Dorian and Mixolydian retain their tonic and map to the API's minor/major
    # modes. Require bass-root and third evidence before preferring a modal
    # interpretation over a relative major/minor with the same scale notes.
    modal = np.array(_PROFILES[-1], dtype=float)
    modal[0, 10], modal[0, 11] = modal[0, 11], modal[0, 10]
    modal[1, 8], modal[1, 9] = modal[1, 9], modal[1, 8]
    modal_scores = _key_matrix(modal) @ (centered / norm) + .045 * np.tile(low, 2)
    modal_index = int(np.argmax(modal_scores))
    root = modal_index % 12
    raw_bass = bass.mean(axis=1)
    modal_used = (modal_scores[modal_index] > scores.max() + .04 and
                  raw_bass[root] >= .8 * raw_bass.max() and
                  vector[(root + (4 if modal_index < 12 else 3)) % 12] >= .25 * vector.max())
    if modal_used:
        scores[modal_index] = modal_scores[modal_index]
    order = np.argsort(scores)[::-1]
    index = int(order[0])
    margin = float(scores[index] - scores[order[1]])
    agreement = profile_votes.count(index) / len(profile_votes)
    confidence = np.clip(margin * 5, 0, 1) * .7 + .3 * agreement * np.clip(scores[index], 0, 1)
    # Assess local keys, rather than letting a strong global mean hide changes.
    # Eight-second windows span several chords but retain modulations; a single
    # global key remains mandatory in the public API, so uncertainty is reduced.
    local_agreement = 1.
    windows = [chroma[:, start:start + 86].mean(axis=1) for start in range(0, chroma.shape[1] - 42, 86)]
    if windows:
        matrix = _key_matrix(_PROFILES[-1])
        local = np.asarray(windows)
        local -= local.mean(axis=1, keepdims=True)
        local /= np.maximum(np.linalg.norm(local, axis=1, keepdims=True), 1e-8)
        local_scores = local @ matrix.T
        votes = np.argmax(local_scores, axis=1)
        agreement = float(np.mean(votes == index))
        local_agreement = agreement
        confidence *= .4 + .6 * agreement
        # The global vector may straddle two unrelated keys. Report the locally
        # dominant key when it has an actual majority and the global one does not.
        counts = np.bincount(votes, minlength=24)
        dominant = int(np.argmax(counts))
        if counts[dominant] >= .6 * len(votes) and counts[index] < .3 * len(votes):
            index = dominant
            confidence = min(float(confidence), .5)
    # A drone or power chord cannot establish major/minor. Profile correlation
    # margins alone can be large even when the distinguishing third is absent.
    tonic = index % 12
    # A strong root plus a clear third is stronger mode evidence than a scale
    # profile's preference for its seventh (common in Mixolydian rock/blues).
    major_third, minor_third = vector[(tonic + 4) % 12], vector[(tonic + 3) % 12]
    third_switch = False
    if bass.mean(axis=1)[tonic] >= .75 * bass.mean(axis=1).max():
        if index >= 12 and major_third > 1.5 * minor_third:
            index -= 12
            third_switch = True
        elif index < 12 and minor_third > 1.5 * major_third:
            index += 12
            third_switch = True
    triad_third = (tonic + (4 if index < 12 else 3)) % 12
    competing_third = (tonic + (3 if index < 12 else 4)) % 12
    third_evidence = vector[triad_third] / max(float(vector.max()), 1e-8)
    mode_ambiguous = third_evidence < .18 or vector[competing_third] > .9 * vector[triad_third]
    if mode_ambiguous:
        confidence = min(float(confidence), .45)
    if modal_used:
        confidence = min(float(confidence), .5)
    if not calibrated:
        return key_names(index % 12, "major" if index < 12 else "minor"), round(float(confidence), 3)
    calibrated = _calibrate("key", float(confidence))
    calibrated *= .5 + .5 * local_agreement
    if modal_used or mode_ambiguous or third_switch:
        calibrated = min(calibrated, .55)
    return key_names(index % 12, "major" if index < 12 else "minor"), round(float(calibrated), 3)


def _key_matrix(profiles):
    return np.asarray([np.roll((np.asarray(p) - np.mean(p)) / np.linalg.norm(np.asarray(p) - np.mean(p)), tonic)
                       for p in profiles for tonic in range(12)])


def analyze(path, *, max_seconds=None, bpm_range=(70, 180)):
    try:
        low, high = (float(n) for n in bpm_range)
        limit = 180. if max_seconds is None else float(max_seconds)
    except (TypeError, ValueError) as exc:
        raise ValueError("Expected a numeric max_seconds and two-value bpm_range") from exc
    if not all(math.isfinite(n) for n in (low, high, limit)) or not 30 <= low < high <= 360:
        raise ValueError("bpm_range must satisfy 30 <= low < high <= 360; max_seconds must be finite")
    if not 2 <= limit <= 3600:
        raise ValueError("max_seconds must be between 2 and 3600")
    y = _decode(path, limit)
    bpm, confidence, candidates = _tempo(y, (low, high))
    key, key_confidence = _key(y)
    return {"bpm": bpm, "bpmConfidence": confidence, "bpmCandidates": candidates,
            **key, "keyConfidence": key_confidence,
            "durationAnalyzed": round(len(y) / SR, 3), "engine": "siphon-analysis/1"}
