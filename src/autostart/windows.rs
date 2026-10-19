//! Windows: `HKCU\…\Run` value, honouring the Task Manager's Startup
//! disable (`StartupApproved\Run`).

use std::path::Path;

#[cfg(windows)]
pub use sys::WinAutostart;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
pub const VALUE_NAME: &str = crate::paths::APP_NAME;
/// `StartupApproved` data for "enabled".
pub const ENABLED_APPROVAL: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Run value for `program`: the quoted path.
pub fn run_value(program: &Path) -> String {
    format!("\"{}\"", program.display())
}

/// Whether a Run value starts `program` (quotes, arguments, case and slash
/// direction ignored).
pub fn run_value_targets(value: &str, program: &Path) -> bool {
    let value = value.trim();
    let target = match value.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => value,
    };
    let normalize = |s: &str| s.replace('/', "\\").to_lowercase();
    !target.is_empty() && normalize(target) == normalize(&program.to_string_lossy())
}

/// Whether `StartupApproved` data means "disabled by the user" (odd first
/// byte).
pub fn startup_approved_disabled(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|b| b % 2 == 1)
}

#[cfg(windows)]
mod sys {
    use std::path::PathBuf;

    use ::windows::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
    };
    use ::windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_BINARY, REG_ROUTINE_FLAGS, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND,
        RRF_RT_REG_BINARY, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
        RegSetKeyValueW,
    };
    use ::windows::core::HSTRING;
    use anyhow::{Context, Result};

    use super::{
        APPROVED_KEY, ENABLED_APPROVAL, RUN_KEY, VALUE_NAME, run_value, run_value_targets,
        startup_approved_disabled,
    };
    use crate::autostart::{Autostart, AutostartState, should_repair};

    pub struct WinAutostart {
        program: PathBuf,
    }

    impl WinAutostart {
        pub fn new(program: PathBuf) -> Self {
            Self { program }
        }

        fn read_state(&self) -> Result<AutostartState> {
            if get_string(RUN_KEY, VALUE_NAME)?.is_none() {
                return Ok(AutostartState::Disabled);
            }
            let user_off = get_binary(APPROVED_KEY, VALUE_NAME)?
                .is_some_and(|b| startup_approved_disabled(&b));
            Ok(if user_off {
                AutostartState::DisabledByUser
            } else {
                AutostartState::Enabled
            })
        }

        fn write_run_value(&self) -> Result<()> {
            let data: Vec<u8> = run_value(&self.program)
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect();
            set_value(RUN_KEY, VALUE_NAME, REG_SZ, &data)
        }
    }

    impl Autostart for WinAutostart {
        fn state(&self) -> AutostartState {
            self.read_state().unwrap_or_else(|e| {
                tracing::warn!(error = %format!("{e:#}"), "reading the autostart state failed");
                AutostartState::Unavailable
            })
        }

        fn set(&self, enabled: bool) -> Result<AutostartState> {
            if enabled {
                // Read first, so a failing read leaves nothing half-applied.
                let user_off = get_binary(APPROVED_KEY, VALUE_NAME)?
                    .is_some_and(|b| startup_approved_disabled(&b));
                self.write_run_value()?;
                // Switching on explicitly overrides a Task Manager disable.
                if user_off {
                    set_value(APPROVED_KEY, VALUE_NAME, REG_BINARY, &ENABLED_APPROVAL)?;
                }
            } else {
                delete_value(RUN_KEY, VALUE_NAME)?;
            }
            self.read_state()
        }

        fn repair(&self) -> Result<()> {
            let targets = get_string(RUN_KEY, VALUE_NAME)?
                .is_some_and(|v| run_value_targets(&v, &self.program));
            if should_repair(self.read_state()?, targets) {
                self.write_run_value()?;
                tracing::info!(program = %self.program.display(), "autostart entry repaired");
            }
            Ok(())
        }
    }

    /// Raw data of a value below `HKEY_CURRENT_USER`; `None` if it or its
    /// key is missing.
    fn get_value(subkey: &str, name: &str, flags: REG_ROUTINE_FLAGS) -> Result<Option<Vec<u8>>> {
        let (subkey_w, name_w) = (HSTRING::from(subkey), HSTRING::from(name));
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let mut len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            let data = (!buf.is_empty()).then(|| buf.as_mut_ptr().cast());
            // SAFETY: valid strings; `data` points to `len` writable bytes.
            let err = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    &subkey_w,
                    &name_w,
                    flags,
                    None,
                    data,
                    Some(&mut len),
                )
            };
            match err {
                ERROR_SUCCESS if !buf.is_empty() || len == 0 => {
                    buf.truncate(len as usize);
                    return Ok(Some(buf));
                }
                // Size query, or the value grew in between.
                ERROR_SUCCESS | ERROR_MORE_DATA => buf.resize(len as usize, 0),
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => return Ok(None),
                e => {
                    return e
                        .ok()
                        .map(|()| None)
                        .with_context(|| format!(r"reading HKCU\{subkey}\{name}"));
                }
            }
        }
    }

    fn get_string(subkey: &str, name: &str) -> Result<Option<String>> {
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        Ok(get_value(subkey, name, flags)?.map(|bytes| {
            let wide: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect();
            let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
            String::from_utf16_lossy(&wide[..end])
        }))
    }

    fn get_binary(subkey: &str, name: &str) -> Result<Option<Vec<u8>>> {
        get_value(subkey, name, RRF_RT_REG_BINARY)
    }

    fn set_value(subkey: &str, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<()> {
        let (subkey_w, name_w) = (HSTRING::from(subkey), HSTRING::from(name));
        let len = u32::try_from(data.len()).context("registry value too large")?;
        // SAFETY: valid strings; `data` holds `len` bytes.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &subkey_w,
                &name_w,
                kind.0,
                Some(data.as_ptr().cast()),
                len,
            )
        }
        .ok()
        .with_context(|| format!(r"setting HKCU\{subkey}\{name}"))
    }

    fn delete_value(subkey: &str, name: &str) -> Result<()> {
        let (subkey_w, name_w) = (HSTRING::from(subkey), HSTRING::from(name));
        // SAFETY: valid strings.
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &subkey_w, &name_w) } {
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(()),
            e => e
                .ok()
                .with_context(|| format!(r"deleting HKCU\{subkey}\{name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\Jörg Ä\Downloads\ntfy native\ntfy-native.exe";

    #[test]
    fn run_value_quotes_path() {
        assert_eq!(
            run_value(Path::new(EXE)),
            r#""C:\Users\Jörg Ä\Downloads\ntfy native\ntfy-native.exe""#
        );
    }

    #[test]
    fn run_value_matches_current_exe() {
        let exe = Path::new(EXE);
        assert!(run_value_targets(&run_value(exe), exe));
        assert!(run_value_targets(EXE, exe), "without quotes");
        assert!(run_value_targets(
            r#"  "c:\users\JÖRG ä\downloads\NTFY NATIVE\ntfy-native.EXE"  "#,
            exe
        ));
        assert!(
            run_value_targets(&format!("\"{EXE}\" --minimized"), exe),
            "arguments"
        );
        assert!(!run_value_targets(
            r#""C:\Users\Jörg Ä\Desktop\ntfy-native.exe""#,
            exe
        ));
        assert!(!run_value_targets("", exe));
        assert!(!run_value_targets(&format!("{EXE}.old"), exe));
    }

    #[test]
    fn startup_approved_bytes() {
        assert!(!startup_approved_disabled(&[
            0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
        ]));
        assert!(!startup_approved_disabled(&ENABLED_APPROVAL));
        assert!(startup_approved_disabled(&[
            0x03, 0, 0, 0, 0x5c, 0x1a, 0x7e, 0x2b, 0, 0, 0, 0
        ]));
        assert!(startup_approved_disabled(&[0x01]));
        assert!(!startup_approved_disabled(&[0x06]));
        assert!(!startup_approved_disabled(&[]));
    }
}
