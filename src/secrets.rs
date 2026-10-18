use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use keyring_core::{CredentialStore, Entry, Error as KeyringError};

use crate::paths::APP_ID;
use crate::settings::{AuthKind, Instance};

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

pub fn account_for(instance_id: &str) -> String {
    format!("instance:{instance_id}")
}

/// OS keychain (macOS Keychain / Windows Credential Manager), service = `APP_ID`.
pub struct KeyringStore {
    store: Arc<CredentialStore>,
}

impl KeyringStore {
    pub fn new() -> Result<Self> {
        Ok(Self {
            store: native_store()?,
        })
    }

    fn entry(&self, account: &str) -> Result<Entry> {
        self.store
            .build(APP_ID, account, None)
            .with_context(|| format!("opening keychain entry {account}"))
    }
}

#[cfg(target_os = "macos")]
fn native_store() -> Result<Arc<CredentialStore>> {
    let store = apple_native_keyring_store::keychain::Store::new()
        .context("initializing macOS keychain")?;
    Ok(store)
}

#[cfg(target_os = "windows")]
fn native_store() -> Result<Arc<CredentialStore>> {
    let store = windows_native_keyring_store::Store::new()
        .context("initializing Windows Credential Manager")?;
    Ok(store)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_store() -> Result<Arc<CredentialStore>> {
    Err(anyhow!("no native secret store on this platform"))
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<String>> {
        match self.entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(anyhow!(e).context(format!("reading secret {account}"))),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        self.entry(account)?
            .set_password(secret)
            .map_err(|e| anyhow!(e).context(format!("storing secret {account}")))
    }

    fn delete(&self, account: &str) -> Result<()> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(anyhow!(e).context(format!("deleting secret {account}"))),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, String>>,
    fail_on_set: bool,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store whose `set` always fails.
    pub fn failing() -> Self {
        Self {
            fail_on_set: true,
            ..Self::default()
        }
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>> {
        Ok(self.map.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        if self.fail_on_set {
            return Err(anyhow!("secret store unavailable"));
        }
        self.map
            .lock()
            .unwrap()
            .insert(account.into(), secret.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.map.lock().unwrap().remove(account);
        Ok(())
    }
}

#[derive(Clone, PartialEq)]
pub enum Auth {
    Token(String),
    Basic { username: String, password: String },
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Token(_) => f.write_str("Token(***)"),
            Auth::Basic { username, .. } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &format_args!("***"))
                .finish(),
        }
    }
}

pub fn resolve_auth(store: &dyn SecretStore, inst: &Instance) -> Result<Option<Auth>> {
    let secret = || {
        store
            .get(&account_for(&inst.id))?
            .ok_or_else(|| anyhow!("no stored credential for {}", inst.url))
    };
    match &inst.auth {
        AuthKind::None => Ok(None),
        AuthKind::Token => Ok(Some(Auth::Token(secret()?))),
        AuthKind::Basic { username } => Ok(Some(Auth::Basic {
            username: username.clone(),
            password: secret()?,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{AuthKind, Instance};

    fn inst(auth: AuthKind) -> Instance {
        Instance {
            id: "a".into(),
            url: "https://ntfy.example.com".into(),
            auth,
            ..Instance::default()
        }
    }

    #[test]
    fn memory_roundtrip() {
        let s = MemoryStore::new();
        assert_eq!(s.get("x").unwrap(), None);
        s.set("x", "v").unwrap();
        assert_eq!(s.get("x").unwrap(), Some("v".to_string()));
        s.delete("x").unwrap();
        assert_eq!(s.get("x").unwrap(), None);
        s.delete("x").unwrap();
    }

    #[test]
    fn failing_store_rejects_set() {
        assert!(MemoryStore::failing().set("x", "v").is_err());
    }

    #[test]
    fn account_format() {
        assert_eq!(account_for("a"), "instance:a");
    }

    #[test]
    fn resolve_none() {
        struct Panicky;
        impl SecretStore for Panicky {
            fn get(&self, _: &str) -> anyhow::Result<Option<String>> {
                panic!("store must not be asked")
            }
            fn set(&self, _: &str, _: &str) -> anyhow::Result<()> {
                panic!()
            }
            fn delete(&self, _: &str) -> anyhow::Result<()> {
                panic!()
            }
        }
        assert_eq!(resolve_auth(&Panicky, &inst(AuthKind::None)).unwrap(), None);
    }

    #[test]
    fn resolve_token() {
        let s = MemoryStore::new();
        s.set(&account_for("a"), "tk_abc").unwrap();
        assert_eq!(
            resolve_auth(&s, &inst(AuthKind::Token)).unwrap(),
            Some(Auth::Token("tk_abc".into()))
        );
    }

    #[test]
    fn resolve_basic() {
        let s = MemoryStore::new();
        s.set(&account_for("a"), "pw").unwrap();
        let i = inst(AuthKind::Basic {
            username: "u".into(),
        });
        assert_eq!(
            resolve_auth(&s, &i).unwrap(),
            Some(Auth::Basic {
                username: "u".into(),
                password: "pw".into()
            })
        );
    }

    #[test]
    fn resolve_missing_secret_errors() {
        let s = MemoryStore::new();
        let err = resolve_auth(&s, &inst(AuthKind::Token)).unwrap_err();
        assert!(
            err.to_string().contains("https://ntfy.example.com"),
            "{err}"
        );
    }

    #[test]
    #[ignore]
    fn keyring_roundtrip_manual() {
        let s = KeyringStore::new().unwrap();
        let acct = format!("test:{}", std::process::id());
        assert_eq!(s.get(&acct).unwrap(), None);
        s.set(&acct, "secret").unwrap();
        assert_eq!(s.get(&acct).unwrap(), Some("secret".to_string()));
        s.delete(&acct).unwrap();
        assert_eq!(s.get(&acct).unwrap(), None);
    }

    #[test]
    fn auth_debug_redacts() {
        let t = format!("{:?}", Auth::Token("tk_secret".into()));
        assert!(!t.contains("tk_secret"), "{t}");
        let b = format!(
            "{:?}",
            Auth::Basic {
                username: "alice".into(),
                password: "hunter2".into()
            }
        );
        assert!(!b.contains("hunter2"), "{b}");
        assert!(b.contains("alice"), "{b}");
    }
}
