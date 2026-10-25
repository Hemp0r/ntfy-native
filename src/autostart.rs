//! Starting the app at login: the main app as a login item via SMAppService
//! (macOS), or a value under the current user's `Run` key (Windows).

use std::sync::{Arc, Mutex};

use anyhow::Result;

pub trait Autostart: Send + Sync {
    /// Registers or unregisters the app; a no-op when already in that state.
    fn apply(&self, enabled: bool) -> Result<()>;
}

/// Does nothing; used where the platform has no autostart or the binary
/// does not run as the installed app.
pub struct NoAutostart;

impl Autostart for NoAutostart {
    fn apply(&self, enabled: bool) -> Result<()> {
        tracing::debug!(enabled, "autostart not supported here, skipped");
        Ok(())
    }
}

/// Records every `apply` for tests.
#[derive(Default)]
pub struct RecordingAutostart {
    pub applied: Mutex<Vec<bool>>,
}

impl Autostart for RecordingAutostart {
    fn apply(&self, enabled: bool) -> Result<()> {
        self.applied.lock().unwrap().push(enabled);
        Ok(())
    }
}

/// Whether autostart should be registered: only once something is set up,
/// so a first start does not register before the wizard asked.
pub fn wanted(settings: &crate::settings::Settings) -> bool {
    settings.autostart && !settings.instances.is_empty()
}

pub fn platform_autostart() -> Arc<dyn Autostart> {
    #[cfg(target_os = "macos")]
    {
        if let Some(a) = macos::LoginItem::new() {
            return Arc::new(a);
        }
    }
    #[cfg(windows)]
    {
        match std::env::current_exe() {
            Ok(exe) => return Arc::new(windows::RunKey { exe }),
            Err(e) => tracing::warn!(error = %e, "locating the executable for autostart failed"),
        }
    }
    Arc::new(NoAutostart)
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{Result, anyhow};
    use objc2_foundation::NSBundle;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use super::Autostart;

    pub struct LoginItem;

    impl LoginItem {
        /// `None` outside an app bundle (e.g. `cargo run`): SMAppService
        /// registers bundles only.
        pub fn new() -> Option<Self> {
            if NSBundle::mainBundle().bundleIdentifier().is_none() {
                tracing::debug!("not running from an app bundle, autostart unavailable");
                return None;
            }
            Some(Self)
        }
    }

    impl Autostart for LoginItem {
        fn apply(&self, enabled: bool) -> Result<()> {
            // SAFETY: plain ServiceManagement calls on the main app service;
            // the class is documented as usable from any thread.
            unsafe {
                let service = SMAppService::mainAppService();
                let status = service.status();
                match (enabled, status) {
                    (true, SMAppServiceStatus::Enabled) => {}
                    (true, SMAppServiceStatus::RequiresApproval) => {
                        // Switched off in System Settings › Login Items; only
                        // the user can turn it back on there.
                        tracing::info!("login item needs approval in System Settings");
                    }
                    (true, _) => {
                        service
                            .registerAndReturnError()
                            .map_err(|e| anyhow!("registering the login item: {e}"))?;
                        tracing::info!("login item registered");
                    }
                    (false, SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval) => {
                        service
                            .unregisterAndReturnError()
                            .map_err(|e| anyhow!("unregistering the login item: {e}"))?;
                        tracing::info!("login item unregistered");
                    }
                    (false, _) => {}
                }
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    use anyhow::{Context, Result};
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    use windows::core::HSTRING;

    use super::Autostart;
    use crate::notify::set_registry_string;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "ntfy-native";

    pub struct RunKey {
        pub exe: PathBuf,
    }

    impl Autostart for RunKey {
        fn apply(&self, enabled: bool) -> Result<()> {
            if enabled {
                // Rewritten every time, so a moved exe is picked up.
                let command = format!("\"{}\"", self.exe.display());
                return set_registry_string(RUN_KEY, Some(VALUE_NAME), &command);
            }
            // SAFETY: valid strings; HKCU is a predefined key.
            let err = unsafe {
                RegDeleteKeyValueW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(RUN_KEY),
                    &HSTRING::from(VALUE_NAME),
                )
            };
            if err == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            err.ok()
                .with_context(|| format!(r"deleting HKCU\{RUN_KEY}\{VALUE_NAME}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Instance, Settings};

    #[test]
    fn wanted_only_once_set_up() {
        let mut s = Settings::default();
        assert!(s.autostart, "on by default");
        assert!(!wanted(&s));
        s.instances.push(Instance::default());
        assert!(wanted(&s));
        s.autostart = false;
        assert!(!wanted(&s));
    }
}
