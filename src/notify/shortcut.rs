//! Decides whether the Start-menu shortcut needs (re)writing. Windows keeps
//! toasts of an unpackaged app in the Notification Center, and lists the app
//! in the notification settings, only if such a shortcut carries its
//! AppUserModelID and toast activator CLSID. The COM side is in `windows.rs`.

/// What the shortcut must point to and carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutProps {
    pub target: String,
    pub app_id: String,
    pub activator_clsid: String,
}

/// `existing` is `None` when the shortcut is missing or unreadable.
pub fn needs_update(existing: Option<&ShortcutProps>, wanted: &ShortcutProps) -> bool {
    let Some(existing) = existing else {
        return true;
    };
    let clsid = |s: &str| s.trim_matches(['{', '}']).to_ascii_uppercase();
    existing.target.to_lowercase() != wanted.target.to_lowercase()
        || existing.app_id != wanted.app_id
        || clsid(&existing.activator_clsid) != clsid(&wanted.activator_clsid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted() -> ShortcutProps {
        ShortcutProps {
            target: r"C:\Apps\ntfy-native\ntfy-native.exe".into(),
            app_id: "dev.hemp0r.ntfy-native".into(),
            activator_clsid: "AE88E831-148B-400C-8273-6ABC4FDEA102".into(),
        }
    }

    #[test]
    fn missing_shortcut_is_written() {
        assert!(needs_update(None, &wanted()));
    }

    #[test]
    fn matching_shortcut_is_left_alone() {
        assert!(!needs_update(Some(&wanted()), &wanted()));
        // Windows paths and GUID spellings are case-insensitive.
        let same = ShortcutProps {
            target: r"c:\apps\NTFY-NATIVE\ntfy-native.EXE".into(),
            activator_clsid: "{ae88e831-148b-400c-8273-6abc4fdea102}".into(),
            ..wanted()
        };
        assert!(!needs_update(Some(&same), &wanted()));
    }

    #[test]
    fn any_difference_rewrites() {
        let moved = ShortcutProps {
            target: r"D:\ntfy-native.exe".into(),
            ..wanted()
        };
        let other_id = ShortcutProps {
            app_id: "other.app".into(),
            ..wanted()
        };
        let no_activator = ShortcutProps {
            activator_clsid: String::new(),
            ..wanted()
        };
        for existing in [moved, other_id, no_activator] {
            assert!(needs_update(Some(&existing), &wanted()), "{existing:?}");
        }
    }
}
