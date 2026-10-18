//! Platform notifier selection and platform-neutral helpers.

use std::sync::Arc;

use crate::notification::{ActivationSink, LogNotifier, Notifier};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::MacNotifier;

/// The native notifier for this platform, or a [`LogNotifier`] if there is
/// none or it fails to initialise.
pub fn platform_notifier(on_activate: ActivationSink) -> Arc<dyn Notifier> {
    #[cfg(target_os = "macos")]
    match MacNotifier::new(on_activate) {
        Ok(n) => return Arc::new(n),
        Err(e) => tracing::warn!(
            error = %format!("{e:#}"),
            "native notifications unavailable, logging notifications instead"
        ),
    }
    #[cfg(not(target_os = "macos"))]
    drop(on_activate);
    Arc::new(LogNotifier)
}

/// Stable identifier for a notification category with the given button
/// labels: `"btn-"` followed by the 64-bit FNV-1a hash of the labels in hex.
pub fn category_id(labels: &[&str]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for label in labels {
        // 0xFF never occurs in UTF-8, so it separates labels unambiguously.
        for &b in label.as_bytes().iter().chain(&[0xFF]) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("btn-{hash:016x}")
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
    }

    #[test]
    fn category_id_separates_labels() {
        assert_ne!(category_id(&["ab", "c"]), category_id(&["a", "bc"]));
        assert_ne!(category_id(&["Open"]), category_id(&["Open", ""]));
    }
}
