//! Brings the freshly opened UI window to the foreground.

/// Call once, on the first frame, from the main thread.
pub fn activate(frame: &eframe::Frame) {
    #[cfg(target_os = "macos")]
    {
        let _ = frame;
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        match MainThreadMarker::new() {
            Some(mtm) => NSApplication::sharedApplication(mtm).activate(),
            None => tracing::warn!("activate called off the main thread"),
        }
    }
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        let handle = frame.window_handle().map(|h| h.as_raw());
        let Ok(RawWindowHandle::Win32(h)) = handle else {
            tracing::warn!("no Win32 window handle to activate");
            return;
        };
        let hwnd = HWND(h.hwnd.get() as *mut core::ffi::c_void);
        // SAFETY: `hwnd` is the live window owned by this eframe frame.
        if !unsafe { SetForegroundWindow(hwnd) }.as_bool() {
            tracing::debug!("SetForegroundWindow refused");
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = frame;
}
