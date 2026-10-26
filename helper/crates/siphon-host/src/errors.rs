//! Map extractor/OS failures to the shared extension contract.

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct HostError {
    pub code: String,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, HostError>;

impl HostError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        HostError {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new("BAD_REQUEST", message)
    }

    pub fn cancelled() -> Self {
        Self::new("CANCELLED", "Download cancelled.")
    }

    pub fn is_cancelled(&self) -> bool {
        self.code == "CANCELLED"
    }

    pub fn details(&self) -> Value {
        json!({"code": self.code, "message": self.message})
    }

    /// Classify a free-text failure (yt-dlp's `ERROR:` line, FFmpeg stderr, ...).
    pub fn classify(message: &str) -> Self {
        let mut message = message
            .trim()
            .strip_prefix("ERROR: ")
            .unwrap_or(message.trim())
            .trim()
            .to_string();
        if message.is_empty() {
            message = "Unknown error".into();
        }
        if let Some(tag) = ffmpeg_http_status(&message) {
            message = format!("HTTP Error {tag} while FFmpeg fetched media. Retry to refresh the media URL. {message}");
        }
        let lower = message.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
        let code = if has(&[
            "decrypt",
            "dpapi",
            "cookie database",
            "could not copy chrome cookie",
            "app-bound",
        ]) {
            return Self::new(
                "LOGIN_REQUIRED",
                format!(
                    "Helium cookies could not be read or decrypted. Enable site-cookie export in Siphon, sign in to the site, and retry. {}",
                    truncate(&message, 1000)
                ),
            );
        } else if has(&[
            "confirm your age",
            "age-restricted",
            "age restricted",
            "inappropriate for some users",
        ]) {
            "AGE_RESTRICTED"
        } else if has(&[
            "private video",
            "sign in",
            "login required",
            "log in",
            "members-only",
            "authentication",
            "http error 401",
        ]) {
            "LOGIN_REQUIRED"
        } else if has(&[
            "not available in your country",
            "geo-restrict",
            "geographic",
            "geo blocked",
            "not available from your location",
        ]) {
            "GEO_BLOCKED"
        } else if lower.contains("drm") {
            "DRM"
        } else if has(&["unsupported url", "no suitable extractor"]) {
            "UNSUPPORTED_URL"
        } else if lower.contains("ffmpeg")
            && has(&[
                "not found",
                "not installed",
                "could not find",
                "unable to find",
            ])
        {
            "FFMPEG_MISSING"
        } else if has(&[
            "no space left",
            "disk full",
            "permission denied",
            "cannot find the path",
            "filename or extension is too long",
        ]) {
            "DISK"
        } else if has(&[
            "http error",
            "timed out",
            "timeout",
            "unable to download",
            "connection",
            "urlopen",
            "name resolution",
            "network",
            "certificate verify",
        ]) {
            "NETWORK"
        } else {
            "INTERNAL"
        };
        if code == "INTERNAL" && has(&["ffmpeg", "audio conversion failed"]) {
            message = format!("Audio processing failed. Retry the download; if it repeats, update FFmpeg and check the host log. {message}");
        }
        Self::new(code, truncate(&message, 4000))
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<std::io::Error> for HostError {
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        // Windows: file/path not found, access denied, write protected, disk full.
        let disk_os = matches!(error.raw_os_error(), Some(2 | 3 | 5 | 19 | 39 | 112 | 206));
        if disk_os
            || matches!(
                error.kind(),
                NotFound | PermissionDenied | ReadOnlyFilesystem | StorageFull
            )
        {
            HostError::new("DISK", error.to_string())
        } else {
            HostError::classify(&error.to_string())
        }
    }
}

pub fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// FFmpeg returns negative AVERROR tags; Windows shows their unsigned 32-bit form,
/// e.g. 3436169992 == -858797304 == HTTP 403.
fn ffmpeg_http_status(message: &str) -> Option<&'static str> {
    let lower = message.to_lowercase();
    let at = lower.find("ffmpeg exited with code")?;
    let rest = lower[at + "ffmpeg exited with code".len()..].trim_start();
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    let value: i64 = digits.parse().ok()?;
    let tag = ((-value) as u32).to_le_bytes();
    if tag[0] != 0xf8 {
        return None;
    }
    Some(match &tag[1..] {
        b"400" => "400 Bad Request",
        b"401" => "401 Unauthorized",
        b"403" => "403 Forbidden",
        b"404" => "404 Not Found",
        b"429" => "429 Too Many Requests",
        b"4XX" => "4xx client error",
        b"5XX" => "5xx server error",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_failures() {
        for (text, code) in [
            (
                "ERROR: [youtube] x: Sign in to confirm your age",
                "AGE_RESTRICTED",
            ),
            (
                "ERROR: [youtube] x: Private video. Sign in",
                "LOGIN_REQUIRED",
            ),
            (
                "ERROR: Unsupported URL: https://example.org",
                "UNSUPPORTED_URL",
            ),
            (
                "ERROR: unable to download video data: HTTP Error 403: Forbidden",
                "NETWORK",
            ),
            (
                "ERROR: [youtube] x: This video is not available in your country",
                "GEO_BLOCKED",
            ),
            ("ERROR: This video is DRM protected", "DRM"),
            ("ffmpeg not found. Please install", "FFMPEG_MISSING"),
            ("something odd", "INTERNAL"),
        ] {
            assert_eq!(HostError::classify(text).code, code, "{text}");
        }
        assert!(HostError::classify("Failed to decrypt with DPAPI")
            .message
            .starts_with("Helium cookies"));
        let ffmpeg = HostError::classify("ffmpeg exited with code 3436169992");
        assert!(
            ffmpeg.message.starts_with("HTTP Error 403 Forbidden"),
            "{}",
            ffmpeg.message
        );
        assert_eq!(ffmpeg.code, "NETWORK");
        assert!(HostError::classify("ffmpeg: conversion broke")
            .message
            .starts_with("Audio processing failed."));
    }
}
