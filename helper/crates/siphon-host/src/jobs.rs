//! Resizable download workers, cooperative cancellation, and bounded progress events.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};

use crate::analysis_service;
use crate::errors::{HostError, Result};
use crate::logging;
use crate::media::{number, string, thumbnail};
use crate::options::{Cookies, Options};
use crate::pipeline;
use crate::runtime::default_output_dir;
use crate::sponsorblock;
use crate::youtube::{self, STRATEGIES};
use crate::ytdlp::{self, Cancel, Event, YtDlp};

pub type Emit = Arc<dyn Fn(Value) + Send + Sync>;

const TERMINAL: [&str; 3] = ["done", "error", "cancelled"];
// Slightly over 200 ms: at most five updates per second even with delivery jitter.
const THROTTLE: Duration = Duration::from_millis(210);

pub fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.as_millis() as i64;
    let (days, ms) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let s = ms / 1000;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}000+00:00",
        s / 3600,
        s % 3600 / 60,
        s % 60,
        ms % 1000
    )
}

/// Shared tools and settings for every job.
pub struct Context {
    pub ytdlp: Option<YtDlp>,
    pub ffmpeg: Option<PathBuf>,
    pub emit: Emit,
}

struct JobState {
    data: Map<String, Value>,
    last_emit: Option<Instant>,
    flush_scheduled: bool,
    completed: Vec<String>,
    processed: HashSet<String>,
    /// (item key, fraction): keeps progress monotonic across YouTube retries.
    progress_max: (Option<String>, f64),
    current: Map<String, Value>,
}

pub struct Job {
    pub id: String,
    pub url: String,
    pub options: Options,
    cookies: Mutex<Option<Cookies>>,
    emit: Emit,
    pub cancel: Arc<Cancel>,
    state: Mutex<JobState>,
    deferred: bool,
}

fn is_terminal(data: &Map<String, Value>) -> bool {
    data.get("state")
        .and_then(Value::as_str)
        .is_some_and(|s| TERMINAL.contains(&s))
}

impl Job {
    fn new(
        id: String,
        url: String,
        options: Options,
        source: String,
        cookies: Cookies,
        emit: Emit,
        deferred: bool,
    ) -> Arc<Job> {
        let now = timestamp();
        let Value::Object(data) = json!({
            "type": "job", "jobId": id, "url": url, "source": source, "options": options.raw, "createdAt": now, "updatedAt": now,
            "state": "queued", "stage": "downloading", "progress": null, "speed": null, "eta": null, "downloadedBytes": 0,
            "totalBytes": null, "item": null, "file": null, "files": null, "error": null, "title": "", "thumbnail": null,
            "analysis": null, "analyses": [], "analysisError": null,
        }) else {
            unreachable!()
        };
        Arc::new(Job {
            id,
            url,
            options,
            cookies: Mutex::new(Some(cookies)),
            emit,
            cancel: Cancel::new(),
            state: Mutex::new(JobState {
                data,
                last_emit: None,
                flush_scheduled: false,
                completed: Vec::new(),
                processed: HashSet::new(),
                progress_max: (None, 0.0),
                current: Map::new(),
            }),
            deferred,
        })
    }

    pub fn snapshot(&self) -> Map<String, Value> {
        self.state.lock().unwrap().data.clone()
    }

    pub fn is_terminal(&self) -> bool {
        is_terminal(&self.state.lock().unwrap().data)
    }

    /// Merge changes; emit at most every 200 ms, except terminal and analyzing states.
    pub fn update(self: &Arc<Self>, changes: Map<String, Value>) {
        let mut state = self.state.lock().unwrap();
        if is_terminal(&state.data) {
            return;
        }
        let analyzing = changes.get("state").and_then(Value::as_str) == Some("analyzing");
        for (key, value) in changes {
            state.data.insert(key, value);
        }
        if state.data["state"] == "done" {
            // Analysis can rename an output between download() returning and
            // this update. Read the authoritative paths under the same lock.
            let files = state.completed.clone();
            state.data.insert("file".into(), json!(files.last()));
            state.data.insert("files".into(), json!(files));
        }
        state
            .data
            .insert("updatedAt".into(), Value::String(timestamp()));
        let now = Instant::now();
        let due = state
            .last_emit
            .is_none_or(|last| now.duration_since(last) >= THROTTLE);
        if is_terminal(&state.data) || analyzing || due {
            state.last_emit = Some(now);
            let message = Value::Object(state.data.clone());
            // Serialize delivery with state changes, including timer flushes and
            // late analysis, so an old snapshot cannot follow a terminal event.
            (self.emit)(message);
        } else if !state.flush_scheduled {
            state.flush_scheduled = true;
            let wait = THROTTLE.saturating_sub(now.duration_since(state.last_emit.unwrap()));
            self.schedule_flush(wait);
        }
    }

    fn schedule_flush(self: &Arc<Self>, wait: Duration) {
        let job = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(wait.max(Duration::from_millis(1)));
            job.flush();
        });
    }

    fn flush(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap();
        state.flush_scheduled = false;
        if is_terminal(&state.data) {
            return;
        }
        // A due update can acquire the lock before a delayed timer. Honor the
        // most recent delivery, and keep any coalesced changes for its deadline.
        if let Some(last) = state.last_emit {
            let wait = THROTTLE.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                state.flush_scheduled = true;
                self.schedule_flush(wait);
                return;
            }
        }
        state.last_emit = Some(Instant::now());
        let message = Value::Object(state.data.clone());
        (self.emit)(message);
    }

    fn set(self: &Arc<Self>, changes: Value) {
        if let Value::Object(map) = changes {
            self.update(map);
        }
    }

    fn describe(&self, info: &Map<String, Value>) -> Map<String, Value> {
        let title = string(info.get("title"), 4096);
        let mut changes = Map::new();
        changes.insert("title".into(), Value::String(title.clone()));
        changes.insert("thumbnail".into(), thumbnail(info));
        let index = number(info.get("playlist_index"));
        if !index.is_null() {
            let count = number(
                info.get("playlist_count")
                    .filter(|v| !v.is_null())
                    .or(info.get("n_entries")),
            );
            changes.insert(
                "item".into(),
                json!({"index": index, "count": count, "title": title}),
            );
        }
        changes
    }

    fn on_pre(self: &Arc<Self>, info: Map<String, Value>) -> ControlFlow<HostError> {
        if self.cancel.is_set() {
            return ControlFlow::Break(HostError::cancelled());
        }
        if ytdlp::is_live(&info) {
            return ControlFlow::Break(HostError::new(
                "LIVE_NOT_SUPPORTED",
                "Live and upcoming streams cannot be downloaded. Try again after the stream ends.",
            ));
        }
        let mut changes = self.describe(&info);
        changes.insert("state".into(), "probing".into());
        self.state.lock().unwrap().current = info;
        self.update(changes);
        ControlFlow::Continue(())
    }

    fn on_progress(self: &Arc<Self>, progress: Map<String, Value>) {
        let finished = progress.get("status").and_then(Value::as_str) == Some("finished");
        let downloaded = progress
            .get("downloaded_bytes")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let total = progress
            .get("total_bytes")
            .filter(|v| !v.is_null())
            .or(progress.get("total_bytes_estimate"))
            .and_then(Value::as_f64);
        let (fraction, current) = {
            let mut state = self.state.lock().unwrap();
            let current = state.current.clone();
            let key = format!(
                "{}#{}",
                string(current.get("id"), 256),
                current
                    .get("playlist_index")
                    .map(Value::to_string)
                    .unwrap_or_default()
            );
            if state.progress_max.0.as_deref() != Some(&key) {
                state.progress_max = (Some(key), 0.0);
            }
            if finished {
                state.progress_max.1 = 1.0;
            } else if let Some(total) = total.filter(|t| *t > 0.0) {
                state.progress_max.1 = state.progress_max.1.max((downloaded / total).min(1.0));
            }
            let fraction = state.progress_max.1;
            (
                if fraction > 0.0 {
                    json!(fraction)
                } else {
                    Value::Null
                },
                current,
            )
        };
        let stage = if finished {
            "converting"
        } else {
            "downloading"
        };
        let mut changes = self.describe(&current);
        for (key, value) in [
            ("state", json!(stage)),
            ("stage", json!(stage)),
            ("progress", if finished { json!(1) } else { fraction }),
            ("speed", number(progress.get("speed"))),
            ("eta", number(progress.get("eta"))),
            ("downloadedBytes", json!(downloaded as u64)),
            ("totalBytes", total.map_or(Value::Null, |t| json!(t as u64))),
        ] {
            changes.insert(key.into(), value);
        }
        if current.is_empty() {
            changes.remove("title");
            changes.remove("thumbnail");
        }
        self.update(changes);
    }

    fn take_cookies(&self) -> Cookies {
        self.cookies.lock().unwrap().take().unwrap_or(Cookies::None)
    }

    fn completed(&self) -> Vec<String> {
        self.state.lock().unwrap().completed.clone()
    }

    fn downloaded_bytes(&self) -> u64 {
        self.state
            .lock()
            .unwrap()
            .data
            .get("downloadedBytes")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    /// A finished output file: record it, then analyze inline or in the background.
    fn finished_file(self: &Arc<Self>, ffmpeg: &Path, path: &Path) -> Result<()> {
        self.cancel.check()?;
        let path = std::path::absolute(path)?.to_string_lossy().into_owned();
        {
            let mut state = self.state.lock().unwrap();
            if !state.processed.insert(path.clone()) {
                return Ok(());
            }
            state.completed.push(path.clone());
        }
        if !self.options.analyze {
            return Ok(());
        }
        if self.deferred {
            {
                let mut state = self.state.lock().unwrap();
                let pending = state
                    .data
                    .get("analysisPending")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + 1;
                state.data.insert("analysisPending".into(), json!(pending));
            }
            let (job, ffmpeg) = (self.clone(), ffmpeg.to_path_buf());
            let original = path.clone();
            if let Err(error) =
                analysis_service::background(move || job.deferred_analysis(&ffmpeg, path))
            {
                let entry =
                    json!({"file": original, "analysis": null, "analysisError": error.details()});
                self.finish_analysis(original, Some(entry));
            }
            return Ok(());
        }
        self.set(json!({"state": "analyzing", "stage": "analyzing", "file": path, "speed": null, "eta": null}));
        let entry = self.analyze_file(ffmpeg, &path)?;
        let (files, analyses, error) = {
            let state = self.state.lock().unwrap();
            let mut analyses = state.data["analyses"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            analyses.push(entry.clone());
            (
                state.completed.clone(),
                analyses,
                entry
                    .get("analysisError")
                    .cloned()
                    .unwrap_or_else(|| state.data["analysisError"].clone()),
            )
        };
        self.set(json!({"file": entry["file"], "files": files, "analysis": entry["analysis"], "analyses": analyses, "analysisError": error}));
        Ok(())
    }

    /// Estimate, tag and rename one file; returns its `analyses` entry.
    fn analyze_file(self: &Arc<Self>, ffmpeg: &Path, path: &str) -> Result<Value> {
        let options = &self.options;
        let check = || self.cancel.check();
        let mut failure: Option<HostError> = None;
        let mut file = path.to_string();
        let analysis = match analysis_service::analyze(
            ffmpeg,
            Path::new(path),
            options.bpm_range,
            None,
            &check,
            None,
        ) {
            Ok(a) => Some(a),
            Err(e) if e.is_cancelled() => return Err(e),
            Err(e) => {
                logging::warn(format!("Analysis failed for {path}: {}", e.message));
                failure = Some(e);
                None
            }
        };
        self.cancel.check()?;
        if let Some(result) = &analysis {
            // Tagging and renaming fail independently; keep the estimate and the audio.
            if options.write_bpm_key_tags {
                if let Err(e) = analysis_service::write_bpm_key(Path::new(path), result) {
                    failure = Some(e);
                }
            }
            self.cancel.check()?;
            if options.append_bpm_key {
                let mut state = self.state.lock().unwrap();
                let renamed =
                    analysis_service::suffix(result, &options.bpm_key_suffix, options.key_notation)
                        .and_then(|suffix| {
                            analysis_service::rename_with_suffix(
                                Path::new(path),
                                &suffix,
                                options.overwrite,
                            )
                        });
                match renamed {
                    Ok(renamed) => {
                        let renamed = renamed.to_string_lossy().into_owned();
                        if let Some(slot) = state.completed.iter_mut().find(|f| **f == path) {
                            *slot = renamed.clone();
                        }
                        if state.data["file"].as_str() == Some(path) {
                            state.data.insert("file".into(), json!(renamed));
                        }
                        if let Some(files) =
                            state.data.get_mut("files").and_then(Value::as_array_mut)
                        {
                            for f in files.iter_mut().filter(|f| f.as_str() == Some(path)) {
                                *f = json!(renamed);
                            }
                        }
                        file = renamed;
                    }
                    Err(e) => failure = Some(e),
                }
            }
        }
        self.cancel.check()?;
        let mut entry =
            json!({"file": file, "analysis": analysis.map(|a| serde_json::to_value(a).unwrap())});
        if let Some(error) = failure {
            entry["analysisError"] = error.details();
        }
        Ok(entry)
    }

    fn deferred_analysis(self: Arc<Self>, ffmpeg: &Path, path: String) {
        let entry = match self.analyze_file(ffmpeg, &path) {
            Ok(entry) => Some(entry),
            Err(e) if e.is_cancelled() => None,
            Err(e) => Some(json!({"file": path, "analysis": null, "analysisError": e.details()})),
        };
        self.finish_analysis(path, entry);
    }

    fn finish_analysis(&self, path: String, entry: Option<Value>) {
        {
            let mut state = self.state.lock().unwrap();
            let pending = state
                .data
                .get("analysisPending")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .saturating_sub(1);
            state.data.insert("analysisPending".into(), json!(pending));
            let Some(entry) = entry else { return };
            // Applied even after the job is terminal: that is the point of deferral.
            if let Some(analyses) = state.data.get_mut("analyses").and_then(Value::as_array_mut) {
                analyses.push(entry.clone());
            }
            if !entry["analysis"].is_null() {
                state
                    .data
                    .insert("analysis".into(), entry["analysis"].clone());
            }
            if let Some(error) = entry.get("analysisError") {
                state.data.insert("analysisError".into(), error.clone());
            }
            state.data.insert("updatedAt".into(), json!(timestamp()));
            let mut event = json!({"type": "analysis", "jobId": self.id, "source": path, "files": state.completed, "pending": pending});
            for (key, value) in entry.as_object().unwrap() {
                event[key] = value.clone();
            }
            (self.emit)(event);
        }
    }
}

struct Queue {
    jobs: HashMap<String, Arc<Job>>,
    order: Vec<String>,
    waiting: VecDeque<Arc<Job>>,
    limit: usize,
    active: usize,
    workers: usize,
    stopped: bool,
}

pub struct JobManager {
    context: Arc<Context>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub defer_analysis: AtomicBool,
}

impl JobManager {
    pub fn new(context: Arc<Context>, concurrency: usize) -> Arc<JobManager> {
        let manager = Arc::new(JobManager {
            context,
            queue: Arc::new((
                Mutex::new(Queue {
                    jobs: HashMap::new(),
                    order: Vec::new(),
                    waiting: VecDeque::new(),
                    limit: 0,
                    active: 0,
                    workers: 0,
                    stopped: false,
                }),
                Condvar::new(),
            )),
            defer_analysis: AtomicBool::new(false),
        });
        manager.resize(&json!(concurrency)).expect("valid default");
        manager
    }

    pub fn resize(&self, value: &Value) -> Result<()> {
        let concurrency = value
            .as_u64()
            .filter(|n| (1..=16).contains(n) && !value.is_boolean() && value.is_u64());
        let concurrency = concurrency.ok_or_else(|| {
            HostError::bad_request("maxConcurrent must be an integer from 1 to 16.")
        })? as usize;
        let (lock, condition) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        queue.limit = concurrency;
        while queue.workers < concurrency {
            queue.workers += 1;
            let (queue_ref, context) = (self.queue.clone(), self.context.clone());
            std::thread::Builder::new()
                .name(format!("download-{}", queue.workers))
                .spawn(move || worker(queue_ref, context))
                .expect("spawn download worker");
        }
        condition.notify_all();
        Ok(())
    }

    pub fn create(
        &self,
        id: String,
        url: String,
        options: Options,
        source: String,
        cookies: Cookies,
    ) -> Result<Arc<Job>> {
        let (lock, _) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        if queue.jobs.contains_key(&id) {
            return Err(HostError::bad_request(
                "jobId already exists; use a new jobId to retry.",
            ));
        }
        // Keep a bounded terminal tail for duplicate-id protection; never cookie text.
        if queue.jobs.len() >= 1000 {
            let terminal: Vec<String> = queue
                .order
                .iter()
                .filter(|id| {
                    let job = queue.jobs[*id].snapshot();
                    is_terminal(&job)
                        && job
                            .get("analysisPending")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                            == 0
                })
                .cloned()
                .collect();
            for id in terminal {
                queue.jobs.remove(&id);
                if queue.jobs.len() < 800 {
                    break;
                }
            }
            let jobs = &queue.jobs;
            let kept: Vec<String> = queue
                .order
                .iter()
                .filter(|id| jobs.contains_key(*id))
                .cloned()
                .collect();
            queue.order = kept;
        }
        if queue.jobs.len() >= 1000 {
            return Err(HostError::bad_request("The download queue is full."));
        }
        let job = Job::new(
            id.clone(),
            url,
            options,
            source,
            cookies,
            self.context.emit.clone(),
            self.defer_analysis.load(Ordering::SeqCst),
        );
        queue.jobs.insert(id.clone(), job.clone());
        queue.order.push(id);
        Ok(job)
    }

    pub fn enqueue(&self, job: Arc<Job>) {
        job.update(Map::new());
        let (lock, condition) = &*self.queue;
        lock.lock().unwrap().waiting.push_back(job);
        condition.notify_all();
    }

    pub fn cancel_job(&self, id: &str) -> bool {
        let (lock, condition) = &*self.queue;
        let queue = lock.lock().unwrap();
        let Some(job) = queue.jobs.get(id).cloned() else {
            return false;
        };
        if job.is_terminal() {
            return false;
        }
        job.cancel.cancel();
        if job.snapshot()["state"] == "queued" {
            // Queued jobs never enter download(), which normally owns cleanup.
            drop(job.take_cookies());
            job.set(json!({"state": "cancelled", "error": null, "speed": null, "eta": null}));
        }
        condition.notify_all();
        true
    }

    pub fn list_jobs(&self) -> Vec<Value> {
        let (lock, _) = &*self.queue;
        let queue = lock.lock().unwrap();
        queue
            .order
            .iter()
            .filter_map(|id| queue.jobs.get(id))
            .map(|j| j.snapshot())
            .filter(|s| !is_terminal(s))
            .map(Value::Object)
            .collect()
    }

    pub fn shutdown(&self) {
        let (lock, condition) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        queue.stopped = true;
        for job in queue.jobs.values() {
            // A done download may still have analysis running or waiting.
            job.cancel.cancel();
            drop(job.take_cookies());
        }
        condition.notify_all();
    }
}

fn worker(queue: Arc<(Mutex<Queue>, Condvar)>, context: Arc<Context>) {
    let (lock, condition) = &*queue;
    loop {
        let job = {
            let mut guard = lock.lock().unwrap();
            loop {
                if guard.stopped {
                    return;
                }
                if guard.active < guard.limit {
                    if let Some(job) = guard.waiting.pop_front() {
                        guard.active += 1;
                        break job;
                    }
                }
                guard = condition.wait(guard).unwrap();
            }
        };
        if job.snapshot()["state"] != "cancelled" {
            run(&context, &job);
        }
        lock.lock().unwrap().active -= 1;
        condition.notify_all();
    }
}

/// Removes the job's private staging directory, retrying while Windows releases handles.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        logging::warn(format!(
            "Could not remove staging directory {}",
            self.0.display()
        ));
    }
}

fn run(context: &Context, job: &Arc<Job>) {
    let result = download(context, job);
    match result {
        Ok(files) => {
            let last = files.last().cloned();
            job.set(json!({"state": "done", "stage": "done", "progress": 1, "speed": null, "eta": null, "file": last, "files": files, "error": null}));
        }
        Err(e) if e.is_cancelled() || job.cancel.is_set() => {
            job.set(json!({"state": "cancelled", "error": null, "speed": null, "eta": null}))
        }
        Err(e) => {
            logging::error(format!(
                "Download {} failed: {} {}",
                job.id, e.code, e.message
            ));
            job.set(json!({"state": "error", "error": e.details(), "speed": null, "eta": null}));
        }
    }
}

fn download(context: &Context, job: &Arc<Job>) -> Result<Vec<String>> {
    job.cancel.check()?;
    let ffmpeg = context.ffmpeg.clone().ok_or_else(|| {
        HostError::new(
            "FFMPEG_MISSING",
            "FFmpeg is required to extract and tag audio. Install FFmpeg and reconnect Siphon.",
        )
    })?;
    let ytdlp = context.ytdlp.clone().ok_or_else(|| {
        HostError::new("INTERNAL", "yt-dlp is missing. Rerun the Siphon installer.")
    })?;
    job.set(json!({"state": "probing"}));
    let options = &job.options;
    let output = options
        .output_dir
        .clone()
        .unwrap_or_else(default_output_dir);
    std::fs::create_dir_all(&output)?;
    let staging = Staging(output.join(format!(
        ".siphon-{:012x}",
        crate::options::random() & 0xffff_ffff_ffff
    )));
    std::fs::create_dir_all(&staging.0)?;
    hide(&staging.0);
    let cookies = job.take_cookies();
    let mut template = options.filename_template.clone();
    if options.playlist_subfolder && options.playlist_all {
        template = format!("%(playlist_title|Playlist)s/{template}");
    }
    let mut common = ytdlp.base_args();
    common.extend(
        [
            "--newline",
            "--progress",
            "--no-simulate",
            "--abort-on-error",
            "--progress-template",
            &format!("download:{}%(progress)j", ytdlp::PROGRESS),
            "--print",
            &format!("pre_process:{}%(.{{{}}})j", ytdlp::PRE, ytdlp::PRE_FIELDS),
            "--print",
            &format!("after_move:{}%(.{{{}}})j", ytdlp::FILE, ytdlp::FILE_FIELDS),
            "-o",
            &template,
            "-o",
            "pl_thumbnail:",
            if options.playlist_all {
                "--yes-playlist"
            } else {
                "--no-playlist"
            },
        ]
        .map(String::from),
    );
    if let Some(rate) = options.rate_limit {
        common.extend(["--limit-rate".into(), rate.to_string()]);
    }
    if options.embed_thumbnail && options.format != "wav" {
        common.extend(["--write-thumbnail", "--convert-thumbnails", "jpg"].map(String::from));
    }
    common.extend(cookies.args());
    let youtube = youtube::is_youtube_url(&job.url);
    let attempts: Vec<(String, Vec<String>)> = if youtube {
        STRATEGIES
            .iter()
            .map(|s| {
                let args = [
                    "--extractor-args",
                    &format!("youtube:player_client={}", s.clients),
                    "-f",
                    s.format,
                    "--http-chunk-size",
                    "10M",
                    "--retries",
                    "4",
                    "--fragment-retries",
                    "4",
                    "--extractor-retries",
                    "3",
                    "--abort-on-unavailable-fragments",
                ]
                .map(String::from)
                .to_vec();
                (s.name.to_string(), args)
            })
            .collect()
    } else {
        // A freshly resolved signed media URL can already be rejected by a CDN: retry once.
        vec![
            ("direct".into(), vec!["-f".into(), "bestaudio/best".into()]),
            ("refresh".into(), vec!["-f".into(), "bestaudio/best".into()]),
        ]
    };
    let total = attempts.len();
    for (index, (name, extra)) in attempts.into_iter().enumerate() {
        job.cancel.check()?;
        let home = staging.0.join(format!("attempt-{}", index + 1));
        std::fs::create_dir_all(&home)?;
        let mut args = common.clone();
        // The attempt directory is private, so partial files can live beside finished ones.
        args.extend(["-P".into(), format!("home:{}", home.display())]);
        args.extend(extra);
        args.push("--".into());
        args.push(job.url.clone());
        if youtube {
            logging::info(format!(
                "[siphon:youtube] attempt={}/{total} strategy={name} cookies={}",
                index + 1,
                if cookies.present() { "yes" } else { "no" }
            ));
        }
        let mut item_error: Option<HostError> = None;
        let result = ytdlp.run(&args, &job.cancel, None, &mut |event| match event {
            Event::Pre(info) => job.on_pre(info),
            Event::Progress(progress) => {
                job.on_progress(progress);
                ControlFlow::Continue(())
            }
            Event::File(info) => match process_item(context, job, &ffmpeg, &home, &output, &info) {
                Ok(()) => ControlFlow::Continue(()),
                Err(e) => {
                    item_error = Some(e.clone());
                    ControlFlow::Break(e)
                }
            },
            Event::Warning(message) => {
                (context.emit)(json!({"type": "log", "level": "warn", "message": message.chars().take(4000).collect::<String>()}));
                ControlFlow::Continue(())
            }
            Event::Stdout(_) => ControlFlow::Continue(()),
        });
        match result {
            Ok(()) => {
                if youtube {
                    logging::info(format!(
                        "[siphon:youtube] strategy={name} succeeded attempt={}",
                        index + 1
                    ));
                }
                break;
            }
            Err(error) => {
                job.cancel.check()?;
                if item_error.is_some() {
                    return Err(error); // processing failures are not extraction failures
                }
                let can_retry = job.completed().is_empty();
                let retry = if youtube {
                    youtube::retryable(&error) && can_retry
                } else {
                    can_retry
                        && job.downloaded_bytes() == 0
                        && error.code == "NETWORK"
                        && error.message.to_lowercase().contains("http error 403")
                };
                if !retry || index + 1 == total {
                    return Err(if youtube && index + 1 == total {
                        youtube::exhausted(error)
                    } else {
                        error
                    });
                }
                logging::warn(format!(
                    "Download {}: {name} failed ({}); trying again",
                    job.id, error.code
                ));
            }
        }
    }
    job.cancel.check()?;
    let files = job.completed();
    if files.is_empty() {
        return Err(HostError::new(
            "INTERNAL",
            "The extractor completed without producing an audio file.",
        ));
    }
    Ok(files)
}

fn process_item(
    context: &Context,
    job: &Arc<Job>,
    ffmpeg: &Path,
    home: &Path,
    output: &Path,
    info: &Map<String, Value>,
) -> Result<()> {
    let _ = context;
    let Some(raw) = info
        .get("filepath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
    else {
        return Ok(());
    };
    let relative = raw
        .strip_prefix(home)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| PathBuf::from(raw.file_name().unwrap_or_default()));
    let destination = output.join(relative.with_extension(""));
    let options = &job.options;
    let mut sponsor = Vec::new();
    let extractor = string(info.get("extractor_key"), 64);
    if options.sponsorblock_remove && extractor.starts_with("Youtube") {
        if let Some(id) = info.get("id").and_then(Value::as_str) {
            match sponsorblock::music_offtopic(id) {
                Ok(segments) => sponsor = segments,
                Err(e) => {
                    let message =
                        format!("SponsorBlock is unavailable; keeping the full audio. {e}");
                    logging::warn(&message);
                    (context.emit)(json!({"type": "log", "level": "warn", "message": message}));
                }
            }
        }
    }
    job.cancel.check()?;
    let mut changes = job.describe(info);
    changes.insert("state".into(), "converting".into());
    changes.insert("stage".into(), "converting".into());
    changes.insert("speed".into(), Value::Null);
    changes.insert("eta".into(), Value::Null);
    job.update(changes);
    let prepared = pipeline::prepare(
        ffmpeg,
        &raw,
        info,
        options,
        &destination,
        &sponsor,
        &job.cancel,
    )?;
    job.cancel.check()?;
    let cover = (options.embed_thumbnail && !prepared.path.extension().is_some_and(|e| e == "wav"))
        .then(|| pipeline::thumbnail_file(info))
        .flatten();
    if options.embed_metadata || cover.is_some() {
        job.set(json!({"state": "tagging", "stage": "tagging"}));
        if let Err(e) = pipeline::write_metadata(
            &prepared.path,
            info,
            options.embed_metadata,
            cover.as_deref(),
        ) {
            logging::warn(format!(
                "Tagging {} failed: {}",
                prepared.path.display(),
                e.message
            ));
            (context.emit)(json!({"type": "log", "level": "warn", "message": e.message}));
        }
    }
    let _ = std::fs::remove_file(&raw);
    job.finished_file(ffmpeg, &prepared.path)
}

#[cfg(windows)]
fn hide(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(
            wide.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN,
        );
    }
}

#[cfg(not(windows))]
fn hide(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_job(emit: Emit) -> Arc<Job> {
        Job::new(
            "test".into(),
            "https://example.test/audio".into(),
            crate::options::validate_options(None).unwrap(),
            String::new(),
            Cookies::None,
            emit,
            true,
        )
    }

    #[test]
    fn done_uses_current_paths_after_deferred_rename() {
        let job = test_job(Arc::new(|_| {}));
        job.state
            .lock()
            .unwrap()
            .completed
            .push("renamed.wav".into());
        job.set(json!({"state": "done", "file": "old.wav", "files": ["old.wav"]}));
        let done = job.snapshot();
        assert_eq!(done["file"], "renamed.wav");
        assert_eq!(done["files"], json!(["renamed.wav"]));
    }

    #[test]
    fn terminal_delivery_waits_for_in_flight_progress() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let job = test_job(Arc::new(move |event| {
            if event["state"] == "queued" {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
            captured.lock().unwrap().push(event);
        }));
        let first = job.clone();
        let first = std::thread::spawn(move || first.update(Map::new()));
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let last = job.clone();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let last = std::thread::spawn(move || {
            last.set(json!({"state": "done"}));
            finished_tx.send(()).unwrap();
        });
        let premature = finished_rx.recv_timeout(Duration::from_millis(50)).is_ok();
        release_tx.send(()).unwrap();
        first.join().unwrap();
        last.join().unwrap();
        assert!(
            !premature,
            "terminal event overtook a pending progress event"
        );
        let events = events.lock().unwrap();
        assert_eq!(events[0]["state"], "queued");
        assert_eq!(events[1]["state"], "done");
    }

    #[test]
    fn queued_cancel_removes_cookies_and_shutdown_stops_done_analysis() {
        let context = Arc::new(Context {
            ytdlp: None,
            ffmpeg: None,
            emit: Arc::new(|_| {}),
        });
        let manager = JobManager::new(context, 1);
        let request = json!({"cookies": "# Netscape HTTP Cookie File\n"});
        let cookies = Cookies::from_request(request.as_object().unwrap(), false).unwrap();
        let cookie_path = PathBuf::from(&cookies.args()[1]);
        let job = manager
            .create(
                "queued".into(),
                "https://example.test/audio".into(),
                crate::options::validate_options(None).unwrap(),
                String::new(),
                cookies,
            )
            .unwrap();
        assert!(cookie_path.is_file());
        assert!(manager.cancel_job(&job.id));
        assert!(!cookie_path.exists());
        let done = manager
            .create(
                "done".into(),
                "https://example.test/audio".into(),
                crate::options::validate_options(None).unwrap(),
                String::new(),
                Cookies::None,
            )
            .unwrap();
        done.set(json!({"state": "done", "analysisPending": 1}));
        manager.shutdown();
        assert!(done.cancel.is_set());
    }

    #[test]
    fn delayed_progress_timer_respects_a_more_recent_delivery() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let job = test_job(Arc::new(move |event| {
            let _ = sender.send((Instant::now(), event));
        }));
        job.set(json!({"state": "downloading", "progress": 0.1}));
        let (last, _) = receiver.recv().unwrap();
        {
            // A due update won the lock ahead of an older timer callback. A
            // further update was then coalesced into that pending callback.
            let mut state = job.state.lock().unwrap();
            state.flush_scheduled = true;
            state.data.insert("progress".into(), json!(0.2));
        }
        job.flush();
        let immediate = receiver.try_recv().ok();
        assert!(
            immediate.is_none(),
            "a stale timer bypassed progress throttling"
        );
        let (next, event) = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(next.duration_since(last) >= THROTTLE - Duration::from_millis(2));
        assert_eq!(
            event["progress"], 0.2,
            "the coalesced update must not be lost"
        );
    }

    #[test]
    fn timestamps_are_iso() {
        let stamp = timestamp();
        assert_eq!(&stamp[10..11], "T");
        assert!(stamp.ends_with("+00:00"));
    }
}
