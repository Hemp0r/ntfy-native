use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::secrets::{SecretStore, account_for};
use crate::settings::{AuthKind, Instance, Settings, Topic};
use crate::store::save_json;

#[derive(Clone, Debug, PartialEq)]
pub struct ImportFile {
    pub urgent_breaks_dnd: Option<bool>,
    pub instances: Vec<ImportInstance>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportInstance {
    pub url: String,
    pub token: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub cache_duration_secs: Option<i64>,
    pub topics: Vec<Topic>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum RawTopic {
    Short(String),
    Long {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        urgent_breaks_dnd: Option<bool>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInstance {
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_duration: Option<String>,
    #[serde(default)]
    topics: Vec<RawTopic>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    urgent_breaks_dnd: Option<bool>,
    #[serde(default)]
    instances: Vec<RawInstance>,
}

fn valid_topic_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn normalize_url(raw: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');
    let parsed = url::Url::parse(url).map_err(|e| anyhow!("invalid url {raw:?}: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("url must start with http:// or https://: {raw:?}");
    }
    Ok(url.to_string())
}

fn convert_instance(raw: RawInstance) -> Result<ImportInstance> {
    let url = normalize_url(&raw.url)?;
    if raw.token.is_some() && (raw.username.is_some() || raw.password.is_some()) {
        bail!("{url}: token and username/password are mutually exclusive");
    }
    if raw.username.is_some() != raw.password.is_some() {
        bail!("{url}: username and password must be set together");
    }
    for (field, value) in [("token", &raw.token), ("password", &raw.password)] {
        if value.as_deref() == Some("") {
            bail!("{url}: {field} must not be empty");
        }
    }
    if raw.username.as_deref() == Some("") {
        bail!("{url}: username must not be empty");
    }
    let cache_duration_secs = match raw.cache_duration {
        None => None,
        Some(text) => {
            let d: Duration = humantime::parse_duration(&text)
                .map_err(|e| anyhow!("{url}: invalid cache_duration {text:?}: {e}"))?;
            let secs = i64::try_from(d.as_secs())
                .map_err(|_| anyhow!("{url}: cache_duration {text:?} too large"))?;
            if secs == 0 {
                bail!("{url}: cache_duration must be positive");
            }
            Some(secs)
        }
    };
    let mut topics: Vec<Topic> = Vec::new();
    for t in raw.topics {
        let (name, urgent) = match t {
            RawTopic::Short(name) => (name, None),
            RawTopic::Long {
                name,
                urgent_breaks_dnd,
            } => (name, urgent_breaks_dnd),
        };
        if !valid_topic_name(&name) {
            bail!("{url}: invalid topic name {name:?} (allowed: -_A-Za-z0-9, 1-64 chars)");
        }
        if topics.iter().any(|t| t.name == name) {
            bail!("{url}: duplicate topic {name:?}");
        }
        topics.push(Topic {
            name,
            muted: false,
            urgent_breaks_dnd: urgent,
        });
    }
    Ok(ImportInstance {
        url,
        token: raw.token,
        username: raw.username,
        password: raw.password,
        cache_duration_secs,
        topics,
    })
}

pub fn parse_import(yaml: &str) -> Result<ImportFile> {
    let raw: RawFile = serde_yaml_ng::from_str(yaml).context("parsing config.yaml")?;
    Ok(ImportFile {
        urgent_breaks_dnd: raw.urgent_breaks_dnd,
        instances: raw
            .instances
            .into_iter()
            .map(convert_instance)
            .collect::<Result<_>>()?,
    })
}

fn unique_id(settings: &Settings, base: &str) -> String {
    let taken = |id: &str| settings.instances.iter().any(|i| i.id == id);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("unbounded range")
}

/// Stores secrets in `secrets` and returns the new settings; does not persist them.
pub fn apply_import(
    existing: Option<Settings>,
    file: &ImportFile,
    secrets: &dyn SecretStore,
) -> Result<Settings> {
    let mut settings = existing.unwrap_or_default();
    if let Some(v) = file.urgent_breaks_dnd {
        settings.urgent_breaks_dnd = v;
    }
    for imp in &file.instances {
        let pos = settings
            .instances
            .iter()
            .position(|i| i.url.trim_end_matches('/') == imp.url);
        let mut inst = match pos {
            Some(p) => settings.instances[p].clone(),
            None => {
                let probe = Instance {
                    url: imp.url.clone(),
                    ..Instance::default()
                };
                Instance {
                    id: unique_id(&settings, &probe.host()),
                    ..probe
                }
            }
        };
        inst.url = imp.url.clone();
        inst.topics = imp.topics.clone();
        if let Some(secs) = imp.cache_duration_secs {
            inst.cache_duration_secs = secs;
        }
        let account = account_for(&inst.id);
        match (&imp.token, &imp.username, &imp.password) {
            (Some(token), _, _) => {
                secrets.set(&account, token)?;
                inst.auth = AuthKind::Token;
            }
            (None, Some(username), Some(password)) => {
                secrets.set(&account, password)?;
                inst.auth = AuthKind::Basic {
                    username: username.clone(),
                };
            }
            _ => {
                secrets.delete(&account)?;
                inst.auth = AuthKind::None;
            }
        }
        match pos {
            Some(p) => settings.instances[p] = inst,
            None => settings.instances.push(inst),
        }
    }
    Ok(settings)
}

/// Everything except token/password, so the file stays a readable record.
pub fn sanitized_yaml(file: &ImportFile) -> String {
    let raw = RawFile {
        urgent_breaks_dnd: file.urgent_breaks_dnd,
        instances: file
            .instances
            .iter()
            .map(|i| RawInstance {
                url: i.url.clone(),
                token: None,
                username: i.username.clone(),
                password: None,
                cache_duration: i
                    .cache_duration_secs
                    .map(|s| humantime::format_duration(Duration::from_secs(s as u64)).to_string()),
                topics: i
                    .topics
                    .iter()
                    .map(|t| match t.urgent_breaks_dnd {
                        None => RawTopic::Short(t.name.clone()),
                        some => RawTopic::Long {
                            name: t.name.clone(),
                            urgent_breaks_dnd: some,
                        },
                    })
                    .collect(),
            })
            .collect(),
    };
    serde_yaml_ng::to_string(&raw).expect("plain data serializes")
}

pub fn import_file(
    path: &Path,
    existing: Option<Settings>,
    secrets: &dyn SecretStore,
    settings_path: &Path,
) -> Result<Settings> {
    let yaml = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let file = parse_import(&yaml)?;
    let settings = apply_import(existing, &file, secrets)?;
    save_json(settings_path, &settings)?;

    let imported = path.with_file_name("config.imported.yaml");
    let tmp = path.with_file_name("config.imported.yaml.tmp");
    fs::write(&tmp, sanitized_yaml(&file))
        .and_then(|()| fs::rename(&tmp, &imported))
        .with_context(|| format!("writing {}", imported.display()))?;
    fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    const EXAMPLE: &str = "\
urgent_breaks_dnd: false
instances:
  - url: https://ntfy.example.org
    token: tk_xxxxxxxx
    cache_duration: 12h
    topics:
      - backups
      - name: frigate
        urgent_breaks_dnd: true
";

    fn one(url: &str, topics: &[&str]) -> String {
        let t: String = topics.iter().map(|t| format!("      - {t}\n")).collect();
        format!("instances:\n  - url: {url}\n    topics:\n{t}")
    }

    #[test]
    fn parses_short_and_long_topics() {
        let f = parse_import(EXAMPLE).unwrap();
        let t = &f.instances[0].topics;
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].name, "backups");
        assert_eq!(t[0].urgent_breaks_dnd, None);
        assert!(!t[0].muted);
        assert_eq!(t[1].name, "frigate");
        assert_eq!(t[1].urgent_breaks_dnd, Some(true));
        assert_eq!(f.urgent_breaks_dnd, Some(false));
    }

    #[test]
    fn parses_duration() {
        let y = EXAMPLE.replace("12h", "36h");
        assert_eq!(
            parse_import(&y).unwrap().instances[0].cache_duration_secs,
            Some(129600)
        );
    }

    #[test]
    fn rejects_invalid_topic_name() {
        let err = parse_import(&one("https://x.org", &["\"bad topic\""])).unwrap_err();
        assert!(err.to_string().contains("bad topic"), "{err}");
    }

    #[test]
    fn rejects_token_and_password() {
        let y =
            "instances:\n  - url: https://x.org\n    token: a\n    username: u\n    password: p\n";
        assert!(parse_import(y).is_err());
    }

    #[test]
    fn rejects_username_without_password() {
        let y = "instances:\n  - url: https://x.org\n    username: u\n";
        assert!(parse_import(y).is_err());
    }

    #[test]
    fn rejects_non_http_url() {
        assert!(parse_import(&one("ftp://x", &["a"])).is_err());
    }

    #[test]
    fn apply_stores_secret_not_in_settings() {
        let store = MemoryStore::new();
        let f = parse_import(EXAMPLE).unwrap();
        let s = apply_import(None, &f, &store).unwrap();
        let inst = &s.instances[0];
        assert_eq!(inst.id, "ntfy.example.org");
        assert_eq!(
            store.get(&account_for(&inst.id)).unwrap().as_deref(),
            Some("tk_xxxxxxxx")
        );
        assert!(!serde_json::to_string(&s).unwrap().contains("tk_"));
        assert_eq!(inst.auth, AuthKind::Token);
        assert_eq!(inst.cache_duration_secs, 43200);
    }

    #[test]
    fn apply_basic_stores_password_keeps_username() {
        let store = MemoryStore::new();
        let y = "instances:\n  - url: https://x.org\n    username: alice\n    password: pw\n";
        let s = apply_import(None, &parse_import(y).unwrap(), &store).unwrap();
        assert_eq!(
            s.instances[0].auth,
            AuthKind::Basic {
                username: "alice".into()
            }
        );
        assert_eq!(store.get("instance:x.org").unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn reimport_keeps_id_and_replaces_topics() {
        let store = MemoryStore::new();
        let first = parse_import(&format!(
            "{}{}",
            one("https://a.org", &["old"]),
            "  - url: https://b.org\n    topics: [keep]\n"
        ))
        .unwrap();
        let s1 = apply_import(None, &first, &store).unwrap();
        let second = parse_import(&one("https://a.org/", &["new1", "new2"])).unwrap();
        let s2 = apply_import(Some(s1.clone()), &second, &store).unwrap();
        assert_eq!(s2.instances.len(), 2);
        assert_eq!(s2.instances[0].id, "a.org");
        let names: Vec<_> = s2.instances[0].topics.iter().map(|t| &t.name).collect();
        assert_eq!(names, ["new1", "new2"]);
        assert_eq!(s2.instances[0].url, "https://a.org");
        assert_eq!(s2.instances[1], s1.instances[1]);
    }

    #[test]
    fn reimport_without_auth_deletes_secret() {
        let store = MemoryStore::new();
        let s1 = apply_import(None, &parse_import(EXAMPLE).unwrap(), &store).unwrap();
        let y = one("https://ntfy.example.org", &["a"]);
        let s2 = apply_import(Some(s1), &parse_import(&y).unwrap(), &store).unwrap();
        assert_eq!(s2.instances[0].auth, AuthKind::None);
        assert_eq!(store.get("instance:ntfy.example.org").unwrap(), None);
    }

    #[test]
    fn global_flag_only_changes_when_set() {
        let store = MemoryStore::new();
        let base = Settings {
            urgent_breaks_dnd: true,
            ..Settings::default()
        };
        let f = parse_import(&one("https://a.org", &["a"])).unwrap();
        let s = apply_import(Some(base), &f, &store).unwrap();
        assert!(s.urgent_breaks_dnd);
    }

    #[test]
    fn ids_unique_per_host() {
        let y = "instances:\n  - url: https://h.org/a\n  - url: https://h.org/b\n";
        let s = apply_import(None, &parse_import(y).unwrap(), &MemoryStore::new()).unwrap();
        assert_eq!(s.instances[0].id, "h.org");
        assert_eq!(s.instances[1].id, "h.org-2");
    }

    #[test]
    fn sanitized_keeps_username_drops_secrets() {
        let y = "instances:\n  - url: https://x.org\n    username: alice\n    password: hunter2\n    cache_duration: 1h\n    topics:\n      - a\n      - name: b\n        urgent_breaks_dnd: true\n";
        let out = sanitized_yaml(&parse_import(y).unwrap());
        assert!(out.contains("alice") && !out.contains("hunter2"), "{out}");
        let back: RawFile = serde_yaml_ng::from_str(&out).unwrap();
        assert_eq!(back.instances[0].topics.len(), 2);
        assert_eq!(back.instances[0].cache_duration.as_deref(), Some("1h"));
    }

    #[test]
    fn import_file_sanitizes_and_removes_original() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.yaml");
        let settings_path = dir.path().join("settings.json");
        let y = EXAMPLE.to_string()
            + "  - url: https://b.org\n    username: u\n    password: password123\n";
        fs::write(&cfg, y).unwrap();
        import_file(&cfg, None, &MemoryStore::new(), &settings_path).unwrap();
        assert!(!cfg.exists());
        let out = fs::read_to_string(dir.path().join("config.imported.yaml")).unwrap();
        assert!(!out.contains("tk_") && !out.contains("password:"), "{out}");
        assert!(settings_path.exists());
    }

    #[test]
    fn import_file_keeps_original_on_secret_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.yaml");
        let settings_path = dir.path().join("settings.json");
        fs::write(&cfg, EXAMPLE).unwrap();
        assert!(import_file(&cfg, None, &MemoryStore::failing(), &settings_path).is_err());
        assert_eq!(fs::read_to_string(&cfg).unwrap(), EXAMPLE);
        assert!(!settings_path.exists());
    }
}
