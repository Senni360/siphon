//! Tuned harmonic pitch-class profiles and key-profile matching (port of `engine._key`).

use realfft::RealFftPlanner;

use crate::confidence::{calibrate, Kind};
use crate::dsp::{self, Mode};
use crate::notation::{key_names, KeyNames};
use crate::SR;

const N_FFT: usize = 4096;
const HOP: usize = 1024;
const RATE: f64 = SR as f64 / 2.0;

/// Published Krumhansl-Kessler, Temperley, Shaath and Faraldo (EDM) profiles, major/minor.
const PROFILES: [[[f64; 12]; 2]; 4] = [
    [
        [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88],
        [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17],
    ],
    [
        [5., 2., 3.5, 2., 4.5, 4., 2., 4.5, 2., 3.5, 1.5, 4.],
        [5., 2., 3.5, 4.5, 2., 4., 2., 4.5, 3.5, 2., 1.5, 4.],
    ],
    [
        [6.6, 2., 3.5, 2.3, 4.6, 4., 2.5, 5.2, 2.4, 3.7, 2.3, 3.4],
        [6.5, 2.7, 3.5, 5.4, 2.6, 3.5, 2.5, 5.2, 4., 2.7, 4.3, 3.2],
    ],
    [
        [1., 0.1573, 0.42, 0.157, 0.5296, 0.3669, 0.1632, 0.7711, 0.1676, 0.3827, 0.2113, 0.2965],
        [1., 0.233, 0.3615, 0.3905, 0.2925, 0.3777, 0.1961, 0.7425, 0.2701, 0.2161, 0.4228, 0.2272],
    ],
];

type Matrix = Vec<Vec<f64>>; // [row][frame]

fn normalize_columns(matrix: &mut Matrix) {
    let frames = matrix[0].len();
    for t in 0..frames {
        let norm = matrix.iter().map(|row| row[t] * row[t]).sum::<f64>().sqrt().max(1e-8);
        matrix.iter_mut().for_each(|row| row[t] /= norm);
    }
}

/// Nonnegative harmonic-template fit, suppressing octave/third overtones.
fn fundamental_chroma(spec: &Matrix, tuning: f64) -> (Matrix, Matrix) {
    let bins = spec.len();
    let frames = spec[0].len();
    let midi: Vec<i32> = (24..96).collect();
    // Sparse templates: (bin, weight) per note.
    let mut templates: Vec<Vec<(usize, f64)>> = Vec::with_capacity(midi.len());
    for &note in &midi {
        let frequency = 440.0 * 2f64.powf((note as f64 - 69.0 + tuning) / 12.0);
        let mut column = vec![0.0; bins];
        for partial in 1..13 {
            let center = frequency * partial as f64 * N_FFT as f64 / RATE;
            if center >= bins as f64 - 2.0 {
                break;
            }
            let lo = (center - 2.0).ceil().max(0.0) as usize;
            let hi = ((center + 2.0).floor() as usize).min(bins - 1);
            for (bin, slot) in column.iter_mut().enumerate().take(hi + 1).skip(lo) {
                let distance = (bin as f64 - center).abs();
                if distance < 2.0 {
                    *slot += (0.5 + 0.5 * (distance.min(2.0) * std::f64::consts::PI / 2.0).cos()) / (partial as f64).powf(1.25);
                }
            }
        }
        let norm = dsp::norm(&column).max(1e-8);
        templates.push(column.iter().enumerate().filter(|(_, v)| **v != 0.0).map(|(b, v)| (b, v / norm)).collect());
    }
    let notes = templates.len();
    let mut gram = vec![vec![0.0; notes]; notes];
    for i in 0..notes {
        for j in 0..notes {
            let (a, b) = (&templates[i], &templates[j]);
            let (mut x, mut y, mut sum) = (0, 0, 0.0);
            while x < a.len() && y < b.len() {
                match a[x].0.cmp(&b[y].0) {
                    std::cmp::Ordering::Less => x += 1,
                    std::cmp::Ordering::Greater => y += 1,
                    std::cmp::Ordering::Equal => {
                        sum += a[x].1 * b[y].1;
                        x += 1;
                        y += 1;
                    }
                }
            }
            gram[i][j] = sum;
        }
    }
    let target: Matrix = templates
        .iter()
        .map(|column| (0..frames).map(|t| column.iter().map(|&(b, w)| w * spec[b][t]).sum::<f64>().max(1e-9)).collect())
        .collect();
    let frame_max: Vec<f64> = (0..frames).map(|t| target.iter().map(|row| row[t]).fold(f64::MIN, f64::max)).collect();
    let mut activation = target.clone();
    let mut product = vec![vec![0.0; frames]; notes];
    for _ in 0..18 {
        for i in 0..notes {
            for t in 0..frames {
                product[i][t] = (0..notes).map(|j| gram[i][j] * activation[j][t]).sum();
            }
        }
        for i in 0..notes {
            for t in 0..frames {
                activation[i][t] *= target[i][t] / (product[i][t] + 0.015 * frame_max[t]).max(1e-9);
            }
        }
    }
    let mut chroma = vec![vec![0.0; frames]; 12];
    let mut bass = vec![vec![0.0; frames]; 12];
    for (i, &note) in midi.iter().enumerate() {
        let pc = (note % 12) as usize;
        for t in 0..frames {
            chroma[pc][t] += activation[i][t];
            if note < 60 {
                bass[pc][t] += activation[i][t];
            }
        }
    }
    normalize_columns(&mut chroma);
    normalize_columns(&mut bass);
    (chroma, bass)
}

fn median_filter_rows(spec: &Matrix, size: usize) -> Matrix {
    let mut scratch = Vec::with_capacity(size);
    spec.iter()
        .map(|row| {
            let mut out = vec![0.0; row.len()];
            dsp::median_filter1d(row, size, Mode::Nearest, &mut out, &mut scratch);
            out
        })
        .collect()
}

fn median_filter_columns(spec: &Matrix, size: usize) -> Matrix {
    let (bins, frames) = (spec.len(), spec[0].len());
    let mut out = vec![vec![0.0; frames]; bins];
    let mut scratch = Vec::with_capacity(size);
    let mut column = vec![0.0; bins];
    let mut filtered = vec![0.0; bins];
    for t in 0..frames {
        for b in 0..bins {
            column[b] = spec[b][t];
        }
        dsp::median_filter1d(&column, size, Mode::Nearest, &mut filtered, &mut scratch);
        for b in 0..bins {
            out[b][t] = filtered[b];
        }
    }
    out
}

/// Returns concatenated (chroma, bass) over three distributed 24-second passages.
fn chroma(y: &[f64]) -> (Matrix, Matrix) {
    let width = y.len().min(24 * SR);
    let span = y.len().saturating_sub(width);
    let mut starts: Vec<usize> = (0..3).map(|i| (span as f64 * i as f64 / 2.0) as usize).collect();
    starts.dedup();
    // The passages are independent; analyze them in parallel, concatenate in order.
    let passages: Vec<(Matrix, Matrix)> = std::thread::scope(|scope| {
        let handles: Vec<_> = starts
            .iter()
            .map(|&start| scope.spawn(move || passage(&y[start..start + width], &mut RealFftPlanner::new())))
            .collect();
        handles.into_iter().map(|h| h.join().expect("key passage thread")).collect()
    });
    let mut chromas: Matrix = vec![Vec::new(); 12];
    let mut basses: Matrix = vec![Vec::new(); 12];
    for (chroma, bass) in passages {
        for pc in 0..12 {
            chromas[pc].extend_from_slice(&chroma[pc]);
            basses[pc].extend_from_slice(&bass[pc]);
        }
    }
    (chromas, basses)
}

/// Combined HPCP and harmonic-template chroma/bass of one passage.
fn passage(segment: &[f64], planner: &mut RealFftPlanner<f64>) -> (Matrix, Matrix) {
    let resolution = RATE / N_FFT as f64;
    {
        let reduced = dsp::decimate_by_two(segment);
        let mut spec = dsp::stft_magnitude(&reduced, N_FFT, HOP, planner);
        let frames = spec[0].len();
        let harmonic = median_filter_rows(&spec, 13);
        let percussive = median_filter_columns(&spec, 13);
        for b in 0..spec.len() {
            for t in 0..frames {
                let h2 = harmonic[b][t] * harmonic[b][t];
                spec[b][t] *= h2 / (h2 + percussive[b][t] * percussive[b][t]).max(1e-12);
            }
        }
        // HPCP: interpolate only local sinusoidal peaks, reducing leakage and noise.
        let mut rows = Vec::new();
        let mut columns = Vec::new();
        for r in 1..spec.len() - 1 {
            let frequency = r as f64 * resolution;
            if !(55.0..=4500.0).contains(&frequency) {
                continue;
            }
            for t in 0..frames {
                if spec[r][t] > spec[r - 1][t] && spec[r][t] >= spec[r + 1][t] {
                    rows.push(r);
                    columns.push(t);
                }
            }
        }
        let band_mean: Vec<f64> = spec.iter().map(|row| dsp::mean(row)).collect();
        let envelope = dsp::uniform_filter1d(&band_mean, 31, Mode::Reflect);
        let envelope_max = envelope.iter().copied().fold(f64::MIN, f64::max);
        let mut frequencies = Vec::with_capacity(rows.len());
        let mut midi = Vec::with_capacity(rows.len());
        let mut weights = Vec::with_capacity(rows.len());
        for (&r, &t) in rows.iter().zip(&columns) {
            let left = spec[r - 1][t].max(1e-10).ln();
            let middle = spec[r][t].max(1e-10).ln();
            let right = spec[r + 1][t].max(1e-10).ln();
            let delta = (0.5 * (left - right) / (left - 2.0 * middle + right).min(-1e-8)).clamp(-0.5, 0.5);
            let frequency = (r as f64 + delta) * resolution;
            frequencies.push(frequency);
            midi.push(69.0 + 12.0 * (frequency / 440.0).log2());
            let whitened = spec[r][t] / envelope[r].max(0.01 * envelope_max).powf(0.55);
            weights.push(whitened.max(0.0).powf(0.8));
        }
        let residual: Vec<f64> = midi.iter().map(|m| m - dsp::round_even(*m)).collect();
        let (histogram, edges) = dsp::histogram(&residual, &weights, 100, -0.5, 0.5);
        let histogram = dsp::gaussian_filter1d(&histogram, 2.0, Mode::Wrap);
        let peak = dsp::argmax(&histogram);
        let tuning = (edges[peak] + edges[peak + 1]) / 2.0;
        let mut chroma = vec![vec![0.0; frames]; 12];
        let mut bass = vec![vec![0.0; frames]; 12];
        for i in 0..midi.len() {
            let pitch = midi[i] - tuning;
            let nearest = dsp::round_even(pitch);
            let weight = weights[i] * ((pitch - nearest).clamp(-0.5, 0.5) * std::f64::consts::PI).cos().powi(2);
            let pc = (nearest as i64).rem_euclid(12) as usize;
            chroma[pc][columns[i]] += weight;
            if frequencies[i] < 250.0 {
                bass[pc][columns[i]] += weight;
            }
        }
        normalize_columns(&mut chroma);
        normalize_columns(&mut bass);
        let (fundamental, fundamental_bass) = fundamental_chroma(&spec, tuning);
        for pc in 0..12 {
            for t in 0..frames {
                chroma[pc][t] = 0.3 * chroma[pc][t] + 0.7 * fundamental[pc][t];
                bass[pc][t] = 0.3 * bass[pc][t] + 0.7 * fundamental_bass[pc][t];
            }
        }
        (chroma, bass)
    }
}

fn standardized(profile: &[f64; 12]) -> [f64; 12] {
    let mean = profile.iter().sum::<f64>() / 12.0;
    let norm = profile.iter().map(|p| (p - mean).powi(2)).sum::<f64>().sqrt();
    let mut out = [0.0; 12];
    for i in 0..12 {
        out[i] = (profile[i] - mean) / norm;
    }
    out
}

/// `np.roll(p, tonic)`: element i moves to i + tonic.
fn rolled(p: &[f64; 12], tonic: usize) -> [f64; 12] {
    let mut out = [0.0; 12];
    for i in 0..12 {
        out[(i + tonic) % 12] = p[i];
    }
    out
}

fn key_matrix(profiles: &[[f64; 12]; 2]) -> Vec<[f64; 12]> {
    profiles.iter().flat_map(|p| (0..12).map(move |tonic| rolled(&standardized(p), tonic))).collect()
}

fn dot(a: &[f64; 12], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn row_means(matrix: &Matrix) -> Vec<f64> {
    matrix.iter().map(|row| dsp::mean(row)).collect()
}

pub fn key_from_chroma(chroma: &Matrix, bass: &Matrix) -> (KeyNames, f64) {
    let vector = row_means(chroma);
    let average = dsp::mean(&vector);
    let centered: Vec<f64> = vector.iter().map(|v| v - average).collect();
    let norm = dsp::norm(&centered);
    if norm < 1e-8 {
        return (key_names(0, false), 0.0);
    }
    let unit: Vec<f64> = centered.iter().map(|v| v / norm).collect();
    let mut scores = [0.0; 24];
    let mut profile_votes = Vec::new();
    for (profiles, weight) in PROFILES.iter().zip([0.0, 0.0, 0.0, 1.0]) {
        let correlations: Vec<f64> = key_matrix(profiles).iter().map(|row| dot(row, &centered) / norm).collect();
        for (score, c) in scores.iter_mut().zip(&correlations) {
            *score += weight * c;
        }
        profile_votes.push(dsp::argmax(&correlations));
    }
    // Bass-root evidence helps separate relative keys with identical scales.
    let low_raw = row_means(bass);
    let low_mean = dsp::mean(&low_raw);
    let low_centered: Vec<f64> = low_raw.iter().map(|v| v - low_mean).collect();
    let low_norm = dsp::norm(&low_centered).max(1e-8);
    let low: Vec<f64> = low_centered.iter().map(|v| v / low_norm).collect();
    for (i, score) in scores.iter_mut().enumerate() {
        *score += 0.045 * low[i % 12];
    }
    // Dorian and Mixolydian keep their tonic and map to minor/major modes.
    let mut modal = PROFILES[3];
    modal[0].swap(10, 11);
    modal[1].swap(8, 9);
    let modal_scores: Vec<f64> =
        key_matrix(&modal).iter().enumerate().map(|(i, row)| dot(row, &unit) + 0.045 * low[i % 12]).collect();
    let modal_index = dsp::argmax(&modal_scores);
    let root = modal_index % 12;
    let raw_bass_max = low_raw.iter().copied().fold(f64::MIN, f64::max);
    let vector_max = vector.iter().copied().fold(f64::MIN, f64::max);
    let best_score = scores.iter().copied().fold(f64::MIN, f64::max);
    let modal_used = modal_scores[modal_index] > best_score + 0.04
        && low_raw[root] >= 0.8 * raw_bass_max
        && vector[(root + if modal_index < 12 { 4 } else { 3 }) % 12] >= 0.25 * vector_max;
    if modal_used {
        scores[modal_index] = modal_scores[modal_index];
    }
    let mut order: Vec<usize> = (0..24).collect();
    order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let mut index = order[0];
    let margin = scores[index] - scores[order[1]];
    let agreement = profile_votes.iter().filter(|v| **v == index).count() as f64 / profile_votes.len() as f64;
    let mut confidence = (margin * 5.0).clamp(0.0, 1.0) * 0.7 + 0.3 * agreement * scores[index].clamp(0.0, 1.0);
    // Assess local keys, rather than letting a strong global mean hide changes.
    let mut local_agreement = 1.0;
    let frames = chroma[0].len();
    let matrix = key_matrix(&PROFILES[3]);
    let mut votes = Vec::new();
    let mut start = 0;
    while (start as isize) < frames as isize - 42 {
        let end = (start + 86).min(frames);
        let mut window: Vec<f64> = (0..12).map(|pc| chroma[pc][start..end].iter().sum::<f64>() / (end - start) as f64).collect();
        let m = dsp::mean(&window);
        window.iter_mut().for_each(|v| *v -= m);
        let n = dsp::norm(&window).max(1e-8);
        window.iter_mut().for_each(|v| *v /= n);
        let local: Vec<f64> = matrix.iter().map(|row| dot(row, &window)).collect();
        votes.push(dsp::argmax(&local));
        start += 86;
    }
    if !votes.is_empty() {
        let agreement = votes.iter().filter(|v| **v == index).count() as f64 / votes.len() as f64;
        local_agreement = agreement;
        confidence *= 0.4 + 0.6 * agreement;
        // Report the locally dominant key when it has a majority and the global one does not.
        let mut counts = [0usize; 24];
        votes.iter().for_each(|v| counts[*v] += 1);
        let dominant = (0..24).fold(0, |best, i| if counts[i] > counts[best] { i } else { best });
        if counts[dominant] as f64 >= 0.6 * votes.len() as f64 && (counts[index] as f64) < 0.3 * votes.len() as f64 {
            index = dominant;
            confidence = confidence.min(0.5);
        }
    }
    // A drone or power chord cannot establish major/minor.
    let tonic = index % 12;
    let (major_third, minor_third) = (vector[(tonic + 4) % 12], vector[(tonic + 3) % 12]);
    let mut third_switch = false;
    if low_raw[tonic] >= 0.75 * raw_bass_max {
        if index >= 12 && major_third > 1.5 * minor_third {
            index -= 12;
            third_switch = true;
        } else if index < 12 && minor_third > 1.5 * major_third {
            index += 12;
            third_switch = true;
        }
    }
    let triad_third = (tonic + if index < 12 { 4 } else { 3 }) % 12;
    let competing_third = (tonic + if index < 12 { 3 } else { 4 }) % 12;
    let third_evidence = vector[triad_third] / vector_max.max(1e-8);
    let mode_ambiguous = third_evidence < 0.18 || vector[competing_third] > 0.9 * vector[triad_third];
    if mode_ambiguous {
        confidence = confidence.min(0.45);
    }
    if modal_used {
        confidence = confidence.min(0.5);
    }
    let mut calibrated = calibrate(Kind::Key, confidence);
    calibrated *= 0.5 + 0.5 * local_agreement;
    if modal_used || mode_ambiguous || third_switch {
        calibrated = calibrated.min(0.55);
    }
    (key_names(index % 12, index >= 12), dsp::round_to(calibrated, 3))
}

pub fn key(y: &[f64]) -> (KeyNames, f64) {
    let (chroma, bass) = chroma(y);
    key_from_chroma(&chroma, &bass)
}
