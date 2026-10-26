//! Bounded, serial stem separation. The engine owns model setup and inference.
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::errors::{HostError, Result};
use crate::jobs::{timestamp, Emit};
use crate::ytdlp::Cancel;

const QUEUE_LIMIT: usize = 8;
const HISTORY_LIMIT: usize = 64;
const THROTTLE: Duration = Duration::from_millis(210);
const NAMES: [&str; 4] = ["vocals.wav", "drums.wav", "bass.wav", "other.wav"];
type Engine =
    fn(&Path, &Path, &Path, &Cancel, &mut dyn FnMut(&str, Option<f64>)) -> Result<Vec<PathBuf>>;

struct Job {
    id: String,
    source: PathBuf,
    cancel: Arc<Cancel>,
    data: Value,
    last_emit: Option<Instant>,
}

struct State {
    jobs: VecDeque<Job>,
    queue: VecDeque<String>,
    stopping: bool,
}

pub struct StemManager {
    state: Mutex<State>,
    wake: Condvar,
    worker: Mutex<Option<JoinHandle<()>>>,
    ffmpeg: Option<PathBuf>,
    emit: Emit,
    engine: Engine,
}

fn terminal(job: &Job) -> bool {
    matches!(
        job.data["state"].as_str(),
        Some("done" | "error" | "cancelled")
    )
}

impl StemManager {
    pub fn new(ffmpeg: Option<PathBuf>, emit: Emit) -> Arc<Self> {
        Self::with_engine(ffmpeg, emit, crate::stems_engine::separate)
    }

    pub(crate) fn with_engine(ffmpeg: Option<PathBuf>, emit: Emit, engine: Engine) -> Arc<Self> {
        let manager = Arc::new(Self {
            state: Mutex::new(State {
                jobs: VecDeque::new(),
                queue: VecDeque::new(),
                stopping: false,
            }),
            wake: Condvar::new(),
            worker: Mutex::new(None),
            ffmpeg,
            emit,
            engine,
        });
        let worker = manager.clone();
        *manager.worker.lock().unwrap() = Some(
            std::thread::Builder::new()
                .name("stem-worker".into())
                .spawn(move || worker.run())
                .expect("spawn stem worker"),
        );
        manager
    }

    /// The acknowledgement runs under the state lock, before any event can escape.
    pub fn submit(
        &self,
        id: String,
        path: Option<&Value>,
        acknowledge: impl FnOnce(),
    ) -> Result<()> {
        if id.trim().is_empty() || id.len() > 256 {
            return Err(HostError::bad_request(
                "jobId must be a nonempty string of at most 256 bytes.",
            ));
        }
        let source = validate_input(path)?;
        if self.ffmpeg.is_none() {
            return Err(HostError::new(
                "FFMPEG_MISSING",
                "Stem separation requires FFmpeg.",
            ));
        }
        let mut state = self.state.lock().unwrap();
        if state.stopping {
            return Err(HostError::bad_request("The host is shutting down."));
        }
        if state.jobs.iter().any(|job| job.id == id) {
            return Err(HostError::bad_request("Stem jobId already exists."));
        }
        if state.queue.len() >= QUEUE_LIMIT {
            return Err(HostError::new(
                "QUEUE_FULL",
                "The stem queue is full. Try again after a job finishes.",
            ));
        }
        let now = timestamp();
        let data = json!({"type":"stem", "jobId":id, "sourceFile":source,
            "state":"queued", "stage":"queued", "progress":null, "files":[],
            "outputDir":null, "error":null, "createdAt":now, "updatedAt":now});
        state.queue.push_back(id.clone());
        state.jobs.push_back(Job {
            id,
            source,
            cancel: Cancel::new(),
            data: data.clone(),
            last_emit: None,
        });
        acknowledge();
        (self.emit)(data);
        self.wake.notify_one();
        Ok(())
    }

    pub fn list(&self) -> Vec<Value> {
        self.state
            .lock()
            .unwrap()
            .jobs
            .iter()
            .map(|job| job.data.clone())
            .collect()
    }

    pub fn cancel(&self, id: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(job) = state.jobs.iter_mut().find(|job| job.id == id) else {
            return false;
        };
        if terminal(job) || job.cancel.is_set() {
            return false;
        }
        job.cancel.cancel();
        if job.data["state"] == "queued" {
            self.finish(job, Err(HostError::cancelled()));
            state.queue.retain(|queued| queued != id);
            prune(&mut state);
        }
        true
    }

    pub fn shutdown(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.stopping = true;
            state.queue.clear();
            for job in &mut state.jobs {
                if !terminal(job) {
                    job.cancel.cancel();
                    if job.data["state"] == "queued" {
                        self.finish(job, Err(HostError::cancelled()));
                    }
                }
            }
            prune(&mut state);
            self.wake.notify_all();
        }
        // Wait for staging cleanup before main's process::exit.
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }

    fn run(&self) {
        loop {
            let (id, source) = {
                let mut state = self.state.lock().unwrap();
                while state.queue.is_empty() && !state.stopping {
                    state = self.wake.wait(state).unwrap();
                }
                if state.stopping {
                    return;
                }
                let id = state.queue.pop_front().unwrap();
                let job = state.jobs.iter_mut().find(|job| job.id == id).unwrap();
                job.data["state"] = json!("running");
                job.data["stage"] = json!("starting");
                job.data["updatedAt"] = json!(timestamp());
                (self.emit)(job.data.clone());
                (id, job.source.clone())
            };
            // Cancellation is shared independently of the manager lock during inference.
            let cancel = self.token(&id);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let staging = Staging::new(source.parent().unwrap())?;
                cancel.check()?;
                let files = (self.engine)(
                    &source,
                    &staging.0,
                    self.ffmpeg.as_deref().unwrap(),
                    &cancel,
                    &mut |stage, progress| self.progress(&id, stage, progress),
                )?;
                let names = validate_outputs(&staging.0, &files)?;
                // Serialize cancellation against publication and the terminal event.
                let mut state = self.state.lock().unwrap();
                let job = state.jobs.iter_mut().find(|job| job.id == id).unwrap();
                cancel.check()?;
                let output = publish(&staging.0, &source)?;
                self.finish(
                    job,
                    Ok((
                        output.clone(),
                        names.iter().map(|name| output.join(name)).collect(),
                    )),
                );
                prune(&mut state);
                Ok(())
            }))
            .unwrap_or_else(|_| Err(HostError::new("INTERNAL", "Stem separation worker failed.")));
            if let Err(error) = outcome {
                let mut state = self.state.lock().unwrap();
                let job = state.jobs.iter_mut().find(|job| job.id == id).unwrap();
                self.finish(job, Err(error));
                prune(&mut state);
            }
        }
    }

    fn token(&self, id: &str) -> Arc<Cancel> {
        self.state
            .lock()
            .unwrap()
            .jobs
            .iter()
            .find(|job| job.id == id)
            .unwrap()
            .cancel
            .clone()
    }

    fn progress(&self, id: &str, stage: &str, progress: Option<f64>) {
        let mut state = self.state.lock().unwrap();
        let Some(job) = state.jobs.iter_mut().find(|job| job.id == id) else {
            return;
        };
        if terminal(job) || job.cancel.is_set() {
            return;
        }
        let stage = stage.chars().take(128).collect::<String>();
        let changed_stage = job.data["stage"].as_str() != Some(stage.as_str());
        job.data["stage"] = json!(stage);
        job.data["progress"] = json!(progress
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1.0)));
        job.data["updatedAt"] = json!(timestamp());
        // Inference can emit no more progress for minutes. Always publish its
        // stage transition, even when decoding finished inside the throttle window.
        if changed_stage || job.last_emit.is_none_or(|last| last.elapsed() >= THROTTLE) {
            job.last_emit = Some(Instant::now());
            (self.emit)(job.data.clone());
        }
    }

    fn finish(&self, job: &mut Job, outcome: Result<(PathBuf, Vec<PathBuf>)>) {
        if terminal(job) {
            return;
        }
        match outcome {
            Ok((directory, files)) => {
                job.data["state"] = json!("done");
                job.data["files"] = json!(files);
                job.data["outputDir"] = json!(directory);
                job.data["progress"] = json!(1.0);
            }
            Err(error) => {
                let cancelled = job.cancel.is_set() || error.is_cancelled();
                job.data["state"] = json!(if cancelled { "cancelled" } else { "error" });
                job.data["progress"] = Value::Null;
                job.data["error"] = if cancelled {
                    Value::Null
                } else {
                    json!({"code": error.code.chars().take(64).collect::<String>(),
                        "message": error.message.chars().take(2000).collect::<String>()})
                };
            }
        }
        job.data["stage"] = job.data["state"].clone();
        job.data["updatedAt"] = json!(timestamp());
        (self.emit)(job.data.clone());
    }
}

fn prune(state: &mut State) {
    while state.jobs.iter().filter(|job| terminal(job)).count() > HISTORY_LIMIT {
        let index = state
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| terminal(job))
            .min_by_key(|(_, job)| job.data["updatedAt"].as_str().unwrap_or(""))
            .unwrap()
            .0;
        state.jobs.remove(index);
    }
}

fn validate_input(value: Option<&Value>) -> Result<PathBuf> {
    let text = value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 1024)
        .ok_or_else(|| HostError::bad_request("path must be an absolute local file path."))?;
    let path = Path::new(text);
    if !path.is_absolute() || text.contains('\0') {
        return Err(HostError::bad_request(
            "path must be an absolute local file path.",
        ));
    }
    #[cfg(windows)]
    if !matches!(path.components().next(), Some(std::path::Component::Prefix(p))
        if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
    {
        return Err(HostError::bad_request(
            "Network and device paths are not supported.",
        ));
    }
    let path = fs::canonicalize(path)?;
    if path.as_os_str().to_string_lossy().len() > 1024 {
        return Err(HostError::bad_request(
            "Stem source paths must be at most 1024 bytes.",
        ));
    }
    #[cfg(windows)]
    if !matches!(path.components().next(), Some(std::path::Component::Prefix(p))
        if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
    {
        return Err(HostError::bad_request(
            "Network and device paths are not supported.",
        ));
    }
    if !path.is_file() || path.file_stem().is_none() {
        return Err(HostError::bad_request(
            "path must refer to an existing regular file.",
        ));
    }
    Ok(path)
}

struct Staging(PathBuf);
impl Staging {
    fn new(parent: &Path) -> Result<Self> {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        for _ in 0..1000 {
            let path = parent.join(format!(
                ".siphon-stems-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(HostError::new(
            "DISK",
            "Could not allocate stem staging directory.",
        ))
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                crate::logging::warn(format!(
                    "Could not clean stem staging {}: {e}",
                    self.0.display()
                ));
            }
        }
    }
}

fn validate_outputs(staging: &Path, files: &[PathBuf]) -> Result<Vec<String>> {
    let invalid = || HostError::new("INTERNAL", "Stem engine returned invalid output files.");
    let root = fs::canonicalize(staging)?;
    if files.len() != NAMES.len() || fs::read_dir(staging)?.count() != NAMES.len() {
        return Err(invalid());
    }
    let mut names = Vec::new();
    for path in files {
        let metadata = fs::symlink_metadata(path)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(invalid)?;
        if !metadata.file_type().is_file()
            || metadata.len() == 0
            || !NAMES.contains(&name)
            || names.iter().any(|n| n == name)
            || fs::canonicalize(path)?.parent() != Some(root.as_path())
        {
            return Err(invalid());
        }
        names.push(name.to_string());
    }
    Ok(names)
}

fn publish(staging: &Path, source: &Path) -> Result<PathBuf> {
    let parent = source.parent().unwrap();
    let stem = source.file_stem().unwrap().to_string_lossy();
    for number in 1..=10000 {
        let name = if number == 1 {
            format!("{stem} - stems")
        } else {
            format!("{stem} - stems ({number})")
        };
        let destination = parent.join(name);
        match rename_no_replace(staging, &destination) {
            Ok(()) => return Ok(destination),
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists
                    || destination.try_exists().unwrap_or(false) =>
            {
                continue
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(HostError::new(
        "DISK",
        "No unused stem output directory name is available.",
    ))
}

// Use OS no-replace primitives: std::fs::rename can replace empty directories,
// and an exists-then-rename check races other writers.
#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // No MOVEFILE_REPLACE_EXISTING and no cross-volume copy fallback.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    } != 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(target_os = "linux")]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn renameat2(
            oldfd: i32,
            old: *const std::ffi::c_char,
            newfd: i32,
            new: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    if unsafe { renameat2(-100, from.as_ptr(), -100, to.as_ptr(), 1) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
fn rename_no_replace(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Atomic stem publication is unsupported on this platform.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_stage_changes_are_delivered_before_indeterminate_inference() {
        let rig = Rig::new(fake_engine);
        let input = rig.input("hold.wav");
        rig.submit("stages", &input).unwrap();
        rig.wait("stages", "running");
        for stage in ["preparing", "decoding", "separating"] {
            rig.manager.progress("stages", stage, None);
        }
        let stages: Vec<_> = rig.events.lock().unwrap().iter()
            .filter_map(|event| event["stage"].as_str().map(String::from)).collect();
        assert!(stages.windows(3).any(|window| window == ["preparing", "decoding", "separating"]));
        rig.manager.cancel("stages");
        rig.wait("stages", "cancelled");
    }

    struct Rig {
        manager: Arc<StemManager>,
        root: PathBuf,
        events: Arc<Mutex<Vec<Value>>>,
    }
    impl Rig {
        fn new(engine: Engine) -> Self {
            let root = Staging::new(&std::env::temp_dir()).unwrap();
            let path = root.0.clone();
            std::mem::forget(root);
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = events.clone();
            let manager = StemManager::with_engine(
                Some("unused-ffmpeg".into()),
                Arc::new(move |event| sink.lock().unwrap().push(event)),
                engine,
            );
            Self {
                manager,
                root: path,
                events,
            }
        }
        fn input(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::write(&path, b"original input").unwrap();
            path
        }
        fn submit(&self, id: &str, source: &Path) -> Result<()> {
            self.manager.submit(id.into(), Some(&json!(source)), || {
                self.events
                    .lock()
                    .unwrap()
                    .push(json!({"type":"ack", "jobId":id}));
            })
        }
        fn wait(&self, id: &str, state: &str) -> Value {
            let start = Instant::now();
            loop {
                if let Some(value) = self
                    .manager
                    .list()
                    .into_iter()
                    .find(|v| v["jobId"] == id && v["state"] == state)
                {
                    return value;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "waiting for {id}/{state}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn no_staging(&self) {
            assert!(fs::read_dir(&self.root).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".siphon-stems-")));
        }
    }
    impl Drop for Rig {
        fn drop(&mut self) {
            self.manager.shutdown();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fake_engine(
        input: &Path,
        staging: &Path,
        _: &Path,
        cancel: &Cancel,
        progress: &mut dyn FnMut(&str, Option<f64>),
    ) -> Result<Vec<PathBuf>> {
        let files: Vec<_> = NAMES.iter().map(|name| staging.join(name)).collect();
        for path in &files {
            fs::write(path, b"RIFF fake test wav")?;
        }
        progress("separating", Some(0.25));
        if input.file_stem().unwrap() == "hold" {
            while !cancel.is_set() {
                std::thread::sleep(Duration::from_millis(2));
            }
            progress("late", Some(0.9));
        }
        for _ in 0..100 {
            progress("separating", Some(0.75));
        }
        Ok(files)
    }

    #[test]
    fn publishes_four_files_without_overwrite_and_acknowledges_first() {
        let rig = Rig::new(fake_engine);
        let input = rig.input("song.mp3");
        let occupied = rig.root.join("song - stems");
        fs::create_dir(&occupied).unwrap(); // Even an empty directory must survive.
        rig.submit("one", &input).unwrap();
        let done = rig.wait("one", "done");
        assert!(Path::new(done["outputDir"].as_str().unwrap()).ends_with("song - stems (2)"));
        assert_eq!(done["files"].as_array().unwrap().len(), 4);
        for path in done["files"].as_array().unwrap() {
            assert!(Path::new(path.as_str().unwrap()).is_file());
        }
        assert!(occupied.is_dir());
        assert_eq!(fs::read(input).unwrap(), b"original input");
        assert!(!rig.manager.cancel("one"));
        rig.manager.shutdown();
        rig.no_staging();
        let events = rig.events.lock().unwrap();
        assert_eq!(events[0]["type"], "ack");
        assert_eq!(events[1]["state"], "queued");
        assert_eq!(events[2]["state"], "running");
        assert_eq!(events.last().unwrap()["state"], "done");
        assert!(events.len() <= 5, "progress burst must be throttled");
    }

    #[test]
    fn bounds_queue_cancels_queued_and_running_and_suppresses_late_progress() {
        let rig = Rig::new(fake_engine);
        let input = rig.input("hold.wav");
        rig.submit("running", &input).unwrap();
        rig.wait("running", "running");
        let other = rig.input("other.mp3");
        for n in 0..QUEUE_LIMIT {
            rig.submit(&format!("queued-{n}"), &other).unwrap();
        }
        assert_eq!(
            rig.submit("overflow", &other).unwrap_err().code,
            "QUEUE_FULL"
        );
        assert!(rig.submit("running", &other).is_err());
        for n in 0..QUEUE_LIMIT {
            assert!(rig.manager.cancel(&format!("queued-{n}")));
        }
        assert!(rig.manager.cancel("running"));
        rig.wait("running", "cancelled");
        rig.manager.shutdown();
        rig.no_staging();
        let events = rig.events.lock().unwrap();
        assert!(!events.iter().any(|v| v["stage"] == "late"));
        for id in std::iter::once("running".to_string())
            .chain((0..QUEUE_LIMIT).map(|n| format!("queued-{n}")))
        {
            let job_events: Vec<_> = events
                .iter()
                .filter(|v| v["type"] == "stem" && v["jobId"] == id)
                .collect();
            assert_eq!(job_events.last().unwrap()["state"], "cancelled");
            assert_eq!(
                job_events
                    .iter()
                    .filter(|v| v["state"] == "cancelled")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn shutdown_cancels_and_cleans_before_returning() {
        let rig = Rig::new(fake_engine);
        let input = rig.input("hold.wav");
        rig.submit("active", &input).unwrap();
        rig.wait("active", "running");
        rig.submit("queued", &input).unwrap();
        rig.manager.shutdown();
        assert!(rig.manager.list().iter().all(|v| v["state"] == "cancelled"));
        assert!(rig.submit("after", &input).is_err());
        rig.no_staging();
    }

    #[test]
    fn validates_inputs_and_missing_ffmpeg() {
        let rig = Rig::new(fake_engine);
        for value in [
            Value::Null,
            json!(12),
            json!("relative.wav"),
            json!(rig.root),
            json!(rig.root.join("absent.wav")),
        ] {
            assert!(rig
                .manager
                .submit("invalid".into(), Some(&value), || panic!(
                    "must not acknowledge"
                ))
                .is_err());
        }
        let input = rig.input("song.wav");
        for id in ["".to_string(), " ".to_string(), "x".repeat(257)] {
            assert!(rig.submit(&id, &input).is_err());
        }
        let missing = StemManager::with_engine(None, Arc::new(|_| {}), fake_engine);
        assert_eq!(
            missing
                .submit("missing".into(), Some(&json!(input)), || {})
                .unwrap_err()
                .code,
            "FFMPEG_MISSING"
        );
        missing.shutdown();
        assert!(rig.manager.list().is_empty());
    }

    #[test]
    fn rejects_escaping_duplicate_and_extra_outputs() {
        let rig = Rig::new(fake_engine);
        let input = rig.input("song.wav");
        let staging = Staging::new(&rig.root).unwrap();
        let files = fake_engine(
            &input,
            &staging.0,
            Path::new("unused"),
            &Cancel::default(),
            &mut |_, _| {},
        )
        .unwrap();
        assert!(validate_outputs(&staging.0, &files).is_ok());
        let mut invalid = files.clone();
        invalid[0] = input;
        assert!(validate_outputs(&staging.0, &invalid).is_err());
        invalid[0] = invalid[1].clone();
        assert!(validate_outputs(&staging.0, &invalid).is_err());
        fs::write(staging.0.join("extra"), b"extra").unwrap();
        assert!(validate_outputs(&staging.0, &files).is_err());
    }

    #[test]
    fn engine_error_and_panic_clean_staging_and_worker_survives() {
        fn failing(
            input: &Path,
            staging: &Path,
            ffmpeg: &Path,
            cancel: &Cancel,
            progress: &mut dyn FnMut(&str, Option<f64>),
        ) -> Result<Vec<PathBuf>> {
            let files = fake_engine(input, staging, ffmpeg, cancel, progress)?;
            match input.file_stem().unwrap().to_str().unwrap() {
                "error" => Err(HostError::new("MODEL", "Model unavailable")),
                "panic" => panic!("engine panic"),
                "unsafe" => Ok(vec![input.to_path_buf()]),
                _ => Ok(files),
            }
        }
        let rig = Rig::new(failing);
        for id in ["error", "panic", "unsafe"] {
            rig.submit(id, &rig.input(&format!("{id}.wav"))).unwrap();
            let failed = rig.wait(id, "error");
            assert!(failed["outputDir"].is_null());
            assert_eq!(failed["files"], json!([]));
            rig.no_staging();
        }
        rig.submit("success", &rig.input("success.wav")).unwrap();
        rig.wait("success", "done");
    }

    #[test]
    fn retains_only_bounded_terminal_history() {
        let rig = Rig::new(fake_engine);
        let input = rig.input("error.wav");
        for n in 0..HISTORY_LIMIT + 3 {
            let id = n.to_string();
            rig.submit(&id, &input).unwrap();
            rig.wait(&id, "done");
        }
        assert_eq!(rig.manager.list().len(), HISTORY_LIMIT);
        assert!(!rig.manager.list().iter().any(|v| v["jobId"] == "0"));
    }
}
