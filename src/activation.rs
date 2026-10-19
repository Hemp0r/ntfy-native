//! Lets the UI child take the foreground. The core just received the tray
//! click, so it may pass its foreground rights on; the child activates
//! itself once its window is up (`ui::focus`).

/// Windows: lets other processes (the UI child) call `SetForegroundWindow`
/// while the core still holds the foreground from the tray click.
pub fn allow_child_foreground() {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
        // SAFETY: plain Win32 call without pointers.
        if let Err(e) = unsafe { AllowSetForegroundWindow(ASFW_ANY) } {
            tracing::debug!(error = %e, "AllowSetForegroundWindow failed");
        }
    }
}

/// Lets an already running child take the foreground when it is asked to
/// come forward. Main thread (macOS).
pub fn bring_child_forward(pid: u32) {
    allow_child_foreground();
    #[cfg(target_os = "macos")]
    {
        activate_core();
        if !yield_to_child(pid) {
            tracing::debug!(pid, "could not yield activation to the UI process");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = pid;
}

/// macOS: makes the core the active app, so it has activation to yield.
/// Main thread only.
#[cfg(target_os = "macos")]
pub fn activate_core() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    match MainThreadMarker::new() {
        Some(mtm) => NSApplication::sharedApplication(mtm).activate(),
        None => tracing::warn!("activate_core called off the main thread"),
    }
}

/// macOS: whether the child has registered as an application yet (only
/// then activation can be yielded to it). Any thread.
#[cfg(target_os = "macos")]
pub fn child_registered(pid: u32) -> bool {
    // Off the main thread nothing drains autoreleased objects otherwise.
    objc2::rc::autoreleasepool(|_| running_app(pid).is_some())
}

/// macOS: lets the child become active when it asks (cooperative
/// activation, macOS 14+). Main thread only; `false` if the child is not
/// (yet) a registered application.
#[cfg(target_os = "macos")]
pub fn yield_to_child(pid: u32) -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("yield_to_child called off the main thread");
        return false;
    };
    let Some(app) = running_app(pid) else {
        return false;
    };
    NSApplication::sharedApplication(mtm).yieldActivationToApplication(&app);
    true
}

#[cfg(target_os = "macos")]
fn running_app(pid: u32) -> Option<objc2::rc::Retained<objc2_app_kit::NSRunningApplication>> {
    let pid = i32::try_from(pid).ok()?;
    objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
}
