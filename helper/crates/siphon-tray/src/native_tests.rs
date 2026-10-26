//! Opt-in checks of the real Win32 presentation path. These briefly show a
//! fixture window without activating it. Run on an unlocked desktop:
//! cargo test -p siphon-tray native_window -- --ignored --nocapture

use super::*;
use serde_json::json;

#[test]
fn shell_drag_data_contains_files_from_same_and_different_folders() {
    unsafe {
        OleInitialize(None).unwrap();
    }
    let root = std::env::temp_dir().join(format!("siphon-drag-data-{}", std::process::id()));
    std::fs::create_dir_all(root.join("other")).unwrap();
    let paths: Vec<String> = [
        root.join("song å.wav"),
        root.join("second.mp3"),
        root.join("other/third.flac"),
    ]
    .into_iter()
    .map(|p| {
        std::fs::write(&p, b"fixture").unwrap();
        tray::normalized(&p.to_string_lossy())
    })
    .collect();
    for files in [vec![paths[0].clone()], paths[..2].to_vec(), paths.clone()] {
        let data = shell_data_object(&files).unwrap();
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        unsafe {
            let mut medium = data
                .GetData(&format)
                .expect("shell object must offer CF_HDROP to a DAW");
            let hdrop = HDROP(medium.u.hGlobal.0);
            assert_eq!(DragQueryFileW(hdrop, u32::MAX, None), files.len() as u32);
            for (index, expected) in files.iter().enumerate() {
                let count = DragQueryFileW(hdrop, index as u32, None);
                let mut text = vec![0; count as usize + 1];
                DragQueryFileW(hdrop, index as u32, Some(&mut text));
                assert_eq!(
                    String::from_utf16(&text[..count as usize]).unwrap(),
                    *expected
                );
            }
            ReleaseStgMedium(&mut medium);
        }
    }
    for file in paths {
        std::fs::remove_file(file).unwrap();
    }
    std::fs::remove_dir(root.join("other")).unwrap();
    std::fs::remove_dir(root).unwrap();
    unsafe {
        OleUninitialize();
    }
}

#[test]
#[ignore = "briefly presents a native window; requires an interactive Windows desktop"]
fn native_window_renders_animates_and_preserves_focus() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        OleInitialize(None).unwrap();
    }
    let hwnd = unsafe { create_window(w!("SiphonTrayTest"), Some(tray_proc)) };
    assert!(!hwnd.is_invalid(), "native window creation failed");
    let (sender, incoming) = channel();
    let (work, scale) = monitor_at_cursor();
    let mut tray = Tray::new(true);
    let fixture =
        std::env::temp_dir().join(format!("siphon-native-tray-{}.wav", std::process::id()));
    std::fs::write(&fixture, b"fixture").unwrap();
    tray.accept(&json!({"pinned": true, "items": [{"files": [fixture], "title": "Rust tray · native window check", "subtitle": "120 BPM · Am · WAV", "service": "youtube"}]}));
    tray.measured(
        tray.cards[0].id,
        Some(
            (0..56)
                .map(|i| 0.2 + (i as f32 * 0.4).sin().abs() * 0.7)
                .collect(),
        ),
    );
    let app = App {
        tray,
        fonts: Fonts::load(),
        hwnd,
        grabber: HWND::default(),
        grab: None,
        grab_attempts: 0,
        scale,
        work,
        last_tick: Instant::now(),
        animating: false,
        visible: false,
        host_pid: None,
        incoming,
        sender,
        cursor: IDC_ARROW,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    let foreground = unsafe { GetForegroundWindow() };
    with_app(|app| {
        app.start();
        app.render();
    });
    assert_eq!(
        unsafe { GetForegroundWindow() },
        foreground,
        "showing a tray must not take focus"
    );
    let until = Instant::now() + Duration::from_millis(900);
    while Instant::now() < until {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    with_app(|app| {
        assert!(app.visible);
        assert!(
            app.tray.cards[0].presence.value > 0.98,
            "WM_TIMER must advance the initial transparent frame"
        );
        let (x, y, w, h) = app.layout_px();
        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(hwnd, &mut rect).unwrap();
            assert!(IsWindowVisible(hwnd).as_bool());
            let styles = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            assert_eq!(styles & WS_EX_NOACTIVATE.0, WS_EX_NOACTIVATE.0);
            assert_eq!(styles & WS_EX_TOPMOST.0, WS_EX_TOPMOST.0);
            assert_eq!(
                SendMessageW(hwnd, WM_MOUSEACTIVATE, None, None),
                LRESULT(MA_NOACTIVATE as isize)
            );
        }
        assert_eq!(
            (
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top
            ),
            (x, y, w as i32, h as i32)
        );
        assert!(rect.left >= app.work.left && rect.right <= app.work.right);
        assert!(rect.top >= app.work.top && rect.bottom <= app.work.bottom);
        let mut pixels = Pixmap::new(w, h).unwrap();
        app.tray.paint(&mut pixels, scale, &app.fonts);
        assert!(pixels.pixels().iter().any(|p| p.alpha() > 200));
        unsafe {
            present(hwnd, &pixels, x, y).unwrap();
        }
        if let Some(path) = std::env::var_os("SIPHON_TRAY_SNAPSHOT") {
            pixels.save_png(path).unwrap();
        }
        println!("Native tray visible at {x},{y}, {w}x{h}, scale {scale}; animation, styles, focus and layered presentation passed");
    });
    APP.with(|cell| {
        cell.borrow_mut().take();
    });
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_FRAME);
        DestroyWindow(hwnd).unwrap();
        OleUninitialize();
    }
    std::fs::remove_file(fixture).unwrap();
}
