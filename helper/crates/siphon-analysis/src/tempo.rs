//! FFT onset periodicity with a beat-grid refinement (port of `engine._tempo`).

use realfft::RealFftPlanner;

use crate::confidence::calibrate;
use crate::dsp::{self, Mode};
use crate::{BpmCandidate, SR};

pub const TEMPO_HOP: usize = 128;
const FRAME_RATE: f64 = SR as f64 / TEMPO_HOP as f64;

fn mel_bank() -> Vec<Vec<(usize, f64)>> {
    let frequencies: Vec<f64> = (0..=512).map(|k| k as f64 * SR as f64 / 1024.0).collect();
    let (low, high) = ((35.0f64 / 700.0).ln_1p(), (10000.0f64 / 700.0).ln_1p());
    let points: Vec<f64> = (0..50).map(|i| 700.0 * (low + (high - low) * i as f64 / 49.0).exp_m1()).collect();
    (0..48)
        .map(|i| {
            let row: Vec<f64> = frequencies
                .iter()
                .map(|f| ((f - points[i]) / (points[i + 1] - points[i])).min((points[i + 2] - f) / (points[i + 2] - points[i + 1])).max(0.0))
                .collect();
            let total = row.iter().sum::<f64>().max(1e-8);
            row.iter().enumerate().filter(|(_, v)| **v > 0.0).map(|(k, v)| (k, v / total)).collect()
        })
        .collect()
}

/// Four normalized onset envelopes: broadband, low, mid and high mel bands.
pub fn onsets(y: &[f64], planner: &mut RealFftPlanner<f64>) -> Vec<Vec<f64>> {
    let spectrum = dsp::stft_magnitude(y, 1024, TEMPO_HOP, planner);
    let frames = spectrum[0].len();
    let bank = mel_bank();
    let mut bands: Vec<Vec<f64>> = bank
        .iter()
        .map(|row| (0..frames).map(|t| row.iter().map(|&(k, w)| w * spectrum[k][t]).sum()).collect())
        .collect();
    for band in bands.iter_mut() {
        let scale = dsp::mean(band).max(1e-5);
        band.iter_mut().for_each(|v| *v = (*v / scale * 10.0).ln_1p());
    }
    // Frequency maximum suppresses vibrato-induced flux (SuperFlux principle).
    let count = frames.saturating_sub(1);
    let mut flux = vec![vec![0.0; count]; 48];
    for b in 0..48usize {
        let (lo, hi) = (b.saturating_sub(1), (b + 1).min(47));
        for t in 0..count {
            let previous = (lo..=hi).map(|k| bands[k][t]).fold(f64::MIN, f64::max);
            flux[b][t] = (bands[b][t + 1] - previous).max(0.0);
        }
    }
    let average = |rows: std::ops::Range<usize>| -> Vec<f64> {
        let size = rows.len() as f64;
        (0..count).map(|t| rows.clone().map(|b| flux[b][t]).sum::<f64>() / size).collect()
    };
    let mut envelopes = vec![average(0..48), average(0..16), average(16..32), average(32..48)];
    let smooth = (0.5 * FRAME_RATE).round() as usize;
    for envelope in envelopes.iter_mut() {
        let trend = dsp::uniform_filter1d(envelope, smooth, Mode::Reflect);
        for (v, t) in envelope.iter_mut().zip(trend) {
            *v = (*v - t).max(0.0);
        }
        let mean = dsp::mean(envelope);
        let std = (envelope.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / envelope.len().max(1) as f64).sqrt();
        let std = std.max(1e-6);
        envelope.iter_mut().for_each(|v| *v /= std);
    }
    envelopes
}

pub struct Tempo {
    pub bpm: f64,
    pub confidence: f64,
    pub candidates: Vec<BpmCandidate>,
}

pub fn tempo_from_onsets(envelopes: &[Vec<f64>], bpm_range: (f64, f64), planner: &mut RealFftPlanner<f64>) -> Tempo {
    let count = envelopes[0].len();
    let size = dsp::next_fast_len(count * 2);
    let transforms: Vec<_> = envelopes.iter().map(|e| dsp::rfft_padded(e, size, planner)).collect();
    let acfs: Vec<Vec<f64>> = transforms
        .iter()
        .map(|transform| {
            let power: Vec<_> = transform.iter().map(|c| realfft::num_complex::Complex::new(c.norm_sqr(), 0.0)).collect();
            let mut acf = dsp::irfft(&power, size, planner);
            acf.truncate(count);
            for (k, v) in acf.iter_mut().enumerate() {
                *v /= (count - k).max(1) as f64;
            }
            let zero = acf[0].max(1e-9);
            acf.iter_mut().for_each(|v| *v /= zero);
            dsp::cubic_spline_coefficients(&dsp::gaussian_filter1d(&acf, 1.3, Mode::Reflect))
        })
        .collect();

    let start = (bpm_range.0 / 2.0).max(30.0);
    let stop = (bpm_range.1 * 2.0).min(360.0) + 0.05;
    let points = ((stop - start) / 0.1).ceil() as usize;
    let grid: Vec<f64> = (0..points).map(|i| start + i as f64 * 0.1).collect();
    let lags: Vec<f64> = grid.iter().map(|g| 60.0 * FRAME_RATE / g).collect();

    // A four-tooth comb rewards stable periodic pulse across several beats.
    let mut band_scores = vec![vec![0.0; grid.len()]; 4];
    for (multiple, weight) in [(1.0, 1.0), (2.0, 0.65), (3.0, 0.4), (4.0, 0.25)] {
        for band in 0..4 {
            for (i, lag) in lags.iter().enumerate() {
                band_scores[band][i] += weight * dsp::cubic_spline_at(&acfs[band], lag * multiple);
            }
        }
    }
    for row in band_scores.iter_mut() {
        row.iter_mut().for_each(|v| *v /= 2.3);
    }
    let periodicity = band_scores.clone();
    // Phase-coherent pulse energy supports the faster metrical level (e.g. DnB).
    for band in 0..4 {
        let total = envelopes[band].iter().sum::<f64>().max(1e-8);
        let pulse: Vec<f64> = transforms[band].iter().map(|c| c.norm() / total).collect();
        for (i, g) in grid.iter().enumerate() {
            band_scores[band][i] += 0.5 * dsp::linear_at(&pulse, g / 60.0 * size as f64 / FRAME_RATE);
        }
    }
    for (i, g) in grid.iter().enumerate() {
        let prior = (-0.5 * ((g / 120.0).log2() / 0.65).powi(2)).exp();
        for row in band_scores.iter_mut() {
            row[i] *= 0.84 + 0.16 * prior;
        }
    }
    let eligible: Vec<bool> = grid.iter().map(|g| *g >= bpm_range.0 - 0.001 && *g <= bpm_range.1 + 0.001).collect();
    let best_of = |scores: &[f64]| -> usize {
        let mut best = None;
        for (i, ok) in eligible.iter().enumerate() {
            if *ok && best.is_none_or(|b: usize| scores[i] > scores[b]) {
                best = Some(i);
            }
        }
        best.unwrap_or(0)
    };
    let mut scores = band_scores[1].clone();
    let mut best = best_of(&scores);
    let broad_scores: Vec<f64> =
        (0..grid.len()).map(|i| (0.5 * band_scores[0][i] + 0.25 * band_scores[1][i] + 0.25 * band_scores[3][i]) / 1.0).collect();
    let broad = best_of(&broad_scores);
    let ratio = grid[best] / grid[broad];
    let eligible_max = scores.iter().zip(&eligible).filter(|(_, e)| **e).map(|(s, _)| *s).fold(f64::MIN, f64::max);
    if eligible_max < 0.12 || [2.0 / 3.0, 1.5, 0.75, 4.0 / 3.0].iter().any(|r| (ratio / r - 1.0).abs() < 0.015) {
        scores = broad_scores;
        best = broad;
    }
    // Prefer the shortest supported beat period when its double is nearly tied.
    let nearest = |target: f64| -> usize {
        let mut index = 0;
        for (i, g) in grid.iter().enumerate() {
            if (g - target).abs() < (grid[index] - target).abs() {
                index = i;
            }
        }
        index
    };
    let double = nearest(grid[best] * 2.0);
    if eligible[double] && (grid[double] - grid[best] * 2.0).abs() < 0.15 && scores[double] >= 0.94 * scores[best] {
        best = double;
    }
    let mut peaks = dsp::local_maxima(&scores);
    peaks.push(best);
    peaks.sort_unstable();
    peaks.dedup();
    peaks.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let mut choices = vec![best];
    for index in peaks {
        if choices.iter().all(|&n| (grid[index] / grid[n]).log2().abs() > 0.045) {
            choices.push(index);
        }
        if choices.len() == 3 {
            break;
        }
    }
    let top = scores[best].max(1e-8);
    let scores_max = scores.iter().copied().fold(f64::MIN, f64::max);
    let mut candidates: Vec<BpmCandidate> = choices
        .iter()
        .map(|&n| BpmCandidate {
            bpm: dsp::round_to(grid[n], 1),
            score: dsp::round_to((scores[n] / top.max(scores_max).max(1e-8)).clamp(0.0, 1.0), 3),
        })
        .collect();
    let runner = choices[1..]
        .iter()
        .filter(|&&n| bpm_range.0 <= grid[n] && grid[n] <= bpm_range.1)
        .map(|&n| scores[n])
        .fold(0.0, f64::max);
    let margin = ((top - runner) / top).max(0.0);
    let (refined, variation, coverage, jitter) = beat_refine(envelopes, grid[best]);
    let mut confidence = (top * 1.6).clamp(0.0, 1.0) * (0.35 + 0.65 * (margin * 5.0).min(1.0));
    confidence *= (coverage / 0.65).min(1.0) * (1.0 - jitter * 2.0).max(0.15);
    confidence *= (1.0 - variation / 2.0).max(0.2);
    if (grid[best] / grid[broad]).log2().abs() > 0.04 {
        confidence = confidence.min(0.45);
    }
    for factor in [0.5, 2.0] {
        let alternate = nearest(grid[best] * factor);
        // A phase-locked subdivision does not prove its metrical level.
        if eligible[alternate]
            && (grid[alternate] / grid[best] - factor).abs() < 0.005
            && periodicity[1][alternate] >= 0.82 * periodicity[1][best]
        {
            confidence = confidence.min(0.55);
        }
    }
    let envelope_max = envelopes.iter().flatten().copied().fold(f64::MIN, f64::max);
    if envelope_max < 1e-5 {
        confidence = 0.0;
    }
    // Only snap a stable grid. Rubato and live performances keep their decimal.
    let mut refined = refined;
    if variation < 0.15 && (refined - refined.round()).abs() < 0.08 {
        refined = refined.round();
    }
    let refined = refined.clamp(bpm_range.0, bpm_range.1);
    candidates[0].bpm = dsp::round_to(refined, 1);
    Tempo { bpm: dsp::round_to(refined, 1), confidence: calibrate(crate::confidence::Kind::Bpm, confidence), candidates }
}

/// Robust beat-grid regression in local passages. Returns (bpm, variation, coverage, jitter).
fn beat_refine(envelopes: &[Vec<f64>], bpm: f64) -> (f64, f64, f64, f64) {
    let period = 60.0 * FRAME_RATE / bpm;
    let weights = [0.5, 0.3, 0.1, 0.1];
    let onset: Vec<f64> = (0..envelopes[0].len()).map(|t| (0..4).map(|b| weights[b] * envelopes[b][t]).sum::<f64>()).collect();
    let width = onset.len().min((24.0 * FRAME_RATE).round() as usize);
    let (mut estimates, mut coverages, mut jitters) = (Vec::new(), Vec::new(), Vec::new());
    let end = (onset.len() as isize - (width / 2) as isize).max(1) as usize;
    let mut start = 0;
    while start < end {
        let part = &onset[start..(start + width).min(onset.len())];
        start += width.max(1);
        if (part.len() as f64) < 4.0 * period {
            continue;
        }
        let peaks = dsp::find_peaks(part, Some(((0.35 * period) as usize).max(1) as f64), Some(0.25));
        if peaks.len() < 5 {
            continue;
        }
        let w: Vec<f64> = peaks.iter().map(|&p| part[p]).collect();
        let times: Vec<f64> = peaks
            .iter()
            .map(|&p| {
                let (l, m, r) = (part[p - 1], part[p], part[p + 1]);
                let delta = 0.5 * (l - r) / (l - 2.0 * m + r).min(-1e-8);
                p as f64 + delta.clamp(-0.5, 0.5)
            })
            .collect();
        let mut histogram = vec![0.0; 48];
        for (t, weight) in times.iter().zip(&w) {
            let phase = t.rem_euclid(period) / period;
            histogram[((phase * 48.0).floor() as usize).min(47)] += weight;
        }
        let histogram = dsp::gaussian_filter1d(&histogram, 1.5, Mode::Wrap);
        let offset = (dsp::argmax(&histogram) as f64 + 0.5) / 48.0 * period;
        let beats: Vec<f64> = times.iter().map(|t| dsp::round_even((t - offset) / period)).collect();
        let keep: Vec<usize> =
            (0..times.len()).filter(|&i| (times[i] - (offset + beats[i] * period)).abs() < 0.18 * period).collect();
        if keep.len() < 5 {
            continue;
        }
        let x: Vec<f64> = keep.iter().map(|&i| beats[i]).collect();
        let t: Vec<f64> = keep.iter().map(|&i| times[i]).collect();
        let ww: Vec<f64> = keep.iter().map(|&i| w[i]).collect();
        // np.polyfit(x, t, 1, w=sqrt(w)): least squares weighted by w.
        let total: f64 = ww.iter().sum();
        let xm = x.iter().zip(&ww).map(|(a, b)| a * b).sum::<f64>() / total;
        let tm = t.iter().zip(&ww).map(|(a, b)| a * b).sum::<f64>() / total;
        let sxx: f64 = x.iter().zip(&ww).map(|(a, b)| b * (a - xm).powi(2)).sum();
        let sxt: f64 = x.iter().zip(&t).zip(&ww).map(|((a, c), b)| b * (a - xm) * (c - tm)).sum();
        if sxx <= 0.0 {
            continue;
        }
        let slope = sxt / sxx;
        let intercept = tm - slope * xm;
        if (slope / period - 1.0).abs() > 0.04 {
            continue;
        }
        let residual_power: f64 =
            x.iter().zip(&t).zip(&ww).map(|((a, c), b)| b * (c - (intercept + slope * a)).powi(2)).sum::<f64>() / total;
        estimates.push(60.0 * FRAME_RATE / slope);
        let mut unique = x.clone();
        unique.sort_by(f64::total_cmp);
        unique.dedup();
        coverages.push((unique.len() as f64 / (part.len() as f64 / period).max(1.0)).min(1.0));
        jitters.push(residual_power.sqrt() / period);
    }
    if estimates.is_empty() {
        return (bpm, 3.0, 0.0, 0.5);
    }
    let center = dsp::median(&estimates);
    let deviations: Vec<f64> = estimates.iter().map(|e| (e - center).abs()).collect();
    (center, dsp::quantile(&deviations, 0.8), dsp::median(&coverages), dsp::median(&jitters))
}
