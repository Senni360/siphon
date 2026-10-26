//! `analyze` requests: an ephemeral URL excerpt, or a read-only local file.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::analysis_service::{self, timeout_error};
use crate::errors::{HostError, Result};
use crate::options::{is_absolute, validate_bpm_range, validate_url, Cookies};
use crate::youtube::{self, STRATEGIES};
use crate::ytdlp::{self, Cancel, Event, YtDlp};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub fn analyze_request(
    request: &Map<String, Value>,
    ytdlp: Option<&YtDlp>,
    ffmpeg: Option<&Path>,
) -> Result<Value> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let truthy = |key: &str| {
        request
            .get(key)
            .is_some_and(|v| !(v.is_null() || v == "" || v == false))
    };
    if truthy("url") == truthy("path") {
        return Err(HostError::bad_request(
            "Supply exactly one audio URL or absolute file path.",
        ));
    }
    let seconds = match request.get("maxSeconds") {
        None => 150.0,
        Some(v) if !v.is_boolean() => v
            .as_f64()
            .filter(|s| s.is_finite() && (2.0..=150.0).contains(s))
            .ok_or_else(|| {
                HostError::bad_request("maxSeconds must be between 2 and 150 seconds.")
            })?,
        Some(_) => {
            return Err(HostError::bad_request(
                "maxSeconds must be between 2 and 150 seconds.",
            ))
        }
    };
    let bpm_range = validate_bpm_range(request.get("bpmRange").unwrap_or(&json!([70, 180])))?;
    let ffmpeg = ffmpeg.ok_or_else(|| {
        HostError::new(
            "FFMPEG_MISSING",
            "FFmpeg is required to analyze audio. Install FFmpeg and reconnect Siphon.",
        )
    })?;
    let never = || Ok(());
    if truthy("path") {
        let value = request["path"]
            .as_str()
            .filter(|p| !p.contains('\0') && is_absolute(p));
        let value = value
            .ok_or_else(|| HostError::bad_request("An absolute audio file path is required."))?;
        let path = Path::new(value);
        if !path.is_file() {
            return Err(HostError::new(
                "DISK",
                "The selected audio file does not exist or is not a file.",
            ));
        }
        let analysis = analysis_service::analyze(
            ffmpeg,
            path,
            bpm_range,
            Some(seconds),
            &never,
            Some(deadline),
        )?;
        return Ok(json!({"analysis": analysis, "source": "file"}));
    }
    let url = validate_url(request.get("url"))?;
    let ytdlp = ytdlp.ok_or_else(|| {
        HostError::new("INTERNAL", "yt-dlp is missing. Rerun the Siphon installer.")
    })?;
    let cookies = Cookies::from_request(
        request,
        request
            .get("cookiesFromBrowser")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )?;
    let directory = TempDir(std::env::temp_dir().join(format!(
        "siphon-analyze-{:012x}",
        crate::options::random() & 0xffff_ffff_ffff
    )));
    std::fs::create_dir_all(&directory.0)?;
    let mut common = ytdlp.base_args();
    common.extend(
        [
            "--no-simulate",
            "--no-playlist",
            "--socket-timeout",
            "10",
            "--retries",
            "1",
            "--fragment-retries",
            "1",
            "--extractor-retries",
            "1",
            "--download-sections",
            &format!("*0-{seconds}"),
            // FFmpeg's output duration bound also covers HLS and missing/wrong durations.
            "--downloader-args",
            "ffmpeg_i:-rw_timeout 10000000",
            "--downloader-args",
            &format!("ffmpeg_o:-t {seconds}"),
            "--print",
            &format!("pre_process:{}%(.{{{}}})j", ytdlp::PRE, ytdlp::PRE_FIELDS),
            "--print",
            &format!("after_move:{}%(.{{filepath,title}})j", ytdlp::FILE),
            "-P",
            &format!("home:{}", directory.0.display()),
            "-o",
            "excerpt.%(ext)s",
        ]
        .map(String::from),
    );
    common.extend(cookies.args());
    let formats: Vec<Vec<String>> = if youtube::is_youtube_url(&url) {
        STRATEGIES
            .iter()
            .map(|s| {
                [
                    "--extractor-args",
                    &format!("youtube:player_client={}", s.clients),
                    "-f",
                    s.audio_only,
                ]
                .map(String::from)
                .to_vec()
            })
            .collect()
    } else {
        vec![vec!["-f".into(), "worstaudio".into()]]
    };
    let cancel = Cancel::new();
    let (mut title, mut file): (String, Option<PathBuf>) = (String::new(), None);
    let total = formats.len();
    for (index, extra) in formats.into_iter().enumerate() {
        let mut args = common.clone();
        args.extend(extra);
        args.extend(["--".into(), url.clone()]);
        let result = ytdlp.run(&args, &cancel, Some(deadline), &mut |event| match event {
            Event::Pre(info) if ytdlp::is_live(&info) => ControlFlow::Break(HostError::new(
                "LIVE_NOT_SUPPORTED",
                "Live and upcoming streams cannot be analyzed. Try again after the stream ends.",
            )),
            Event::File(info) => {
                title = crate::media::string(info.get("title"), 4096);
                file = info
                    .get("filepath")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                ControlFlow::Continue(())
            }
            _ => ControlFlow::Continue(()),
        });
        match result {
            Ok(()) => break,
            Err(e) if e.message == "The operation timed out." => return Err(timeout_error()),
            Err(e) if index + 1 < total && youtube::retryable(&e) => continue,
            Err(e) => return Err(if total > 1 { youtube::exhausted(e) } else { e }),
        }
    }
    let file = file.filter(|f| f.is_file()).or_else(|| {
        std::fs::read_dir(&directory.0)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_stem().is_some_and(|s| s == "excerpt")
                    && !p.extension().is_some_and(|e| e == "part" || e == "ytdl")
            })
    });
    let file = file.ok_or_else(|| {
        HostError::new(
            "INTERNAL",
            "The extractor completed without producing an audio excerpt.",
        )
    })?;
    let analysis = analysis_service::analyze(
        ffmpeg,
        &file,
        bpm_range,
        Some(seconds),
        &never,
        Some(deadline),
    )?;
    Ok(json!({"analysis": analysis, "source": "url", "title": title}))
}
