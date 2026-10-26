//! `siphon-analyze [--max-seconds N] [--range LOW HIGH] FILE...`
//!
//! Prints one JSON line per file: `{"path", "analysis"|"error", "seconds"}`.
//! Used for benchmarking and for parity checks against the original engine.

use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut max_seconds, mut range, mut files) = (None, (70.0, 180.0), Vec::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--max-seconds" => max_seconds = args.next().and_then(|v| v.parse().ok()),
            "--range" => {
                let low = args.next().and_then(|v| v.parse().ok());
                let high = args.next().and_then(|v| v.parse().ok());
                if let (Some(low), Some(high)) = (low, high) {
                    range = (low, high);
                }
            }
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let Some(ffmpeg) = siphon_analysis::find_ffmpeg() else {
        eprintln!("FFmpeg was not found");
        std::process::exit(2);
    };
    let stop = || false;
    let options = siphon_analysis::Options { max_seconds, bpm_range: range, ffmpeg: &ffmpeg, stop: &stop };
    for path in files {
        let start = Instant::now();
        let result = siphon_analysis::analyze_file(&path, &options);
        let mut line = serde_json::json!({"path": path, "seconds": start.elapsed().as_secs_f64()});
        match result {
            Ok(analysis) => line["analysis"] = serde_json::to_value(analysis).unwrap(),
            Err(error) => line["error"] = error.to_string().into(),
        }
        println!("{line}");
    }
}
