//! Child processes without console windows, killable as a whole tree.
//!
//! On Windows every child is placed in its own Job Object, so cancelling a
//! download also stops the FFmpeg processes yt-dlp started for it.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use siphon_analysis::hide_window;

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn for_child(child: &std::process::Child) -> Option<Job> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if AssignProcessToJobObject(handle, child.as_raw_handle() as HANDLE) == 0 {
                    CloseHandle(handle);
                    return None;
                }
                Some(Job(handle))
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod job {
    pub struct Job;
    impl Job {
        pub fn for_child(_: &std::process::Child) -> Option<Job> {
            None
        }
        pub fn terminate(&self) {}
    }
}

/// A running child and its process tree; cloneable handle for cancellation.
#[derive(Clone)]
pub struct Tree {
    inner: Arc<Mutex<(Child, Option<job::Job>)>>,
}

impl Tree {
    pub fn spawn(command: &mut Command) -> std::io::Result<Tree> {
        let child = hide_window(command).spawn()?;
        let job = job::Job::for_child(&child);
        Ok(Tree {
            inner: Arc::new(Mutex::new((child, job))),
        })
    }

    pub fn take_stdout(&self) -> Option<std::process::ChildStdout> {
        self.inner.lock().unwrap().0.stdout.take()
    }

    pub fn take_stderr(&self) -> Option<std::process::ChildStderr> {
        self.inner.lock().unwrap().0.stderr.take()
    }

    pub fn kill(&self) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(job) = &guard.1 {
            job.terminate();
        }
        let _ = guard.0.kill();
    }

    pub fn try_wait(&self) -> std::io::Result<Option<ExitStatus>> {
        self.inner.lock().unwrap().0.try_wait()
    }

    /// Poll until exit; `stop` returning true kills the tree.
    pub fn wait(
        &self,
        stop: &dyn Fn() -> bool,
        deadline: Option<Instant>,
    ) -> std::io::Result<(ExitStatus, bool)> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok((status, false));
            }
            if stop() || deadline.is_some_and(|d| Instant::now() >= d) {
                self.kill();
                let status = self.inner.lock().unwrap().0.wait()?;
                return Ok((status, true));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

/// Line events from a child's stdout and stderr.
pub enum Line {
    Out(String),
    Err(String),
}

/// Forward both pipes line by line (lossy UTF-8) into one channel.
pub fn lines(tree: &Tree) -> mpsc::Receiver<Line> {
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = tree.take_stdout() {
        let sender = sender.clone();
        std::thread::spawn(move || forward(stdout, |l| sender.send(Line::Out(l)).is_ok()));
    }
    if let Some(stderr) = tree.take_stderr() {
        std::thread::spawn(move || forward(stderr, |l| sender.send(Line::Err(l)).is_ok()));
    }
    receiver
}

fn forward(stream: impl Read, mut send: impl FnMut(String) -> bool) {
    let mut reader = BufReader::new(stream);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buffer)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                if !send(line) {
                    return;
                }
            }
        }
    }
}

/// Run to completion, capturing output, killing the tree after `timeout`.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    command.stdin(Stdio::null());
    let tree = Tree::spawn(command)?;
    let stdout = tree.take_stdout();
    let stderr = tree.take_stderr();
    let read = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut s) = stream {
                let _ = s.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let out = read(stdout.map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = read(stderr.map(|s| Box::new(s) as Box<dyn Read + Send>));
    let (status, killed) = tree.wait(&|| false, Some(Instant::now() + timeout))?;
    // A child can exit leaving descendants with inherited pipe handles open.
    // Close its process tree before joining the pipe readers.
    tree.kill();
    let output = Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    };
    if killed {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "process timed out",
        ));
    }
    Ok(output)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn exited_parent_does_not_leave_pipe_readers_waiting_on_descendants() {
        let mut command = Command::new("node");
        command.args(["-e", "require('child_process').spawn(process.execPath, ['-e', 'setTimeout(() => {}, 4000)'], {stdio: 'inherit', windowsHide: true}); setTimeout(() => { console.log('ready'); process.exit(0); }, 150);"])
            .stdout(Stdio::piped()).stderr(Stdio::piped());
        let start = Instant::now();
        let output = output_with_timeout(&mut command, Duration::from_secs(2)).unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("ready"));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "waited for the orphan's inherited pipes"
        );
    }

    #[test]
    fn timeout_reaps_child() {
        let mut command = Command::new("node");
        command
            .args(["-e", "setTimeout(() => {}, 4000)"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        assert_eq!(
            output_with_timeout(&mut command, Duration::from_millis(150))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
    }
}
