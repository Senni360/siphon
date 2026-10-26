//! Request routing. Slow native operations never block the protocol reader.

use std::ops::ControlFlow;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::desktop;
use crate::errors::{HostError, Result};
use crate::framing::{read_message, FrameError, Writer};
use crate::jobs::{Context, JobManager};
use crate::logging;
use crate::media::media_info;
use crate::options::{validate_options, validate_url, Cookies};
use crate::runtime::{self, default_output_dir, Tool};
use crate::stems::StemManager;
use crate::ytdlp::{Cancel, Event, YtDlp};
use crate::ytdlp_update;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Bumped when the native-messaging contract changes, independently of releases.
/// 1: `hello.protocolVersion`; jobs finish before BPM/key, which follow as `analysis` events.
pub const PROTOCOL_VERSION: u64 = 1;

pub struct Host {
    writer: Arc<Writer>,
    jobs: Arc<JobManager>,
    stems: Arc<StemManager>,
    ffmpeg: Option<Tool>,
    node: Option<Tool>,
    ytdlp: Option<YtDlp>,
    requests: SyncSender<Map<String, Value>>,
    update_lock: Mutex<()>,
}

impl Host {
    pub fn new(writer: Writer) -> Arc<Host> {
        let writer = Arc::new(writer);
        // Each discovery runs a child process; do them side by side.
        let (ffmpeg, node) = std::thread::scope(|scope| {
            let ffmpeg = scope.spawn(runtime::discover_ffmpeg);
            let node = scope.spawn(runtime::discover_node);
            (ffmpeg.join().unwrap_or(None), node.join().unwrap_or(None))
        });
        let ytdlp = runtime::discover_ytdlp().map(|path| YtDlp {
            path,
            ffmpeg: ffmpeg.as_ref().map(|t| t.path.clone()),
            node: node.as_ref().map(|t| t.path.clone()),
        });
        logging::info(format!(
            "ffmpeg={:?} node={:?} yt-dlp={:?}",
            ffmpeg.as_ref().map(|t| &t.path),
            node.as_ref().map(|t| &t.path),
            ytdlp.as_ref().map(|y| &y.path)
        ));
        let emit_writer = writer.clone();
        let context = Arc::new(Context {
            ytdlp: ytdlp.clone(),
            ffmpeg: ffmpeg.as_ref().map(|t| t.path.clone()),
            emit: Arc::new(move |message| {
                let _ = emit_writer.write(&message);
            }),
        });
        let (sender, receiver) = sync_channel(64);
        let stem_writer = writer.clone();
        let stems = StemManager::new(
            ffmpeg.as_ref().map(|tool| tool.path.clone()),
            Arc::new(move |message| {
                let _ = stem_writer.write(&message);
            }),
        );
        let host = Arc::new(Host {
            writer,
            jobs: JobManager::new(context, 3),
            stems,
            ffmpeg,
            node,
            ytdlp,
            requests: sender,
            update_lock: Mutex::new(()),
        });
        let warm = host.clone();
        std::thread::spawn(move || {
            warm.ytdlp_version();
        });
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..4 {
            let (host, receiver) = (host.clone(), receiver.clone());
            std::thread::Builder::new()
                .name(format!("request-{}", index + 1))
                .spawn(move || host.request_worker(receiver))
                .expect("spawn");
        }
        host
    }

    fn result(&self, id: Option<&Value>, outcome: Result<Value>) {
        let id = id
            .filter(|v| v.as_str().is_some_and(|s| s.len() <= 256))
            .cloned()
            .unwrap_or(Value::Null);
        let message = match outcome {
            Ok(data) => json!({"id": id, "type": "result", "ok": true, "data": data}),
            Err(error) => {
                json!({"id": id, "type": "result", "ok": false, "error": error.details()})
            }
        };
        if self.writer.write(&message).is_err() {
            let _ = self.writer.write(&json!({"id": id, "type": "result", "ok": false, "error": {
                "code": "BAD_REQUEST", "message": "The result is too large for native messaging. Try a smaller playlist."}}));
        }
    }

    pub fn dispatch(self: &Arc<Self>, request: Map<String, Value>) {
        let outcome = self.route(&request);
        if let Some(outcome) = outcome {
            self.result(request.get("id"), outcome);
        }
    }

    /// `None` when the request was queued and will be answered by a worker.
    fn route(self: &Arc<Self>, request: &Map<String, Value>) -> Option<Result<Value>> {
        if !request
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty() && id.len() <= 256)
        {
            return Some(Err(HostError::bad_request(
                "Every request needs a nonempty string id.",
            )));
        }
        let kind = request.get("type").and_then(Value::as_str).unwrap_or("");
        Some(match kind {
            "hello" => self.hello(request),
            "listJobs" => Ok(json!({"jobs": self.jobs.list_jobs()})),
            "listStems" => Ok(json!({"jobs": self.stems.list()})),
            "cancelStems" => {
                job_id(request).map(|id| json!({"jobId": id, "cancelled": self.stems.cancel(&id)}))
            }
            "splitStems" => {
                let submitted = job_id(request).and_then(|id| {
                    self.stems.submit(id.clone(), request.get("path"), || {
                        self.result(
                            request.get("id"),
                            Ok(json!({"jobId": id, "accepted": true})),
                        );
                    })
                });
                match submitted {
                    Ok(()) => return None,
                    Err(error) => Err(error),
                }
            }
            "cancel" => job_id(request)
                .map(|id| json!({"jobId": id, "cancelled": self.jobs.cancel_job(&id)})),
            "download" => {
                let created = (|| -> Result<_> {
                    let url = validate_url(request.get("url"))?;
                    let options = validate_options(request.get("options"))?;
                    match request.get("cookies") {
                        None | Some(Value::Null) => {}
                        Some(Value::String(text)) if text.len() <= 8 * 1024 * 1024 => {}
                        Some(_) => {
                            return Err(HostError::bad_request(
                                "cookies must be Netscape text smaller than 8 MiB.",
                            ))
                        }
                    }
                    let cookies = Cookies::from_request(request, options.cookies_from_browser)?;
                    let source: String = request
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .chars()
                        .take(256)
                        .collect();
                    self.jobs
                        .create(job_id(request)?, url, options, source, cookies)
                })();
                match created {
                    Ok(job) => {
                        // The acknowledgement is guaranteed to precede every job event.
                        self.result(
                            request.get("id"),
                            Ok(json!({"jobId": job.id, "accepted": true})),
                        );
                        self.jobs.enqueue(job);
                        return None;
                    }
                    Err(e) => Err(e),
                }
            }
            "showDrop" => self.show_drop(request),
            "grab" => self.grab(request),
            "probe" | "analyze" | "pickFile" | "pickFolder" | "reveal" | "open" | "update" => {
                match self.requests.try_send(request.clone()) {
                    Ok(()) => return None,
                    Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => Err(
                        HostError::bad_request("Too many pending requests; try again shortly."),
                    ),
                }
            }
            _ => Err(HostError::bad_request("Unknown native request type.")),
        })
    }

    fn ytdlp_version(&self) -> Option<String> {
        // The disk cache is cheap and is invalidated when a pending update is
        // applied, including by another native-messaging host.
        self.ytdlp.as_ref().and_then(YtDlp::version)
    }

    fn hello(&self, request: &Map<String, Value>) -> Result<Value> {
        if let Some(limit) = request.get("maxConcurrent") {
            self.jobs.resize(limit)?;
        }
        let client = request
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.jobs
            .defer_analysis
            .store(client >= 1, std::sync::atomic::Ordering::SeqCst);
        let ffmpeg = &self.ffmpeg;
        let data = json!({
            "hostVersion": VERSION, "protocolVersion": PROTOCOL_VERSION,
            "ytdlpVersion": self.ytdlp_version(), "ffmpeg": runtime::ffmpeg_status(ffmpeg),
            "jsRuntime": self.node.as_ref().map(|n| json!({"name": "node", "version": n.version})),
            "defaultOutputDir": default_output_dir(), "platform": if cfg!(windows) { "win32" } else { std::env::consts::OS },
            "dropzone": desktop::tray_executable().is_some(),
            "stems": true,
        });
        let ffmpeg = self.ffmpeg.as_ref().map(|t| t.path.clone());
        std::thread::spawn(move || {
            if let Err(e) = desktop::warm_tray(ffmpeg.as_deref()) {
                logging::warn(format!("Could not start the drag tray: {}", e.message));
            }
        });
        Ok(data)
    }

    fn drop_items(value: Option<&Value>, single: bool) -> Result<Vec<Value>> {
        let items: Vec<Value> = if single {
            value.cloned().into_iter().collect()
        } else {
            value
                .and_then(Value::as_array)
                .filter(|l| (1..=20).contains(&l.len()))
                .cloned()
                .ok_or_else(|| HostError::bad_request("items must list 1 to 20 downloads."))?
        };
        let mut clean = Vec::new();
        for item in items {
            let files = item
                .get("files")
                .and_then(Value::as_array)
                .filter(|f| (1..=500).contains(&f.len()));
            let files = files.ok_or_else(|| {
                HostError::bad_request(if single {
                    "item needs a files list."
                } else {
                    "Each item needs a files list."
                })
            })?;
            let mut existing = Vec::new();
            for value in files {
                let path = desktop::existing_path(Some(value))
                    .ok()
                    .filter(|p| p.is_file());
                existing.extend(path.map(|p| Value::String(p.to_string_lossy().into_owned())));
            }
            if existing.is_empty() {
                return Err(HostError::new(
                    "DISK",
                    "The selected file no longer exists.",
                ));
            }
            let mut entry = json!({"files": existing});
            for key in ["title", "subtitle", "service"] {
                if let Some(text) = item.get(key).and_then(Value::as_str) {
                    entry[key] = Value::String(text.chars().take(300).collect());
                }
            }
            if let Some(replaces) = item
                .get("replaces")
                .and_then(Value::as_array)
                .filter(|l| (1..=500).contains(&l.len()))
            {
                // Earlier paths of a visible card (renamed by late analysis); they need not exist.
                entry["replaces"] = replaces
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| Value::String(s.to_string()))
                    .collect();
            }
            clean.push(entry);
        }
        Ok(clean)
    }

    fn show_drop(&self, request: &Map<String, Value>) -> Result<Value> {
        if desktop::tray_executable().is_none() {
            return Err(HostError::bad_request(
                "The drag tray is not installed. Rerun the Siphon installer.",
            ));
        }
        let items = Self::drop_items(request.get("items"), false)?;
        let shown = items.len();
        let ffmpeg = self.ffmpeg.as_ref().map(|t| t.path.clone());
        desktop::tray(
            &json!({"items": items, "pinned": request.get("pinned").and_then(Value::as_bool).unwrap_or(false), "ffmpeg": ffmpeg}),
            true,
        )?;
        Ok(json!({"shown": shown}))
    }

    fn grab(&self, request: &Map<String, Value>) -> Result<Value> {
        if desktop::tray_executable().is_none() {
            return Err(HostError::bad_request(
                "Dragging needs the drag tray. Rerun the Siphon installer.",
            ));
        }
        let item = Self::drop_items(request.get("item"), true)?.remove(0);
        // A cold tray needs a moment to start; by then the gesture is usually over.
        Ok(json!({"started": desktop::tray(&json!({"grab": item}), false)?}))
    }

    fn request_worker(self: Arc<Self>, receiver: Arc<Mutex<Receiver<Map<String, Value>>>>) {
        loop {
            let request = match receiver.lock().unwrap().recv() {
                Ok(request) => request,
                Err(_) => return,
            };
            let outcome = self.slow_request(&request);
            if let Err(e) = &outcome {
                logging::warn(format!(
                    "Request {} failed: {} {}",
                    request.get("type").and_then(Value::as_str).unwrap_or(""),
                    e.code,
                    e.message
                ));
            }
            self.result(request.get("id"), outcome);
        }
    }

    fn slow_request(&self, request: &Map<String, Value>) -> Result<Value> {
        match request.get("type").and_then(Value::as_str).unwrap_or("") {
            "analyze" => crate::analyze_request::analyze_request(
                request,
                self.ytdlp.as_ref(),
                self.ffmpeg.as_ref().map(|t| t.path.as_path()),
            ),
            "pickFile" => Ok(json!({"path": desktop::pick_file()?})),
            "pickFolder" => Ok(json!({"path": desktop::pick_folder(request.get("initial"))?})),
            "probe" => self.probe(request),
            "reveal" => {
                desktop::reveal(&desktop::existing_path(request.get("path"))?).map(|_| json!({}))
            }
            "open" => {
                desktop::open(&desktop::existing_path(request.get("path"))?).map(|_| json!({}))
            }
            "update" => {
                let Ok(_guard) = self.update_lock.try_lock() else {
                    return Err(HostError::bad_request("An update is already running."));
                };
                let before = self.ytdlp_version();
                let mut result = ytdlp_update::update(before.as_deref())?;
                result["restartRequired"] = json!(self.ytdlp.as_ref().is_none_or(
                    |tool| tool.path != runtime::managed_ytdlp_dir().join("yt-dlp.exe")
                ));
                Ok(result)
            }
            _ => Err(HostError::bad_request("Unknown native request type.")),
        }
    }

    fn probe(&self, request: &Map<String, Value>) -> Result<Value> {
        let url = validate_url(request.get("url"))?;
        let ytdlp = self.ytdlp.as_ref().ok_or_else(|| {
            HostError::new("INTERNAL", "yt-dlp is missing. Rerun the Siphon installer.")
        })?;
        let cookies = Cookies::from_request(
            request,
            request
                .get("cookiesFromBrowser")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        )?;
        let mut args = ytdlp.base_args();
        args.extend(["-J", "--flat-playlist", "--playlist-end", "201"].map(String::from));
        args.push(
            if request
                .get("flatPlaylist")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "--yes-playlist"
            } else {
                "--no-playlist"
            }
            .into(),
        );
        args.extend(cookies.args());
        args.extend(["--".into(), url.clone()]);
        let mut output: Option<Map<String, Value>> = None;
        ytdlp.run(&args, &Cancel::default(), None, &mut |event| {
            if let Event::Stdout(line) = event {
                if let Ok(Value::Object(info)) = serde_json::from_str::<Value>(&line) {
                    output = Some(info);
                }
            }
            ControlFlow::Continue(())
        })?;
        let info = output.ok_or_else(|| {
            HostError::new(
                "UNSUPPORTED_URL",
                "No supported media was found on this page.",
            )
        })?;
        Ok(media_info(&info, &url))
    }

    pub fn read_loop(self: &Arc<Self>, stream: &mut impl std::io::Read) {
        while !self.writer.closed() {
            match read_message(stream) {
                Ok(Some(request)) => self.dispatch(request),
                Ok(None) => break,
                Err(FrameError::BadRequest(message)) => {
                    logging::warn(format!("Rejected native messaging frame: {message}"));
                    self.result(None, Err(HostError::bad_request(message)));
                }
                Err(FrameError::Fatal(message)) => {
                    logging::error(format!(
                        "Native messaging input closed or malformed: {message}"
                    ));
                    break;
                }
            }
        }
        self.shutdown();
    }

    pub fn shutdown(&self) {
        self.writer.close();
        self.jobs.shutdown();
        self.stems.shutdown();
    }
}

fn job_id(request: &Map<String, Value>) -> Result<String> {
    request
        .get("jobId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .map(str::to_string)
        .ok_or_else(|| HostError::bad_request("jobId must be a nonempty string."))
}

#[cfg(test)]
mod stem_protocol_tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Capture {
        fn messages(&self) -> Vec<Value> {
            let mut reader = Cursor::new(self.0.lock().unwrap().clone());
            let mut messages = Vec::new();
            while let Some(message) = read_message(&mut reader).unwrap() {
                messages.push(Value::Object(message));
            }
            messages
        }
    }

    fn engine(
        _: &Path,
        _: &Path,
        _: &Path,
        cancel: &Cancel,
        progress: &mut dyn FnMut(&str, Option<f64>),
    ) -> Result<Vec<PathBuf>> {
        progress("waiting-test", None);
        while !cancel.is_set() {
            std::thread::sleep(Duration::from_millis(2));
        }
        cancel.check()?;
        unreachable!()
    }

    fn host() -> (Arc<Host>, Capture) {
        let capture = Capture::default();
        let writer = Arc::new(Writer::new(capture.clone()));
        let event_writer = writer.clone();
        let stems = StemManager::with_engine(
            Some("unused".into()),
            Arc::new(move |event| {
                let _ = event_writer.write(&event);
            }),
            engine,
        );
        let context = Arc::new(Context {
            ytdlp: None,
            ffmpeg: None,
            emit: Arc::new(|_| {}),
        });
        let (requests, _) = sync_channel(1);
        (
            Arc::new(Host {
                writer,
                jobs: JobManager::new(context, 1),
                stems,
                ffmpeg: None,
                node: None,
                ytdlp: None,
                requests,
                update_lock: Mutex::new(()),
            }),
            capture,
        )
    }
    fn dispatch(host: &Arc<Host>, request: Value) {
        host.dispatch(request.as_object().unwrap().clone());
    }

    #[test]
    fn routes_stem_requests_and_preserves_ack_order_over_native_frames() {
        let (host, capture) = host();
        // This test never discovers tools or starts model setup.
        let path = std::env::current_exe().unwrap();
        dispatch(
            &host,
            json!({"id":"split", "type":"splitStems", "jobId":"stem-1", "path":path}),
        );
        dispatch(&host, json!({"id":"list", "type":"listStems"}));
        dispatch(
            &host,
            json!({"id":"duplicate", "type":"splitStems", "jobId":"stem-1", "path":path}),
        );
        dispatch(
            &host,
            json!({"id":"cancel", "type":"cancelStems", "jobId":"stem-1"}),
        );
        let start = Instant::now();
        while host.stems.list()[0]["state"] != "cancelled" {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        dispatch(
            &host,
            json!({"id":"again", "type":"cancelStems", "jobId":"stem-1"}),
        );
        let messages = capture.messages();
        assert_eq!(
            messages[0],
            json!({"id":"split", "type":"result", "ok":true, "data":{"jobId":"stem-1", "accepted":true}})
        );
        assert_eq!(messages[1]["state"], "queued");
        let result = |id: &str| messages.iter().find(|v| v["id"] == id).unwrap();
        assert_eq!(result("list")["data"]["jobs"][0]["jobId"], "stem-1");
        assert_eq!(result("duplicate")["error"]["code"], "BAD_REQUEST");
        assert_eq!(result("cancel")["data"]["cancelled"], true);
        assert_eq!(result("again")["data"]["cancelled"], false);
        assert_eq!(
            messages
                .iter()
                .filter(|v| v["type"] == "stem" && v["state"] == "cancelled")
                .count(),
            1
        );
        host.shutdown();
    }

    #[test]
    fn rejects_invalid_stem_requests_and_eof_cancels_worker() {
        let (host, capture) = host();
        dispatch(
            &host,
            json!({"id":"invalid", "type":"splitStems", "jobId":"x", "path":"relative.wav"}),
        );
        dispatch(&host, json!({"id":"missing", "type":"cancelStems"}));
        dispatch(
            &host,
            json!({"id":"unknown", "type":"cancelStems", "jobId":"unknown"}),
        );
        dispatch(
            &host,
            json!({"id":"active", "type":"splitStems", "jobId":"active", "path":std::env::current_exe().unwrap()}),
        );
        host.read_loop(&mut Cursor::new(Vec::<u8>::new()));
        assert_eq!(host.stems.list()[0]["state"], "cancelled");
        let messages = capture.messages();
        assert_eq!(messages[0]["error"]["code"], "BAD_REQUEST");
        assert_eq!(messages[1]["error"]["code"], "BAD_REQUEST");
        assert_eq!(messages[2]["data"]["cancelled"], false);
        assert!(host.writer.closed());
    }
}
