//! Keep Siphon's unpacked yt-dlp current from its official GitHub releases.
//!
//! The release archive is verified against the release's SHA2-256SUMS, unpacked
//! beside the current copy, then swapped in when no yt-dlp process is running.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::errors::{HostError, Result};
use crate::logging;
use crate::runtime::managed_ytdlp_dir;

const RELEASE: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";
const ASSET: &str = "yt-dlp_win.zip";

fn lock_file(name: &str) -> Result<std::fs::File> {
    let parent = managed_ytdlp_dir().parent().unwrap().to_path_buf();
    lock_file_at(&parent, name)
}

fn lock_file_at(parent: &Path, name: &str) -> Result<std::fs::File> {
    std::fs::create_dir_all(&parent)?;
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(name))?)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .new_agent()
}

fn get(url: &str) -> Result<Vec<u8>> {
    let mut response = agent()
        .get(url)
        .header("User-Agent", "Siphon")
        .call()
        .map_err(|e| HostError::new("NETWORK", format!("yt-dlp update failed: {e}")))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(200 * 1024 * 1024)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|e| HostError::new("NETWORK", format!("yt-dlp update failed: {e}")))?;
    Ok(bytes)
}

/// Download, verify and stage the latest release if it differs from `current`.
/// Returns the staged version (or the current one when already up to date).
pub fn update(current: Option<&str>) -> Result<Value> {
    // Native messaging may launch several hosts. Protect staging across all of
    // them, not just the request threads of this host.
    let update_lock = lock_file("yt-dlp.update.lock")?;
    update_lock
        .try_lock()
        .map_err(|_| HostError::bad_request("An update is already running."))?;
    let release: Value = serde_json::from_slice(&get(RELEASE)?)
        .map_err(|_| HostError::new("NETWORK", "yt-dlp update failed: unreadable release data."))?;
    let tag = release["tag_name"].as_str().unwrap_or("").to_string();
    if tag.is_empty() {
        return Err(HostError::new(
            "NETWORK",
            "yt-dlp update failed: the release has no version.",
        ));
    }
    let installed = managed_ytdlp_dir().join("yt-dlp.exe").is_file();
    if installed && current == Some(tag.as_str()) {
        return Ok(json!({"before": current, "after": tag, "restartRequired": false}));
    }
    let asset = |name: &str| -> Option<String> {
        release["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"] == name)?["browser_download_url"]
            .as_str()
            .map(str::to_string)
    };
    let (Some(archive_url), Some(sums_url)) = (asset(ASSET), asset("SHA2-256SUMS")) else {
        return Err(HostError::new(
            "NETWORK",
            "yt-dlp update failed: the release is missing its Windows build or checksums.",
        ));
    };
    let sums = String::from_utf8_lossy(&get(&sums_url)?).into_owned();
    let expected = sums
        .lines()
        .find_map(|line| {
            line.split_once(char::is_whitespace)
                .filter(|(_, name)| name.trim().trim_start_matches('*') == ASSET)
                .map(|(h, _)| h.to_lowercase())
        })
        .ok_or_else(|| {
            HostError::new(
                "NETWORK",
                "yt-dlp update failed: no checksum for the Windows build.",
            )
        })?;
    let archive = get(&archive_url)?;
    let actual: String = Sha256::digest(&archive)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        return Err(HostError::new(
            "NETWORK",
            "yt-dlp update failed: the download did not match its published checksum.",
        ));
    }
    stage_archive(archive, &managed_ytdlp_dir())?;
    logging::info(format!("yt-dlp {tag} staged (was {current:?})"));
    let applied = apply_pending_locked(&managed_ytdlp_dir());
    Ok(json!({"before": current, "after": tag, "restartRequired": false, "applied": applied}))
}

struct Unpacking(PathBuf);

impl Drop for Unpacking {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stage_archive(archive: Vec<u8>, current: &Path) -> Result<()> {
    // A crash or extraction failure must never leave a runnable partial release
    // in yt-dlp.new. Publish only after the entire archive has been extracted.
    let unpacking = Unpacking(
        current.with_file_name(format!("yt-dlp.unpack-{:016x}", crate::options::random())),
    );
    let staged = &unpacking.0;
    std::fs::create_dir_all(staged)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|e| HostError::new("INTERNAL", format!("yt-dlp update failed: {e}")))?;
    zip.extract(&staged)
        .map_err(|e| HostError::new("INTERNAL", format!("yt-dlp update failed: {e}")))?;
    if !staged.join("yt-dlp.exe").is_file() {
        return Err(HostError::new(
            "INTERNAL",
            "yt-dlp update failed: the archive has no yt-dlp.exe.",
        ));
    }
    let pending = current.with_file_name("yt-dlp.new");
    if pending.exists() {
        std::fs::remove_dir_all(&pending)?;
    }
    std::fs::rename(staged, pending)?;
    Ok(())
}

/// Swap a staged release into place when no yt-dlp is running. Returns true if swapped.
pub fn apply_pending() -> bool {
    let Ok(update_lock) = lock_file("yt-dlp.update.lock") else {
        return false;
    };
    if update_lock.try_lock().is_err() {
        return false;
    }
    apply_pending_locked(&managed_ytdlp_dir())
}

/// Caller holds the update lock, so an extracted executable cannot be mistaken
/// for a complete release while its dependencies are still being unpacked.
fn apply_pending_locked(current: &Path) -> bool {
    let Ok(swap_lock) = lock_file_at(current.parent().unwrap(), "yt-dlp.running.lock") else {
        return false;
    };
    if swap_lock.try_lock().is_err() {
        return false;
    }
    let staged = current.with_file_name("yt-dlp.new");
    if !staged.join("yt-dlp.exe").is_file() {
        return false;
    }
    let retired = current.with_file_name(format!(
        "yt-dlp.old-{:08x}",
        crate::options::random() as u32
    ));
    if current.exists() && std::fs::rename(&current, &retired).is_err() {
        return false;
    }
    if let Err(e) = std::fs::rename(&staged, &current) {
        logging::error(format!("yt-dlp swap failed: {e}"));
        let _ = std::fs::rename(&retired, &current);
        return false;
    }
    remove_retired(current.parent().unwrap_or(Path::new(".")));
    // Unpacked launchers can have the same size/timestamp across releases.
    let _ = std::fs::remove_file(current.with_file_name("yt-dlp.version.json"));
    true
}

fn remove_retired(parent: &Path) {
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("yt-dlp.old-")
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// A shared OS lock prevents another host from swapping a live installation.
pub struct Running {
    _lock: std::fs::File,
}

impl Running {
    pub fn start() -> Result<Running> {
        apply_pending();
        let lock = lock_file("yt-dlp.running.lock")?;
        lock.lock_shared()?;
        Ok(Running { _lock: lock })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn incomplete_archive_never_replaces_pending_release() {
        let dir = Unpacking(std::env::temp_dir().join(format!(
            "siphon-update-test-{:016x}",
            crate::options::random()
        )));
        let current = dir.0.join("yt-dlp");
        let pending = dir.0.join("yt-dlp.new");
        std::fs::create_dir_all(&pending).unwrap();
        std::fs::write(pending.join("yt-dlp.exe"), b"previous").unwrap();
        assert!(stage_archive(archive(&[("dependency.dll", b"incomplete")]), &current).is_err());
        assert_eq!(
            std::fs::read(pending.join("yt-dlp.exe")).unwrap(),
            b"previous"
        );
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
        stage_archive(
            archive(&[
                ("yt-dlp.exe", b"new"),
                ("_internal/dependency.dll", b"ready"),
            ]),
            &current,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(pending.join("_internal/dependency.dll")).unwrap(),
            b"ready"
        );
    }

    #[test]
    fn swap_waits_for_all_readers_and_invalidates_version() {
        let dir = Unpacking(std::env::temp_dir().join(format!(
            "siphon-swap-test-{:016x}",
            crate::options::random()
        )));
        let current = dir.0.join("yt-dlp");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("yt-dlp.exe"), b"old").unwrap();
        std::fs::write(dir.0.join("yt-dlp.version.json"), b"cached").unwrap();
        stage_archive(archive(&[("yt-dlp.exe", b"new")]), &current).unwrap();
        let first = lock_file_at(&dir.0, "yt-dlp.running.lock").unwrap();
        let second = lock_file_at(&dir.0, "yt-dlp.running.lock").unwrap();
        first.lock_shared().unwrap();
        second.lock_shared().unwrap();
        assert!(!apply_pending_locked(&current));
        drop(first);
        assert!(!apply_pending_locked(&current));
        assert_eq!(std::fs::read(current.join("yt-dlp.exe")).unwrap(), b"old");
        drop(second);
        assert!(apply_pending_locked(&current));
        assert_eq!(std::fs::read(current.join("yt-dlp.exe")).unwrap(), b"new");
        assert!(!dir.0.join("yt-dlp.version.json").exists());
        assert!(!dir.0.join("yt-dlp.new").exists());
    }
}
