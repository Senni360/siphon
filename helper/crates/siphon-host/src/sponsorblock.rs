//! SponsorBlock `music_offtopic` segments, via the k-anonymous hash-prefix API.

use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

const API: &str = "https://sponsor.ajay.app/api/skipSegments/";

/// Non-music segments (start, end) in seconds, merged and sorted.
pub fn music_offtopic(video_id: &str) -> Result<Vec<(f64, f64)>, String> {
    let digest = Sha256::digest(video_id.as_bytes());
    let prefix: String = digest.iter().take(2).map(|b| format!("{b:02x}")).collect();
    let url = format!("{API}{prefix}?service=YouTube&categories=%5B%22music_offtopic%22%5D&actionTypes=%5B%22skip%22%5D");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent
        .get(&url)
        .header("User-Agent", "Siphon")
        .call()
        .map_err(|e| e.to_string())?;
    if response.status() == 404 {
        return Ok(Vec::new()); // No video with this prefix has segments.
    }
    if !response.status().is_success() {
        return Err(format!(
            "SponsorBlock API returned HTTP {}",
            response.status()
        ));
    }
    let body: Value = response.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(segments_for(&body, video_id))
}

pub fn segments_for(body: &Value, video_id: &str) -> Vec<(f64, f64)> {
    let mut segments: Vec<(f64, f64)> = body
        .as_array()
        .into_iter()
        .flatten()
        .filter(|video| video.get("videoID").and_then(Value::as_str) == Some(video_id))
        .flat_map(|video| {
            video
                .get("segments")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter(|s| s.get("category").and_then(Value::as_str) == Some("music_offtopic"))
        .filter(|s| {
            s.get("actionType")
                .and_then(Value::as_str)
                .is_none_or(|a| a == "skip")
        })
        .filter_map(|s| {
            let pair = s.get("segment")?.as_array()?;
            let (start, end) = (pair.first()?.as_f64()?, pair.get(1)?.as_f64()?);
            (start.is_finite() && end > start).then_some((start.max(0.0), end))
        })
        .collect();
    segments.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in segments {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn filters_and_merges() {
        let body = json!([
            {"videoID": "other", "segments": [{"category": "music_offtopic", "segment": [0, 5]}]},
            {"videoID": "me", "segments": [
                {"category": "music_offtopic", "segment": [30, 40], "actionType": "skip"},
                {"category": "music_offtopic", "segment": [0, 10]},
                {"category": "sponsor", "segment": [50, 60]},
                {"category": "music_offtopic", "segment": [35, 45]},
            ]},
        ]);
        assert_eq!(
            super::segments_for(&body, "me"),
            vec![(0.0, 10.0), (30.0, 45.0)]
        );
    }
}
