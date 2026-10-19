//! Start at login. The operating system owns the state: there is no
//! "autostart on" setting of our own, and nothing here turns it on except an
//! explicit [`Autostart::set`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::bail;
use serde::{Deserialize, Serialize};

pub mod macos;
pub mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutostartState {
    Enabled,
    Disabled,
    /// Registered, but turned off by the user in the OS (Windows Task
    /// Manager › Startup).
    DisabledByUser,
    /// macOS: the user has to allow it in System Settings › Login Items.
    NeedsApproval,
    /// Not possible here (macOS without an app bundle, other platforms).
    Unavailable,
}

pub trait Autostart: Send + Sync {
    fn state(&self) -> AutostartState;
    /// Turns autostart on or off; returns the state afterwards.
    fn set(&self, enabled: bool) -> anyhow::Result<AutostartState>;
    /// Points an enabled entry at the current program if it targets another
    /// one (app moved). Never turns autostart on.
    fn repair(&self) -> anyhow::Result<()>;
}

/// Whether [`Autostart::repair`] rewrites the entry: only an active entry
/// pointing elsewhere.
pub fn should_repair(state: AutostartState, targets_program: bool) -> bool {
    state == AutostartState::Enabled && !targets_program
}

/// Autostart for this platform and `program`, or one that reports
/// [`AutostartState::Unavailable`].
pub fn platform_autostart(program: PathBuf) -> Arc<dyn Autostart> {
    native(program).unwrap_or_else(|e| {
        tracing::info!(error = %format!("{e:#}"), "autostart unavailable");
        Arc::new(UnavailableAutostart)
    })
}

#[cfg(target_os = "macos")]
fn native(program: PathBuf) -> anyhow::Result<Arc<dyn Autostart>> {
    Ok(Arc::new(macos::MacAutostart::new(program)?))
}

#[cfg(windows)]
fn native(program: PathBuf) -> anyhow::Result<Arc<dyn Autostart>> {
    Ok(Arc::new(windows::WinAutostart::new(program)))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn native(_program: PathBuf) -> anyhow::Result<Arc<dyn Autostart>> {
    bail!("no autostart on this platform")
}

/// Autostart where there is none.
pub struct UnavailableAutostart;

impl Autostart for UnavailableAutostart {
    fn state(&self) -> AutostartState {
        AutostartState::Unavailable
    }

    fn set(&self, _enabled: bool) -> anyhow::Result<AutostartState> {
        bail!("autostart is not available")
    }

    fn repair(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// In-memory autostart for tests.
#[derive(Debug)]
pub struct MemoryAutostart {
    inner: Mutex<Memory>,
}

#[derive(Debug)]
struct Memory {
    state: AutostartState,
    fail_next: bool,
    stale_target: bool,
}

impl MemoryAutostart {
    pub fn new(state: AutostartState) -> Self {
        Self {
            inner: Mutex::new(Memory {
                state,
                fail_next: false,
                stale_target: false,
            }),
        }
    }

    /// Simulates a change made in the OS.
    pub fn set_state(&self, state: AutostartState) {
        self.lock().state = state;
    }

    /// Makes the next `set` or `repair` fail without changing anything.
    pub fn fail_next(&self) {
        self.lock().fail_next = true;
    }

    /// Simulates an entry pointing at another program.
    pub fn set_stale_target(&self, stale: bool) {
        self.lock().stale_target = stale;
    }

    pub fn stale_target(&self) -> bool {
        self.lock().stale_target
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Autostart for MemoryAutostart {
    fn state(&self) -> AutostartState {
        self.lock().state
    }

    fn set(&self, enabled: bool) -> anyhow::Result<AutostartState> {
        let mut m = self.lock();
        if std::mem::take(&mut m.fail_next) {
            bail!("simulated autostart failure");
        }
        m.state = if enabled {
            AutostartState::Enabled
        } else {
            AutostartState::Disabled
        };
        m.stale_target = false;
        Ok(m.state)
    }

    fn repair(&self) -> anyhow::Result<()> {
        let mut m = self.lock();
        if std::mem::take(&mut m.fail_next) {
            bail!("simulated autostart failure");
        }
        if should_repair(m.state, !m.stale_target) {
            m.stale_target = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AutostartState::*;

    #[test]
    fn repair_never_enables_user_disabled() {
        for state in [Disabled, DisabledByUser, NeedsApproval, Unavailable] {
            assert!(!should_repair(state, false), "{state:?}");
            let a = MemoryAutostart::new(state);
            a.set_stale_target(true);
            a.repair().unwrap();
            assert_eq!(a.state(), state);
            assert!(a.stale_target(), "{state:?} entry left alone");
        }
        assert!(should_repair(Enabled, false));
        assert!(!should_repair(Enabled, true));
        let a = MemoryAutostart::new(Enabled);
        a.set_stale_target(true);
        a.repair().unwrap();
        assert_eq!(a.state(), Enabled);
        assert!(!a.stale_target(), "enabled entry repaired");
    }

    #[test]
    fn memory_autostart_set_and_fail() {
        let a = MemoryAutostart::new(Disabled);
        assert_eq!(a.set(true).unwrap(), Enabled);
        assert_eq!(a.state(), Enabled);
        a.fail_next();
        assert!(a.set(false).is_err());
        assert_eq!(a.state(), Enabled, "failed set changes nothing");
        assert_eq!(a.set(false).unwrap(), Disabled, "failure is one-shot");
        a.fail_next();
        assert!(a.repair().is_err());
        assert!(a.repair().is_ok());
        let user_off = MemoryAutostart::new(DisabledByUser);
        assert_eq!(user_off.set(true).unwrap(), Enabled, "explicit switch on");
    }

    #[test]
    fn unavailable_autostart_refuses_set() {
        let a = UnavailableAutostart;
        assert_eq!(a.state(), Unavailable);
        assert!(a.set(true).is_err());
        assert!(a.repair().is_ok());
    }

    #[test]
    fn state_round_trips_through_json() {
        for state in [
            Enabled,
            Disabled,
            DisabledByUser,
            NeedsApproval,
            Unavailable,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(
                serde_json::from_str::<AutostartState>(&json).unwrap(),
                state
            );
        }
    }

    #[test]
    fn autostart_is_object_safe_and_shareable() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Autostart>();
        let a: Arc<dyn Autostart> = Arc::new(MemoryAutostart::new(Disabled));
        assert_eq!(a.state(), Disabled);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn platform_autostart_unavailable_outside_bundle() {
        let a = platform_autostart(std::env::current_exe().unwrap());
        assert_eq!(a.state(), Unavailable);
        assert!(a.set(true).is_err());
        assert!(a.repair().is_ok());
    }
}
