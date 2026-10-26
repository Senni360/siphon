//! Machine discovery: FFmpeg, Node, yt-dlp and Siphon's own folders.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use crate::process;

pub fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Local"))
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn siphon_dir() -> PathBuf {
    local_app_data().join("Siphon")
}

pub fn default_output_dir() -> PathBuf {
    home().join("Music").join("Siphon")
}

/// Per-user local-socket name of the drag tray (a named pipe on Windows).
pub fn tray_pipe_name() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    format!(
        "siphon-dropzone-{}",
        user.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    )
}

pub fn executable_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// First line of `path flag` output, bounded to 8 seconds.
pub fn executable_version(path: &Path, flag: &str) -> Option<String> {
    let mut command = Command::new(path);
    command
        .arg(flag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = process::output_with_timeout(&mut command, Duration::from_secs(8)).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .next()
        .map(|line| line.chars().take(250).collect())
}

fn on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub path: PathBuf,
    pub version: String,
}

pub fn discover_ffmpeg() -> Option<Tool> {
    let path = siphon_analysis::find_ffmpeg()?;
    let version = executable_version(&path, "-version")?;
    Some(Tool { path, version })
}

pub fn ffmpeg_status(tool: &Option<Tool>) -> Value {
    match tool {
        Some(t) => json!({"found": true, "version": t.version, "path": t.path}),
        None => json!({"found": false, "version": null, "path": siphon_analysis::find_ffmpeg()}),
    }
}

pub fn discover_node() -> Option<Tool> {
    let path = on_path(if cfg!(windows) { "node.exe" } else { "node" })?;
    let version = executable_version(&path, "--version")?;
    Some(Tool { path, version })
}

/// Siphon's own yt-dlp: the unpacked build (fast start) in `bin\yt-dlp\`, then a
/// single-file build next to this executable or in `bin\`, else PATH.
pub fn discover_ytdlp() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    let bin = siphon_dir().join("bin");
    [
        executable_dir().join("yt-dlp").join(name),
        bin.join("yt-dlp").join(name),
        executable_dir().join(name),
        bin.join(name),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .or_else(|| on_path(name))
}

/// Where Siphon installs and updates its unpacked yt-dlp.
pub fn managed_ytdlp_dir() -> PathBuf {
    siphon_dir().join("bin").join("yt-dlp")
}

/// `yt-dlp --version` costs ~0.5 s; remember it per executable size and timestamp.
pub fn cached_version(executable: &Path) -> Option<String> {
    let metadata = std::fs::metadata(executable).ok()?;
    let stamp = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let key = format!("{}|{}|{stamp}", executable.display(), metadata.len());
    let cache = siphon_dir().join("bin").join("yt-dlp.version.json");
    if let Ok(text) = std::fs::read_to_string(&cache) {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            if value["key"] == key.as_str() {
                return value["version"].as_str().map(str::to_string);
            }
        }
    }
    let version = executable_version(executable, "--version")?;
    let _ = std::fs::create_dir_all(cache.parent().unwrap());
    let _ = std::fs::write(&cache, json!({"key": key, "version": version}).to_string());
    Some(version)
}
