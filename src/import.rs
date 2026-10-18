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
    pub credentials: Credentials,
    pub cache_duration_secs: Option<i64>,
    pub topics: Vec<Topic>,
}

/// The credential fields of an instance in the import file.
#[derive(Clone, Debug, PartialEq)]
pub enum Credentials {
    /// No credential fields: an existing instance keeps its credentials.
    Unspecified,
    /// `auth: none`: removes stored credentials.
    None,
    Token(String),
    Basic {
        username: String,
        password: String,
    },
    /// `username` without `password` (as written to `config.imported.yaml`):
    /// keeps the stored password of an existing basic-auth instance.
    Username(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawAuth {
    None,
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
    auth: Option<RawAuth>,
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

/// The parsed URL's serialization (lowercase host, no default port) without
/// trailing `/`, so equivalent spellings map to the same instance.
fn normalize_url(raw: &str) -> Result<String> {
    // Never echo the raw value: it may carry credentials.
    let parsed = url::Url::parse(raw.trim()).map_err(|e| anyhow!("invalid instance url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("instance url must start with http:// or https://");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!(
            "instance url for host {} must not contain credentials; use token or username/password",
            parsed.host_str().unwrap_or_default()
        );
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!(
            "instance url for host {} must not contain a query or fragment",
            parsed.host_str().unwrap_or_default()
        );
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// Whether a stored instance URL (possibly saved in an older, unnormalized
/// form) denotes the normalized import URL.
fn same_url(stored: &str, normalized: &str) -> bool {
    match normalize_url(stored) {
        Ok(n) => n == normalized,
        Err(_) => stored.trim_end_matches('/') == normalized,
    }
}

/// Errors name positions, never values (they may be secrets in the wrong
/// field); the caller adds the instance number.
fn convert_instance(raw: RawInstance) -> Result<ImportInstance> {
    let url = normalize_url(&raw.url)?;
    if raw.token.is_some() && (raw.username.is_some() || raw.password.is_some()) {
        bail!("token and username/password are mutually exclusive");
    }
    if raw.auth.is_some()
        && (raw.token.is_some() || raw.username.is_some() || raw.password.is_some())
    {
        bail!("auth: none excludes token, username and password");
    }
    if raw.password.is_some() && raw.username.is_none() {
        bail!("password requires a username");
    }
    for (field, value) in [("token", &raw.token), ("password", &raw.password)] {
        if value.as_deref() == Some("") {
            bail!("{field} must not be empty");
        }
    }
    if raw.username.as_deref() == Some("") {
        bail!("username must not be empty");
    }
    let cache_duration_secs = match raw.cache_duration {
        None => None,
        Some(text) => {
            let d: Duration = humantime::parse_duration(&text)
                .map_err(|_| anyhow!("invalid cache_duration (e.g. 12h, 72h)"))?;
            let secs =
                i64::try_from(d.as_secs()).map_err(|_| anyhow!("cache_duration too large"))?;
            if secs == 0 {
                bail!("cache_duration must be positive");
            }
            Some(secs)
        }
    };
    let mut topics: Vec<Topic> = Vec::new();
    for (n, t) in (1..).zip(raw.topics) {
        let (name, urgent) = match t {
            RawTopic::Short(name) => (name, None),
            RawTopic::Long {
                name,
                urgent_breaks_dnd,
            } => (name, urgent_breaks_dnd),
        };
        if !valid_topic_name(&name) {
            bail!("topic {n}: invalid topic name (allowed: A-Z a-z 0-9 _ -, 1–64 chars)");
        }
        if topics.iter().any(|t| t.name == name) {
            bail!("topic {n}: duplicate topic name");
        }
        topics.push(Topic {
            name,
            muted: false,
            urgent_breaks_dnd: urgent,
        });
    }
    let credentials = match (raw.auth, raw.token, raw.username, raw.password) {
        (Some(RawAuth::None), ..) => Credentials::None,
        (None, Some(token), _, _) => Credentials::Token(token),
        (None, None, Some(username), Some(password)) => Credentials::Basic { username, password },
        (None, None, Some(username), None) => Credentials::Username(username),
        (None, None, None, _) => Credentials::Unspecified,
    };
    Ok(ImportInstance {
        url,
        credentials,
        cache_duration_secs,
        topics,
    })
}

/// Makes a serde_yaml_ng error fit for users and notifications: counts
/// instances from 1 (serde prints a 0-based path like `instances[1]: `) and
/// drops quoted values, which may be secrets pasted into the wrong field.
fn humanize_yaml_error(msg: &str) -> String {
    let index = msg
        .strip_prefix("instances[")
        .and_then(|rest| rest.split_once(']'))
        .and_then(|(n, rest)| Some((n.parse::<usize>().ok()?, rest)));
    let msg = match index {
        Some((n, rest)) => {
            let rest = rest.strip_prefix('.').unwrap_or(rest);
            format!(
                "instance {}{}{rest}",
                n + 1,
                if rest.starts_with(':') { "" } else { ": " }
            )
        }
        None => msg.to_owned(),
    };
    redact_values(&msg)
}

/// Removes the quoted value (`` `value` `` or `"value"`) from a serde
/// message, keeping key names after "field " (missing/unknown/duplicate
/// field). serde puts at most one value before the schema part
/// (", expected …"), which starts at the *last* ", expected" because a value
/// may contain that text; the value runs from the first quote to the last
/// matching one before it, as backticks inside values are not escaped.
fn redact_values(msg: &str) -> String {
    let (head, tail) = msg.split_at(msg.rfind(", expected").unwrap_or(msg.len()));
    let Some(start) = head.find(['`', '"']) else {
        return msg.to_owned();
    };
    let delim = head[start..].chars().next().expect("found above");
    let before = &head[..start];
    let mut out = before.to_owned();
    match head.rfind(delim).filter(|&end| end > start) {
        Some(end) => {
            if delim == '`' && before.ends_with("field ") {
                out.push_str(&head[start..=end]);
            } else {
                out.truncate(out.trim_end().len());
            }
            let after = &head[end + 1..];
            // Only a location like " at line 3 column 5" may follow.
            if !after.contains(['`', '"']) {
                out.push_str(after);
            }
        }
        // Unterminated: drop the rest of the head.
        None => out.truncate(out.trim_end().len()),
    }
    out.push_str(tail);
    out
}

/// Errors are self-contained in their outermost message (shown in a
/// notification) and never echo secrets.
pub fn parse_import(yaml: &str) -> Result<ImportFile> {
    let raw: RawFile =
        serde_yaml_ng::from_str(yaml).map_err(|e| anyhow!(humanize_yaml_error(&e.to_string())))?;
    let instances = raw
        .instances
        .into_iter()
        .zip(1..)
        .map(|(raw, n)| convert_instance(raw).map_err(|e| anyhow!("instance {n}: {e}")))
        .collect::<Result<Vec<_>>>()?;
    for (i, inst) in instances.iter().enumerate() {
        if instances[..i].iter().any(|o| o.url == inst.url) {
            bail!("duplicate instance url {}", inst.url);
        }
    }
    Ok(ImportFile {
        urgent_breaks_dnd: raw.urgent_breaks_dnd,
        instances,
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

/// How an import changes an instance's credentials.
enum AuthPlan {
    Set(AuthKind, String),
    Keep(AuthKind),
    Remove,
}

fn auth_plan(imp: &ImportInstance, existing: Option<&AuthKind>) -> Result<AuthPlan> {
    Ok(match (&imp.credentials, existing) {
        (Credentials::Token(token), _) => AuthPlan::Set(AuthKind::Token, token.clone()),
        (Credentials::Basic { username, password }, _) => AuthPlan::Set(
            AuthKind::Basic {
                username: username.clone(),
            },
            password.clone(),
        ),
        (Credentials::Username(username), Some(AuthKind::Basic { username: stored }))
            if stored == username =>
        {
            AuthPlan::Keep(AuthKind::Basic {
                username: username.clone(),
            })
        }
        (Credentials::Username(_), Some(AuthKind::Basic { .. })) => {
            bail!("{}: changed username needs a password", imp.url)
        }
        (Credentials::Username(_), _) => bail!(
            "{}: username without password needs an already stored password",
            imp.url
        ),
        (Credentials::Unspecified, Some(kind)) if *kind != AuthKind::None => {
            AuthPlan::Keep(kind.clone())
        }
        (Credentials::Unspecified | Credentials::None, _) => AuthPlan::Remove,
    })
}

/// Stores secrets in `secrets` and returns the new settings; does not persist
/// them. All instances are validated before any secret is touched.
pub fn apply_import(
    existing: Option<Settings>,
    file: &ImportFile,
    secrets: &dyn SecretStore,
) -> Result<Settings> {
    let mut settings = existing.unwrap_or_default();
    if let Some(v) = file.urgent_breaks_dnd {
        settings.urgent_breaks_dnd = v;
    }
    let position = |settings: &Settings, url: &str| {
        settings
            .instances
            .iter()
            .position(|i| same_url(&i.url, url))
    };
    let plans = file
        .instances
        .iter()
        .map(|imp| {
            let existing = position(&settings, &imp.url).map(|p| &settings.instances[p].auth);
            auth_plan(imp, existing)
        })
        .collect::<Result<Vec<_>>>()?;
    for (imp, plan) in file.instances.iter().zip(plans) {
        let pos = position(&settings, &imp.url);
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
        inst.topics = imp
            .topics
            .iter()
            .map(|t| Topic {
                muted: inst.topics.iter().any(|o| o.name == t.name && o.muted),
                ..t.clone()
            })
            .collect();
        if let Some(secs) = imp.cache_duration_secs {
            inst.cache_duration_secs = secs;
        }
        let account = account_for(&inst.id);
        inst.auth = match plan {
            AuthPlan::Set(kind, secret) => {
                secrets.set(&account, &secret)?;
                kind
            }
            AuthPlan::Keep(kind) => kind,
            AuthPlan::Remove => {
                secrets.delete(&account)?;
                AuthKind::None
            }
        };
        match pos {
            Some(p) => settings.instances[p] = inst,
            None => settings.instances.push(inst),
        }
    }
    Ok(settings)
}

/// Starts `config.imported.yaml`.
const SANITIZED_HEADER: &str = "\
# ntfy-native – importierte Konfiguration / imported configuration
#
# DE: Token und Passwörter wurden in den Schlüsselbund (macOS) bzw. die
#     Windows-Anmeldeinformationsverwaltung verschoben und aus dieser Datei
#     entfernt. Die Datei kann bearbeitet und mit
#     `ntfy-native --import <pfad>` erneut importiert werden (App vorher
#     beenden). Gespeicherte Zugangsdaten bleiben dabei erhalten;
#     `auth: none` bei einer Instanz entfernt sie.
# EN: Tokens and passwords were moved to the Keychain (macOS) or the Windows
#     Credential Manager and removed from this file. You can edit this file
#     and re-import it with `ntfy-native --import <path>` (quit the app
#     first). Stored credentials are kept; `auth: none` on an instance
#     removes them.

";

/// Everything except token/password, so the file stays a readable record.
pub fn sanitized_yaml(file: &ImportFile) -> String {
    let raw = RawFile {
        urgent_breaks_dnd: file.urgent_breaks_dnd,
        instances: file
            .instances
            .iter()
            .map(|i| RawInstance {
                url: i.url.clone(),
                auth: (i.credentials == Credentials::None).then_some(RawAuth::None),
                token: None,
                username: match &i.credentials {
                    Credentials::Basic { username, .. } | Credentials::Username(username) => {
                        Some(username.clone())
                    }
                    _ => None,
                },
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
    let yaml = serde_yaml_ng::to_string(&raw).expect("plain data serializes");
    format!("{SANITIZED_HEADER}{yaml}")
}

/// Notepad and PowerShell may save UTF-16; YAML must be UTF-8 here.
fn decode_utf8(bytes: Vec<u8>) -> std::result::Result<String, &'static str> {
    // ASCII text in UTF-16 without a byte order mark is valid UTF-8 with NULs.
    let utf16 =
        bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) || bytes.contains(&0);
    if utf16 {
        return Err("must be saved as UTF-8 (UTF-16 detected?)");
    }
    String::from_utf8(bytes).map_err(|_| "must be saved as UTF-8")
}

const SANITIZED_NAME: &str = "config.imported.yaml";

pub fn import_file(
    path: &Path,
    existing: Option<Settings>,
    secrets: &dyn SecretStore,
    settings_path: &Path,
) -> Result<Settings> {
    let bytes = fs::read(path).map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
    let yaml = decode_utf8(bytes).map_err(|e| anyhow!("{} {e}", path.display()))?;
    let file = parse_import(&yaml)?;
    let settings = apply_import(existing, &file, secrets)?;
    save_json(settings_path, &settings)?;

    let imported = path.with_file_name(SANITIZED_NAME);
    let tmp = path.with_file_name("config.imported.yaml.tmp");
    fs::write(&tmp, sanitized_yaml(&file))
        .and_then(|()| fs::rename(&tmp, &imported))
        .with_context(|| format!("writing {}", imported.display()))?;
    // Re-importing the sanitized file itself rewrote it in place.
    // `imported` shares `path`'s parent, so only the name needs comparing
    // (case-insensitively, for case-insensitive filesystems).
    let is_target = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(SANITIZED_NAME));
    if !is_target {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
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
        let y = one("https://x.org", &["ok", "\"bad topic\""]);
        let err = parse_import(&y).unwrap_err().to_string();
        assert_eq!(
            err,
            "instance 1: topic 2: invalid topic name (allowed: A-Z a-z 0-9 _ -, 1–64 chars)"
        );
    }

    #[test]
    fn value_errors_name_the_position() {
        let y = "instances:\n  - url: https://a.org\n  - url: https://b.org\n    cache_duration: soon\n";
        assert_eq!(
            parse_import(y).unwrap_err().to_string(),
            "instance 2: invalid cache_duration (e.g. 12h, 72h)"
        );
        let y = one("https://x.org", &["a", "b", "a"]);
        assert_eq!(
            parse_import(&y).unwrap_err().to_string(),
            "instance 1: topic 3: duplicate topic name"
        );
        let y =
            "instances:\n  - url: https://x.org\n    token: a\n    username: u\n    password: p\n";
        assert_eq!(
            parse_import(y).unwrap_err().to_string(),
            "instance 1: token and username/password are mutually exclusive"
        );
    }

    #[test]
    fn rejects_token_and_password() {
        let y =
            "instances:\n  - url: https://x.org\n    token: a\n    username: u\n    password: p\n";
        assert!(parse_import(y).is_err());
    }

    #[test]
    fn username_without_password_needs_stored_password() {
        let store = MemoryStore::new();
        let y = "instances:\n  - url: https://x.org\n    username: u\n";
        let f = parse_import(y).unwrap();
        assert!(apply_import(None, &f, &store).is_err());
        let token = "instances:\n  - url: https://x.org\n    token: tk\n";
        let s = apply_import(None, &parse_import(token).unwrap(), &store).unwrap();
        assert!(apply_import(Some(s), &f, &store).is_err());
    }

    #[test]
    fn username_only_keeps_stored_password() {
        let store = MemoryStore::new();
        let y = "instances:\n  - url: https://x.org\n    username: alice\n    password: pw\n";
        let s1 = apply_import(None, &parse_import(y).unwrap(), &store).unwrap();
        let y2 = "instances:\n  - url: https://x.org\n    username: alice\n";
        let s2 = apply_import(Some(s1), &parse_import(y2).unwrap(), &store).unwrap();
        assert_eq!(
            s2.instances[0].auth,
            AuthKind::Basic {
                username: "alice".into()
            }
        );
        assert_eq!(store.get("instance:x.org").unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn changed_username_requires_password() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let store = MemoryStore::new();
        let cfg = dir.path().join("config.yaml");
        fs::write(
            &cfg,
            "instances:\n  - url: https://x.org\n    username: alice\n    password: pw\n",
        )
        .unwrap();
        let s1 = import_file(&cfg, None, &store, &settings_path).unwrap();
        let before = fs::read(&settings_path).unwrap();

        fs::write(
            &cfg,
            "instances:\n  - url: https://x.org\n    username: bob\n",
        )
        .unwrap();
        let err = import_file(&cfg, Some(s1), &store, &settings_path).unwrap_err();
        assert!(err.to_string().contains("changed username"), "{err}");
        assert_eq!(fs::read(&settings_path).unwrap(), before);
        assert_eq!(store.get("instance:x.org").unwrap().as_deref(), Some("pw"));
        assert!(cfg.exists());
    }

    #[test]
    fn failed_validation_touches_no_secret() {
        let store = MemoryStore::new();
        let y = "instances:\n  - url: https://a.org\n    token: tk\n  - url: https://b.org\n    username: u\n";
        assert!(apply_import(None, &parse_import(y).unwrap(), &store).is_err());
        assert_eq!(store.get("instance:a.org").unwrap(), None);
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
    fn reimport_without_credentials_keeps_secret() {
        let store = MemoryStore::new();
        let s1 = apply_import(None, &parse_import(EXAMPLE).unwrap(), &store).unwrap();
        let y = one("https://ntfy.example.org", &["a"]);
        let s2 = apply_import(Some(s1), &parse_import(&y).unwrap(), &store).unwrap();
        assert_eq!(s2.instances[0].auth, AuthKind::Token);
        assert_eq!(
            store.get("instance:ntfy.example.org").unwrap().as_deref(),
            Some("tk_xxxxxxxx")
        );
    }

    #[test]
    fn explicit_auth_none_removes_secret() {
        let store = MemoryStore::new();
        let s1 = apply_import(None, &parse_import(EXAMPLE).unwrap(), &store).unwrap();
        let y = "instances:\n  - url: https://ntfy.example.org\n    auth: none\n    topics: [a]\n";
        let f = parse_import(y).unwrap();
        assert!(sanitized_yaml(&f).contains("auth: none"));
        let s2 = apply_import(Some(s1), &f, &store).unwrap();
        assert_eq!(s2.instances[0].auth, AuthKind::None);
        assert_eq!(store.get("instance:ntfy.example.org").unwrap(), None);
    }

    #[test]
    fn auth_none_excludes_credentials() {
        let y = "instances:\n  - url: https://x.org\n    auth: none\n    token: tk\n";
        assert!(parse_import(y).is_err());
        let y = "instances:\n  - url: https://x.org\n    auth: basic\n";
        assert!(parse_import(y).is_err());
    }

    #[test]
    fn normalize_url_case_and_default_port() {
        let url = |raw: &str| normalize_url(raw).unwrap();
        assert_eq!(
            url(" https://NTFY.Example.org:443/ "),
            "https://ntfy.example.org"
        );
        assert_eq!(url("http://h.org:80"), "http://h.org");
        assert_eq!(url("https://h.org:8443/sub/"), "https://h.org:8443/sub");
        let y = "instances:\n  - url: https://X.org\n  - url: https://x.org:443\n";
        assert!(parse_import(y).is_err(), "same instance twice");

        // Settings saved with an unnormalized URL still match on re-import.
        let store = MemoryStore::new();
        let mut s = apply_import(
            None,
            &parse_import(&one("https://x.org", &["a"])).unwrap(),
            &store,
        )
        .unwrap();
        s.instances[0].url = "https://X.ORG:443/".into();
        let s2 = apply_import(
            Some(s),
            &parse_import(&one("https://x.org/", &["b"])).unwrap(),
            &store,
        )
        .unwrap();
        assert_eq!(s2.instances.len(), 1);
        assert_eq!(s2.instances[0].url, "https://x.org");
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
    fn reimport_of_sanitized_file_keeps_file_and_secret() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.yaml");
        let settings_path = dir.path().join("settings.json");
        let store = MemoryStore::new();
        let y = EXAMPLE.to_string() + "  - url: https://b.org\n    username: u\n    password: pw\n";
        fs::write(&cfg, y).unwrap();
        let s1 = import_file(&cfg, None, &store, &settings_path).unwrap();

        let imported = dir.path().join("config.imported.yaml");
        let text = fs::read_to_string(&imported).unwrap();
        assert!(text.starts_with("# ntfy-native"), "{text}");
        assert!(
            text.contains("Schlüsselbund") && text.contains("--import"),
            "{text}"
        );
        fs::write(&imported, text.replace("- backups", "- backups2")).unwrap();

        let s2 = import_file(&imported, Some(s1), &store, &settings_path).unwrap();
        assert!(imported.exists());
        assert!(fs::read_to_string(&imported).unwrap().contains("backups2"));
        assert_eq!(s2.instances[0].topics[0].name, "backups2");
        assert_eq!(s2.instances[0].auth, AuthKind::Token);
        assert_eq!(
            store.get("instance:ntfy.example.org").unwrap().as_deref(),
            Some("tk_xxxxxxxx")
        );
        assert_eq!(
            s2.instances[1].auth,
            AuthKind::Basic {
                username: "u".into()
            }
        );
        assert_eq!(store.get("instance:b.org").unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn reimport_case_variant_path_keeps_file() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let variant = dir.path().join("Config.Imported.yaml");
        fs::write(&variant, one("https://x.org", &["a"])).unwrap();
        import_file(&variant, None, &MemoryStore::new(), &settings_path).unwrap();
        assert!(dir.path().join("config.imported.yaml").exists());
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

    #[test]
    fn rejects_url_with_credentials() {
        let err = parse_import(&one("https://u:s3cretpw@host.org", &["a"])).unwrap_err();
        assert!(!format!("{err:#}").contains("s3cretpw"), "{err:#}");
        assert!(parse_import(&one("https://u@host.org", &["a"])).is_err());
    }

    #[test]
    fn rejects_url_with_query_or_fragment() {
        assert!(parse_import(&one("https://h.org/?a=b", &["a"])).is_err());
        assert!(parse_import(&one("https://h.org/#f", &["a"])).is_err());
    }

    #[test]
    fn rejects_duplicate_urls() {
        let y = "instances:\n  - url: https://x.org/\n  - url: https://x.org\n";
        assert!(parse_import(y).is_err());
    }

    #[test]
    fn rejects_password_without_username() {
        let y = "instances:\n  - url: https://x.org\n    password: p\n";
        assert!(parse_import(y).is_err());
    }

    #[test]
    fn rejects_empty_token() {
        let y = "instances:\n  - url: https://x.org\n    token: \"\"\n";
        assert!(parse_import(y).is_err());
    }

    #[test]
    fn missing_url_names_field_and_instance() {
        let y = "instances:\n  - url: https://a.org\n  - topics: [a]\n";
        // The outermost message is shown in a notification, so it must
        // name the problem on its own.
        let err = parse_import(y).unwrap_err().to_string();
        assert!(err.contains("instance 2"), "{err}");
        assert!(err.contains("missing field `url`"), "{err}");
    }

    #[test]
    fn yaml_error_paths_count_from_one() {
        assert_eq!(
            humanize_yaml_error("instances[0].password: invalid type: sequence"),
            "instance 1: password: invalid type: sequence"
        );
        assert_eq!(
            humanize_yaml_error("instances[9]: missing field `url` at line 4"),
            "instance 10: missing field `url` at line 4"
        );
        assert_eq!(
            humanize_yaml_error("urgent_breaks_dnd: invalid type"),
            "urgent_breaks_dnd: invalid type"
        );
    }

    #[test]
    fn parse_errors_never_echo_secrets() {
        let cases = [
            // Errors elsewhere in a file holding a token or password.
            "instances:\n  - url: https://a.org\n    token: tk_s3cret\n    topcs: [a]\n",
            "instances:\n  - url: https://a.org\n    token: tk_s3cret\n  - topics: [a]\n",
            "urgent_breaks_dnd: maybe\ninstances:\n  - url: https://a.org\n    token: tk_s3cret\n",
            "instances:\n  - url: https://a.org\n    token: tk_s3cret\n    topics: [\"bad topic\"]\n",
            "instances:\n  - url: https://a.org\n    username: u\n    password: tk_s3cret\n    cache_duration: soon\n",
            "instances:\n  - url: https://a.org\n    token: tk_s3cret\n    topics: [a\n",
            // A secret of the wrong type.
            "instances:\n  - url: https://a.org\n    username: u\n    password: [tk_s3cret]\n",
            // A secret pasted into the wrong field: serde quotes the value.
            "instances:\n  - url: https://a.org\n    auth: tk_s3cret\n",
            "instances:\n  - url: https://a.org\n    auth: Bearer tk_s3cret\n",
            "instances:\n  - url: https://a.org\n    topics: tk_s3cret\n",
            "urgent_breaks_dnd: tk_s3cret\n",
            "urgent_breaks_dnd: 4711s3cret0815\n",
            "instances: tk_s3cret\n",
            "instances:\n  - tk_s3cret\n",
            "instances:\n  - url: https://a.org\n    topics:\n      - name: a\n        urgent_breaks_dnd: tk_s3cret\n",
            // Backticks and quotes inside values are not escaped by serde.
            "instances:\n  - url: https://a.org\n    auth: \"tk_`s3cret\"\n",
            "instances:\n  - url: https://a.org\n    auth: \"a`, b s3cret\"\n",
            "instances:\n  - url: https://a.org\n    auth: \"x`, expected `none` s3cret\"\n",
            "instances:\n  - url: https://a.org\n    auth: \"s3cret`\"\n",
            "instances:\n  - url: https://a.org\n    auth: \"`s3cret`\"\n",
            "urgent_breaks_dnd: \"a\\\"`s3cret\"\n",
            "urgent_breaks_dnd: \"x\\\", expected s3cret\"\n",
            // Values our own validation rejects.
            "instances:\n  - url: https://a.org\n    cache_duration: s3cret\n",
            "instances:\n  - url: https://a.org\n    cache_duration: 12 s3cret\n",
            "instances:\n  - url: https://a.org\n    topics: [\"bad s3cret\"]\n",
            "instances:\n  - url: https://a.org\n    topics: [s3cret, s3cret]\n",
        ];
        for y in cases {
            let err = format!("{:#}", parse_import(y).unwrap_err());
            assert!(!err.contains("s3cret"), "{y}\n=> {err}");
        }
    }

    #[test]
    fn yaml_errors_drop_quoted_values_but_keep_keys() {
        let cases = [
            (
                "instances[0].auth: unknown variant `tk_x`, expected `none` at line 3 column 11",
                "instance 1: auth: unknown variant, expected `none` at line 3 column 11",
            ),
            (
                "instances[0].topics: invalid type: string \"tk \\\"x\\\" y\", expected a sequence",
                "instance 1: topics: invalid type: string, expected a sequence",
            ),
            (
                "urgent_breaks_dnd: invalid value: integer `4711`, expected a boolean",
                "urgent_breaks_dnd: invalid value: integer, expected a boolean",
            ),
            (
                "instances[1]: unknown field `topcs`, expected one of `url`, `topics`",
                "instance 2: unknown field `topcs`, expected one of `url`, `topics`",
            ),
            (
                "instances[0]: missing field `url` at line 2 column 5",
                "instance 1: missing field `url` at line 2 column 5",
            ),
            (
                "duplicate field `url` at line 3",
                "duplicate field `url` at line 3",
            ),
            // "expected" inside a value does not end the redaction.
            (
                "unknown variant `expected tk_x`, expected `none`",
                "unknown variant, expected `none`",
            ),
            // Backticks inside a value are not escaped by serde.
            (
                "unknown variant `tk_`x`, expected `none` at line 3 column 11",
                "unknown variant, expected `none` at line 3 column 11",
            ),
            (
                "unknown variant `a`, b tk_x`, expected `none`",
                "unknown variant, expected `none`",
            ),
            (
                "unknown variant `x`, expected `none` tk_x`, expected `none`",
                "unknown variant, expected `none`",
            ),
            // An unterminated quote drops the rest.
            ("invalid type: string \"tk_x", "invalid type: string"),
        ];
        for (raw, want) in cases {
            assert_eq!(humanize_yaml_error(raw), want, "{raw}");
        }
    }

    #[test]
    fn utf16_file_asks_for_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.yaml");
        let settings_path = dir.path().join("settings.json");
        for bom in [[0xFF, 0xFE], [0xFE, 0xFF]] {
            let mut bytes = bom.to_vec();
            for u in one("https://x.org", &["a"]).encode_utf16() {
                bytes.extend(if bom[0] == 0xFF {
                    u.to_le_bytes()
                } else {
                    u.to_be_bytes()
                });
            }
            fs::write(&cfg, bytes).unwrap();
            let err = import_file(&cfg, None, &MemoryStore::new(), &settings_path)
                .unwrap_err()
                .to_string();
            assert!(err.contains("must be saved as UTF-8"), "{err}");
            assert!(err.contains("UTF-16"), "{err}");
            assert!(cfg.exists());
        }
    }

    #[test]
    fn invalid_utf8_asks_for_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.yaml");
        fs::write(&cfg, b"instances: [\xE4]\n").unwrap();
        let err = import_file(&cfg, None, &MemoryStore::new(), &dir.path().join("s.json"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be saved as UTF-8"), "{err}");
    }

    #[test]
    fn utf8_bom_is_accepted() {
        let y = format!("\u{feff}{}", one("https://x.org", &["a"]));
        assert_eq!(parse_import(&y).unwrap().instances.len(), 1);
    }

    #[test]
    fn unreadable_file_error_names_path_and_cause() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.yaml");
        let err = import_file(&cfg, None, &MemoryStore::new(), &dir.path().join("s.json"))
            .unwrap_err()
            .to_string();
        let prefix = format!("cannot read {}: ", cfg.display());
        assert!(
            err.len() > prefix.len() && err.starts_with(&prefix),
            "{err}"
        );
    }

    #[test]
    fn reimport_preserves_muted_by_name() {
        let store = MemoryStore::new();
        let mut s = apply_import(
            None,
            &parse_import(&one("https://a.org", &["keep", "drop"])).unwrap(),
            &store,
        )
        .unwrap();
        for t in &mut s.instances[0].topics {
            t.muted = true;
        }
        let y = "instances:\n  - url: https://a.org\n    topics:\n      - keep\n      - fresh\n      - name: drop2\n        urgent_breaks_dnd: true\n";
        let s2 = apply_import(Some(s), &parse_import(y).unwrap(), &store).unwrap();
        let t = &s2.instances[0].topics;
        assert!(t[0].muted && t[0].name == "keep");
        assert!(!t[1].muted && t[1].name == "fresh");
        assert!(!t[2].muted && t[2].urgent_breaks_dnd == Some(true));
    }
}
