//! File-only diagnostics in `%LOCALAPPDATA%\Siphon\logs\host.log` (2 MB, 3 backups).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 2_000_000;
const BACKUPS: usize = 3;

struct Log {
    path: PathBuf,
    file: Option<File>,
}

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

pub fn init(directory: PathBuf, name: &str) {
    let _ = std::fs::create_dir_all(&directory);
    let path = directory.join(name);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    let _ = LOG.set(Mutex::new(Log { path, file }));
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        write("ERROR", &format!("panic: {info}\n{backtrace}"));
    }));
}

/// UTC `YYYY-MM-DD HH:MM:SS.mmm`, without a date library.
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let (seconds, millis) = (now.as_secs() as i64, now.subsec_millis());
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

pub fn write(level: &str, message: &str) {
    let Some(log) = LOG.get() else { return };
    let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
    let thread = std::thread::current();
    let line = format!(
        "{} {level} {} {message}\n",
        timestamp(),
        thread.name().unwrap_or("thread")
    );
    if log
        .file
        .as_ref()
        .and_then(|f| f.metadata().ok())
        .is_some_and(|m| m.len() + line.len() as u64 > MAX_BYTES)
    {
        log.file = None;
        for index in (1..BACKUPS).rev() {
            let _ = std::fs::rename(
                log.path.with_extension(format!("log.{index}")),
                log.path.with_extension(format!("log.{}", index + 1)),
            );
        }
        let _ = std::fs::rename(&log.path, log.path.with_extension("log.1"));
        log.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log.path)
            .ok();
    }
    if let Some(file) = log.file.as_mut() {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn debug(message: impl AsRef<str>) {
    write("DEBUG", message.as_ref());
}
pub fn info(message: impl AsRef<str>) {
    write("INFO", message.as_ref());
}
pub fn warn(message: impl AsRef<str>) {
    write("WARNING", message.as_ref());
}
pub fn error(message: impl AsRef<str>) {
    write("ERROR", message.as_ref());
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamp_shape() {
        let stamp = super::timestamp();
        assert_eq!(stamp.len(), 24);
        assert!(stamp.starts_with("20"));
    }
}
