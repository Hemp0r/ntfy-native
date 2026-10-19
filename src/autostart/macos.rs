//! macOS: `SMAppService.mainApp`, falling back to a LaunchAgent when the
//! app is not properly signed.

use std::path::{Path, PathBuf};

use super::AutostartState;
use crate::paths::APP_ID;

#[cfg(target_os = "macos")]
pub use sys::MacAutostart;

/// `SMAppServiceStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmStatus {
    Enabled,
    NotRegistered,
    NotFound,
    RequiresApproval,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    ServiceManagement,
    LaunchAgent,
}

/// `~/Library/LaunchAgents/dev.hemp0r.ntfy-native.plist`.
pub fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{APP_ID}.plist"))
}

/// LaunchAgent that starts `program` once at login.
pub fn launch_agent_plist(label: &str, program: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{}</string>
	<key>AssociatedBundleIdentifiers</key>
	<array>
		<string>{}</string>
	</array>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
</dict>
</plist>
"#,
        xml_escape(label),
        xml_escape(APP_ID),
        xml_escape(&program.to_string_lossy())
    )
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// `kSMErrorInvalidSignature`.
pub const SM_ERROR_INVALID_SIGNATURE: isize = 3;
/// `kSMErrorLaunchDeniedByUser`.
pub const SM_ERROR_LAUNCH_DENIED_BY_USER: isize = 11;

/// A failed `SMAppService.register`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmError {
    pub code: isize,
    pub message: String,
    /// Status read after the failure.
    pub status: SmStatus,
}

/// Which mechanism serves autostart after trying `SMAppService.register`
/// (`register` = status after it, or its error), and the resulting state.
/// Only a refused signature (ad-hoc builds) or a missing app falls back to a
/// LaunchAgent; a login item the user denied is never bypassed.
pub fn choose_mechanism(
    in_bundle: bool,
    register: Result<SmStatus, SmError>,
) -> Result<(Mechanism, AutostartState), String> {
    use Mechanism::{LaunchAgent, ServiceManagement};
    if !in_bundle {
        return Ok((ServiceManagement, AutostartState::Unavailable));
    }
    let status = match register {
        Err(e) if e.code == SM_ERROR_INVALID_SIGNATURE => {
            return Ok((LaunchAgent, AutostartState::Enabled));
        }
        Err(e) if e.status == SmStatus::NotRegistered => return Err(e.message),
        Err(e) => e.status,
        Ok(status) => status,
    };
    match status {
        SmStatus::Enabled => Ok((ServiceManagement, AutostartState::Enabled)),
        SmStatus::RequiresApproval => Ok((ServiceManagement, AutostartState::NeedsApproval)),
        SmStatus::NotFound => Ok((LaunchAgent, AutostartState::Enabled)),
        SmStatus::NotRegistered => Err("the login item was not registered".into()),
    }
}

/// Current state from the login item's status and our LaunchAgent's
/// background-item status (`None` without a LaunchAgent). Either one active
/// counts as enabled; a LaunchAgent turned off under "Allow in the
/// Background" as disabled by the user.
pub fn current_state(
    in_bundle: bool,
    sm: SmStatus,
    launch_agent: Option<SmStatus>,
) -> AutostartState {
    if !in_bundle {
        return AutostartState::Unavailable;
    }
    match (sm, launch_agent) {
        (SmStatus::Enabled, _) | (_, Some(SmStatus::Enabled)) => AutostartState::Enabled,
        (_, Some(SmStatus::RequiresApproval)) => AutostartState::DisabledByUser,
        (SmStatus::RequiresApproval, _) => AutostartState::NeedsApproval,
        _ => AutostartState::Disabled,
    }
}

/// Whether macOS runs the app from a temporary copy (Gatekeeper App
/// Translocation); such a path is gone after the next login.
pub fn is_translocated(program: &Path) -> bool {
    program
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation")
}

#[cfg(target_os = "macos")]
mod sys {
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;

    use anyhow::{Context, Result, anyhow, bail};
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSBundle, NSString, NSURL};
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use super::{
        Mechanism, SM_ERROR_INVALID_SIGNATURE, SmError, SmStatus, choose_mechanism, current_state,
        is_translocated, launch_agent_path, launch_agent_plist,
    };
    use crate::autostart::{Autostart, AutostartState, should_repair};
    use crate::notify::write_if_changed;
    use crate::paths::APP_ID;

    pub struct MacAutostart {
        program: PathBuf,
        plist: PathBuf,
        in_bundle: bool,
        force_launch_agent: bool,
    }

    impl MacAutostart {
        pub fn new(program: PathBuf) -> Result<Self> {
            let home = directories::BaseDirs::new()
                .context("could not determine the home directory")?
                .home_dir()
                .to_path_buf();
            let in_bundle =
                autoreleasepool(|_| NSBundle::mainBundle().bundleIdentifier().is_some());
            Ok(Self {
                program,
                plist: launch_agent_path(&home),
                in_bundle,
                force_launch_agent: false,
            })
        }

        /// For manual checks: `set(true)` skips the login item as if the
        /// signature had been refused.
        pub fn launch_agent_only(program: PathBuf) -> Result<Self> {
            Ok(Self {
                force_launch_agent: true,
                ..Self::new(program)?
            })
        }

        /// Background-item status of our LaunchAgent, `None` without one.
        fn launch_agent_status(&self) -> Option<SmStatus> {
            if !self.plist.is_file() {
                return None;
            }
            let path = NSString::from_str(&self.plist.to_string_lossy());
            // SAFETY: status query for a file URL.
            let status = autoreleasepool(|_| unsafe {
                SMAppService::statusForLegacyURL(&NSURL::fileURLWithPath(&path))
            });
            Some(map_status(status))
        }

        fn write_launch_agent(&self) -> Result<()> {
            let plist = launch_agent_plist(APP_ID, &self.program);
            write_if_changed(&self.plist, plist.as_bytes())
                .with_context(|| format!("writing {}", self.plist.display()))?;
            Ok(())
        }

        fn remove_launch_agent(&self) -> Result<()> {
            match fs::remove_file(&self.plist) {
                Err(e) if e.kind() != ErrorKind::NotFound => {
                    Err(e).with_context(|| format!("removing {}", self.plist.display()))
                }
                _ => Ok(()),
            }
        }
    }

    fn map_status(status: SMAppServiceStatus) -> SmStatus {
        if status == SMAppServiceStatus::Enabled {
            SmStatus::Enabled
        } else if status == SMAppServiceStatus::RequiresApproval {
            SmStatus::RequiresApproval
        } else if status == SMAppServiceStatus::NotFound {
            SmStatus::NotFound
        } else {
            SmStatus::NotRegistered
        }
    }

    fn sm_status() -> SmStatus {
        // SAFETY: plain status query of the main app service.
        map_status(autoreleasepool(|_| unsafe {
            SMAppService::mainAppService().status()
        }))
    }

    /// `register` (unless already enabled) and the status afterwards.
    fn sm_register() -> Result<SmStatus, SmError> {
        if sm_status() == SmStatus::Enabled {
            return Ok(SmStatus::Enabled);
        }
        // SAFETY: registers the running app's own bundle.
        autoreleasepool(|_| unsafe { SMAppService::mainAppService().registerAndReturnError() })
            .map_err(|e| SmError {
                code: e.code(),
                message: format!("{} (code {})", e.localizedDescription(), e.code()),
                status: sm_status(),
            })?;
        Ok(sm_status())
    }

    fn sm_unregister() -> Result<(), String> {
        // SAFETY: unregisters the running app's own bundle.
        autoreleasepool(|_| unsafe { SMAppService::mainAppService().unregisterAndReturnError() })
            .map_err(|e| format!("{} (code {})", e.localizedDescription(), e.code()))
    }

    impl Autostart for MacAutostart {
        fn state(&self) -> AutostartState {
            if !self.in_bundle {
                return AutostartState::Unavailable;
            }
            current_state(true, sm_status(), self.launch_agent_status())
        }

        fn set(&self, enabled: bool) -> Result<AutostartState> {
            if !self.in_bundle {
                bail!("autostart needs the app bundle");
            }
            if enabled {
                if is_translocated(&self.program) {
                    bail!("app is running from a temporary location; move it to /Applications");
                }
                let register = if self.force_launch_agent {
                    Err(SmError {
                        code: SM_ERROR_INVALID_SIGNATURE,
                        message: "login item skipped".into(),
                        status: SmStatus::NotRegistered,
                    })
                } else {
                    sm_register()
                };
                if let Err(e) = &register {
                    tracing::info!(error = %e.message, "login item registration failed");
                }
                match choose_mechanism(true, register)
                    .map_err(|e| anyhow!(e))
                    .context("registering the login item")?
                {
                    (_, AutostartState::Unavailable) => bail!("autostart needs the app bundle"),
                    (Mechanism::LaunchAgent, _) => self.write_launch_agent()?,
                    // No second start, and no bypass of a denied login
                    // item, through a leftover LaunchAgent.
                    (Mechanism::ServiceManagement, _) => self.remove_launch_agent()?,
                }
            } else {
                let unregistered = match sm_status() {
                    SmStatus::NotRegistered | SmStatus::NotFound => Ok(()),
                    SmStatus::Enabled | SmStatus::RequiresApproval => sm_unregister(),
                };
                self.remove_launch_agent()?;
                if let Err(e) = unregistered {
                    let state = self.state();
                    if state != AutostartState::Disabled {
                        return Err(anyhow!(e)).context("removing the login item");
                    }
                    tracing::debug!(error = %e, "unregistering the login item failed");
                }
            }
            Ok(self.state())
        }

        fn repair(&self) -> Result<()> {
            // The login item always refers to the running bundle; only the
            // LaunchAgent names a path. A temporary location is no target.
            if !self.in_bundle || is_translocated(&self.program) {
                return Ok(());
            }
            let Some(agent) = self.launch_agent_status() else {
                return Ok(());
            };
            let targets = fs::read_to_string(&self.plist)
                .is_ok_and(|s| s == launch_agent_plist(APP_ID, &self.program));
            let agent_state = current_state(true, SmStatus::NotRegistered, Some(agent));
            if should_repair(agent_state, targets) {
                self.write_launch_agent()?;
                tracing::info!(program = %self.program.display(), "autostart entry repaired");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AutostartState::*;

    #[test]
    fn launch_agent_plist_is_valid() {
        let program = Path::new("/Applications/Tom & Jerry <dev>.app/Contents/MacOS/ntfy-native");
        let plist = launch_agent_plist(APP_ID, program);
        assert!(plist.starts_with("<?xml"), "{plist}");
        assert!(
            plist.contains("<key>Label</key>\n\t<string>dev.hemp0r.ntfy-native</string>"),
            "{plist}"
        );
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"), "{plist}");
        assert!(
            plist.contains(
                "<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/Applications/Tom &amp; Jerry &lt;dev&gt;.app/Contents/MacOS/ntfy-native</string>\n\t</array>"
            ),
            "{plist}"
        );
        assert!(!plist.contains("Tom & Jerry"), "{plist}");
        assert!(
            plist.contains(
                "<key>AssociatedBundleIdentifiers</key>\n\t<array>\n\t\t<string>dev.hemp0r.ntfy-native</string>\n\t</array>"
            ),
            "{plist}"
        );
        let mut reader = quick_xml::Reader::from_str(&plist);
        let mut depth = 0usize;
        loop {
            match reader.read_event().expect("well-formed XML") {
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Start(_) => depth += 1,
                quick_xml::events::Event::End(_) => depth -= 1,
                _ => {}
            }
        }
        assert_eq!(depth, 0, "balanced elements");
    }

    fn sm_error(code: isize, status: SmStatus) -> SmError {
        SmError {
            code,
            message: format!("error {code}"),
            status,
        }
    }

    #[test]
    fn mac_mechanism_choice() {
        use Mechanism::{LaunchAgent, ServiceManagement};
        assert_eq!(
            choose_mechanism(false, Ok(SmStatus::Enabled)).unwrap().1,
            Unavailable
        );
        let no_bundle = Err(sm_error(SM_ERROR_INVALID_SIGNATURE, SmStatus::NotFound));
        assert_eq!(choose_mechanism(false, no_bundle).unwrap().1, Unavailable);
        assert_eq!(
            choose_mechanism(true, Ok(SmStatus::Enabled)).unwrap(),
            (ServiceManagement, Enabled)
        );
        assert_eq!(
            choose_mechanism(true, Ok(SmStatus::RequiresApproval)).unwrap(),
            (ServiceManagement, NeedsApproval)
        );
        assert_eq!(
            choose_mechanism(true, Ok(SmStatus::NotFound)).unwrap(),
            (LaunchAgent, Enabled)
        );
        assert!(choose_mechanism(true, Ok(SmStatus::NotRegistered)).is_err());
        // Ad-hoc or broken signature: LaunchAgent.
        let bad_signature = sm_error(SM_ERROR_INVALID_SIGNATURE, SmStatus::NotRegistered);
        assert_eq!(
            choose_mechanism(true, Err(bad_signature)).unwrap(),
            (LaunchAgent, Enabled)
        );
        assert_eq!(
            choose_mechanism(true, Err(sm_error(1, SmStatus::NotFound))).unwrap(),
            (LaunchAgent, Enabled)
        );
        // Denied by the user: no bypass through a LaunchAgent.
        let denied = sm_error(SM_ERROR_LAUNCH_DENIED_BY_USER, SmStatus::RequiresApproval);
        assert_eq!(
            choose_mechanism(true, Err(denied)).unwrap(),
            (ServiceManagement, NeedsApproval)
        );
        assert_eq!(
            choose_mechanism(true, Err(sm_error(12, SmStatus::Enabled))).unwrap(),
            (ServiceManagement, Enabled),
            "already registered"
        );
        let other = choose_mechanism(true, Err(sm_error(1, SmStatus::NotRegistered)));
        assert_eq!(other.unwrap_err(), "error 1");
        let denied_unregistered = sm_error(SM_ERROR_LAUNCH_DENIED_BY_USER, SmStatus::NotRegistered);
        assert!(choose_mechanism(true, Err(denied_unregistered)).is_err());
    }

    #[test]
    fn mac_state_from_status() {
        use SmStatus as S;
        assert_eq!(
            current_state(false, S::Enabled, Some(S::Enabled)),
            Unavailable
        );
        assert_eq!(current_state(true, S::Enabled, None), Enabled);
        assert_eq!(
            current_state(true, S::NotFound, Some(S::Enabled)),
            Enabled,
            "LaunchAgent"
        );
        assert_eq!(
            current_state(true, S::NotRegistered, Some(S::RequiresApproval)),
            DisabledByUser,
            "LaunchAgent turned off under Allow in the Background"
        );
        assert_eq!(
            current_state(true, S::NotRegistered, Some(S::NotRegistered)),
            Disabled
        );
        assert_eq!(
            current_state(true, S::NotRegistered, Some(S::NotFound)),
            Disabled
        );
        assert_eq!(
            current_state(true, S::RequiresApproval, None),
            NeedsApproval
        );
        assert_eq!(
            current_state(true, S::RequiresApproval, Some(S::Enabled)),
            Enabled
        );
        assert_eq!(current_state(true, S::NotRegistered, None), Disabled);
        assert_eq!(current_state(true, S::NotFound, None), Disabled);
        assert_eq!(
            current_state(true, S::Enabled, Some(S::RequiresApproval)),
            Enabled
        );
    }

    #[test]
    fn translocated_paths_detected() {
        assert!(is_translocated(Path::new(
            "/private/var/folders/xy/T/AppTranslocation/1234-ABCD/d/ntfy-native.app/Contents/MacOS/ntfy-native"
        )));
        assert!(!is_translocated(Path::new(
            "/Applications/ntfy-native.app/Contents/MacOS/ntfy-native"
        )));
        assert!(!is_translocated(Path::new(
            "/Users/a/AppTranslocationX/ntfy-native"
        )));
    }

    #[test]
    fn launch_agent_path_in_home() {
        assert_eq!(
            launch_agent_path(Path::new("/Users/jörg")),
            Path::new("/Users/jörg/Library/LaunchAgents/dev.hemp0r.ntfy-native.plist")
        );
    }
}
