use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::secrets::{SecretStore, account_for};
use crate::settings::{AuthKind, Instance, Settings, Topic};
use crate::validate::{normalize_url, same_origin, same_url, unique_id, validate_topics};

const MIN_CACHE_SECS: i64 = 3600;
const MAX_CACHE_SECS: i64 = 2_592_000;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceDraft {
    /// `None` adds a new instance.
    pub id: Option<String>,
    pub url: String,
    pub auth: AuthDraft,
    pub cache_duration_secs: i64,
    pub topics: Vec<Topic>,
}

/// A `None` secret keeps the stored one.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum AuthDraft {
    None,
    Token {
        token: Option<String>,
    },
    Basic {
        username: String,
        password: Option<String>,
    },
}

fn redact(secret: &Option<String>) -> &'static str {
    if secret.is_some() {
        "Some(***)"
    } else {
        "None"
    }
}

impl std::fmt::Debug for AuthDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthDraft::None => f.write_str("None"),
            AuthDraft::Token { token } => f
                .debug_struct("Token")
                .field("token", &format_args!("{}", redact(token)))
                .finish(),
            AuthDraft::Basic { username, password } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &format_args!("{}", redact(password)))
                .finish(),
        }
    }
}

/// The user may type `user:password@host`; keep credentials out of logs.
fn redacted_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            if !u.username().is_empty() {
                let _ = u.set_username("***");
            }
            if u.password().is_some() {
                let _ = u.set_password(Some("***"));
            }
            u.to_string()
        }
        Err(_) => "<invalid url>".into(),
    }
}

impl std::fmt::Debug for InstanceDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceDraft")
            .field("id", &self.id)
            .field("url", &format_args!("{}", redacted_url(&self.url)))
            .field("auth", &self.auth)
            .field("cache_duration_secs", &self.cache_duration_secs)
            .field("topics", &self.topics)
            .finish()
    }
}

enum SecretPlan {
    Set(String),
    Keep,
    Delete,
}

/// A secret deletion planned by `apply_draft`/`remove_instance`. The caller
/// commits it only after `settings.json` was written, so a failed write never
/// leaves a still-configured instance without its credentials.
#[derive(Debug, PartialEq)]
#[must_use = "commit after settings.json was written"]
pub struct SecretChange(Option<String>);

impl SecretChange {
    pub fn commit(self, secrets: &dyn SecretStore) -> Result<()> {
        match self.0 {
            Some(account) => secrets.delete(&account),
            None => Ok(()),
        }
    }
}

/// Errors name fields and positions, never the entered values.
fn validate_fields(draft: &InstanceDraft) -> Result<String> {
    let url = normalize_url(&draft.url)?;
    if draft.topics.is_empty() {
        bail!("at least one topic is required");
    }
    validate_topics(&draft.topics)?;
    if !(MIN_CACHE_SECS..=MAX_CACHE_SECS).contains(&draft.cache_duration_secs) {
        bail!("cache duration must be between 1 hour and 30 days");
    }
    Ok(url)
}

/// `moved`: an edit to another origin, where a stored secret must not be kept.
fn plan_auth(
    draft: &AuthDraft,
    existing: Option<&AuthKind>,
    stored: bool,
    moved: bool,
) -> Result<(AuthKind, SecretPlan)> {
    let keeps = matches!(
        draft,
        AuthDraft::Token { token: None } | AuthDraft::Basic { password: None, .. }
    );
    if keeps && moved {
        bail!("credentials required for a new server address");
    }
    Ok(match draft {
        AuthDraft::None => (AuthKind::None, SecretPlan::Delete),
        AuthDraft::Token { token: Some(t) } => {
            if t.trim().is_empty() {
                bail!("token required");
            }
            (AuthKind::Token, SecretPlan::Set(t.clone()))
        }
        AuthDraft::Token { token: None } => {
            if !(matches!(existing, Some(AuthKind::Token)) && stored) {
                bail!("token required");
            }
            (AuthKind::Token, SecretPlan::Keep)
        }
        AuthDraft::Basic { username, password } => {
            if username.trim().is_empty() {
                bail!("username required");
            }
            let kind = AuthKind::Basic {
                username: username.clone(),
            };
            match password {
                Some(p) if p.is_empty() => bail!("password required"),
                Some(p) => (kind, SecretPlan::Set(p.clone())),
                None => {
                    if !(existing == Some(&kind) && stored) {
                        bail!("password required");
                    }
                    (kind, SecretPlan::Keep)
                }
            }
        }
    })
}

/// Validates everything first, then writes a newly entered secret. Returns the
/// new settings (not persisted), the instance id and the planned secret
/// deletion (when auth becomes none).
pub fn apply_draft(
    settings: &Settings,
    draft: &InstanceDraft,
    secrets: &dyn SecretStore,
) -> Result<(Settings, String, SecretChange)> {
    let url = validate_fields(draft)?;
    let pos = match &draft.id {
        Some(id) => Some(
            settings
                .instances
                .iter()
                .position(|i| &i.id == id)
                .ok_or_else(|| anyhow!("instance not found"))?,
        ),
        None => None,
    };
    if settings
        .instances
        .iter()
        .enumerate()
        .any(|(i, inst)| Some(i) != pos && same_url(&inst.url, &url))
    {
        bail!("an instance with this url already exists");
    }

    let mut settings = settings.clone();
    let mut inst = match pos {
        Some(p) => settings.instances[p].clone(),
        None => {
            let probe = Instance {
                url: url.clone(),
                ..Instance::default()
            };
            Instance {
                id: unique_id(&settings, &probe.host()),
                ..probe
            }
        }
    };
    let account = account_for(&inst.id);
    let stored = match draft.auth {
        AuthDraft::Token { token: None } | AuthDraft::Basic { password: None, .. } => {
            secrets.get(&account)?.is_some()
        }
        _ => false,
    };
    let existing = pos.map(|_| inst.auth.clone());
    let moved = pos.is_some() && !same_origin(&inst.url, &url);
    let (kind, plan) = plan_auth(&draft.auth, existing.as_ref(), stored, moved)?;

    let change = match plan {
        SecretPlan::Set(secret) => {
            secrets.set(&account, &secret)?;
            SecretChange(None)
        }
        SecretPlan::Keep => SecretChange(None),
        SecretPlan::Delete => SecretChange(Some(account)),
    };
    inst.url = url;
    inst.auth = kind;
    inst.cache_duration_secs = draft.cache_duration_secs;
    inst.topics = draft.topics.clone();
    let id = inst.id.clone();
    match pos {
        Some(p) => settings.instances[p] = inst,
        None => settings.instances.push(inst),
    }
    Ok((settings, id, change))
}

/// Returns the settings without the instance (not persisted) and the planned
/// secret deletion.
pub fn remove_instance(settings: &Settings, id: &str) -> Result<(Settings, SecretChange)> {
    if settings.instance(id).is_none() {
        bail!("instance not found");
    }
    let mut settings = settings.clone();
    settings.instances.retain(|i| i.id != id);
    Ok((settings, SecretChange(Some(account_for(id)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    fn topic(name: &str) -> Topic {
        Topic {
            name: name.into(),
            ..Topic::default()
        }
    }

    fn draft(url: &str) -> InstanceDraft {
        InstanceDraft {
            id: None,
            url: url.into(),
            auth: AuthDraft::None,
            cache_duration_secs: 43200,
            topics: vec![topic("a")],
        }
    }

    fn add(s: &Settings, d: &InstanceDraft, store: &MemoryStore) -> (Settings, String) {
        let (s, id, change) = apply_draft(s, d, store).unwrap();
        change.commit(store).unwrap();
        (s, id)
    }

    #[test]
    fn adds_new_instance_with_unique_id() {
        let store = MemoryStore::new();
        let (s, id1) = add(&Settings::default(), &draft("https://h/one"), &store);
        let (s, id2) = add(&s, &draft("https://h/two"), &store);
        assert_eq!((id1.as_str(), id2.as_str()), ("h", "h-2"));
        assert_eq!(s.instances.len(), 2);
        assert_eq!(s.instances[1].url, "https://h/two");
    }

    #[test]
    fn edit_keeps_id_and_replaces_fields() {
        let store = MemoryStore::new();
        let (s, id) = add(&Settings::default(), &draft("https://h"), &store);
        let mut d = draft("https://H:443/");
        d.id = Some(id.clone());
        d.topics = vec![Topic {
            muted: true,
            ..topic("b")
        }];
        d.cache_duration_secs = 7200;
        let (s, id2) = add(&s, &d, &store);
        assert_eq!(id2, id);
        assert_eq!(s.instances.len(), 1);
        assert_eq!(s.instances[0].url, "https://h");
        assert_eq!(s.instances[0].topics, d.topics);
        assert_eq!(s.instances[0].cache_duration_secs, 7200);
        d.id = Some("nope".into());
        assert!(apply_draft(&s, &d, &store).is_err());
    }

    #[test]
    fn rejects_duplicate_url_of_other_instance() {
        let store = MemoryStore::new();
        let (s, _) = add(&Settings::default(), &draft("https://h"), &store);
        assert!(apply_draft(&s, &draft("https://H:443/"), &store).is_err());
        let (s, id) = add(&s, &draft("https://g"), &store);
        let mut d = draft("https://h");
        d.id = Some(id);
        assert!(apply_draft(&s, &d, &store).is_err());
    }

    #[test]
    fn rejects_empty_or_invalid_topics_and_bad_duration() {
        let store = MemoryStore::new();
        let s = Settings::default();
        let with = |f: &dyn Fn(&mut InstanceDraft)| {
            let mut d = draft("https://h");
            f(&mut d);
            apply_draft(&s, &d, &store).unwrap_err().to_string()
        };
        with(&|d| d.topics = vec![]);
        let e = with(&|d| d.topics = vec![topic("a"), topic("bad topic")]);
        assert!(e.starts_with("topic 2: invalid topic name"), "{e}");
        let e = with(&|d| d.topics = vec![topic("a"), topic("a")]);
        assert!(e.starts_with("topic 2: duplicate"), "{e}");
        with(&|d| d.cache_duration_secs = 60);
        with(&|d| d.cache_duration_secs = 2_592_001);
        let mut d = draft("https://h");
        d.cache_duration_secs = 3600;
        assert!(apply_draft(&s, &d, &store).is_ok());
        d.cache_duration_secs = 2_592_000;
        assert!(apply_draft(&s, &d, &store).is_ok());
    }

    #[test]
    fn token_keep_requires_stored_token() {
        let store = MemoryStore::new();
        let mut d = draft("https://h");
        d.auth = AuthDraft::Token { token: None };
        assert!(apply_draft(&Settings::default(), &d, &store).is_err());

        d.auth = AuthDraft::Token {
            token: Some("tk_1".into()),
        };
        let (s, id) = add(&Settings::default(), &d, &store);
        assert_eq!(s.instances[0].auth, AuthKind::Token);
        d.id = Some(id.clone());
        d.auth = AuthDraft::Token { token: None };
        let (s, _) = add(&s, &d, &store);
        assert_eq!(store.get(&account_for(&id)).unwrap().unwrap(), "tk_1");
        assert_eq!(s.instances[0].auth, AuthKind::Token);

        store.delete(&account_for(&id)).unwrap();
        assert!(apply_draft(&s, &d, &store).is_err());
    }

    #[test]
    fn basic_keep_only_for_same_username() {
        let store = MemoryStore::new();
        let mut d = draft("https://h");
        d.auth = AuthDraft::Basic {
            username: "u".into(),
            password: Some("pw".into()),
        };
        let (s, id) = add(&Settings::default(), &d, &store);
        d.id = Some(id.clone());
        d.auth = AuthDraft::Basic {
            username: "u".into(),
            password: None,
        };
        add(&s, &d, &store);
        assert_eq!(store.get(&account_for(&id)).unwrap().unwrap(), "pw");
        d.auth = AuthDraft::Basic {
            username: "other".into(),
            password: None,
        };
        assert!(apply_draft(&s, &d, &store).is_err());
        d.auth = AuthDraft::Basic {
            username: "other".into(),
            password: Some("new".into()),
        };
        let (s, _) = add(&s, &d, &store);
        assert_eq!(
            s.instances[0].auth,
            AuthKind::Basic {
                username: "other".into()
            }
        );
        assert_eq!(store.get(&account_for(&id)).unwrap().unwrap(), "new");
    }

    #[test]
    fn keeping_a_secret_requires_the_same_origin() {
        let store = MemoryStore::new();
        let mut d = draft("https://h/one");
        d.auth = AuthDraft::Token {
            token: Some("tk_1".into()),
        };
        let (s, id) = add(&Settings::default(), &d, &store);
        let account = account_for(&id);
        d.id = Some(id.clone());
        d.auth = AuthDraft::Token { token: None };

        for other in ["https://evil.example", "http://h/one", "https://h:8443/one"] {
            d.url = other.into();
            let e = format!("{:#}", apply_draft(&s, &d, &store).unwrap_err());
            assert_eq!(e, "credentials required for a new server address");
            assert_eq!(store.get(&account).unwrap().as_deref(), Some("tk_1"));
        }

        d.url = "https://H:443/two".into();
        let (s2, _) = add(&s, &d, &store);
        assert_eq!(s2.instances[0].url, "https://h/two");
        assert_eq!(s2.instances[0].auth, AuthKind::Token);
        assert_eq!(store.get(&account).unwrap().as_deref(), Some("tk_1"));

        // A newly entered secret may go anywhere.
        d.url = "https://g".into();
        d.auth = AuthDraft::Token {
            token: Some("tk_2".into()),
        };
        let (s3, _) = add(&s, &d, &store);
        assert_eq!(s3.instances[0].url, "https://g");
        assert_eq!(store.get(&account).unwrap().as_deref(), Some("tk_2"));

        // Basic: same user, other host, no password.
        let store = MemoryStore::new();
        let mut d = draft("https://h");
        d.auth = AuthDraft::Basic {
            username: "u".into(),
            password: Some("pw".into()),
        };
        let (s, id) = add(&Settings::default(), &d, &store);
        d.id = Some(id.clone());
        d.url = "https://g".into();
        d.auth = AuthDraft::Basic {
            username: "u".into(),
            password: None,
        };
        assert!(apply_draft(&s, &d, &store).is_err());
        assert_eq!(store.get(&account_for(&id)).unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn auth_none_deletes_secret() {
        let store = MemoryStore::new();
        let mut d = draft("https://h");
        d.auth = AuthDraft::Token {
            token: Some("tk".into()),
        };
        let (s, id) = add(&Settings::default(), &d, &store);
        d.id = Some(id.clone());
        d.auth = AuthDraft::None;
        let (s, _) = add(&s, &d, &store);
        assert_eq!(s.instances[0].auth, AuthKind::None);
        assert_eq!(store.get(&account_for(&id)).unwrap(), None);
    }

    #[test]
    fn deletions_wait_for_commit() {
        let store = MemoryStore::new();
        let mut d = draft("https://h");
        d.auth = AuthDraft::Token {
            token: Some("tk".into()),
        };
        let (s, id) = add(&Settings::default(), &d, &store);
        let account = account_for(&id);

        d.id = Some(id.clone());
        d.auth = AuthDraft::None;
        let (_, _, change) = apply_draft(&s, &d, &store).unwrap();
        assert_eq!(store.get(&account).unwrap().as_deref(), Some("tk"));
        change.commit(&store).unwrap();
        assert_eq!(store.get(&account).unwrap(), None);

        store.set(&account, "tk").unwrap();
        let (_, change) = remove_instance(&s, &id).unwrap();
        assert_eq!(store.get(&account).unwrap().as_deref(), Some("tk"));
        change.commit(&store).unwrap();
        assert_eq!(store.get(&account).unwrap(), None);
    }

    #[test]
    fn invalid_draft_writes_no_secret() {
        let store = MemoryStore::new();
        let mut d = draft("https://h");
        d.auth = AuthDraft::Token {
            token: Some("tk".into()),
        };
        d.topics = vec![];
        assert!(apply_draft(&Settings::default(), &d, &store).is_err());
        assert_eq!(store.get(&account_for("h")).unwrap(), None);
        d.topics = vec![topic("a")];
        assert!(apply_draft(&Settings::default(), &d, &MemoryStore::failing()).is_err());
    }

    #[test]
    fn apply_errors_never_echo_input() {
        let store = MemoryStore::new();
        let s = Settings::default();
        let check = |d: &InstanceDraft| {
            let e = format!("{:#}", apply_draft(&s, d, &store).unwrap_err());
            assert!(!e.contains("s3cret"), "{e}");
        };
        let mut d = draft("https://u:s3cret@x");
        d.topics = vec![topic("s3cret x")];
        d.auth = AuthDraft::Token {
            token: Some("s3cret".into()),
        };
        d.cache_duration_secs = 1;
        check(&d);
        d.url = "https://x".into();
        check(&d);
        d.topics = vec![topic("a")];
        check(&d);
        d.url = "s3cret".into();
        check(&d);
        d.url = "https://x?s3cret=1".into();
        check(&d);
        d.url = "https://x".into();
        d.cache_duration_secs = 43200;
        d.auth = AuthDraft::Basic {
            username: " ".into(),
            password: Some("s3cret".into()),
        };
        check(&d);
        d.auth = AuthDraft::Basic {
            username: "s3cret".into(),
            password: None,
        };
        check(&d);
        d.id = Some("s3cret".into());
        check(&d);
    }

    #[test]
    fn draft_debug_redacts_secrets() {
        let mut d = draft("https://h");
        d.auth = AuthDraft::Token {
            token: Some("s3cret".into()),
        };
        let t = format!("{d:?}");
        assert!(!t.contains("s3cret"), "{t}");
        d.auth = AuthDraft::Basic {
            username: "alice".into(),
            password: Some("s3cret".into()),
        };
        let b = format!("{d:?} {:?}", d.auth);
        assert!(!b.contains("s3cret") && b.contains("alice"), "{b}");
    }

    #[test]
    fn draft_debug_redacts_url_userinfo() {
        let t = format!("{:?}", draft("https://alice:s3cret@host.example/path"));
        assert!(!t.contains("s3cret") && !t.contains("alice"), "{t}");
        assert!(t.contains("host.example"), "{t}");
        let t = format!("{:?}", draft("s3cret"));
        assert!(!t.contains("s3cret") && t.contains("<invalid url>"), "{t}");
        let t = format!("{:?}", draft("https://tok3n@host.example"));
        assert!(!t.contains("tok3n"), "{t}");
    }

    #[test]
    fn draft_serde_roundtrip() {
        let mut d = draft("https://h");
        d.auth = AuthDraft::Basic {
            username: "u".into(),
            password: None,
        };
        let back: InstanceDraft =
            serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn remove_deletes_secret_and_instance() {
        let store = MemoryStore::new();
        let mut d = draft("https://h");
        d.auth = AuthDraft::Token {
            token: Some("tk".into()),
        };
        let (s, id) = add(&Settings::default(), &d, &store);
        let (s, change) = remove_instance(&s, &id).unwrap();
        change.commit(&store).unwrap();
        assert!(s.instances.is_empty());
        assert_eq!(store.get(&account_for(&id)).unwrap(), None);
        assert!(remove_instance(&s, &id).is_err());
    }
}
