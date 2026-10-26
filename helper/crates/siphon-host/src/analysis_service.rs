//! Bounded BPM/key analysis, tag writing and the filename suffix rename.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use siphon_analysis::{Analysis, Notation};

use crate::errors::{HostError, Result};

type BackgroundTask = Box<dyn FnOnce() + Send>;

/// Queue late analysis on two workers instead of allocating a waiting OS thread
/// for every playlist item. Numerical work also shares the request-side slots.
pub fn background(task: impl FnOnce() + Send + 'static) -> Result<()> {
    static QUEUE: OnceLock<std::result::Result<mpsc::Sender<BackgroundTask>, String>> =
        OnceLock::new();
    let sender = QUEUE
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<BackgroundTask>();
            let receiver = Arc::new(Mutex::new(receiver));
            for index in 0..2 {
                let receiver = receiver.clone();
                std::thread::Builder::new()
                    .name(format!("analysis-{}", index + 1))
                    .spawn(move || loop {
                        let task = match receiver.lock().unwrap().recv() {
                            Ok(task) => task,
                            Err(_) => return,
                        };
                        task();
                    })
                    .map_err(|e| e.to_string())?;
            }
            Ok(sender)
        })
        .as_ref()
        .map_err(|e| {
            HostError::new(
                "INTERNAL",
                format!("Could not start background analysis: {e}"),
            )
        })?;
    sender
        .send(Box::new(task))
        .map_err(|_| HostError::new("INTERNAL", "Background analysis is unavailable."))
}

/// At most two numerical analyses run at once, across jobs and requests.
struct Slots {
    used: Mutex<usize>,
    freed: Condvar,
}

fn slots() -> &'static Slots {
    static SLOTS: OnceLock<Slots> = OnceLock::new();
    SLOTS.get_or_init(|| Slots {
        used: Mutex::new(0),
        freed: Condvar::new(),
    })
}

struct Slot;

impl Slot {
    fn acquire(stop: &dyn Fn() -> Result<()>) -> Result<Slot> {
        let slots = slots();
        let mut used = slots.used.lock().unwrap();
        loop {
            stop()?;
            if *used < 2 {
                *used += 1;
                return Ok(Slot);
            }
            used = slots
                .freed
                .wait_timeout(used, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        *slots().used.lock().unwrap() -= 1;
        slots().freed.notify_one();
    }
}

pub fn timeout_error() -> HostError {
    HostError::new(
        "NETWORK",
        "Audio analysis timed out. Try a local file or a shorter clip.",
    )
}

/// Analyze one file; `check` is polled for cancellation, `deadline` bounds waiting.
pub fn analyze(
    ffmpeg: &Path,
    path: &Path,
    bpm_range: (f64, f64),
    max_seconds: Option<f64>,
    check: &(dyn Fn() -> Result<()> + Sync),
    deadline: Option<Instant>,
) -> Result<Analysis> {
    let stop_check = || -> Result<()> {
        check()?;
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(timeout_error());
        }
        Ok(())
    };
    let _slot = Slot::acquire(&stop_check)?;
    let stop = || stop_check().is_err();
    let options = siphon_analysis::Options {
        max_seconds,
        bpm_range,
        ffmpeg,
        stop: &stop,
    };
    let result = siphon_analysis::analyze_file(path, &options);
    stop_check()?;
    result.map_err(|error| match error {
        siphon_analysis::Error::Cancelled => HostError::cancelled(),
        other => HostError::classify(&other.to_string()),
    })
}

/// TBPM/TKEY-style tags, plus the WAV `acid` chunk.
pub fn write_bpm_key(path: &Path, analysis: &Analysis) -> Result<()> {
    siphon_analysis::tags::write_tags(path, analysis.bpm, &analysis.key.key_short)
        .map_err(|m| HostError::new("INTERNAL", m))?;
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
    {
        siphon_analysis::tags::write_acid(path, analysis.bpm, &analysis.key.key_short)
            .map_err(|m| HostError::new("INTERNAL", m))?;
    }
    Ok(())
}

pub fn suffix(analysis: &Analysis, template: &str, notation: Notation) -> Result<String> {
    siphon_analysis::format_suffix(analysis.bpm, &analysis.key.key_short, template, notation)
        .map_err(HostError::bad_request)
}

fn units(value: &str) -> usize {
    value.encode_utf16().count()
}

fn truncate(value: &str, limit: usize) -> String {
    let mut text = value.to_string();
    while !text.is_empty() && units(&text) > limit {
        text.pop();
    }
    text.trim_end_matches([' ', '.']).to_string()
}

/// Append `suffix` before the extension within Windows' 259 UTF-16-unit path budget.
pub fn rename_with_suffix(path: &Path, suffix: &str, overwrite: bool) -> Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if suffix.is_empty() || stem.ends_with(suffix) {
        return Ok(path);
    }
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new(""));
    let limit = 255.min(259usize.saturating_sub(units(&parent.to_string_lossy()) + 1));
    for index in 1..=10_000 {
        let collision = if index > 1 {
            format!(" ({index})")
        } else {
            String::new()
        };
        let fixed = units(&format!("{suffix}{collision}{extension}"));
        if fixed >= limit {
            return Err(HostError::new(
                "DISK",
                "The output directory is too long to append the BPM/key suffix.",
            ));
        }
        let mut short = truncate(&stem, limit - fixed);
        if short.is_empty() {
            short = "_".into();
        }
        let target = parent.join(format!("{short}{suffix}{collision}{extension}"));
        if target == path {
            return Ok(path);
        }
        if overwrite {
            std::fs::rename(&path, &target)?;
            return Ok(target);
        }
        if target.exists() {
            continue;
        }
        match rename_no_replace(&path, &target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(HostError::new(
        "DISK",
        "Too many files already use this BPM/key filename.",
    ))
}

/// std::fs::rename replaces an existing destination on Windows too. Publishing
/// a non-overwriting result needs an atomic operation, not an exists/rename pair.
pub fn rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                0,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        // Outputs and their temporary files are on the same filesystem.
        std::fs::hard_link(source, destination)?;
        std::fs::remove_file(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing_without_overwrite_preserves_a_competing_output() {
        let dir =
            std::env::temp_dir().join(format!("siphon-publish-{:016x}", crate::options::random()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("temporary.wav");
        let destination = dir.join("finished.wav");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&destination, b"existing").unwrap();
        let error = rename_no_replace(&source, &destination).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
        assert_eq!(std::fs::read(&source).unwrap(), b"new");
        std::fs::remove_file(&destination).unwrap();
        rename_no_replace(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn background_work_queues_instead_of_spawning_per_file() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let release = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = mpsc::channel();
        for _ in 0..8 {
            let (release, started, done_tx) = (release.clone(), started.clone(), done_tx.clone());
            background(move || {
                started.fetch_add(1, Ordering::SeqCst);
                while !release.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                done_tx.send(()).unwrap();
            })
            .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while started.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let active = started.load(Ordering::SeqCst);
        release.store(true, Ordering::SeqCst);
        for _ in 0..8 {
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert_eq!(active, 2);
    }

    #[test]
    fn rename_collisions_and_skip() {
        let dir = std::env::temp_dir().join(format!("siphon-rename-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("track.mp3");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(dir.join("track [120 BPM Am].mp3"), b"old").unwrap();
        let renamed = rename_with_suffix(&a, " [120 BPM Am]", false).unwrap();
        assert_eq!(renamed.file_name().unwrap(), "track [120 BPM Am] (2).mp3");
        assert_eq!(rename_with_suffix(&renamed, "", false).unwrap(), renamed);
        let again = dir.join("x [1 BPM C].mp3");
        std::fs::write(&again, b"x").unwrap();
        assert_eq!(
            rename_with_suffix(&again, " [1 BPM C]", false).unwrap(),
            again
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn truncation_respects_units() {
        assert_eq!(truncate("abc. ", 4), "abc");
        assert_eq!(truncate("😀😀", 3), "😀");
    }
}
