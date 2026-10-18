//! Platform notifier selection and platform-neutral helpers.

use std::path::Path;
use std::sync::Arc;
use std::{fs, io};

use crate::notification::{ActivationSink, LogNotifier, Notifier};

pub mod toast_xml;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::MacNotifier;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{TOAST_ACTIVATOR_CLSID, WinNotifier};

/// The native notifier for this platform, or a [`LogNotifier`] if there is
/// none or it fails to initialise. On Windows the app icon for toasts is
/// written to `data_dir`.
pub fn platform_notifier(on_activate: ActivationSink, data_dir: &Path) -> Arc<dyn Notifier> {
    #[cfg(target_os = "macos")]
    match MacNotifier::new(on_activate) {
        Ok(n) => return Arc::new(n),
        Err(e) => tracing::warn!(
            error = %format!("{e:#}"),
            "native notifications unavailable, logging notifications instead"
        ),
    }
    #[cfg(windows)]
    match WinNotifier::new(on_activate) {
        Ok(n) => {
            // Without it toasts show a generic icon.
            if let Err(e) = self::windows::register_icon(data_dir) {
                tracing::warn!(error = %format!("{e:#}"), "registering the toast icon failed");
            }
            return Arc::new(n);
        }
        Err(e) => tracing::warn!(
            error = %format!("{e:#}"),
            "native notifications unavailable, logging notifications instead"
        ),
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    drop(on_activate);
    #[cfg(not(windows))]
    let _ = data_dir;
    Arc::new(LogNotifier)
}

/// Writes `bytes` to `path` unless it already holds exactly them; returns
/// whether it wrote. Goes through a temporary sibling, so readers never see
/// a partial file.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    if fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(true)
}

/// Stable identifier for a notification category with the given button
/// labels: `"btn-"` followed by the 64-bit FNV-1a hash of the labels in hex.
pub fn category_id(labels: &[&str]) -> String {
    format!("btn-{:016x}", fnv1a(labels))
}

/// 64-bit FNV-1a hash of `parts`, each followed by a 0xFF separator (which
/// never occurs in UTF-8, so parts are separated unambiguously).
pub(crate) fn fnv1a(parts: &[&str]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for part in parts {
        for &b in part.as_bytes().iter().chain(&[0xFF]) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_id_stable() {
        let a = category_id(&["Open", "Ack"]);
        assert_eq!(a, category_id(&["Open", "Ack"]));
        assert_ne!(a, category_id(&["Ack", "Open"]));
        assert!(a.starts_with("btn-"), "{a}");
        assert_eq!(a, "btn-76040e93b051ace0");
    }

    #[test]
    fn write_if_changed_writes_only_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("icon.png");
        assert!(write_if_changed(&path, b"one").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"one");
        assert!(!write_if_changed(&path, b"one").unwrap());
        assert!(write_if_changed(&path, b"two").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["icon.png"], "no temporary file left behind");
    }

    #[test]
    fn category_id_separates_labels() {
        assert_ne!(category_id(&["ab", "c"]), category_id(&["a", "bc"]));
        assert_ne!(category_id(&["Open"]), category_id(&["Open", ""]));
    }
}
