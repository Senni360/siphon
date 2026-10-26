//! Windows desktop integration: folder/file pickers, Explorer, and the drag tray.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::errors::{HostError, Result};
use crate::process;
use crate::runtime::{default_output_dir, executable_dir, tray_pipe_name};

const PICK_FOLDER: &str = r#"Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = 'Choose a folder for Siphon audio'
$dialog.UseDescriptionForTitle = $true
$dialog.SelectedPath = $env:SIPHON_FOLDER_INITIAL
try {
  if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    ConvertTo-Json -Compress -InputObject $dialog.SelectedPath
  } else { 'null' }
} finally { $dialog.Dispose() }
"#;

const PICK_FILE: &str = r#"Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$dialog = New-Object System.Windows.Forms.OpenFileDialog
$dialog.Title = 'Open audio file'
$dialog.Filter = 'Audio files|*.mp3;*.m4a;*.aac;*.flac;*.wav;*.ogg;*.opus;*.webm;*.aiff|All files|*.*'
$dialog.CheckFileExists = $true
$dialog.Multiselect = $false
try {
  if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    ConvertTo-Json -Compress -InputObject $dialog.FileName
  } else { 'null' }
} finally { $dialog.Dispose() }
"#;

fn powershell(script: &str, initial: Option<&str>, what: &str) -> Result<Value> {
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-STA", "-NonInteractive", "-Command", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(initial) = initial {
        command.env("SIPHON_FOLDER_INITIAL", initial);
    }
    // The helper process is hidden; its dialog is intentionally visible.
    let output = process::output_with_timeout(&mut command, Duration::from_secs(1800))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(1000)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(HostError::new(
            "INTERNAL",
            format!("The {what} could not be opened. {tail}"),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim_start_matches('\u{feff}').trim();
    Ok(serde_json::from_str(if text.is_empty() { "null" } else { text }).unwrap_or(Value::Null))
}

pub fn pick_folder(initial: Option<&Value>) -> Result<Value> {
    let initial = match initial {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.contains('\0') => Some(s.clone()),
        Some(_) => return Err(HostError::bad_request("initial must be a directory path.")),
    };
    let initial = initial.unwrap_or_else(|| default_output_dir().to_string_lossy().into_owned());
    powershell(PICK_FOLDER, Some(&initial), "folder picker")
}

pub fn pick_file() -> Result<Value> {
    powershell(PICK_FILE, None, "audio file picker")
}

/// An existing absolute path from a request.
pub fn existing_path(value: Option<&Value>) -> Result<PathBuf> {
    let text = value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && !s.contains('\0') && crate::options::is_absolute(s))
        .ok_or_else(|| HostError::bad_request("An absolute file path is required."))?;
    let path = PathBuf::from(text);
    if !path.exists() {
        return Err(HostError::new(
            "DISK",
            "The selected file no longer exists.",
        ));
    }
    Ok(path)
}

pub fn reveal(path: &Path) -> Result<()> {
    let mut command = Command::new("explorer.exe");
    command
        .arg("/select,")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    process::hide_window(&mut command).spawn()?;
    Ok(())
}

pub fn open(path: &Path) -> Result<()> {
    // Explorer hands a file to its default application, like ShellExecute "open".
    let mut command = Command::new("explorer.exe");
    command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    process::hide_window(&mut command).spawn()?;
    Ok(())
}

pub fn tray_executable() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "siphon-tray.exe"
    } else {
        "siphon-tray"
    };
    Some(executable_dir().join(name)).filter(|p| p.is_file())
}

/// Deliver one JSON payload to the running tray over its pipe; else start one with it.
pub fn tray(payload: &Value, spawn: bool) -> Result<bool> {
    let mut message = serde_json::to_vec(payload).expect("JSON");
    message.push(b'\n');
    let pipe = format!(r"\\.\pipe\{}", tray_pipe_name());
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::OpenOptions::new().write(true).open(&pipe) {
            Ok(mut stream) => {
                stream.write_all(&message)?;
                return Ok(true);
            }
            // Every pipe instance busy (231): the server makes a new one shortly.
            Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(_) => break,
        }
    }
    if !spawn {
        if payload.get("grab").is_some() {
            let _ = warm_tray(None);
        }
        return Ok(false);
    }
    let executable = tray_executable().ok_or_else(|| {
        HostError::bad_request("The drag tray is not installed. Rerun the Siphon installer.")
    })?;
    let mut payload = payload.clone();
    if payload.get("resident").is_none() {
        payload["resident"] = Value::Bool(true);
        payload["hostPid"] = std::process::id().into();
    }
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = spawn_detached(&mut command)?;
    let mut stdin = child.stdin.expect("piped");
    stdin.write_all(&serde_json::to_vec(&payload).expect("JSON"))?;
    Ok(true)
}

/// Start the resident tray (if installed) so a later drag can be taken over.
pub fn warm_tray(ffmpeg: Option<&Path>) -> Result<()> {
    if tray_executable().is_some() {
        tray(
            &serde_json::json!({"resident": true, "hostPid": std::process::id(), "ffmpeg": ffmpeg}),
            true,
        )?;
    }
    Ok(())
}

/// Outlive the browser's job object, so the tray survives a host restart.
#[cfg(windows)]
fn spawn_detached(command: &mut Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    command
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| {
            command
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .spawn()
        })
}

#[cfg(not(windows))]
fn spawn_detached(command: &mut Command) -> std::io::Result<std::process::Child> {
    command.spawn()
}
