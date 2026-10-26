//! The standalone yt-dlp executable: arguments, structured output, cancellation.
//!
//! yt-dlp reports through `--print`/`--progress-template` lines carrying a
//! marker and one JSON object; everything else on stderr is diagnostics.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::errors::{HostError, Result};
use crate::logging;
use crate::process::{self, Line, Tree};

pub const PRE: &str = "SIPHON-PRE ";
pub const PROGRESS: &str = "SIPHON-PROGRESS ";
pub const FILE: &str = "SIPHON-FILE ";

/// Info fields printed after each item moves into place: enough to tag and file it.
pub const FILE_FIELDS: &str = "id,title,track,upload_date,release_date,release_year,description,webpage_url,original_url,\
track_number,artist,artists,creator,creators,uploader,uploader_id,channel,composer,composers,genre,genres,album,album_artist,\
album_artists,disc_number,filepath,ext,acodec,asr,duration,chapters,playlist_index,playlist_count,playlist_title,playlist,\
thumbnail,thumbnails,extractor,extractor_key,is_live,live_status";

pub const PRE_FIELDS: &str = "id,title,is_live,live_status,playlist_index,playlist_count,n_entries,thumbnail,thumbnails,extractor_key";

/// Cooperative cancellation shared by a job and the processes it runs.
#[derive(Default)]
pub struct Cancel {
    flag: AtomicBool,
    /// Running processes, innermost last (yt-dlp, then FFmpeg started while it runs).
    current: Mutex<Vec<Tree>>,
}

impl Cancel {
    pub fn new() -> Arc<Cancel> {
        Arc::new(Cancel::default())
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        for tree in self.current.lock().unwrap().iter() {
            tree.kill();
        }
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<()> {
        if self.is_set() {
            Err(HostError::cancelled())
        } else {
            Ok(())
        }
    }

    pub fn attach(&self, tree: &Tree) -> Result<()> {
        self.current.lock().unwrap().push(tree.clone());
        // A cancel that raced the spawn still stops the new process.
        if self.is_set() {
            tree.kill();
            return Err(HostError::cancelled());
        }
        Ok(())
    }

    pub fn detach(&self) {
        self.current.lock().unwrap().pop();
    }
}

pub enum Event {
    Pre(Map<String, Value>),
    Progress(Map<String, Value>),
    File(Map<String, Value>),
    Warning(String),
    Stdout(String),
}

#[derive(Clone)]
pub struct YtDlp {
    pub path: PathBuf,
    pub ffmpeg: Option<PathBuf>,
    pub node: Option<PathBuf>,
}

impl YtDlp {
    pub fn version(&self) -> Option<String> {
        let _running = crate::ytdlp_update::Running::start().ok()?;
        crate::runtime::cached_version(&self.path)
    }

    /// Settings shared by every invocation (the library's `base_params`).
    pub fn base_args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "--ignore-config",
            "--encoding",
            "utf-8",
            "--no-colors",
            "--socket-timeout",
            "20",
            "--retries",
            "3",
            "--fragment-retries",
            "3",
            "--extractor-retries",
            "2",
            "--windows-filenames",
            "--no-restrict-filenames",
        ]
        .map(String::from)
        .to_vec();
        if let Some(ffmpeg) = &self.ffmpeg {
            args.extend([
                "--ffmpeg-location".into(),
                ffmpeg.to_string_lossy().into_owned(),
            ]);
        }
        if let Some(node) = &self.node {
            args.extend([
                "--js-runtimes".into(),
                format!("node:{}", node.to_string_lossy()),
            ]);
        }
        args
    }

    /// Run yt-dlp; `on_event` may stop it early with an error.
    pub fn run(
        &self,
        args: &[String],
        cancel: &Cancel,
        deadline: Option<Instant>,
        on_event: &mut dyn FnMut(Event) -> ControlFlow<HostError>,
    ) -> Result<()> {
        cancel.check()?;
        let _running = crate::ytdlp_update::Running::start()?;
        let mut command = Command::new(&self.path);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // yt-dlp is a frozen Python program; keep its own output encoding predictable.
        command
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1");
        let tree = Tree::spawn(&mut command)
            .map_err(|e| HostError::new("INTERNAL", format!("yt-dlp could not start: {e}")))?;
        let result = match cancel.attach(&tree) {
            Ok(()) => {
                let result = self.pump(&tree, cancel, deadline, on_event);
                cancel.detach();
                result
            }
            Err(e) => {
                cancel.detach();
                Err(e)
            }
        };
        if result.is_err() {
            tree.kill();
            // Keep the update guard until the executable actually exits.
            let _ = tree.wait(&|| true, None);
        }
        result
    }

    fn pump(
        &self,
        tree: &Tree,
        cancel: &Cancel,
        deadline: Option<Instant>,
        on_event: &mut dyn FnMut(Event) -> ControlFlow<HostError>,
    ) -> Result<()> {
        let lines = process::lines(tree);
        let mut errors: Vec<String> = Vec::new();
        let mut tail: Vec<String> = Vec::new();
        loop {
            if cancel.is_set() {
                return Err(HostError::cancelled());
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(HostError::new("NETWORK", "The operation timed out."));
            }
            let line = match lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let event = match line {
                Line::Out(text) => {
                    let parse = |rest: &str| serde_json::from_str::<Map<String, Value>>(rest).ok();
                    if let Some(map) = text.strip_prefix(PROGRESS).and_then(parse) {
                        Event::Progress(map)
                    } else if let Some(map) = text.strip_prefix(PRE).and_then(parse) {
                        Event::Pre(map)
                    } else if let Some(map) = text.strip_prefix(FILE).and_then(parse) {
                        Event::File(map)
                    } else {
                        Event::Stdout(text)
                    }
                }
                Line::Err(text) => {
                    if let Some(message) = text.strip_prefix("ERROR: ") {
                        logging::error(format!("yt-dlp: {message}"));
                        errors.push(message.to_string());
                        continue;
                    }
                    if let Some(message) = text.strip_prefix("WARNING: ") {
                        logging::warn(format!("yt-dlp: {message}"));
                        Event::Warning(message.to_string())
                    } else {
                        logging::debug(format!("yt-dlp: {text}"));
                        tail.push(text);
                        if tail.len() > 20 {
                            tail.remove(0);
                        }
                        continue;
                    }
                }
            };
            if let ControlFlow::Break(error) = on_event(event) {
                return Err(error);
            }
        }
        let (status, killed) = tree
            .wait(&|| cancel.is_set(), deadline)
            .map_err(HostError::from)?;
        cancel.check()?;
        if killed {
            return Err(HostError::new("NETWORK", "The operation timed out."));
        }
        if status.success() {
            return Ok(());
        }
        let message = errors
            .first()
            .cloned()
            .or_else(|| tail.last().cloned())
            .unwrap_or_else(|| format!("yt-dlp exited with {status}"));
        Err(HostError::classify(&message))
    }
}

pub fn is_live(info: &Map<String, Value>) -> bool {
    info.get("is_live")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || matches!(
            info.get("live_status").and_then(Value::as_str),
            Some("is_live" | "is_upcoming")
        )
}
