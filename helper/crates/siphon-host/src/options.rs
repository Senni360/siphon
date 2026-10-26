//! Validate browser options; unknown fields (notably cookies) are never retained.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use siphon_analysis::Notation;

use crate::errors::{HostError, Result};

pub const BOOLEANS: [&str; 12] = [
    "embedMetadata",
    "embedThumbnail",
    "embedChapters",
    "playlistSubfolder",
    "cookiesFromBrowser",
    "sponsorblockRemove",
    "normalize",
    "overwrite",
    "trimSilence",
    "analyze",
    "appendBpmKey",
    "writeBpmKeyTags",
];

pub fn defaults() -> Map<String, Value> {
    let Value::Object(map) = json!({
        "format": "mp3", "quality": "320", "outputDir": null,
        "filenameTemplate": "%(artist,uploader)s - %(title)s.%(ext)s",
        "embedMetadata": true, "embedThumbnail": true, "embedChapters": false,
        "playlist": "single", "playlistSubfolder": true, "cookiesFromBrowser": false,
        "sponsorblockRemove": true, "normalize": false, "trim": null, "trimSilence": true,
        "overwrite": false, "rateLimit": null,
        "analyze": true, "appendBpmKey": true, "writeBpmKeyTags": true,
        "keyNotation": "standard", "bpmKeySuffix": " [{bpm} BPM {key}]", "bpmRange": [70, 180],
    }) else {
        unreachable!()
    };
    map
}

/// Validated download options. `raw` is the canonical JSON echoed in job events.
#[derive(Clone, Debug)]
pub struct Options {
    pub raw: Map<String, Value>,
    pub format: String,
    pub quality: String,
    pub output_dir: Option<PathBuf>,
    pub filename_template: String,
    pub embed_metadata: bool,
    pub embed_thumbnail: bool,
    pub embed_chapters: bool,
    pub playlist_all: bool,
    pub playlist_subfolder: bool,
    pub cookies_from_browser: bool,
    pub sponsorblock_remove: bool,
    pub normalize: bool,
    pub trim: Option<(f64, Option<f64>)>,
    pub trim_silence: bool,
    pub overwrite: bool,
    pub rate_limit: Option<u64>,
    pub analyze: bool,
    pub append_bpm_key: bool,
    pub write_bpm_key_tags: bool,
    pub key_notation: Notation,
    pub bpm_key_suffix: String,
    pub bpm_range: (f64, f64),
}

fn finite(value: &Value) -> Option<f64> {
    // JSON booleans are not numbers here (unlike Python, where bool is an int).
    value.as_f64().filter(|v| v.is_finite())
}

pub fn validate_bpm_range(value: &Value) -> Result<(f64, f64)> {
    let error =
        || HostError::bad_request("bpmRange must contain two increasing bounds within 30–360 BPM.");
    let list = value
        .as_array()
        .filter(|l| l.len() == 2)
        .ok_or_else(error)?;
    let (low, high) = (
        finite(&list[0]).ok_or_else(error)?,
        finite(&list[1]).ok_or_else(error)?,
    );
    if !(30.0 <= low && low < high && high <= 360.0) {
        return Err(error());
    }
    Ok((low, high))
}

pub fn validate_url(value: Option<&Value>) -> Result<String> {
    let error = || HostError::bad_request("A valid HTTP(S) media URL is required.");
    let text = value
        .and_then(Value::as_str)
        .filter(|t| t.len() <= 16384)
        .ok_or_else(error)?;
    let lower = text.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or_else(error)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(userinfo, host)| {
            // Credentials in URLs are refused (they would be logged and persisted).
            if !userinfo.is_empty() {
                ""
            } else {
                host
            }
        });
    if authority.contains('@')
        || host.is_empty()
        || host.starts_with(':')
        || text.chars().any(|c| c.is_control() || c == ' ')
    {
        return Err(error());
    }
    Ok(text.to_string())
}

pub fn parse_rate_limit(value: &Value) -> Result<Option<u64>> {
    let error = |m: &str| HostError::bad_request(m.to_string());
    let text = match value {
        Value::Null => return Ok(None),
        Value::String(s) if s.is_empty() => return Ok(None),
        Value::String(s) => s.trim(),
        _ => return Err(error("rateLimit must be a size such as 2M.")),
    };
    let digits_end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(digits_end);
    let number: f64 = number
        .parse()
        .map_err(|_| error("rateLimit must be a size such as 2M."))?;
    let unit = unit.trim();
    let (prefix, suffix) = unit.split_at(
        unit.chars()
            .next()
            .filter(|c| "kKmMgGtT".contains(*c))
            .map_or(0, |_| 1),
    );
    if !matches!(suffix, "" | "b" | "B" | "ib" | "iB" | "Ib" | "IB") {
        return Err(error("rateLimit must be a size such as 2M."));
    }
    let power =
        "kmgt"
            .find(&prefix.to_lowercase())
            .map_or(0, |i| if prefix.is_empty() { 0 } else { i + 1 });
    let result = number * 1024f64.powi(power as i32);
    if !result.is_finite() || !(1.0..=9.2e18).contains(&result) {
        return Err(error("rateLimit must be a positive, finite size."));
    }
    Ok(Some(result as u64))
}

pub fn validate_options(value: Option<&Value>) -> Result<Options> {
    let supplied = match value {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err(HostError::bad_request("options must be an object.")),
    };
    let mut raw = defaults();
    for (key, value) in supplied {
        if raw.contains_key(&key) {
            raw.insert(key, value);
        }
    }
    fn text(raw: &Map<String, Value>, key: &str) -> Option<String> {
        raw[key].as_str().map(str::to_string)
    }
    let format = text(&raw, "format")
        .filter(|f| ["mp3", "m4a", "opus", "flac", "wav", "ogg", "best"].contains(&f.as_str()));
    let format = format.ok_or_else(|| HostError::bad_request("Unsupported audio format."))?;
    let quality = text(&raw, "quality")
        .filter(|q| ["320", "256", "192", "128", "V0", "best"].contains(&q.as_str()));
    let quality = quality.ok_or_else(|| HostError::bad_request("Unsupported audio quality."))?;
    let playlist = text(&raw, "playlist").filter(|p| p == "single" || p == "all");
    let playlist =
        playlist.ok_or_else(|| HostError::bad_request("playlist must be single or all."))?;
    for key in BOOLEANS {
        if !raw[key].is_boolean() {
            return Err(HostError::bad_request(format!("{key} must be a boolean.")));
        }
    }
    let flags: std::collections::HashMap<&str, bool> = BOOLEANS
        .iter()
        .map(|k| (*k, raw[*k].as_bool().unwrap()))
        .collect();
    let flag = |key: &str| flags[key];
    let bpm_range = validate_bpm_range(&raw["bpmRange"])?;
    raw["bpmRange"] = json!([raw["bpmRange"][0], raw["bpmRange"][1]]);
    let key_notation: Notation =
        serde_json::from_value(raw["keyNotation"].clone()).map_err(|_| {
            HostError::bad_request("keyNotation must be standard, camelot, or openkey.")
        })?;
    let suffix = text(&raw, "bpmKeySuffix")
        .filter(|s| s.chars().count() <= 2048)
        .ok_or_else(|| {
            HostError::bad_request(
                "bpmKeySuffix must be a suffix template of at most 2048 characters.",
            )
        })?;
    siphon_analysis::format_suffix(120.0, "Am", &suffix, key_notation)
        .map_err(HostError::bad_request)?;
    let output_dir = match &raw["outputDir"] {
        Value::Null => None,
        Value::String(s) if !s.trim().is_empty() && !s.contains('\0') && is_absolute(s) => {
            Some(PathBuf::from(s))
        }
        _ => {
            return Err(HostError::bad_request(
                "outputDir must be an absolute directory path.",
            ))
        }
    };
    let template = text(&raw, "filenameTemplate")
        .filter(|t| {
            !t.is_empty()
                && t.chars().count() <= 2048
                && !t.contains(['/', '\\', '\0', '\r', '\n'])
                && t != "."
                && t != ".."
        })
        .ok_or_else(|| {
            HostError::bad_request(
                "filenameTemplate must be a filename without directory separators.",
            )
        })?;
    let rate_limit = parse_rate_limit(&raw["rateLimit"])?;
    let trim = match &raw["trim"] {
        Value::Null => None,
        Value::Object(trim) => {
            let mut bounds = [None, None];
            for (i, key) in ["start", "end"].iter().enumerate() {
                match trim.get(*key) {
                    None | Some(Value::Null) => {}
                    Some(v) => match finite(v).filter(|n| *n >= 0.0) {
                        Some(n) if !v.is_boolean() => bounds[i] = Some(n),
                        _ => {
                            return Err(HostError::bad_request(
                                "Trim times must be finite nonnegative seconds.",
                            ))
                        }
                    },
                }
            }
            let start = bounds[0].unwrap_or(0.0);
            if bounds[1].is_some_and(|end| end <= start) {
                return Err(HostError::bad_request("Trim end must follow start."));
            }
            raw["trim"] = json!({"start": trim.get("start").cloned().unwrap_or(Value::Null), "end": trim.get("end").cloned().unwrap_or(Value::Null)});
            Some((start, bounds[1]))
        }
        _ => {
            return Err(HostError::bad_request(
                "trim must contain start/end seconds.",
            ))
        }
    };
    Ok(Options {
        format,
        quality,
        output_dir,
        filename_template: template,
        embed_metadata: flag("embedMetadata"),
        embed_thumbnail: flag("embedThumbnail"),
        embed_chapters: flag("embedChapters"),
        playlist_all: playlist == "all",
        playlist_subfolder: flag("playlistSubfolder"),
        cookies_from_browser: flag("cookiesFromBrowser"),
        sponsorblock_remove: flag("sponsorblockRemove"),
        normalize: flag("normalize"),
        trim: trim.filter(|(start, end)| *start > 0.0 || end.is_some()),
        trim_silence: flag("trimSilence"),
        overwrite: flag("overwrite"),
        rate_limit,
        analyze: flag("analyze"),
        append_bpm_key: flag("appendBpmKey"),
        write_bpm_key_tags: flag("writeBpmKeyTags"),
        key_notation,
        bpm_key_suffix: suffix,
        bpm_range,
        raw,
    })
}

/// Windows drive (`C:\`) or UNC (`\\server\share`) paths; `/...` elsewhere.
pub fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    if cfg!(windows) {
        (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
            || path.starts_with("\\\\")
            || path.starts_with("//")
    } else {
        Path::new(path).is_absolute()
    }
}

/// Netscape cookie text for one operation, in a private temporary file deleted on drop.
pub struct CookieFile(pub PathBuf);

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub enum Cookies {
    None,
    File(CookieFile),
    Browser(PathBuf),
}

impl Cookies {
    pub fn from_request(request: &Map<String, Value>, from_browser: bool) -> Result<Cookies> {
        match request.get("cookies") {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) if text.len() <= 8 * 1024 * 1024 => {
                let header = text.trim_start();
                if !text.is_empty()
                    && !header.starts_with("# Netscape HTTP Cookie File")
                    && !header.starts_with("# HTTP Cookie File")
                {
                    return Err(HostError::bad_request(
                        "cookies must use the Netscape cookie file format.",
                    ));
                }
                let path =
                    std::env::temp_dir().join(format!("siphon-{:016x}.cookies.txt", random()));
                write_private(&path, text.as_bytes())?;
                return Ok(Cookies::File(CookieFile(path)));
            }
            Some(_) => {
                return Err(HostError::bad_request(
                    "cookies must be Netscape cookie text smaller than 8 MiB.",
                ))
            }
        }
        if from_browser {
            return Ok(Cookies::Browser(
                crate::runtime::local_app_data()
                    .join("imput")
                    .join("Helium")
                    .join("User Data"),
            ));
        }
        Ok(Cookies::None)
    }

    pub fn args(&self) -> Vec<String> {
        match self {
            Cookies::None => vec![],
            Cookies::File(file) => vec!["--cookies".into(), file.0.to_string_lossy().into_owned()],
            Cookies::Browser(profile) => vec![
                "--cookies-from-browser".into(),
                format!("chromium:{}", profile.to_string_lossy()),
            ],
        }
    }

    pub fn present(&self) -> bool {
        !matches!(self, Cookies::None)
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

pub fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_unknown_fields() {
        let options =
            validate_options(Some(&json!({"cookies": "secret", "format": "wav"}))).unwrap();
        assert_eq!(options.format, "wav");
        assert!(!options.raw.contains_key("cookies"));
        assert_eq!(options.raw.len(), defaults().len());
        assert!(options.trim_silence && options.analyze && options.trim.is_none());
    }

    #[test]
    fn rejects_bad_values() {
        for bad in [
            json!({"format": "aiff"}),
            json!({"quality": 320}),
            json!({"embedMetadata": 1}),
            json!({"outputDir": "relative"}),
            json!({"filenameTemplate": "a/b"}),
            json!({"rateLimit": "fast"}),
            json!({"trim": {"start": 5, "end": 2}}),
            json!({"trim": {"start": -1}}),
            json!({"bpmRange": [100, 90]}),
            json!({"keyNotation": "roman"}),
            json!({"bpmKeySuffix": "{nope}"}),
        ] {
            assert_eq!(
                validate_options(Some(&bad)).unwrap_err().code,
                "BAD_REQUEST",
                "{bad}"
            );
        }
        assert!(validate_options(Some(&json!([]))).is_err());
    }

    #[test]
    fn rate_limits_and_urls() {
        assert_eq!(
            parse_rate_limit(&json!("2M")).unwrap(),
            Some(2 * 1024 * 1024)
        );
        assert_eq!(parse_rate_limit(&json!("32K")).unwrap(), Some(32 * 1024));
        assert_eq!(parse_rate_limit(&json!("1.5KiB")).unwrap(), Some(1536));
        assert_eq!(parse_rate_limit(&json!("500")).unwrap(), Some(500));
        assert!(parse_rate_limit(&json!("0")).is_err());
        assert!(validate_url(Some(&json!("https://www.youtube.com/watch?v=x"))).is_ok());
        assert!(validate_url(Some(&json!("http://127.0.0.1:8000/media"))).is_ok());
        for bad in [
            "ftp://x",
            "https://user:pw@host/",
            "https:///path",
            "javascript:alert(1)",
            "https://a b",
        ] {
            assert!(validate_url(Some(&json!(bad))).is_err(), "{bad}");
        }
    }

    #[test]
    fn trim_normalized() {
        let options = validate_options(Some(&json!({"trim": {"start": 2, "end": 7}}))).unwrap();
        assert_eq!(options.trim, Some((2.0, Some(7.0))));
        let options = validate_options(Some(&json!({"trim": {"start": 0, "end": null}}))).unwrap();
        assert_eq!(options.trim, None);
    }
}
