//! Keep extractor data small and JSON-safe at the protocol boundary.

use serde_json::{json, Map, Value};

pub fn number(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Number(n)) if n.as_f64().is_some_and(f64::is_finite) => {
            Value::Number(n.clone())
        }
        _ => Value::Null,
    }
}

pub fn string(value: Option<&Value>, limit: usize) -> String {
    let text = match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(false)) => String::new(),
        Some(other) => other.to_string(),
    };
    text.chars().take(limit).collect()
}

fn first_text<'a>(info: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .filter_map(|k| info.get(*k))
        .find(|v| !v.is_null() && v.as_str() != Some(""))
}

pub fn thumbnail(info: &Map<String, Value>) -> Value {
    let direct = info
        .get("thumbnail")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let listed = info
        .get("thumbnails")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .rev()
                .filter_map(|t| t.get("url").and_then(Value::as_str))
                .find(|s| !s.is_empty())
        });
    match direct.or(listed) {
        Some(url) => Value::String(url.chars().take(4096).collect()),
        None => Value::Null,
    }
}

fn http_url(candidate: &str) -> bool {
    crate::options::validate_url(Some(&Value::String(candidate.to_string()))).is_ok()
}

/// Resolve a relative path against the parent page (only `/`, `./`, `../` forms).
fn join_url(base: &str, value: &str) -> String {
    if let Some(rest) = value.strip_prefix("//") {
        let scheme = base.split("://").next().unwrap_or("https");
        return format!("{scheme}://{rest}");
    }
    let Some(scheme_end) = base.find("://") else {
        return value.to_string();
    };
    let after = &base[scheme_end + 3..];
    let origin_end = scheme_end + 3 + after.find('/').unwrap_or(after.len());
    if value.starts_with('/') {
        return format!("{}{value}", &base[..origin_end]);
    }
    let path = &base[..base[..base.find(['?', '#']).unwrap_or(base.len())]
        .rfind('/')
        .filter(|i| *i >= origin_end)
        .map_or(base.len(), |i| i + 1)];
    let mut segments: Vec<&str> = path[origin_end..]
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let mut rest = value;
    loop {
        if let Some(r) = rest.strip_prefix("./") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix("../") {
            segments.pop();
            rest = r;
        } else {
            break;
        }
    }
    let mut joined = base[..origin_end].to_string();
    for segment in segments {
        joined.push('/');
        joined.push_str(segment);
    }
    format!("{joined}/{rest}")
}

pub fn entry_preview(entry: &Map<String, Value>, parent_url: &str) -> Value {
    let mut result = json!({"title": string(entry.get("title"), 512), "duration": number(entry.get("duration"))});
    let mut candidates: Vec<&Value> = ["webpage_url", "url", "original_url"]
        .iter()
        .filter_map(|k| entry.get(*k))
        .collect();
    let formats = entry
        .get("formats")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let audio = |f: &Value| {
        f.get("vcodec").and_then(Value::as_str) == Some("none")
            || f.get("acodec")
                .and_then(Value::as_str)
                .is_some_and(|a| a != "none")
    };
    let format_urls: Vec<Value> = formats
        .iter()
        .filter(|f| audio(f))
        .chain(formats.iter())
        .filter_map(|f| f.get("url").cloned())
        .collect();
    candidates.extend(format_urls.iter());
    for value in candidates {
        let Some(text) = value.as_str().filter(|s| !s.is_empty() && s.len() <= 16384) else {
            continue;
        };
        // A bare extractor id is not a relative URL (e.g. YouTube flat entries).
        let candidate =
            if text.starts_with('/') || text.starts_with("./") || text.starts_with("../") {
                join_url(parent_url, text)
            } else {
                text.to_string()
            };
        if http_url(&candidate) {
            result["url"] = Value::String(candidate);
            return result;
        }
    }
    let identifier = first_text(entry, &["id", "url"]);
    let key = string(first_text(entry, &["ie_key", "extractor_key"]), 4096).to_lowercase();
    match identifier {
        Some(id) if key.contains("youtube") => {
            let id = string(Some(id), 4096);
            let encoded: String = id
                .bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect();
            result["url"] = Value::String(format!("https://www.youtube.com/watch?v={encoded}"));
        }
        // Never invent a playable URL from an opaque id or repeat the parent URL.
        _ => result["downloadable"] = Value::Bool(false),
    }
    result
}

pub fn media_info(info: &Map<String, Value>, url: &str) -> Value {
    let kind = info.get("_type").and_then(Value::as_str);
    let playlist = matches!(kind, Some("playlist" | "multi_video")) || info.contains_key("entries");
    let webpage = string(
        first_text(info, &["webpage_url"]).or(Some(&Value::String(url.into()))),
        4096,
    );
    let formats: Vec<Value> = info
        .get("formats")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .take(300)
                .filter(|f| f.get("acodec").and_then(Value::as_str).is_some_and(|a| a != "none"))
                .map(|f| {
                    json!({"formatId": string(f.get("format_id"), 4096), "ext": string(f.get("ext"), 4096),
                           "acodec": string(f.get("acodec"), 4096), "abr": number(f.get("abr")),
                           "filesize": number(f.get("filesize").filter(|v| !v.is_null()).or(f.get("filesize_approx")))})
                })
                .collect()
        })
        .unwrap_or_default();
    let mut result = json!({
        "url": url, "webpageUrl": webpage,
        "extractor": string(first_text(info, &["extractor", "extractor_key"]), 4096),
        "id": string(info.get("id"), 4096), "title": string(info.get("title"), 4096),
        "uploader": string(first_text(info, &["uploader", "artist"]), 4096),
        "duration": number(info.get("duration")), "thumbnail": thumbnail(info),
        "isPlaylist": playlist,
        "isLive": info.get("is_live").and_then(Value::as_bool).unwrap_or(false) || info.get("live_status").and_then(Value::as_str) == Some("is_live"),
        "formatsAudio": formats,
    });
    if playlist {
        // A 1 MiB frame cannot carry arbitrarily large playlists; keep the full count.
        let entries: Vec<&Map<String, Value>> = info
            .get("entries")
            .and_then(Value::as_array)
            .map(|l| l.iter().filter_map(Value::as_object).take(201).collect())
            .unwrap_or_default();
        let shown = &entries[..entries.len().min(200)];
        result["entries"] = shown.iter().map(|e| entry_preview(e, &webpage)).collect();
        let count = number(first_text(info, &["playlist_count", "n_entries"]));
        result["entryCount"] = if count.as_f64().is_some_and(|c| c != 0.0) {
            count
        } else {
            json!(shown.len())
        };
        if entries.len() > 200 {
            result["entriesTruncated"] = Value::Bool(true);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_resolve_relative_and_youtube_ids() {
        let entry = json!({"title": "A", "url": "/track/a"});
        assert_eq!(
            entry_preview(entry.as_object().unwrap(), "https://x.bandcamp.com/album/b")["url"],
            "https://x.bandcamp.com/track/a"
        );
        let entry = json!({"title": "B", "url": "abc123", "ie_key": "Youtube"});
        assert_eq!(
            entry_preview(
                entry.as_object().unwrap(),
                "https://youtube.com/playlist?list=1"
            )["url"],
            "https://www.youtube.com/watch?v=abc123"
        );
        let entry = json!({"title": "C", "url": "opaque"});
        assert_eq!(
            entry_preview(entry.as_object().unwrap(), "https://x")["downloadable"],
            false
        );
        assert_eq!(
            join_url("https://a.com/x/y/z?q", "../w"),
            "https://a.com/x/w"
        );
    }

    #[test]
    fn media_info_shapes() {
        let info = json!({"_type": "playlist", "title": "P", "webpage_url": "https://s.com/set", "entries": [{"title": "t", "url": "https://s.com/t"}], "playlist_count": 12});
        let result = media_info(info.as_object().unwrap(), "https://s.com/set");
        assert_eq!(result["isPlaylist"], true);
        assert_eq!(result["entryCount"], 12);
        let single = json!({"title": "S", "formats": [{"format_id": "1", "acodec": "opus", "abr": 160.0}, {"format_id": "2", "acodec": "none"}]});
        let result = media_info(single.as_object().unwrap(), "https://s.com/x");
        assert_eq!(result["formatsAudio"].as_array().unwrap().len(), 1);
        assert_eq!(result["thumbnail"], Value::Null);
    }
}
