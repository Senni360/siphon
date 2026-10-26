//! Pinned, Python-free HTDemucs inference. See helper/stems-engine.md.
use crate::{
    errors::{HostError, Result},
    process::Tree,
    ytdlp::Cancel,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const RUNTIME_URL: &str = "https://github.com/0xShug0/audio.cpp/releases/download/v0.9.2-drama3-base-small/audio-v0.9.2-drama3-base-small-bin-windows-x64-cpu-portable.zip";
const RUNTIME_HASH: &str = "9654c8f66f87bda768196d934059be896399b43703b06e5f2b857430775bec3b";
const VULKAN_URL: &str = "https://github.com/0xShug0/audio.cpp/releases/download/v0.9.2-drama3-base-small/audio-v0.9.2-drama3-base-small-bin-windows-x64-vulkan-portable.zip";
const VULKAN_HASH: &str = "9c943d406af3543cc5005d158ed583210cd7a13371bffcd4b7f262f7a6a5ad16";
const MODEL_URL: &str = "https://huggingface.co/audio-cpp/audio.cpp-gguf/resolve/de8b09820cb1d274e18c93534280fc51049096ad/HTDemucs-GGUF/htdemucs-f16.gguf";
const MODEL_HASH: &str = "f8c54ba35df95aafea7881eac214b58ec18a73cbcb6ff5a90f4b762631a286b6";
const STEMS: [&str; 4] = ["vocals", "drums", "bass", "other"];
fn err(message: impl Into<String>) -> HostError {
    HostError::new("STEMS_FAILED", message)
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hash(path: &Path, cancel: &Cancel) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        cancel.check()?;
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>())
}

fn download(
    url: &str,
    path: &Path,
    expected: &str,
    size: u64,
    stage: &str,
    cancel: &Cancel,
    progress: &mut dyn FnMut(&str, Option<f64>),
) -> Result<()> {
    progress(stage, Some(0.0));
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(10)))
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_per_read(Some(Duration::from_secs(3)))
        .build()
        .new_agent();
    cancel.check()?;
    let mut response = agent
        .get(url)
        .header("User-Agent", "Siphon-stems/1")
        .call()
        .map_err(|e| HostError::new("NETWORK", format!("Stem download: {e}")))?;
    let mut reader = response.body_mut().with_config().limit(size + 1).reader();
    let mut file = File::create(path)?;
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut buf = [0; 65536];
    loop {
        cancel.check()?;
        let n = reader
            .read(&mut buf)
            .map_err(|e| HostError::new("NETWORK", format!("Stem download: {e}")))?;
        cancel.check()?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > size {
            return Err(err("Stem download exceeded pinned size"));
        }
        file.write_all(&buf[..n])?;
        digest.update(&buf[..n]);
        progress(stage, Some(count as f64 / size as f64));
    }
    if count != size
        || digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            != expected
    {
        return Err(err("Stem download checksum/size mismatch"));
    }
    file.sync_all()?;
    Ok(())
}

fn runtime(
    backend: Option<&str>,
    cancel: &Cancel,
    progress: &mut dyn FnMut(&str, Option<f64>),
) -> Result<PathBuf> {
    let root = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or_else(|| err("LOCALAPPDATA is unavailable"))?,
    )
    .join("Siphon/stems");
    fs::create_dir_all(&root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("install.lock"))?;
    let start = Instant::now();
    loop {
        cancel.check()?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock)
                if start.elapsed() < Duration::from_secs(660) =>
            {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(err(format!("Stem cache lock: {e}"))),
        }
    }
    let work = root.join(format!("install-{}", std::process::id()));
    // This directory belongs exclusively to this locked installer, including recovery after a crash.
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    fs::create_dir(&work)?;
    let _cleanup = Scratch(work.clone());
    // The model is shared across backends and acquired exactly once per separation.
    let Some(backend) = backend else {
        let model = root.join("htdemucs-f16.gguf");
        if !model.is_file() || hash(&model, cancel)? != MODEL_HASH {
            let part = work.join("model.gguf");
            download(
                MODEL_URL,
                &part,
                MODEL_HASH,
                84059168,
                "downloading-model",
                cancel,
                progress,
            )?;
            if model.exists() {
                fs::remove_file(&model)?;
            }
            fs::rename(part, &model)?;
        }
        return Ok(model);
    };
    let (url, expected, size) = if backend == "vulkan" {
        (VULKAN_URL, VULKAN_HASH, 63227361)
    } else {
        (RUNTIME_URL, RUNTIME_HASH, 28786884)
    };
    let installed = root.join(format!("audio-v0.9.2-{backend}-portable"));
    let archive = root.join(format!("audio-v0.9.2-{backend}-portable.zip"));
    if !archive.is_file() || hash(&archive, cancel)? != expected {
        let part = work.join("runtime.zip");
        download(
            url,
            &part,
            expected,
            size,
            "downloading-engine",
            cancel,
            progress,
        )?;
        if archive.exists() {
            fs::remove_file(&archive)?;
        }
        fs::rename(part, &archive)?;
    }
    // Revalidate installed executable and DLLs against the verified archive on every use.
    let mut zip = zip::ZipArchive::new(File::open(&archive)?).map_err(|e| err(e.to_string()))?;
    let unpacked = work.join("runtime");
    fs::create_dir(&unpacked)?;
    let mut valid = installed.is_dir();
    let mut total = 0u64;
    for i in 0..zip.len() {
        cancel.check()?;
        let mut entry = zip.by_index(i).map_err(|e| err(e.to_string()))?;
        let path = entry
            .enclosed_name()
            .ok_or_else(|| err("Unsafe runtime archive path"))?;
        if entry.is_dir() {
            continue;
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(err("Runtime archive contains a symlink"));
        }
        // Only the CLI, its bundled runtime DLLs and license are needed. No scripts are installed.
        if path.components().count() != 1
            || !(path == Path::new("audiocpp_cli.exe")
                || path == Path::new("LICENSE")
                || path.extension().is_some_and(|s| s == "dll"))
        {
            continue;
        }
        if path.to_string_lossy().contains(':') {
            return Err(err("Unsafe runtime archive filename"));
        }
        total += entry.size();
        if total > 100 * 1024 * 1024 {
            return Err(err("Runtime archive exceeds extraction limit"));
        }
        let dest = unpacked.join(&path);
        let mut out = File::create(&dest)?;
        let mut buf = [0; 65536];
        loop {
            cancel.check()?;
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
        }
        drop(out);
        let old = installed.join(&path);
        valid &= old.is_file() && hash(&old, cancel)? == hash(&dest, cancel)?;
    }
    if !unpacked.join("audiocpp_cli.exe").is_file() {
        return Err(err("Runtime CLI missing from archive"));
    }
    if !valid {
        if installed.exists() {
            fs::remove_dir_all(&installed)?;
        }
        fs::rename(&unpacked, &installed)?;
    }
    Ok(installed.join("audiocpp_cli.exe"))
}

fn run(command: &mut Command, log: &Path, cancel: &Cancel, timeout: Duration) -> Result<()> {
    cancel.check()?;
    let output = File::create(log)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(output));
    let tree = Tree::spawn(command)?;
    let attached = cancel.attach(&tree);
    let result = if attached.is_ok() {
        tree.wait(&|| cancel.is_set(), Some(Instant::now() + timeout))
            .map_err(HostError::from)
    } else {
        Err(attached.unwrap_err())
    };
    cancel.detach();
    tree.kill();
    if result.is_err() {
        let _ = tree.wait(&|| true, None);
    }
    cancel.check()?;
    let (status, timed_out) = result?;
    if timed_out {
        return Err(err("Stem processing timed out"));
    }
    if !status.success() {
        let mut file = File::open(log)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(4096)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        return Err(err(format!(
            "Stem process failed ({status}): {}",
            String::from_utf8_lossy(&bytes)
        )));
    }
    Ok(())
}

/// Validate uncompressed 44.1-kHz stereo PCM16 RIFF and return sample frames.
fn wav_frames(path: &Path) -> Result<u64> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = [0; 12];
    file.read_exact(&mut head)?;
    if &head[..4] != b"RIFF" || &head[8..] != b"WAVE" {
        return Err(err("Invalid stem WAV header"));
    }
    let mut format_ok = false;
    let mut data = None;
    while file.stream_position()? + 8 <= len {
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk)?;
        let size = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
        let end = file
            .stream_position()?
            .checked_add(size)
            .ok_or_else(|| err("WAV size overflow"))?;
        if end > len {
            return Err(err("Truncated stem WAV"));
        }
        if &chunk[..4] == b"fmt " && size >= 16 {
            let mut fmt = [0; 16];
            file.read_exact(&mut fmt)?;
            format_ok = u16::from_le_bytes([fmt[0], fmt[1]]) == 1
                && u16::from_le_bytes([fmt[2], fmt[3]]) == 2
                && u32::from_le_bytes(fmt[4..8].try_into().unwrap()) == 44100
                && u16::from_le_bytes([fmt[14], fmt[15]]) == 16;
        }
        if &chunk[..4] == b"data" {
            if size == 0 || size % 4 != 0 {
                return Err(err("Empty or incomplete stem samples"));
            }
            data = Some(size / 4);
        }
        file.seek(SeekFrom::Start(end + size % 2))?;
    }
    if !format_ok {
        return Err(err("Stem must be stereo PCM16 at 44100 Hz"));
    }
    data.ok_or_else(|| err("Stem WAV has no audio"))
}

fn infer(
    cli: &Path,
    backend: &str,
    model: &Path,
    decoded: &Path,
    raw: &Path,
    work: &Path,
    frames: u64,
    cancel: &Cancel,
) -> Result<()> {
    let log = work.join(format!("separate-{backend}.log"));
    // Bound a stalled GPU before falling back. CPU retains the existing long-job limit.
    let timeout = if backend == "vulkan" {
        Duration::from_secs((frames / 44100 * 12).clamp(120, 24 * 3600))
    } else {
        Duration::from_secs(24 * 3600)
    };
    run(
        Command::new(cli)
            .args([
                "--task",
                "sep",
                "--family",
                "htdemucs",
                "--backend",
                backend,
                "--model",
            ])
            .arg(model)
            .arg("--audio")
            .arg(decoded)
            .arg("--out-dir")
            .arg(raw)
            .args(["--out-format", "pcm16", "--threads", "4", "--log"]),
        &log,
        cancel,
        timeout,
    )?;
    // Explicit backend selection plus actual model allocation evidence: never call a
    // silently CPU-executed request a successful GPU attempt.
    if backend == "vulkan"
        && !fs::read_to_string(&log)?.contains("htdemucs.weights.buffer_name Vulkan")
    {
        return Err(err("HTDemucs did not allocate its weights on Vulkan"));
    }
    for stem in STEMS {
        if wav_frames(&raw.join(format!("{stem}.wav")))? != frames {
            return Err(err("Stem duration differs from decoded input"));
        }
    }
    Ok(())
}

pub fn separate(
    input: &Path,
    staging: &Path,
    ffmpeg: &Path,
    cancel: &Cancel,
    progress: &mut dyn FnMut(&str, Option<f64>),
) -> Result<Vec<PathBuf>> {
    cancel.check()?;
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return Err(err("Native stem runtime currently supports Windows x64"));
    }
    if fs::read_dir(staging)?.next().is_some() {
        return Err(err("Stem staging directory must be empty"));
    }
    let input = fs::canonicalize(input)?;
    let staging = fs::canonicalize(staging)?;
    progress("preparing", None);
    let model = runtime(None, cancel, progress)?;
    let work = staging.join(".engine");
    fs::create_dir(&work)?;
    let _cleanup = Scratch(work.clone());
    let decoded = work.join("input.wav");
    progress("decoding", None);
    run(
        Command::new(ffmpeg)
            .args(["-v", "error", "-nostdin", "-y", "-i"])
            .arg(&input)
            .args([
                "-map",
                "0:a:0",
                "-vn",
                "-ar",
                "44100",
                "-ac",
                "2",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&decoded),
        &work.join("decode.log"),
        cancel,
        Duration::from_secs(1800),
    )?;
    let frames = wav_frames(&decoded)?;
    let raw = work.join("stems");
    let gpu_result = (|| {
        let cli = runtime(Some("vulkan"), cancel, progress)?;
        progress("checking-gpu", None);
        let devices = work.join("devices.log");
        run(
            Command::new(&cli).arg("--list-devices"),
            &devices,
            cancel,
            Duration::from_secs(20),
        )?;
        if !fs::read_to_string(devices)?
            .lines()
            .any(|line| line.starts_with("Vulkan:0 ") && line.contains("[GPU]"))
        {
            return Err(err("No Vulkan GPU device is available"));
        }
        progress("separating-gpu", None);
        infer(
            &cli, "vulkan", &model, &decoded, &raw, &work, frames, cancel,
        )
    })();
    if let Err(gpu_error) = gpu_result {
        // Cancellation is terminal, including races after a failed GPU operation.
        cancel.check()?;
        if gpu_error.code == "CANCELLED" {
            return Err(gpu_error);
        }
        progress("cpu-fallback", None);
        // Discard partial GPU output so it can never mask incomplete CPU output.
        if raw.exists() {
            fs::remove_dir_all(&raw)?;
        }
        let cli = runtime(Some("cpu"), cancel, progress)?;
        progress("separating-cpu", None);
        infer(&cli, "cpu", &model, &decoded, &raw, &work, frames, cancel)?;
    }
    cancel.check()?;
    let mut outputs = Vec::new();
    for stem in STEMS {
        let name = format!("{stem}.wav");
        let output = staging.join(&name);
        if let Err(e) = fs::rename(raw.join(name), &output) {
            for p in &outputs {
                let _ = fs::remove_file(p);
            }
            return Err(e.into());
        }
        outputs.push(output);
    }
    progress("separating", Some(1.0));
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_chunks_and_rejects_truncated_audio() {
        let root =
            std::env::temp_dir().join(format!("siphon-stems-wav-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let _cleanup = Scratch(root.clone());
        let path = root.join("test.wav");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&44100u32.to_le_bytes());
        bytes.extend_from_slice(&176400u32.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 0, 2, 0]);
        fs::write(&path, &bytes).unwrap();
        assert_eq!(wav_frames(&path).unwrap(), 1);
        bytes.pop();
        fs::write(&path, &bytes).unwrap();
        assert!(wav_frames(&path).is_err());
    }

    #[test]
    fn precancelled_does_not_touch_paths_or_report_progress() {
        let cancel = Cancel::new();
        cancel.cancel();
        let missing = Path::new("nonexistent-stems-cancel-test");
        let result = separate(missing, missing, missing, &cancel, &mut |_, _| {
            panic!("unexpected progress")
        });
        assert_eq!(result.unwrap_err().code, "CANCELLED");
    }
}
