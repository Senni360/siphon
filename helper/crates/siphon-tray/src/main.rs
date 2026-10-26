//! Floating drag-out tray for finished downloads (Windows).
//!
//! Started by the native helper with one JSON payload on stdin:
//! `{"items": [{"files", "title", "subtitle", "service", "replaces"?}], "pinned", "ffmpeg"}`,
//! `{"grab": item}`, or `{"resident": true, "hostPid"}`. A second launch hands its
//! payload to the running tray over a per-user named pipe and exits, so finished
//! downloads stack in one place. A resident tray stays warm while the helper lives,
//! which makes "grab" possible: the extension reports a drag gesture, and the tray
//! takes over the held mouse button with a card under the cursor, so the drag that
//! continues is a real file drag instead of the browser's.
//!
//! The windows are layered (per-pixel alpha, so transparent areas click through),
//! always on top, and never take focus, so they float over a maximized DAW.

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod paint;
mod tray;

#[cfg(all(test, windows))]
mod native_tests;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use serde_json::Value;
use tiny_skia::Pixmap;

use paint::Fonts;
use tray::{Card, Tray};

#[cfg(windows)]
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        Storage::FileSystem::*,
        System::{
            Com::*, LibraryLoader::GetModuleHandleW, Memory::*, Ole::*, Pipes::*, Threading::*,
        },
        UI::{
            HiDpi::*, Input::KeyboardAndMouse::*, Shell::Common::ITEMIDLIST, Shell::*,
            WindowsAndMessaging::*,
        },
    },
};

const WM_INCOMING: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_FRAME: usize = 1;
const TIMER_WATCHDOG: usize = 2;
const TIMER_REPLAY: usize = 3;
const TIMER_EXPIRE: usize = 4;

enum Incoming {
    Payload(Value),
    Peaks(u64, Option<Vec<f32>>),
}

fn log(message: impl AsRef<str>) {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let dir = std::path::PathBuf::from(base).join("Siphon").join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("tray.log"))
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let _ = writeln!(
            file,
            "{}.{:03} {}",
            now.as_secs(),
            now.subsec_millis(),
            message.as_ref()
        );
    }
}

fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    format!(
        r"\\.\pipe\siphon-dropzone-{}",
        user.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    )
}

/// Hand the payload to a running tray. True if one received it.
fn forward(payload: &Value) -> bool {
    let mut message = serde_json::to_vec(payload).unwrap_or_default();
    message.push(b'\n');
    let deadline = Instant::now() + Duration::from_millis(400);
    loop {
        match std::fs::OpenOptions::new().write(true).open(pipe_name()) {
            Ok(mut stream) => return stream.write_all(&message).is_ok(),
            Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(_) => return false,
        }
    }
}

#[cfg(windows)]
fn notify(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut _)),
            WM_INCOMING,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// Serve the per-user pipe: one newline-delimited JSON payload per connection.
#[cfg(windows)]
fn serve(first: HANDLE, sender: Sender<Incoming>, hwnd: isize) {
    let name = HSTRING::from(pipe_name());
    let mut pipe = first;
    loop {
        unsafe {
            let connected =
                ConnectNamedPipe(pipe, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED;
            if connected {
                let mut data = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let mut read = 0u32;
                    if ReadFile(pipe, Some(&mut buffer), Some(&mut read), None).is_err()
                        || read == 0
                    {
                        break;
                    }
                    data.extend_from_slice(&buffer[..read as usize]);
                    if data.len() > 4 * 1024 * 1024 {
                        break;
                    }
                }
                for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                    match serde_json::from_slice::<Value>(line) {
                        Ok(value @ Value::Object(_)) => {
                            let _ = sender.send(Incoming::Payload(value));
                            notify(hwnd);
                        }
                        _ => log("Rejected tray message"),
                    }
                }
            }
            // Keep one instance alive while creating its replacement. Otherwise
            // a concurrent launcher can acquire FIRST_PIPE_INSTANCE in the gap
            // and create a second tray.
            let next = CreateNamedPipeW(
                &name,
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                65536,
                0,
                None,
            );
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
            pipe = next;
            if pipe.is_invalid() {
                log("Could not recreate the tray pipe");
                return;
            }
        }
    }
}

fn find_ffmpeg(hint: Option<&str>) -> Option<std::path::PathBuf> {
    if let Some(path) = hint.map(std::path::PathBuf::from).filter(|p| p.is_file()) {
        return Some(path);
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|p| p.join("ffmpeg.exe"))
        .find(|p| p.is_file())
}

/// Measure a card's waveform off the UI thread.
fn measure(id: u64, file: String, ffmpeg: Option<String>, sender: Sender<Incoming>, hwnd: isize) {
    std::thread::spawn(move || {
        let peaks = (|| {
            let ffmpeg = find_ffmpeg(ffmpeg.as_deref())?;
            let mut command = std::process::Command::new(ffmpeg);
            command
                .args([
                    "-v", "error", "-nostdin", "-i", &file, "-vn", "-map", "0:a:0", "-ac", "1",
                    "-ar", "2000", "-f", "f32le", "-",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let mut child = command.spawn().ok()?;
            let mut bytes = Vec::new();
            child.stdout.take()?.read_to_end(&mut bytes).ok()?;
            let _ = child.wait();
            let samples: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]).abs())
                .collect();
            tray::envelope(&samples, 56)
        })();
        let _ = sender.send(Incoming::Peaks(id, peaks));
        #[cfg(windows)]
        notify(hwnd);
    });
}

#[cfg(windows)]
struct App {
    tray: Tray,
    fonts: Fonts,
    hwnd: HWND,
    grabber: HWND,
    grab: Option<(Card, Pixmap)>,
    grab_attempts: u32,
    scale: f32,
    work: RECT,
    last_tick: Instant,
    animating: bool,
    visible: bool,
    host_pid: Option<u32>,
    incoming: Receiver<Incoming>,
    sender: Sender<Incoming>,
    cursor: PCWSTR,
}

#[cfg(windows)]
thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

#[cfg(windows)]
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut app| app.as_mut().map(f))
    })
}

#[cfg(windows)]
fn primary_button() -> (VIRTUAL_KEY, MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    // SM_SWAPBUTTON: the physical primary button.
    if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        (VK_RBUTTON, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_RIGHTDOWN)
    } else {
        (VK_LBUTTON, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_LEFTDOWN)
    }
}

#[cfg(windows)]
fn button_held() -> bool {
    unsafe { GetAsyncKeyState(primary_button().0 .0 as i32) as u16 & 0x8000 != 0 }
}

#[cfg(windows)]
fn monitor_at_cursor() -> (RECT, f32) {
    unsafe {
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut x, mut y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
        (info.rcWork, x as f32 / 96.0)
    }
}

/// Copy a premultiplied RGBA pixmap into a 32-bit top-down BGRA DIB section.
#[cfg(windows)]
unsafe fn dib_from(pixmap: &Pixmap, dc: HDC) -> Option<HBITMAP> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: pixmap.width() as i32,
            biHeight: -(pixmap.height() as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    let destination = std::slice::from_raw_parts_mut(bits as *mut u8, pixmap.data().len());
    for (out, pixel) in destination
        .chunks_exact_mut(4)
        .zip(pixmap.data().chunks_exact(4))
    {
        out.copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    Some(bitmap)
}

#[cfg(windows)]
unsafe fn present(hwnd: HWND, pixmap: &Pixmap, x: i32, y: i32) -> windows::core::Result<()> {
    let screen = GetDC(None);
    let memory = CreateCompatibleDC(Some(screen));
    let result = if let Some(bitmap) = dib_from(pixmap, memory) {
        let old = SelectObject(memory, bitmap.into());
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE {
            cx: pixmap.width() as i32,
            cy: pixmap.height() as i32,
        };
        let result = UpdateLayeredWindow(
            hwnd,
            Some(screen),
            Some(&POINT { x, y }),
            Some(&size),
            Some(memory),
            Some(&POINT::default()),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        SelectObject(memory, old);
        let _ = DeleteObject(bitmap.into());
        result
    } else {
        Err(windows::core::Error::from_thread())
    };
    let _ = DeleteDC(memory);
    ReleaseDC(None, screen);
    result
}

#[cfg(windows)]
impl App {
    fn layout_px(&self) -> (i32, i32, u32, u32) {
        let (w, h) = self.tray.size();
        let (pw, ph) = (
            (w * self.scale).ceil() as u32,
            (h * self.scale).ceil() as u32,
        );
        let center = (self.work.left + self.work.right) / 2;
        let x = center - pw as i32 / 2;
        let y = self.work.bottom - (18.0 * self.scale) as i32 - ph as i32
            + (tray::MARGIN * 0.4 * self.scale) as i32;
        (x, y, pw, ph)
    }

    fn render(&mut self) {
        if self.tray.cards.is_empty() {
            return;
        }
        let (x, y, w, h) = self.layout_px();
        if !self.visible {
            log(format!(
                "showing {} card(s) at {x},{y} size {w}x{h} scale {}",
                self.tray.cards.len(),
                self.scale
            ));
        }
        let Some(mut pixmap) = Pixmap::new(w.max(1), h.max(1)) else {
            return;
        };
        self.tray.paint(&mut pixmap, self.scale, &self.fonts);
        if let Err(error) = unsafe { present(self.hwnd, &pixmap, x, y) } {
            log(format!("Could not present tray: {error}"));
            return;
        }
        if !self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            self.visible = true;
        }
    }

    fn start(&mut self) {
        self.last_tick = Instant::now();
        if !self.animating {
            self.animating = true;
            unsafe { SetTimer(Some(self.hwnd), TIMER_FRAME, 8, None) };
        }
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_tick)
            .as_secs_f32()
            .min(1.0 / 30.0);
        self.last_tick = now;
        let busy = self.tray.tick(dt, now);
        if self.tray.cards.is_empty() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.animating = false;
            self.visible = false;
            if !self.tray.resident {
                unsafe { PostQuitMessage(0) };
            }
            return;
        }
        self.render();
        if !busy {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
            }
            self.animating = false;
        }
    }

    fn accept(&mut self, payload: &Value) {
        let effects = self.tray.accept(payload);
        for (id, file) in effects.measure {
            measure(
                id,
                file,
                self.tray.ffmpeg.clone(),
                self.sender.clone(),
                self.hwnd.0 as isize,
            );
        }
        if effects.show {
            if !self.visible {
                // Place on the screen that has the cursor, like a notification.
                (self.work, self.scale) = monitor_at_cursor();
            }
            self.start();
            self.render();
        }
    }

    fn incoming(&mut self) {
        while let Ok(message) = self.incoming.try_recv() {
            match message {
                Incoming::Payload(payload) => {
                    if let Some(pid) = payload.get("hostPid").and_then(Value::as_u64) {
                        self.host_pid = Some(pid as u32);
                    }
                    if payload.get("resident").and_then(Value::as_bool) == Some(true)
                        && !self.tray.resident
                    {
                        self.tray.resident = true;
                        unsafe { SetTimer(Some(self.hwnd), TIMER_WATCHDOG, 5000, None) };
                    }
                    if let Some(ffmpeg) = payload.get("ffmpeg").and_then(Value::as_str) {
                        self.tray.ffmpeg = Some(ffmpeg.to_string());
                    }
                    if let Some(item) = payload.get("grab") {
                        self.arm_grab(item);
                    } else if payload.get("items").is_some() {
                        self.accept(&payload);
                    }
                }
                Incoming::Peaks(id, peaks) => {
                    self.tray.measured(id, peaks);
                    self.start();
                }
            }
        }
        if !self.tray.resident && self.tray.cards.is_empty() && self.grab.is_none() {
            // Files may disappear between the helper's validation and delivery.
            // An empty one-shot request must not leave a hidden process behind.
            unsafe { PostQuitMessage(0) };
        }
    }

    fn mouse(&self, lparam: LPARAM) -> (f32, f32) {
        let x = (lparam.0 & 0xffff) as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
        (x / self.scale, y / self.scale)
    }

    fn height(&self) -> f32 {
        self.tray.size().1
    }

    // Grab: continue a drag the user began in the browser as a native file drag.
    fn arm_grab(&mut self, item: &Value) {
        let files: Vec<String> = item
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|f| std::path::Path::new(f).is_file())
            .map(tray::normalized)
            .collect();
        if files.is_empty() {
            return;
        }
        if !button_held() {
            log("grab ignored: the mouse button was already released");
            return;
        }
        let (work, scale) = monitor_at_cursor();
        let _ = work;
        let card = Card::new(0, item, files, false);
        let mut ghost = Tray::ghost(&card, scale, &self.fonts);
        // An almost invisible fill keeps the whole window hit-testable; fully
        // transparent pixels would let the replayed press fall through.
        for pixel in ghost.pixels_mut() {
            if pixel.alpha() == 0 {
                *pixel = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 0, 1).unwrap();
            }
        }
        self.grab = Some((card, ghost));
        self.grab_attempts = 0;
        self.follow();
        unsafe {
            let _ = ShowWindow(self.grabber, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                self.grabber,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = KillTimer(Some(self.grabber), TIMER_EXPIRE);
            SetTimer(Some(self.grabber), TIMER_REPLAY, 10, None);
        }
    }

    fn follow(&self) {
        let Some((_, ghost)) = &self.grab else { return };
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
            let (_, scale) = monitor_at_cursor();
            if let Err(error) = present(
                self.grabber,
                ghost,
                point.x - (28.0 * scale) as i32,
                point.y - ghost.height() as i32 / 2,
            ) {
                log(format!("Could not present drag card: {error}"));
            }
        }
    }

    /// Move the held press onto the grabber, only once it is really under the cursor.
    /// Replaying earlier would deliver the press to whatever lies beneath (an unwanted
    /// click in another app), so wait until the window is hit-testable at the cursor.
    fn replay(&mut self) {
        if self.grab.is_none() {
            unsafe {
                let _ = KillTimer(Some(self.grabber), TIMER_REPLAY);
            }
            return;
        }
        if !button_held() {
            log("grab abandoned: the mouse button was released first");
            return self.abort_grab();
        }
        self.follow();
        let mut point = POINT::default();
        let under = unsafe {
            let _ = GetCursorPos(&mut point);
            GetAncestor(WindowFromPoint(point), GA_ROOT)
        };
        if under != self.grabber {
            self.grab_attempts += 1;
            if self.grab_attempts > 25 {
                log("grab abandoned: the drag card never appeared under the cursor");
                self.abort_grab();
            }
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.grabber), TIMER_REPLAY);
            let (_, up, down) = primary_button();
            let input = |flags| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
            SendInput(
                &[input(up), input(down)],
                std::mem::size_of::<INPUT>() as i32,
            );
            SetTimer(Some(self.grabber), TIMER_EXPIRE, 700, None);
        }
        log(format!("grab replayed after {} checks", self.grab_attempts));
    }

    fn abort_grab(&mut self) {
        self.grab = None;
        unsafe {
            let _ = KillTimer(Some(self.grabber), TIMER_REPLAY);
            let _ = KillTimer(Some(self.grabber), TIMER_EXPIRE);
            let _ = ShowWindow(self.grabber, SW_HIDE);
        }
    }
}

/// Build the same shell data object used by an Explorer file drag.
#[cfg(windows)]
fn shell_data_object(files: &[String]) -> windows::core::Result<IDataObject> {
    unsafe {
        let absolute: Vec<*mut ITEMIDLIST> = files
            .iter()
            .map(|f| ILCreateFromPathW(&HSTRING::from(f.as_str())))
            .filter(|p| !p.is_null())
            .collect();
        if absolute.is_empty() {
            return Err(windows::core::Error::new(E_INVALIDARG, "No files to drag"));
        }
        let parents: Vec<&str> = files
            .iter()
            .map(|f| f.rsplit_once('\\').map_or("", |(p, _)| p))
            .collect();
        let same_parent = parents.windows(2).all(|w| w[0] == w[1]);
        let folder = if same_parent {
            ILCreateFromPathW(&HSTRING::from(parents[0]))
        } else {
            std::ptr::null_mut()
        };
        let children: Vec<*const ITEMIDLIST> = if folder.is_null() {
            absolute.iter().map(|p| *p as *const _).collect()
        } else {
            absolute
                .iter()
                .map(|p| ILFindLastID(*p) as *const _)
                .collect()
        };
        let data: windows::core::Result<IDataObject> = SHCreateDataObject(
            if folder.is_null() {
                None
            } else {
                Some(folder as *const _)
            },
            Some(&children),
            None,
        );
        for pidl in absolute {
            ILFree(Some(pidl as *const _));
        }
        if !folder.is_null() {
            ILFree(Some(folder as *const _));
        }
        let data = data?;
        // SHCreateDataObject supplies shell IDs, but does not promise CF_HDROP.
        // DAWs commonly require the explicit Unicode DROPFILES representation.
        // https://learn.microsoft.com/en-us/windows/win32/shell/clipboard
        let mut dropfiles = Vec::new();
        for word in [std::mem::size_of::<DROPFILES>() as u32, 0, 0, 0, 1] {
            dropfiles.extend_from_slice(&word.to_le_bytes());
        }
        for file in files {
            for unit in file.encode_utf16().chain(Some(0)) {
                dropfiles.extend_from_slice(&unit.to_le_bytes());
            }
        }
        dropfiles.extend_from_slice(&0u16.to_le_bytes());
        set_drag_bytes(&data, CF_HDROP.0, &dropfiles)?;
        let text: Vec<u8> = files
            .join("\r\n")
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        set_drag_bytes(&data, CF_UNICODETEXT.0, &text)?;
        Ok(data)
    }
}

#[cfg(windows)]
unsafe fn set_drag_bytes(
    data: &IDataObject,
    clipboard_format: u16,
    bytes: &[u8],
) -> windows::core::Result<()> {
    let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
    let target = GlobalLock(global) as *mut u8;
    if target.is_null() {
        let error = windows::core::Error::from_thread();
        let _ = GlobalFree(Some(global));
        return Err(error);
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
    let _ = GlobalUnlock(global);
    let format = FORMATETC {
        cfFormat: clipboard_format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        pUnkForRelease: std::mem::ManuallyDrop::new(None),
    };
    let result = data.SetData(&format, &medium, true);
    if result.is_err() {
        let _ = GlobalFree(Some(global));
    }
    result
}

/// Attach the ghost image and run the modal drag. Returns whether it was dropped.
#[cfg(windows)]
fn drag_files(hwnd: HWND, files: &[String], ghost: &Pixmap, scale: f32) -> bool {
    unsafe {
        match shell_data_object(files) {
            Ok(data) => {
                if let Ok(helper) = CoCreateInstance::<_, IDragSourceHelper>(
                    &CLSID_DragDropHelper,
                    None,
                    CLSCTX_INPROC_SERVER,
                ) {
                    let dc = GetDC(None);
                    if let Some(bitmap) = dib_from(ghost, dc) {
                        let image = SHDRAGIMAGE {
                            sizeDragImage: SIZE {
                                cx: ghost.width() as i32,
                                cy: ghost.height() as i32,
                            },
                            ptOffset: POINT {
                                x: (28.0 * scale) as i32,
                                y: ghost.height() as i32 / 2,
                            },
                            hbmpDragImage: bitmap,
                            crColorKey: COLORREF(0xFFFF_FFFF),
                        };
                        // The helper owns the bitmap once this succeeds.
                        if helper.InitializeFromBitmap(&image, &data).is_err() {
                            let _ = DeleteObject(bitmap.into());
                        }
                    }
                    ReleaseDC(None, dc);
                }
                match SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_LINK) {
                    Ok(effect) => effect != DROPEFFECT_NONE,
                    Err(e) => {
                        log(format!("drag failed: {e}"));
                        false
                    }
                }
            }
            Err(e) => {
                log(format!("could not create the drag data: {e}"));
                false
            }
        }
    }
}

#[cfg(windows)]
fn host_alive(pid: Option<u32>) -> bool {
    let Some(pid) = pid else { return true };
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(handle) => {
                let alive = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
                let _ = CloseHandle(handle);
                alive
            }
            Err(_) => false,
        }
    }
}

#[cfg(windows)]
fn begin_drag(id: u64) {
    // Take what the drag needs, then release the app: DoDragDrop runs a modal loop
    // that dispatches our own window messages.
    let prepared = with_app(|app| {
        app.tray.dragging = true;
        app.tray.pressed = None;
        unsafe {
            let _ = ReleaseCapture();
        }
        let card = app.tray.cards.iter().find(|c| c.id == id)?;
        Some((
            app.hwnd,
            card.files.clone(),
            Tray::ghost(card, app.scale, &app.fonts),
            app.scale,
        ))
    })
    .flatten();
    let Some((hwnd, files, ghost, scale)) = prepared else {
        return;
    };
    let dropped = drag_files(hwnd, &files, &ghost, scale);
    with_app(|app| {
        app.tray.dragging = false;
        if dropped {
            app.tray.dropped(id);
        }
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(app.hwnd, &mut point);
        }
        let height = app.height();
        app.tray.hover(
            point.x as f32 / app.scale,
            point.y as f32 / app.scale,
            height,
        );
        app.start();
    });
}

#[cfg(windows)]
fn grabbed() {
    let prepared = with_app(|app| {
        unsafe {
            let _ = KillTimer(Some(app.grabber), TIMER_EXPIRE);
        }
        let (card, ghost) = app.grab.take()?;
        Some((app.grabber, card, ghost, monitor_at_cursor().1))
    })
    .flatten();
    let Some((grabber, card, ghost, scale)) = prepared else {
        with_app(|app| unsafe {
            let _ = ShowWindow(app.grabber, SW_HIDE);
        });
        return;
    };
    let dropped = drag_files(grabber, &card.files, &ghost, scale);
    log(format!("grab drag finished: dropped={dropped}"));
    with_app(|app| {
        unsafe {
            let _ = ShowWindow(app.grabber, SW_HIDE);
        }
        if dropped {
            // Confirm the drop with a brief card in the tray.
            let payload = serde_json::json!({"items": [{"files": card.files, "title": card.title, "subtitle": card.subtitle}]});
            app.accept(&payload);
            let files = card.files.clone();
            if let Some(shown) = app.tray.cards.iter_mut().find(|c| c.files == files) {
                shown.pinned = false;
                shown.remaining = 1.2;
                shown.dropped_at = Some(Instant::now());
                shown.flash.target = 1.0;
            }
            app.start();
        }
    });
}

#[cfg(windows)]
extern "system" fn tray_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_INCOMING => {
            with_app(App::incoming);
        }
        WM_TIMER => match wparam.0 {
            TIMER_FRAME => {
                with_app(App::frame);
            }
            TIMER_WATCHDOG => {
                with_app(|app| {
                    if app.tray.cards.is_empty() && !host_alive(app.host_pid) {
                        log("helper ended; tray exiting");
                        unsafe { PostQuitMessage(0) };
                    }
                });
            }
            _ => {}
        },
        WM_MOUSEMOVE => {
            let drag = with_app(|app| {
                let (x, y) = app.mouse(lparam);
                unsafe {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
                if let Some((id, px, py)) = app.tray.pressed {
                    let threshold = unsafe { GetSystemMetrics(SM_CXDRAG) } as f32 / app.scale;
                    if !app.tray.dragging && (x - px).abs() + (y - py).abs() >= threshold.max(2.0) {
                        return Some(id);
                    }
                }
                let height = app.height();
                app.tray.hover(x, y, height);
                let over_card = app.tray.hovered.is_some();
                app.cursor = if over_card { IDC_HAND } else { IDC_ARROW };
                app.start();
                None
            })
            .flatten();
            if let Some(id) = drag {
                begin_drag(id);
            }
        }
        WM_MOUSELEAVE => {
            with_app(|app| {
                if app.tray.pressed.is_none() {
                    app.tray.leave();
                    app.start();
                }
            });
        }
        WM_SETCURSOR => {
            if (lparam.0 & 0xffff) as u32 == HTCLIENT {
                with_app(|app| unsafe {
                    if let Ok(cursor) = LoadCursorW(None, app.cursor) {
                        SetCursor(Some(cursor));
                    }
                });
                return LRESULT(1);
            }
        }
        WM_LBUTTONDOWN => {
            with_app(|app| {
                let (x, y) = app.mouse(lparam);
                let height = app.height();
                if let Some(id) = app.tray.card_at(x, y, height) {
                    app.tray.pressed = Some((id, x, y));
                    unsafe { SetCapture(hwnd) };
                    app.start();
                }
            });
        }
        WM_LBUTTONUP => {
            with_app(|app| {
                unsafe {
                    let _ = ReleaseCapture();
                }
                let (x, y) = app.mouse(lparam);
                if let Some((id, _, _)) = app.tray.pressed.take() {
                    if !app.tray.dragging {
                        let height = app.height();
                        app.tray.click(id, x, y, height);
                    }
                }
                app.start();
            });
        }
        WM_RBUTTONDOWN => {
            with_app(|app| {
                let (x, y) = app.mouse(lparam);
                let height = app.height();
                if let Some(id) = app.tray.card_at(x, y, height) {
                    app.tray.dismiss(id);
                    app.start();
                }
            });
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            with_app(|app| {
                (app.work, app.scale) = monitor_at_cursor();
                app.render();
            });
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(windows)]
extern "system" fn grab_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER => match wparam.0 {
            TIMER_REPLAY => {
                with_app(App::replay);
            }
            TIMER_EXPIRE => {
                with_app(App::abort_grab);
            }
            _ => {}
        },
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            log("grab press received");
            grabbed();
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            with_app(|app| unsafe {
                let _ = ShowWindow(app.grabber, SW_HIDE);
            });
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(windows)]
unsafe fn create_window(class: PCWSTR, proc: WNDPROC) -> HWND {
    let instance = GetModuleHandleW(None).unwrap_or_default();
    let wc = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: instance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        log(format!("RegisterClassW failed: {:?}", GetLastError()));
    }
    let created = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        class,
        w!("Siphon"),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(instance.into()),
        None,
    );
    match created {
        Ok(hwnd) => hwnd,
        Err(e) => {
            log(format!("CreateWindowExW failed: {e}"));
            HWND::default()
        }
    }
}

#[cfg(windows)]
fn animations_enabled() -> bool {
    // Honor Windows' "Animation effects" accessibility setting.
    let mut value = windows::core::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut value as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    value.as_bool()
}

#[cfg(windows)]
fn main() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let payload: Value = match serde_json::from_str(if input.trim().is_empty() {
        "{}"
    } else {
        &input
    }) {
        Ok(value @ Value::Object(_)) => value,
        _ => std::process::exit(2),
    };
    // Become the tray, or hand the payload to the one already running.
    let name = HSTRING::from(pipe_name());
    let first = unsafe {
        CreateNamedPipeW(
            &name,
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            0,
            65536,
            0,
            None,
        )
    };
    if first.is_invalid() {
        std::process::exit(if forward(&payload) { 0 } else { 1 });
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);
    }
    let (sender, incoming) = channel();
    let hwnd = unsafe { create_window(w!("SiphonTray"), Some(tray_proc)) };
    let grabber = unsafe { create_window(w!("SiphonGrab"), Some(grab_proc)) };
    if hwnd.is_invalid() || grabber.is_invalid() {
        std::process::exit(1);
    }
    let (work, scale) = monitor_at_cursor();
    let app = App {
        tray: Tray::new(animations_enabled()),
        fonts: Fonts::load(),
        hwnd,
        grabber,
        grab: None,
        grab_attempts: 0,
        scale,
        work,
        last_tick: Instant::now(),
        animating: false,
        visible: false,
        host_pid: None,
        incoming,
        sender: sender.clone(),
        cursor: IDC_ARROW,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    let raw = hwnd.0 as isize;
    let pipe_sender = sender.clone();
    let first = first.0 as isize;
    std::thread::spawn(move || serve(HANDLE(first as *mut _), pipe_sender, raw));
    let resident = payload.get("resident").and_then(Value::as_bool) == Some(true);
    let has_work = payload.get("items").is_some() || payload.get("grab").is_some();
    let _ = sender.send(Incoming::Payload(payload));
    notify(raw);
    if !resident && !has_work {
        return;
    }
    log(format!("tray started (resident={resident})"));
    unsafe {
        let mut message = MSG::default();
        loop {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status <= 0 {
                if status < 0 {
                    log(format!("Message loop failed: {:?}", GetLastError()));
                }
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        OleUninitialize();
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("siphon-tray runs on Windows only");
}
