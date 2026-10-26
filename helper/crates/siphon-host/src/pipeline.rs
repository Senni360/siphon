//! Turn a raw yt-dlp download into the requested audio file in one FFmpeg pass.
//!
//! SponsorBlock cuts, trim, leading-silence removal and loudness normalization
//! become one filter graph, so lossy audio is encoded once. Without any of them
//! a matching codec is stream-copied (packets unchanged). lofty then writes the
//! metadata and cover art, keeping whatever FFmpeg wrote (such as chapters).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::errors::{HostError, Result};
use crate::options::Options;
use crate::process::{self, Tree};
use crate::ytdlp::Cancel;
use crate::{logging, media};

/// Loudness target (EBU R128 integrated), true-peak ceiling and range.
pub const LOUDNESS: (f64, f64, f64) = (-14.0, -1.0, 11.0);

pub struct Source {
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub duration: Option<f64>,
}

pub fn ffprobe_path(ffmpeg: &Path) -> PathBuf {
    let windows = ffmpeg
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    ffmpeg.with_file_name(if windows { "ffprobe.exe" } else { "ffprobe" })
}

pub fn probe(ffmpeg: &Path, path: &Path) -> Result<Source> {
    let mut command = Command::new(ffprobe_path(ffmpeg));
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = process::output_with_timeout(&mut command, Duration::from_secs(30))
        .map_err(|e| HostError::new("FFMPEG_MISSING", format!("ffprobe could not run: {e}")))?;
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    let stream = json["streams"].get(0).ok_or_else(|| {
        HostError::new(
            "INTERNAL",
            format!(
                "The download contains no audio. {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )
    })?;
    Ok(Source {
        codec: stream["codec_name"].as_str().unwrap_or("").to_string(),
        sample_rate: stream["sample_rate"].as_str().and_then(|r| r.parse().ok()),
        duration: json["format"]["duration"]
            .as_str()
            .and_then(|d| d.parse().ok())
            .filter(|d: &f64| d.is_finite() && *d > 0.0),
    })
}

#[derive(Debug, PartialEq)]
pub struct Target {
    pub ext: &'static str,
    /// FFmpeg encoder; `None` copies the source stream.
    pub encoder: Option<&'static str>,
}

fn native(codec: &str) -> Option<(&'static str, &'static str)> {
    Some(match codec {
        "aac" => ("m4a", "aac"),
        "mp3" => ("mp3", "libmp3lame"),
        "opus" => ("opus", "libopus"),
        "vorbis" => ("ogg", "libvorbis"),
        "flac" => ("flac", "flac"),
        "alac" => ("m4a", "alac"),
        "pcm_s16le" | "pcm_s24le" => ("wav", "pcm_s16le"),
        _ => return None,
    })
}

/// Container and encoder for the requested format. `filtering` forces an encode.
pub fn target(format: &str, source_codec: &str, filtering: bool) -> Target {
    if format == "best" {
        return match native(source_codec) {
            // pcm_s24le must be re-encoded to keep "wav" 16-bit, like the original.
            Some((ext, encoder)) if filtering || source_codec == "pcm_s24le" => Target {
                ext,
                encoder: Some(encoder),
            },
            Some((ext, _)) => Target { ext, encoder: None },
            None => Target {
                ext: "mp3",
                encoder: Some("libmp3lame"),
            },
        };
    }
    let (ext, encoder, same) = match format {
        "mp3" => ("mp3", "libmp3lame", "mp3"),
        "m4a" => ("m4a", "aac", "aac"),
        "opus" => ("opus", "libopus", "opus"),
        "ogg" => ("ogg", "libvorbis", "vorbis"),
        "flac" => ("flac", "flac", "flac"),
        _ => ("wav", "pcm_s16le", "pcm_s16le"),
    };
    Target {
        ext,
        encoder: if !filtering && source_codec == same {
            None
        } else {
            Some(encoder)
        },
    }
}

/// Encoder settings: yt-dlp's quality mapping plus Siphon's safe channel/rate limits.
pub fn quality_args(encoder: &str, quality: &str) -> Vec<String> {
    if matches!(encoder, "flac" | "pcm_s16le" | "alac") {
        return vec![];
    }
    let numeric: f64 = quality.parse().unwrap_or(0.0); // V0/best → 0
    let mut args: Vec<String> = vec![];
    match encoder {
        // libopus rejects >256 kbps per channel; cap every re-encode at 160k.
        "libopus" => {
            return vec![
                "-ar".into(),
                "48000".into(),
                "-b:a".into(),
                format!(
                    "{}k",
                    (if numeric > 10.0 { numeric } else { 160.0 }).min(160.0) as u32
                ),
            ]
        }
        // Fixed 320k is invalid for mono Vorbis; VBR adapts to channels.
        "libvorbis" => {
            let level = match numeric as u32 {
                320 => "9",
                256 => "8",
                192 => "6",
                128 => "4",
                _ => "10",
            };
            return vec!["-ar".into(), "48000".into(), "-q:a".into(), level.into()];
        }
        _ if numeric > 10.0 => args.extend(["-b:a".into(), format!("{}k", numeric as u32)]),
        "libmp3lame" => args.extend(["-q:a".into(), "0".into()]),
        "aac" => args.extend(["-q:a".into(), "4".into()]),
        _ => {}
    }
    args.extend(["-ar".into(), "48000".into()]);
    args
}

/// Seconds before the first sample louder than -60 dBFS, measured from `start`.
/// Zero when the audible start is immediate or nothing is audible within 120 s.
pub fn leading_silence(ffmpeg: &Path, path: &Path, start: f64, cancel: &Cancel) -> Result<f64> {
    const RATE: usize = 44100;
    let threshold = 10f32.powf(-60.0 / 20.0);
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-ss",
            &start.to_string(),
            "-t",
            "120",
            "-i",
        ])
        .arg(path)
        .args([
            "-vn",
            "-map",
            "0:a:0",
            "-ac",
            "2",
            "-ar",
            &RATE.to_string(),
            "-f",
            "f32le",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let tree = Tree::spawn(&mut command)?;
    if let Err(e) = cancel.attach(&tree) {
        cancel.detach();
        return Err(e);
    }
    let mut stdout = tree.take_stdout().expect("piped");
    let mut buffer = vec![0u8; RATE * 8];
    let (mut frames, mut carry) = (0usize, Vec::new());
    let result = loop {
        let n = match stdout.read(&mut buffer) {
            Ok(0) | Err(_) => break 0.0,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buffer[..n]);
        let whole = carry.len() / 8 * 8;
        let found = carry[..whole].chunks_exact(8).position(|frame| {
            let left = f32::from_le_bytes(frame[..4].try_into().unwrap());
            let right = f32::from_le_bytes(frame[4..].try_into().unwrap());
            left.abs().max(right.abs()) > threshold
        });
        if let Some(index) = found {
            break (frames + index) as f64 / RATE as f64;
        }
        frames += whole / 8;
        carry.drain(..whole);
    };
    tree.kill();
    let _ = tree.wait(&|| false, None);
    cancel.detach();
    cancel.check()?;
    Ok(result)
}

/// Intervals of the source to keep, after trim and SponsorBlock removal.
pub fn keep_intervals(
    start: f64,
    end: Option<f64>,
    duration: Option<f64>,
    cuts: &[(f64, f64)],
) -> Vec<(f64, Option<f64>)> {
    let mut keep = vec![(start, end)];
    for &(cut_start, cut_end) in cuts {
        let mut next = Vec::new();
        for (a, b) in keep {
            let b_value = b.or(duration).unwrap_or(f64::INFINITY);
            if cut_end <= a || cut_start >= b_value {
                next.push((a, b));
                continue;
            }
            if cut_start > a {
                next.push((a, Some(cut_start)));
            }
            if cut_end < b_value {
                next.push((cut_end, b));
            }
        }
        keep = next;
    }
    keep.retain(|(a, b)| b.is_none_or(|b| b - a >= 0.05));
    keep
}

/// Map a time on the source timeline onto the kept timeline.
fn shift(time: f64, keep: &[(f64, Option<f64>)]) -> Option<f64> {
    let mut offset = 0.0;
    for &(a, b) in keep {
        let end = b.unwrap_or(f64::INFINITY);
        if time < a {
            return Some(offset);
        }
        if time <= end {
            return Some(offset + time - a);
        }
        offset += end - a;
    }
    Some(offset)
}

fn ffmetadata_chapters(info: &Map<String, Value>, keep: &[(f64, Option<f64>)]) -> Option<String> {
    let chapters = info.get("chapters")?.as_array()?;
    let mut text = String::from(";FFMETADATA1\n");
    let mut count = 0;
    for chapter in chapters {
        let (Some(start), Some(end)) =
            (chapter["start_time"].as_f64(), chapter["end_time"].as_f64())
        else {
            continue;
        };
        let (Some(a), Some(b)) = (shift(start, keep), shift(end, keep)) else {
            continue;
        };
        if b - a < 0.001 {
            continue;
        }
        let title = media::string(chapter.get("title"), 512);
        let escaped: String = title
            .chars()
            .flat_map(|c| {
                if "=;#\\\n".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        text.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={escaped}\n",
            (a * 1000.0).round() as i64,
            (b * 1000.0).round() as i64
        ));
        count += 1;
    }
    (count > 0).then_some(text)
}

fn filter_graph(
    keep: &[(f64, Option<f64>)],
    whole: bool,
    loudnorm: Option<&str>,
) -> Option<String> {
    let mut graph = String::new();
    let mut label = "0:a:0".to_string();
    if !whole {
        let trims: Vec<String> = keep
            .iter()
            .map(|(a, b)| match b {
                Some(b) => format!("atrim=start={a:.6}:end={b:.6},asetpts=PTS-STARTPTS"),
                None => format!("atrim=start={a:.6},asetpts=PTS-STARTPTS"),
            })
            .collect();
        if trims.len() == 1 {
            graph.push_str(&format!("[0:a:0]{}[cut]", trims[0]));
        } else {
            let outputs: String = (0..trims.len()).map(|i| format!("[s{i}]")).collect();
            graph.push_str(&format!("[0:a:0]asplit={}{outputs}", trims.len()));
            for (i, trim) in trims.iter().enumerate() {
                graph.push_str(&format!(";[s{i}]{trim}[k{i}]"));
            }
            let inputs: String = (0..trims.len()).map(|i| format!("[k{i}]")).collect();
            graph.push_str(&format!(";{inputs}concat=n={}:v=0:a=1[cut]", trims.len()));
        }
        label = "cut".into();
    }
    match loudnorm {
        Some(filter) => {
            if !graph.is_empty() {
                graph.push(';');
            }
            graph.push_str(&format!("[{label}]{filter}[out]"));
        }
        None if graph.is_empty() => return None,
        None => graph = graph.replace("[cut]", "[out]"),
    }
    Some(graph)
}

fn run_ffmpeg(ffmpeg: &Path, args: &[String], cancel: &Cancel) -> Result<String> {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-nostdin", "-hide_banner", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let tree = Tree::spawn(&mut command)?;
    if let Err(e) = cancel.attach(&tree) {
        cancel.detach();
        return Err(e);
    }
    let mut stderr = tree.take_stderr().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let waited = tree.wait(&|| cancel.is_set(), None);
    cancel.detach();
    let stderr = reader.join().unwrap_or_default();
    cancel.check()?;
    let (status, _) = waited?;
    if !status.success() {
        logging::error(format!("FFmpeg processing failed; full stderr:\n{stderr}"));
        let lines: Vec<&str> = stderr.trim().lines().collect();
        let tail = lines[lines.len().saturating_sub(8)..].join("\n");
        let tail: String = tail
            .chars()
            .rev()
            .take(1800)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(HostError::classify(&format!(
            "FFmpeg audio processing failed. {tail}"
        )));
    }
    Ok(stderr)
}

/// First pass of two-pass loudnorm: the measured input statistics.
fn measure_loudness(
    ffmpeg: &Path,
    input: &Path,
    keep: &[(f64, Option<f64>)],
    whole: bool,
    cancel: &Cancel,
) -> Result<Map<String, Value>> {
    let (i, tp, lra) = LOUDNESS;
    let filter = format!("loudnorm=I={i}:TP={tp}:LRA={lra}:print_format=json");
    let graph = filter_graph(keep, whole, Some(&filter)).expect("loudnorm graph");
    let args: Vec<String> = [
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-filter_complex".into(),
        graph,
        "-map".into(),
        "[out]".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]
    .into_iter()
    .collect();
    let stderr = run_ffmpeg(
        ffmpeg,
        &[vec!["-loglevel".into(), "info".into()], args].concat(),
        cancel,
    )?;
    let start = stderr.rfind('{').ok_or_else(|| {
        HostError::new(
            "INTERNAL",
            "Audio processing failed: loudness measurement missing (ffmpeg).",
        )
    })?;
    let end = stderr[start..]
        .find('}')
        .map(|e| start + e + 1)
        .unwrap_or(stderr.len());
    serde_json::from_str(&stderr[start..end]).map_err(|_| {
        HostError::new(
            "INTERNAL",
            "Audio processing failed: unreadable loudness measurement (ffmpeg).",
        )
    })
}

pub struct Prepared {
    pub path: PathBuf,
}

/// Encode `raw` into `destination` (a path without extension). Returns the final file.
pub fn prepare(
    ffmpeg: &Path,
    raw: &Path,
    info: &Map<String, Value>,
    options: &Options,
    destination: &Path,
    sponsor: &[(f64, f64)],
    cancel: &Cancel,
) -> Result<Prepared> {
    let source = probe(ffmpeg, raw)?;
    let (mut start, end) = options.trim.unwrap_or((0.0, None));
    let requested_start = start;
    if options.trim_silence {
        // An off-topic intro removed by SponsorBlock would leave the cut point mid-file.
        for &(a, b) in sponsor.iter().filter(|_| options.sponsorblock_remove) {
            if a <= start + 0.05 && start + 0.05 < b {
                start = b;
            }
        }
        let offset = leading_silence(ffmpeg, raw, start, cancel).unwrap_or_else(|e| {
            if e.is_cancelled() {
                return 0.0;
            }
            logging::warn(format!("Could not measure leading silence: {e}"));
            0.0
        });
        cancel.check()?;
        // A stream copy keeps the audio bit-exact; don't re-encode it for an encoder's
        // few-millisecond start-up ramp. Otherwise trim anything from 5 ms.
        let copyable = options.trim.is_none()
            && !options.normalize
            && target(&options.format, &source.codec, false)
                .encoder
                .is_none();
        let minimum = if copyable { 0.05 } else { 0.005 };
        if end.is_none_or(|end| start + offset < end)
            && (start + offset) - requested_start >= minimum
        {
            start += offset;
        } else {
            // The SponsorBlock cut below still removes an off-topic intro.
            start = requested_start;
        }
    }
    let cuts: Vec<(f64, f64)> = if options.sponsorblock_remove {
        sponsor.to_vec()
    } else {
        vec![]
    };
    let keep = keep_intervals(start, end, source.duration, &cuts);
    if keep.is_empty() {
        return Err(HostError::bad_request(
            "Trim and SponsorBlock removal leave no audio.",
        ));
    }
    let whole = keep.len() == 1
        && keep[0].0 < 0.0005
        && keep[0]
            .1
            .is_none_or(|b| source.duration.is_some_and(|d| b >= d - 0.0005));
    let filtering = !whole || options.normalize;
    let target = target(&options.format, &source.codec, filtering);
    // `destination` already has the source extension removed. Replacing an
    // extension here would also remove the last dotted part of the title.
    let mut filename = destination.as_os_str().to_os_string();
    filename.push(format!(".{}", target.ext));
    let final_path = PathBuf::from(filename);
    if final_path.exists() && !options.overwrite {
        // Like yt-dlp's post-processor without overwrites: keep the existing file.
        logging::info(format!("Keeping existing {}", final_path.display()));
        return Ok(Prepared { path: final_path });
    }
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stem = final_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = final_path.with_file_name(format!(
        "{stem}.siphon-{:08x}.{}",
        crate::options::random() as u32,
        target.ext
    ));
    let chapters_file = if options.embed_chapters {
        ffmetadata_chapters(info, &keep)
    } else {
        None
    };
    let chapters_path = chapters_file
        .as_ref()
        .map(|_| temporary.with_extension("chapters.txt"));
    let cleanup = |paths: &[&Option<PathBuf>]| {
        for path in paths.iter().copied().flatten() {
            let _ = std::fs::remove_file(path);
        }
    };
    let result = (|| -> Result<()> {
        if let (Some(text), Some(path)) = (&chapters_file, &chapters_path) {
            std::fs::write(path, text)?;
        }
        let loudnorm = if options.normalize {
            let measured = measure_loudness(ffmpeg, raw, &keep, whole, cancel)?;
            let get = |key: &str| {
                measured
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("0")
                    .to_string()
            };
            let (i, tp, lra) = LOUDNESS;
            // loudnorm works at 192 kHz internally; resample to a normal output rate.
            let rate = if matches!(target.encoder, Some("flac" | "pcm_s16le")) {
                source.sample_rate.unwrap_or(48000)
            } else {
                48000
            };
            Some(format!(
                "loudnorm=I={i}:TP={tp}:LRA={lra}:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true,aresample={rate}",
                get("input_i"), get("input_tp"), get("input_lra"), get("input_thresh"), get("target_offset")
            ))
        } else {
            None
        };
        let mut args: Vec<String> = vec![
            "-loglevel".into(),
            "error".into(),
            "-i".into(),
            raw.to_string_lossy().into_owned(),
        ];
        if let Some(path) = &chapters_path {
            args.extend([
                "-f".into(),
                "ffmetadata".into(),
                "-i".into(),
                path.to_string_lossy().into_owned(),
            ]);
        }
        match filter_graph(&keep, whole, loudnorm.as_deref()) {
            Some(graph) => args.extend([
                "-filter_complex".into(),
                graph,
                "-map".into(),
                "[out]".into(),
            ]),
            None => args.extend(["-map".into(), "0:a:0".into()]),
        }
        args.extend([
            "-vn".into(),
            "-sn".into(),
            "-dn".into(),
            "-map_metadata".into(),
            "-1".into(),
        ]);
        args.extend([
            "-map_chapters".into(),
            if chapters_path.is_some() {
                "1".into()
            } else {
                "-1".into()
            },
        ]);
        match target.encoder {
            None => {
                args.extend(["-c:a".into(), "copy".into()]);
                if source.codec == "aac" && target.ext == "m4a" {
                    args.extend(["-bsf:a".into(), "aac_adtstoasc".into()]);
                }
            }
            Some(encoder) => {
                args.extend(["-c:a".into(), encoder.into()]);
                args.extend(quality_args(encoder, &options.quality));
            }
        }
        args.push(temporary.to_string_lossy().into_owned());
        run_ffmpeg(ffmpeg, &args, cancel)?;
        if options.overwrite {
            std::fs::rename(&temporary, &final_path)?;
        } else {
            match crate::analysis_service::rename_no_replace(&temporary, &final_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {} // keep the competing output
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    })();
    cleanup(&[&chapters_path, &Some(temporary.clone())]);
    result?;
    Ok(Prepared { path: final_path })
}

fn join_values(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.trim().is_empty() => Some(s.replace('\0', "")),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(|v| join_values(Some(v))).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn first(info: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| join_values(info.get(*k)))
}

/// The cover image yt-dlp wrote (converted to JPEG), if any.
pub fn thumbnail_file(info: &Map<String, Value>) -> Option<PathBuf> {
    info.get("thumbnails")?
        .as_array()?
        .iter()
        .rev()
        .filter_map(|t| t.get("filepath").and_then(Value::as_str))
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// yt-dlp's FFmpegMetadataPP field mapping, written with lofty.
pub fn write_metadata(
    path: &Path,
    info: &Map<String, Value>,
    metadata: bool,
    cover: Option<&Path>,
) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::tag::{ItemKey, Tag};

    if !metadata && cover.is_none() {
        return Ok(());
    }
    let mut tagged = lofty::read_from_path(path).map_err(|e| {
        HostError::new(
            "INTERNAL",
            format!("Could not read the audio file to tag it: {e}"),
        )
    })?;
    if tagged.primary_tag_mut().is_none() {
        let kind = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(kind));
    }
    let tag = tagged.primary_tag_mut().expect("inserted");
    if metadata {
        let date = first(info, &["release_date", "upload_date"])
            .filter(|d| d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()));
        let fields: [(ItemKey, Option<String>); 11] = [
            (ItemKey::TrackTitle, first(info, &["track", "title"])),
            (
                ItemKey::TrackArtist,
                first(
                    info,
                    &[
                        "artist",
                        "artists",
                        "creator",
                        "creators",
                        "uploader",
                        "uploader_id",
                    ],
                ),
            ),
            (ItemKey::AlbumTitle, first(info, &["album"])),
            (
                ItemKey::AlbumArtist,
                first(info, &["album_artist", "album_artists"]),
            ),
            (ItemKey::Composer, first(info, &["composer", "composers"])),
            (ItemKey::Genre, first(info, &["genre", "genres"])),
            (ItemKey::TrackNumber, first(info, &["track_number"])),
            (ItemKey::DiscNumber, first(info, &["disc_number"])),
            (
                ItemKey::RecordingDate,
                date.map(|d| format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..])),
            ),
            (ItemKey::Comment, first(info, &["webpage_url"])),
            (ItemKey::Description, first(info, &["description"])),
        ];
        for (key, value) in fields {
            if let Some(value) = value {
                tag.insert_text(key, value);
            }
        }
    }
    if let Some(cover) = cover {
        match std::fs::read(cover) {
            Ok(data) => {
                let mime = if data.starts_with(&[0x89, b'P', b'N', b'G']) {
                    MimeType::Png
                } else {
                    MimeType::Jpeg
                };
                tag.push_picture(
                    Picture::unchecked(data)
                        .pic_type(PictureType::CoverFront)
                        .mime_type(mime)
                        .build(),
                );
            }
            Err(e) => logging::warn(format!("Skipping artwork: {e}")),
        }
    }
    tagged
        .save_to_path(path, WriteOptions::default().use_id3v23(true))
        .map_err(|e| HostError::new("INTERNAL", format!("Could not write tags: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_are_clipped_at_trim_boundaries() {
        let info = serde_json::json!({"chapters": [
            {"start_time": 0, "end_time": 4, "title": "before"},
            {"start_time": 3, "end_time": 12, "title": "overlap"},
            {"start_time": 12, "end_time": 16, "title": "after"}
        ]});
        let text = ffmetadata_chapters(info.as_object().unwrap(), &[(5.0, Some(10.0))]).unwrap();
        assert!(text.contains("START=0\nEND=5000\ntitle=overlap"), "{text}");
        assert_eq!(text.matches("[CHAPTER]").count(), 1);
    }

    #[test]
    fn targets_copy_only_without_filters() {
        assert_eq!(
            target("best", "aac", false),
            Target {
                ext: "m4a",
                encoder: None
            }
        );
        assert_eq!(
            target("best", "aac", true),
            Target {
                ext: "m4a",
                encoder: Some("aac")
            }
        );
        assert_eq!(
            target("best", "weird", false),
            Target {
                ext: "mp3",
                encoder: Some("libmp3lame")
            }
        );
        assert_eq!(
            target("opus", "opus", false),
            Target {
                ext: "opus",
                encoder: None
            }
        );
        assert_eq!(
            target("opus", "aac", false),
            Target {
                ext: "opus",
                encoder: Some("libopus")
            }
        );
        assert_eq!(target("ogg", "vorbis", false).encoder, None);
        assert_eq!(
            target("wav", "opus", false),
            Target {
                ext: "wav",
                encoder: Some("pcm_s16le")
            }
        );
    }

    #[test]
    fn quality_mapping() {
        assert_eq!(
            quality_args("libmp3lame", "320"),
            ["-b:a", "320k", "-ar", "48000"]
        );
        assert_eq!(
            quality_args("libmp3lame", "V0"),
            ["-q:a", "0", "-ar", "48000"]
        );
        assert_eq!(
            quality_args("libopus", "320"),
            ["-ar", "48000", "-b:a", "160k"]
        );
        assert_eq!(
            quality_args("libopus", "128"),
            ["-ar", "48000", "-b:a", "128k"]
        );
        assert_eq!(
            quality_args("libvorbis", "best"),
            ["-ar", "48000", "-q:a", "10"]
        );
        assert!(quality_args("flac", "320").is_empty());
    }

    #[test]
    fn intervals_and_timeline() {
        let keep = keep_intervals(2.0, Some(60.0), Some(100.0), &[(0.0, 5.0), (20.0, 30.0)]);
        assert_eq!(keep, vec![(5.0, Some(20.0)), (30.0, Some(60.0))]);
        assert_eq!(shift(25.0, &keep), Some(15.0));
        assert_eq!(shift(40.0, &keep), Some(25.0));
        assert_eq!(
            keep_intervals(0.0, None, Some(10.0), &[]),
            vec![(0.0, None)]
        );
        assert!(keep_intervals(0.0, Some(3.0), None, &[(0.0, 5.0)]).is_empty());
        let graph = filter_graph(&keep, false, None).unwrap();
        assert!(
            graph.contains("asplit=2") && graph.contains("concat=n=2:v=0:a=1[out]"),
            "{graph}"
        );
        assert_eq!(filter_graph(&[(0.0, None)], true, None), None);
    }
}
