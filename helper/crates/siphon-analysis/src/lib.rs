//! Siphon's local tempo and musical-key analysis.
//!
//! A port of the NumPy/SciPy engine (FFT onset periodicity with beat-grid
//! refinement, tuned harmonic pitch-class profiles). No models, network or
//! shell; FFmpeg decodes the audio. Tempo and key run on parallel threads.

pub mod confidence;
pub mod dsp;
pub mod key;
pub mod notation;
pub mod tags;
pub mod tempo;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use realfft::RealFftPlanner;
use serde::Serialize;

pub use notation::{format_suffix, key_names, parse_key, KeyNames, Notation};

pub const SR: usize = 22050;
pub const ENGINE: &str = "siphon-analysis/1";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BpmCandidate {
    pub bpm: f64,
    pub score: f64,
}

/// The JSON-compatible analysis result.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub bpm: f64,
    pub bpm_confidence: f64,
    pub bpm_candidates: Vec<BpmCandidate>,
    #[serde(flatten)]
    pub key: KeyNames,
    pub key_confidence: f64,
    pub duration_analyzed: f64,
    pub engine: &'static str,
}

#[derive(Debug)]
pub enum Error {
    /// Invalid options (a programming or request error).
    Invalid(String),
    /// The audio cannot yield an estimate, or could not be decoded.
    Audio(String),
    /// The caller asked to stop.
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Invalid(message) | Error::Audio(message) => f.write_str(message),
            Error::Cancelled => f.write_str("Audio analysis stopped."),
        }
    }
}

impl std::error::Error for Error {}

pub struct Options<'a> {
    /// At most this many seconds from the middle of the file (default 180).
    pub max_seconds: Option<f64>,
    pub bpm_range: (f64, f64),
    pub ffmpeg: &'a Path,
    /// Polled between stages and while decoding; true stops the analysis.
    pub stop: &'a (dyn Fn() -> bool + Sync),
}

#[cfg(windows)]
pub fn hide_window(command: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
pub fn hide_window(command: &mut Command) -> &mut Command {
    command
}

/// Container headers are enough for common formats, without starting ffprobe.
fn header_duration(path: &Path) -> Option<f64> {
    use lofty::file::AudioFile;
    let probe = lofty::probe::Probe::open(path).ok()?.options(lofty::config::ParseOptions::new().read_tags(false));
    let seconds = probe.guess_file_type().ok()?.read().ok()?.properties().duration().as_secs_f64();
    (seconds > 0.0).then_some(seconds)
}

fn probe_text(command: &mut Command, stop: &dyn Fn() -> bool, timeout: Duration) -> Option<String> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = hide_window(command).spawn().ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // ffprobe emits just one duration; bound unexpected output as well.
        stdout.take(1024).read_to_end(&mut bytes).ok()?;
        String::from_utf8(bytes).ok()
    });
    let deadline = Instant::now() + timeout;
    let success = loop {
        if stop() || Instant::now() >= deadline {
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break false,
        }
    };
    if !success {
        let _ = child.kill();
        let _ = child.wait();
    }
    let text = reader.join().ok().flatten();
    if success { text } else { None }
}

fn probe_duration(ffmpeg: &Path, path: &Path, stop: &dyn Fn() -> bool) -> Option<f64> {
    if let Some(seconds) = header_duration(path) {
        return Some(seconds);
    }
    let name = if ffmpeg.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) { "ffprobe.exe" } else { "ffprobe" };
    let probe = ffmpeg.with_file_name(name);
    if !probe.is_file() {
        return None;
    }
    let mut command = Command::new(probe);
    command
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    // Match the Python reference's bound; a stalled probe must not prevent
    // decode fallback or hold a cancelled analysis slot indefinitely.
    let output = probe_text(&mut command, stop, Duration::from_secs(12))?;
    let value: f64 = output.trim().parse().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

/// Decode a mono 22.05 kHz excerpt from the middle of the file, with edge silence removed.
fn decode(path: &Path, limit: f64, options: &Options) -> Result<Vec<f64>, Error> {
    let path = path.canonicalize().map_err(|e| Error::Audio(format!("Expected a local audio file: {e}")))?;
    if !path.is_file() {
        return Err(Error::Audio("Expected a local audio file".into()));
    }
    let duration = probe_duration(options.ffmpeg, &path, options.stop);
    if (options.stop)() {
        return Err(Error::Cancelled);
    }
    let start = duration.map_or(0.0, |d| ((d - limit) / 2.0).max(0.0));
    let mut command = Command::new(options.ffmpeg);
    command
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-ss", &format!("{start:.6}"), "-i"])
        .arg(&path)
        .args(["-t", &format!("{limit:.6}"), "-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-ar"])
        .arg(SR.to_string())
        .args(["-f", "f32le", "-acodec", "pcm_f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = hide_window(&mut command).spawn().map_err(|e| Error::Audio(format!("Audio decoding failed: {e}")))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs_f64((limit / 2.0).max(30.0));
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| Error::Audio(e.to_string()))? {
            break status;
        }
        if (options.stop)() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if (options.stop)() { Error::Cancelled } else { Error::Audio("Audio decoding failed: timed out".into()) });
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader.join().unwrap_or_else(|_| Ok(Vec::new())).map_err(|e| Error::Audio(e.to_string()))?;
    let stderr = errors.join().unwrap_or_default();
    if !status.success() {
        let text = String::from_utf8_lossy(&stderr);
        let tail: String = text.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(Error::Audio(format!("FFmpeg could not decode audio: {tail}")));
    }
    let mut y: Vec<f64> =
        bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64).collect();
    if y.len() < SR * 2 || y.iter().any(|v| !v.is_finite()) {
        return Err(Error::Audio("At least two seconds of finite audio are required".into()));
    }
    // Remove edge silence, not quiet passages inside the selected section.
    let block = SR / 4;
    let energy: Vec<f64> = y.chunks_exact(block).map(|c| (c.iter().map(|v| v * v).sum::<f64>() / block as f64).sqrt()).collect();
    let peak = energy.iter().copied().fold(0.0, f64::max);
    let active: Vec<usize> = (0..energy.len()).filter(|&i| energy[i] > (peak * 0.015).max(1e-6)).collect();
    let (Some(&first), Some(&last)) = (active.first(), active.last()) else {
        return Err(Error::Audio("Audio is silent".into()));
    };
    let begin = first.saturating_sub(1) * block;
    let end = y.len().min((last + 2) * block);
    y = y[begin..end].to_vec();
    if y.len() < 2 * SR {
        return Err(Error::Audio("Too little non-silent audio to analyze".into()));
    }
    let scale = y.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-8);
    y.iter_mut().for_each(|v| *v /= scale);
    Ok(y)
}

/// Decode local audio and estimate tempo and key.
pub fn analyze_file(path: impl AsRef<Path>, options: &Options) -> Result<Analysis, Error> {
    let (low, high) = options.bpm_range;
    let limit = options.max_seconds.unwrap_or(180.0);
    validate_range((low, high))?;
    if !(limit.is_finite() && (2.0..=3600.0).contains(&limit)) {
        return Err(Error::Invalid("max_seconds must be between 2 and 3600".into()));
    }
    if (options.stop)() {
        return Err(Error::Cancelled);
    }
    let y = decode(path.as_ref(), limit, options)?;
    if (options.stop)() {
        return Err(Error::Cancelled);
    }
    let analysis = analyze_samples(&y, (low, high))?;
    if (options.stop)() {
        return Err(Error::Cancelled);
    }
    Ok(analysis)
}

fn validate_range((low, high): (f64, f64)) -> Result<(), Error> {
    if !(low.is_finite() && high.is_finite() && 30.0 <= low && low < high && high <= 360.0) {
        return Err(Error::Invalid("bpm_range must satisfy 30 <= low < high <= 360".into()));
    }
    Ok(())
}

/// Estimate tempo and key from normalized mono samples at [`SR`].
/// Requires at least two seconds of finite, non-silent samples and a valid BPM range.
pub fn analyze_samples(y: &[f64], bpm_range: (f64, f64)) -> Result<Analysis, Error> {
    validate_range(bpm_range)?;
    if y.len() < SR * 2 || y.iter().any(|v| !v.is_finite()) {
        return Err(Error::Audio("At least two seconds of finite audio are required".into()));
    }
    if y.iter().all(|v| v.abs() <= 1e-6) {
        return Err(Error::Audio("Audio is silent".into()));
    }
    let (tempo, (key, key_confidence)) = std::thread::scope(|scope| {
        let key = scope.spawn(|| key::key(y));
        let mut planner = RealFftPlanner::new();
        let tempo = tempo::tempo_from_onsets(&tempo::onsets(y, &mut planner), bpm_range, &mut planner);
        (tempo, key.join().expect("key analysis thread"))
    });
    Ok(Analysis {
        bpm: tempo.bpm,
        bpm_confidence: tempo.confidence,
        bpm_candidates: tempo.candidates,
        key,
        key_confidence,
        duration_analyzed: dsp::round_to(y.len() as f64 / SR as f64, 3),
        engine: ENGINE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_reject_invalid_audio_without_panicking() {
        for samples in [vec![], vec![0.5; SR * 2 - 1], vec![0.0; SR * 2], vec![f64::NAN; SR * 2], vec![f64::INFINITY; SR * 2]] {
            assert!(matches!(analyze_samples(&samples, (70.0, 180.0)), Err(Error::Audio(_))));
        }
    }

    #[test]
    fn samples_validate_range_before_dsp() {
        for range in [(180.0, 70.0), (70.0, 70.0), (29.0, 180.0), (70.0, 361.0), (f64::NAN, 180.0), (70.0, f64::INFINITY)] {
            assert!(matches!(analyze_samples(&[], range), Err(Error::Invalid(_))));
        }
    }

    #[test]
    fn cancelled_analysis_does_not_access_the_file_or_decoder() {
        let options = Options { max_seconds: None, bpm_range: (70.0, 180.0), ffmpeg: Path::new("missing-ffmpeg"), stop: &|| true };
        assert!(matches!(analyze_file("missing-audio", &options), Err(Error::Cancelled)));
    }

    #[test]
    fn duration_probe_timeout_and_cancellation() {
        // Reuse the test executable as a stalled child, without requiring a
        // shell, FFmpeg, or Node in the unit-test environment.
        if std::env::var_os("SIPHON_ANALYSIS_TEST_STALLED_PROBE").is_some() {
            std::thread::sleep(Duration::from_secs(20));
            return;
        }
        for cancel in [false, true] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "tests::duration_probe_timeout_and_cancellation"])
                .env("SIPHON_ANALYSIS_TEST_STALLED_PROBE", "1");
            let start = Instant::now();
            assert!(probe_text(&mut command, &|| cancel, Duration::from_millis(50)).is_none());
            assert!(start.elapsed() < Duration::from_secs(5), "probe child was not reaped promptly");
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.arg("--help");
        assert!(probe_text(&mut command, &|| false, Duration::from_secs(5)).is_some());
    }
}

/// FFmpeg on PATH, a WinGet Gyan.FFmpeg package, or Siphon's portable copy.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(found) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|p| p.join(name)).find(|p| p.is_file())
    }) {
        return Some(found);
    }
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let packages = base.join("Microsoft").join("WinGet").join("Packages");
    let mut matches: Vec<PathBuf> = std::fs::read_dir(&packages)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("Gyan.FFmpeg"))
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
        .map(|build| build.path().join("bin").join(name))
        .filter(|p| p.is_file())
        .collect();
    matches.sort();
    matches.pop().or_else(|| Some(base.join("Siphon").join("ffmpeg").join("bin").join(name)).filter(|p| p.is_file()))
}
