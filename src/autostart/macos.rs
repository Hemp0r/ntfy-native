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

/// Which mechanism serves autostart after trying `SMAppService.register`
/// (`register` = status after it, or its error), and the resulting state.
/// Registration fails or does not take for apps without a proper signature;
/// those fall back to a LaunchAgent.
pub fn choose_mechanism(
    in_bundle: bool,
    register: Result<SmStatus, String>,
) -> (Mechanism, AutostartState) {
    if !in_bundle {
        return (Mechanism::ServiceManagement, AutostartState::Unavailable);
    }
    match register {
        Ok(SmStatus::Enabled) => (Mechanism::ServiceManagement, AutostartState::Enabled),
        Ok(SmStatus::RequiresApproval) => {
            (Mechanism::ServiceManagement, AutostartState::NeedsApproval)
        }
        Ok(SmStatus::NotFound | SmStatus::NotRegistered) | Err(_) => {
            (Mechanism::LaunchAgent, AutostartState::Enabled)
        }
    }
}

/// Current state from the `SMAppService` status and whether our
/// LaunchAgent exists; either one active counts as enabled.
pub fn current_state(in_bundle: bool, sm: SmStatus, launch_agent: bool) -> AutostartState {
    match (in_bundle, sm, launch_agent) {
        (false, _, _) => AutostartState::Unavailable,
        (true, SmStatus::Enabled, _) | (true, _, true) => AutostartState::Enabled,
        (true, SmStatus::RequiresApproval, false) => AutostartState::NeedsApproval,
        (true, SmStatus::NotRegistered | SmStatus::NotFound, false) => AutostartState::Disabled,
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;

    use anyhow::{Context, Result, anyhow, bail};
    use objc2::rc::autoreleasepool;
    use objc2_foundation::NSBundle;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use super::{
        Mechanism, SmStatus, choose_mechanism, current_state, launch_agent_path, launch_agent_plist,
    };
    use crate::autostart::{Autostart, AutostartState, should_repair};
    use crate::notify::write_if_changed;
    use crate::paths::APP_ID;

    pub struct MacAutostart {
        program: PathBuf,
        plist: PathBuf,
        in_bundle: bool,
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
            })
        }

        fn launch_agent_present(&self) -> bool {
            self.plist.is_file()
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

    fn sm_status() -> SmStatus {
        // SAFETY: plain status query of the main app service.
        let status = autoreleasepool(|_| unsafe { SMAppService::mainAppService().status() });
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

    /// `register` (unless already enabled) and the status afterwards.
    fn sm_register() -> Result<SmStatus, String> {
        if sm_status() == SmStatus::Enabled {
            return Ok(SmStatus::Enabled);
        }
        // SAFETY: registers the running app's own bundle.
        autoreleasepool(|_| unsafe { SMAppService::mainAppService().registerAndReturnError() })
            .map_err(|e| format!("{} (code {})", e.localizedDescription(), e.code()))?;
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
            current_state(true, sm_status(), self.launch_agent_present())
        }

        fn set(&self, enabled: bool) -> Result<AutostartState> {
            if !self.in_bundle {
                bail!("autostart needs the app bundle");
            }
            if enabled {
                let register = sm_register();
                if let Err(e) = &register {
                    tracing::info!(error = %e, "login item registration failed, using a LaunchAgent");
                }
                match choose_mechanism(true, register) {
                    (_, AutostartState::Unavailable) => bail!("autostart needs the app bundle"),
                    (Mechanism::LaunchAgent, _) => self.write_launch_agent()?,
                    // Avoid a second start through a leftover LaunchAgent.
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
            // LaunchAgent names a path.
            if !self.in_bundle || !self.launch_agent_present() {
                return Ok(());
            }
            let targets = fs::read_to_string(&self.plist)
                .is_ok_and(|s| s == launch_agent_plist(APP_ID, &self.program));
            if should_repair(self.state(), targets) {
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

    #[test]
    fn mac_mechanism_choice() {
        assert_eq!(
            choose_mechanism(false, Ok(SmStatus::Enabled)).1,
            Unavailable
        );
        assert_eq!(
            choose_mechanism(false, Err("no bundle".into())).1,
            Unavailable
        );
        assert_eq!(
            choose_mechanism(true, Ok(SmStatus::Enabled)),
            (Mechanism::ServiceManagement, Enabled)
        );
        assert_eq!(
            choose_mechanism(true, Ok(SmStatus::RequiresApproval)),
            (Mechanism::ServiceManagement, NeedsApproval)
        );
        assert_eq!(
            choose_mechanism(true, Ok(SmStatus::NotFound)),
            (Mechanism::LaunchAgent, Enabled)
        );
        assert_eq!(
            choose_mechanism(true, Ok(SmStatus::NotRegistered)),
            (Mechanism::LaunchAgent, Enabled)
        );
        assert_eq!(
            choose_mechanism(true, Err("Invalid code signature".into())),
            (Mechanism::LaunchAgent, Enabled)
        );
    }

    #[test]
    fn mac_state_from_status() {
        assert_eq!(current_state(false, SmStatus::Enabled, true), Unavailable);
        assert_eq!(current_state(true, SmStatus::Enabled, false), Enabled);
        assert_eq!(
            current_state(true, SmStatus::NotFound, true),
            Enabled,
            "LaunchAgent"
        );
        assert_eq!(
            current_state(true, SmStatus::RequiresApproval, false),
            NeedsApproval
        );
        assert_eq!(
            current_state(true, SmStatus::RequiresApproval, true),
            Enabled
        );
        assert_eq!(
            current_state(true, SmStatus::NotRegistered, false),
            Disabled
        );
        assert_eq!(current_state(true, SmStatus::NotFound, false), Disabled);
    }

    #[test]
    fn launch_agent_path_in_home() {
        assert_eq!(
            launch_agent_path(Path::new("/Users/jörg")),
            Path::new("/Users/jörg/Library/LaunchAgents/dev.hemp0r.ntfy-native.plist")
        );
    }
}
