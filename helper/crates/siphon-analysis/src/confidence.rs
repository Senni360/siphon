//! Conservative empirical confidence mapping; provenance in host/bench.
//!
//! Scores express exact BPM (within one beat/minute) and exact major/minor key,
//! never octave-equivalent tempo or harmonically related key correctness.

#[derive(Clone, Copy)]
pub enum Kind {
    Bpm,
    Key,
}

pub fn calibrate(kind: Kind, quality: f64) -> f64 {
    let quality = quality.clamp(0.0, 1.0);
    // PAVA + one-sided 90% Wilson bounds, from 54 development recordings (47 explicit
    // keys); see calibration.json. Taper the near-zero region for silence/no tonality.
    let value = match kind {
        Kind::Bpm => (if quality < 0.6 { 0.576 } else { 0.931 }) * (quality / 0.2).min(1.0),
        Kind::Key => (if quality < 0.2 { 0.36 } else if quality < 0.8 { 0.682 } else { 0.81 }) * (quality / 0.1).min(1.0),
    };
    crate::dsp::round_to(value, 3)
}
