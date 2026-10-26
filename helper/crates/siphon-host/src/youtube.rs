//! YouTube-only recovery: fresh extractions with different player clients/formats.
//!
//! Client policies were checked against yt-dlp 2026.8.19: default = visionos/web
//! (different when authenticated); android_vr has known 403s; web/Safari may expose
//! only SABR; missing_pot exposes unsafe URLs. yt-dlp enforces its PO-token policy.

use crate::errors::HostError;

pub struct Strategy {
    pub name: &'static str,
    pub clients: &'static str,
    pub format: &'static str,
    /// Lowest-bitrate audio for analysis excerpts (never a merged video fallback).
    pub audio_only: &'static str,
}

pub const STRATEGIES: [Strategy; 5] = [
    Strategy {
        name: "default",
        clients: "default",
        format: "bestaudio/best[height<=360]",
        audio_only: "worstaudio",
    },
    Strategy {
        name: "embedded",
        clients: "web_embedded",
        format: "bestaudio/best[height<=360]",
        audio_only: "worstaudio",
    },
    // Change client and source codec after a rejected Opus URL; keep cookies.
    Strategy {
        name: "visionos-aac",
        clients: "visionos",
        format: "bestaudio[ext=m4a]/bestaudio",
        audio_only: "worstaudio[ext=m4a]/worstaudio",
    },
    Strategy {
        name: "safari-hls",
        clients: "web_safari",
        format: "bestaudio[protocol^=m3u8]/best[protocol^=m3u8][height<=360]",
        audio_only: "worstaudio[protocol^=m3u8]",
    },
    // With a PO-token provider this offers token-bearing audio; itag 18 is a token-free last resort.
    Strategy {
        name: "mweb",
        clients: "mweb",
        format: "bestaudio/best[format_id=18]",
        audio_only: "worstaudio",
    },
];

pub fn is_youtube_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let Some(rest) = lower.split("://").nth(1) else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    [
        "youtube.com",
        "youtu.be",
        "youtube-nocookie.com",
        "youtubekids.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

/// Whether another client/format might succeed where this failure happened.
pub fn retryable(error: &HostError) -> bool {
    let message = error.message.to_lowercase();
    // A different client cannot grant permission to private/member-only content.
    if [
        "private video",
        "members-only",
        "video has been removed",
        "video is unavailable",
    ]
    .iter()
    .any(|w| message.contains(w))
    {
        return false;
    }
    if [
        "AGE_RESTRICTED",
        "GEO_BLOCKED",
        "DISK",
        "FFMPEG_MISSING",
        "UNSUPPORTED_URL",
        "CANCELLED",
        "LIVE_NOT_SUPPORTED",
        "BAD_REQUEST",
    ]
    .contains(&error.code.as_str())
    {
        return false;
    }
    [
        "http error 403",
        "http error 429",
        "http error 500",
        "http error 502",
        "http error 503",
        "http error 504",
        "timed out",
        "timeout",
        "connection reset",
        "connection aborted",
        "requested format is not available",
        "no video formats",
        "only images are available",
        "only drm formats",
        "page needs to be reloaded",
        "not available on this app",
        "sign in",
        "not a bot",
        "bot check",
        "this content isn't available, try again later",
    ]
    .iter()
    .any(|w| message.contains(w))
}

/// After every strategy failed: explain bot checks instead of a raw 403.
pub fn exhausted(error: HostError) -> HostError {
    let message = error.message.to_lowercase();
    let blocked = [
        "403",
        "429",
        "sign in",
        "bot",
        "requested format",
        "no video formats",
        "only images",
        "reloaded",
        "this content isn't available",
    ]
    .iter()
    .any(|w| message.contains(w));
    if blocked {
        HostError::new(
            "LOGIN_REQUIRED",
            "YouTube's bot check or playback restrictions blocked every audio strategy. Open the video in Helium, complete any \
             sign-in/bot check, enable site-cookie export in Siphon, and retry. If it persists, update yt-dlp from Siphon and reconnect.",
        )
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_ids_and_retry_policy() {
        assert!(is_youtube_url("https://music.youtube.com/watch?v=abc"));
        assert!(is_youtube_url("https://youtu.be/abc"));
        assert!(!is_youtube_url("https://notyoutube.com/x"));
        assert!(!is_youtube_url("https://youtube.com.evil.org/x"));
        assert!(retryable(&HostError::classify(
            "ERROR: unable to download video data: HTTP Error 403: Forbidden"
        )));
        assert!(!retryable(&HostError::classify(
            "ERROR: Private video. Sign in if you've been granted access"
        )));
        assert!(!retryable(&HostError::classify(
            "ERROR: Sign in to confirm your age"
        )));
        assert_eq!(
            exhausted(HostError::classify("HTTP Error 403: Forbidden")).code,
            "LOGIN_REQUIRED"
        );
    }
}
