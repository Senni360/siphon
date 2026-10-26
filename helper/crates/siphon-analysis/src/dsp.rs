//! Numerical building blocks with the exact edge and rounding conventions of
//! the NumPy/SciPy routines the original engine used, so estimates carry over.

use std::f64::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// Boundary handling of `scipy.ndimage` filters.
#[derive(Clone, Copy)]
pub enum Mode {
    /// `d c b a | a b c d | d c b a`
    Reflect,
    /// `a b c d | a b c d | a b c d`
    Wrap,
    /// `a a a a | a b c d | d d d d`
    Nearest,
}

#[inline]
fn extend(i: isize, n: usize, mode: Mode) -> usize {
    let n = n as isize;
    if (0..n).contains(&i) {
        return i as usize;
    }
    match mode {
        Mode::Nearest => i.clamp(0, n - 1) as usize,
        Mode::Wrap => i.rem_euclid(n) as usize,
        Mode::Reflect => {
            let period = 2 * n;
            let k = i.rem_euclid(period);
            (if k < n { k } else { period - 1 - k }) as usize
        }
    }
}

/// `scipy.ndimage.correlate1d` with origin 0 (window `i - len/2 ..`).
pub fn correlate1d(input: &[f64], weights: &[f64], mode: Mode) -> Vec<f64> {
    let n = input.len();
    let half = (weights.len() / 2) as isize;
    (0..n)
        .map(|i| {
            let base = i as isize - half;
            weights.iter().enumerate().map(|(j, w)| w * input[extend(base + j as isize, n, mode)]).sum()
        })
        .collect()
}

pub fn uniform_filter1d(input: &[f64], size: usize, mode: Mode) -> Vec<f64> {
    let n = input.len();
    if n == 0 {
        return Vec::new();
    }
    let half = (size / 2) as isize;
    let mut out = Vec::with_capacity(n);
    let mut sum: f64 = (0..size as isize).map(|j| input[extend(j - half, n, mode)]).sum();
    out.push(sum / size as f64);
    for i in 1..n as isize {
        sum += input[extend(i - half + size as isize - 1, n, mode)] - input[extend(i - half - 1, n, mode)];
        out.push(sum / size as f64);
    }
    out
}

pub fn gaussian_kernel(sigma: f64) -> Vec<f64> {
    let radius = (4.0 * sigma + 0.5) as isize;
    let mut kernel: Vec<f64> = (-radius..=radius).map(|x| (-0.5 / (sigma * sigma) * (x * x) as f64).exp()).collect();
    let total: f64 = kernel.iter().sum();
    kernel.iter_mut().for_each(|v| *v /= total);
    kernel
}

pub fn gaussian_filter1d(input: &[f64], sigma: f64, mode: Mode) -> Vec<f64> {
    correlate1d(input, &gaussian_kernel(sigma), mode)
}

/// Median of a `size`-wide centered window, `scipy.ndimage.median_filter` style.
pub fn median_filter1d(input: &[f64], size: usize, mode: Mode, out: &mut [f64], scratch: &mut Vec<f64>) {
    let n = input.len();
    let half = (size / 2) as isize;
    for i in 0..n {
        scratch.clear();
        let base = i as isize - half;
        scratch.extend((0..size as isize).map(|j| input[extend(base + j, n, mode)]));
        let middle = size / 2;
        let (_, value, _) = scratch.select_nth_unstable_by(middle, |a, b| a.total_cmp(b));
        out[i] = *value;
    }
}

/// `scipy.fft.next_fast_len(target)`: the next 11-smooth length.
pub fn next_fast_len(target: usize) -> usize {
    if target <= 6 {
        return target.max(1);
    }
    let mut n = target;
    loop {
        let mut m = n;
        for p in [2, 3, 5, 7, 11] {
            while m % p == 0 {
                m /= p;
            }
        }
        if m == 1 {
            return n;
        }
        n += 1;
    }
}

/// Periodic Hann window (`scipy.signal.windows.hann(n, sym=False)`).
pub fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()).collect()
}

/// Magnitude STFT, zero-padded by `n_fft / 2` on both sides. Returns `[bin][frame]`.
/// Long signals are split across a few threads by frame range.
pub fn stft_magnitude(y: &[f64], n_fft: usize, hop: usize, planner: &mut RealFftPlanner<f64>) -> Vec<Vec<f64>> {
    let window = hann(n_fft);
    let fft: Arc<dyn RealToComplex<f64>> = planner.plan_fft_forward(n_fft);
    let pad = n_fft / 2;
    let frames = (y.len() + 2 * pad - n_fft) / hop + 1;
    let bins = n_fft / 2 + 1;
    let threads = if frames >= 4096 { std::thread::available_parallelism().map_or(1, |n| n.get()).min(4) } else { 1 };
    let chunk = frames.div_ceil(threads);
    // Each thread fills frame-major rows for its range; transpose afterwards.
    let parts: Vec<Vec<f64>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|part| {
                let (fft, window) = (fft.clone(), &window);
                scope.spawn(move || {
                    let range = part * chunk..((part + 1) * chunk).min(frames);
                    let mut out = Vec::with_capacity(range.len() * bins);
                    let mut input = fft.make_input_vec();
                    let mut spectrum = fft.make_output_vec();
                    let mut scratch = fft.make_scratch_vec();
                    for frame in range {
                        let start = frame * hop;
                        for (k, slot) in input.iter_mut().enumerate() {
                            let index = (start + k) as isize - pad as isize;
                            let sample = if index >= 0 && (index as usize) < y.len() { y[index as usize] } else { 0.0 };
                            *slot = sample * window[k];
                        }
                        fft.process_with_scratch(&mut input, &mut spectrum, &mut scratch).expect("fft sizes match");
                        out.extend(spectrum.iter().map(|value| value.norm()));
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("stft thread")).collect()
    });
    let mut out = vec![vec![0.0; frames]; bins];
    let mut frame = 0;
    for part in parts {
        for row in part.chunks_exact(bins) {
            for (bin, value) in row.iter().enumerate() {
                out[bin][frame] = *value;
            }
            frame += 1;
        }
    }
    out
}

pub fn rfft_padded(signal: &[f64], size: usize, planner: &mut RealFftPlanner<f64>) -> Vec<Complex<f64>> {
    let fft = planner.plan_fft_forward(size);
    let mut input = vec![0.0; size];
    input[..signal.len().min(size)].copy_from_slice(&signal[..signal.len().min(size)]);
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output).expect("fft sizes match");
    output
}

/// Normalized inverse real FFT (`numpy.fft.irfft(spectrum, n=size)`).
pub fn irfft(spectrum: &[Complex<f64>], size: usize, planner: &mut RealFftPlanner<f64>) -> Vec<f64> {
    let fft = planner.plan_fft_inverse(size);
    let mut input = spectrum.to_vec();
    // The DC and Nyquist bins of a real signal's spectrum are real.
    input[0].im = 0.0;
    if size % 2 == 0 {
        if let Some(last) = input.last_mut() {
            last.im = 0.0;
        }
    }
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output).expect("fft sizes match");
    output.iter_mut().for_each(|v| *v /= size as f64);
    output
}

/// Cubic B-spline coefficients with mirror-symmetric boundaries (`spline_filter1d`, order 3).
pub fn cubic_spline_coefficients(input: &[f64]) -> Vec<f64> {
    let n = input.len();
    let mut c: Vec<f64> = input.iter().map(|v| v * 6.0).collect();
    if n < 2 {
        return input.to_vec();
    }
    let z = 3f64.sqrt() - 2.0;
    // Causal initialization over the whole mirrored signal.
    let z_n_1 = z.powi(n as i32 - 1);
    let mut z_i = z;
    let mut first = c[0] + z_n_1 * c[n - 1];
    for i in 1..n - 1 {
        first += z_i * (c[i] + z_n_1 * c[n - 1 - i]);
        z_i *= z;
    }
    c[0] = first / (1.0 - z_n_1 * z_n_1);
    for i in 1..n {
        c[i] += z * c[i - 1];
    }
    c[n - 1] = (z / (z * z - 1.0)) * (z * c[n - 2] + c[n - 1]);
    for i in (0..n - 1).rev() {
        c[i] = z * (c[i + 1] - c[i]);
    }
    c
}

/// Evaluate cubic B-spline coefficients at `x`; zero outside `[0, n-1]` (`mode="constant"`).
pub fn cubic_spline_at(coefficients: &[f64], x: f64) -> f64 {
    let n = coefficients.len();
    if n == 0 || !(x >= 0.0 && x <= (n - 1) as f64) {
        return 0.0;
    }
    let floor = x.floor();
    let t = x - floor;
    let weights = [
        (1.0 - t).powi(3) / 6.0,
        (4.0 - 6.0 * t * t + 3.0 * t * t * t) / 6.0,
        (1.0 + 3.0 * t + 3.0 * t * t - 3.0 * t * t * t) / 6.0,
        t * t * t / 6.0,
    ];
    let start = floor as isize - 1;
    weights
        .iter()
        .enumerate()
        .map(|(j, w)| w * coefficients[mirror(start + j as isize, n)])
        .sum()
}

fn mirror(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let n = n as isize;
    let period = 2 * n - 2;
    let k = i.rem_euclid(period);
    (if k < n { k } else { period - k }) as usize
}

/// Linear interpolation; zero outside `[0, n-1]` (`map_coordinates`, order 1).
pub fn linear_at(values: &[f64], x: f64) -> f64 {
    let n = values.len();
    if n == 0 || !(x >= 0.0 && x <= (n - 1) as f64) {
        return 0.0;
    }
    let i = x.floor() as usize;
    let t = x - i as f64;
    if i + 1 >= n {
        return values[n - 1];
    }
    (1.0 - t) * values[i] + t * values[i + 1]
}

/// `scipy.signal.find_peaks` local maxima, with plateau midpoints.
pub fn local_maxima(x: &[f64]) -> Vec<usize> {
    let mut peaks = Vec::new();
    if x.len() < 3 {
        return peaks;
    }
    let i_max = x.len() - 1;
    let mut i = 1;
    while i < i_max {
        if x[i - 1] < x[i] {
            let mut ahead = i + 1;
            while ahead < i_max && x[ahead] == x[i] {
                ahead += 1;
            }
            if x[ahead] < x[i] {
                peaks.push((i + ahead - 1) / 2);
                i = ahead;
            }
        }
        i += 1;
    }
    peaks
}

/// `scipy.signal.find_peaks(x, distance=..., prominence=...)`.
pub fn find_peaks(x: &[f64], distance: Option<f64>, prominence: Option<f64>) -> Vec<usize> {
    let mut peaks = local_maxima(x);
    if let Some(distance) = distance {
        let distance = distance.ceil() as usize;
        let mut order: Vec<usize> = (0..peaks.len()).collect();
        order.sort_by(|&a, &b| x[peaks[a]].total_cmp(&x[peaks[b]]));
        let mut keep = vec![true; peaks.len()];
        for &j in order.iter().rev() {
            if !keep[j] {
                continue;
            }
            let mut k = j;
            while k > 0 && peaks[j] - peaks[k - 1] < distance {
                keep[k - 1] = false;
                k -= 1;
            }
            let mut k = j + 1;
            while k < peaks.len() && peaks[k] - peaks[j] < distance {
                keep[k] = false;
                k += 1;
            }
        }
        peaks = peaks.into_iter().zip(keep).filter_map(|(p, k)| k.then_some(p)).collect();
    }
    if let Some(minimum) = prominence {
        peaks.retain(|&peak| peak_prominence(x, peak) >= minimum);
    }
    peaks
}

fn peak_prominence(x: &[f64], peak: usize) -> f64 {
    let height = x[peak];
    let mut left_min = height;
    let mut i = peak as isize;
    while i >= 0 && x[i as usize] <= height {
        left_min = left_min.min(x[i as usize]);
        i -= 1;
    }
    let mut right_min = height;
    let mut i = peak;
    while i < x.len() && x[i] <= height {
        right_min = right_min.min(x[i]);
        i += 1;
    }
    height - left_min.max(right_min)
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..200 {
        term *= half / k as f64;
        let square = term * term;
        sum += square;
        if square < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// `scipy.signal.resample_poly(x, 1, 2)`: Kaiser(5.0) FIR anti-aliasing, then decimation.
pub fn decimate_by_two(x: &[f64]) -> Vec<f64> {
    const HALF: usize = 20;
    let taps = 2 * HALF + 1;
    let alpha = (taps - 1) as f64 / 2.0;
    let beta = 5.0;
    let mut h: Vec<f64> = (0..taps)
        .map(|j| {
            let m = j as f64 - HALF as f64;
            let arg = 0.5 * m;
            let sinc = if arg == 0.0 { 1.0 } else { (PI * arg).sin() / (PI * arg) };
            let ratio = (j as f64 - alpha) / alpha;
            0.5 * sinc * bessel_i0(beta * (1.0 - ratio * ratio).max(0.0).sqrt()) / bessel_i0(beta)
        })
        .collect();
    let total: f64 = h.iter().sum();
    h.iter_mut().for_each(|v| *v /= total);
    let n_out = x.len().div_ceil(2);
    (0..n_out)
        .map(|m| {
            let center = 2 * m + HALF;
            h.iter()
                .enumerate()
                .filter_map(|(j, w)| center.checked_sub(j).and_then(|i| x.get(i)).map(|v| w * v))
                .sum()
        })
        .collect()
}

/// `numpy.histogram(values, bins, range=(low, high), weights)` for uniform bins.
pub fn histogram(values: &[f64], weights: &[f64], bins: usize, low: f64, high: f64) -> (Vec<f64>, Vec<f64>) {
    let edges: Vec<f64> = (0..=bins).map(|i| low + (high - low) * i as f64 / bins as f64).collect();
    let mut counts = vec![0.0; bins];
    let norm = bins as f64 / (high - low);
    for (&value, &weight) in values.iter().zip(weights) {
        if !(value >= low && value <= high) {
            continue;
        }
        let mut index = ((value - low) * norm) as usize;
        if index == bins {
            index -= 1;
        }
        if value < edges[index] {
            index -= 1;
        } else if index != bins - 1 && value >= edges[index + 1] {
            index += 1;
        }
        counts[index] += weight;
    }
    (counts, edges)
}

/// NumPy's round-half-to-even.
#[inline]
pub fn round_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// Python's `round(x, digits)`: correctly rounded decimal value.
pub fn round_to(x: f64, digits: usize) -> f64 {
    format!("{x:.digits$}").parse().unwrap_or(x)
}

pub fn argmax(values: &[f64]) -> usize {
    let mut best = 0;
    for (i, v) in values.iter().enumerate() {
        if *v > values[best] {
            best = i;
        }
    }
    best
}

pub fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// `numpy.quantile(values, q)` with linear interpolation.
pub fn quantile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = q * (sorted.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = (low + 1).min(sorted.len() - 1);
    let fraction = position - low as f64;
    sorted[low] + fraction * (sorted[high] - sorted[low])
}

pub fn norm(values: &[f64]) -> f64 {
    values.iter().map(|v| v * v).sum::<f64>().sqrt()
}

pub fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflect_and_wrap_indices() {
        assert_eq!(extend(-1, 4, Mode::Reflect), 0);
        assert_eq!(extend(-2, 4, Mode::Reflect), 1);
        assert_eq!(extend(4, 4, Mode::Reflect), 3);
        assert_eq!(extend(5, 4, Mode::Reflect), 2);
        assert_eq!(extend(-1, 4, Mode::Wrap), 3);
        assert_eq!(extend(9, 4, Mode::Nearest), 3);
    }

    #[test]
    fn uniform_matches_direct_correlation() {
        let x: Vec<f64> = (0..50).map(|i| ((i * 7919) % 13) as f64).collect();
        for size in [3, 4, 86] {
            let direct = correlate1d(&x, &vec![1.0 / size as f64; size], Mode::Reflect);
            for (a, b) in uniform_filter1d(&x, size, Mode::Reflect).iter().zip(direct) {
                assert!((a - b).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn spline_interpolates_samples_exactly() {
        let x: Vec<f64> = (0..40).map(|i| (i as f64 * 0.3).sin()).collect();
        let c = cubic_spline_coefficients(&x);
        for i in 2..38 {
            assert!((cubic_spline_at(&c, i as f64) - x[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn peaks_with_plateaus_distance_and_prominence() {
        let x = [0., 2., 2., 2., 0., 1., 0., 5., 0., 4.5, 0.];
        assert_eq!(local_maxima(&x), vec![2, 5, 7, 9]);
        assert_eq!(find_peaks(&x, Some(3.0), None), vec![2, 7]);
        assert_eq!(find_peaks(&x, None, Some(1.5)), vec![2, 7, 9]);
    }

    #[test]
    fn decimation_preserves_dc_and_length() {
        let y = decimate_by_two(&vec![1.0; 101]);
        assert_eq!(y.len(), 51);
        assert!((y[25] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn fast_lengths_and_rounding() {
        assert_eq!(next_fast_len(13), 14);
        assert_eq!(next_fast_len(17), 18);
        assert_eq!(round_even(2.5), 2.0);
        assert_eq!(round_to(0.1235, 3), 0.123);
        assert_eq!(round_to(120.25, 1), 120.2);
    }
}
