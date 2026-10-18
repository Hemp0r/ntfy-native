use serde::{Deserialize, Serialize};

const DEFAULT_CACHE_DURATION_SECS: i64 = 43200;

fn default_version() -> u32 {
    1
}

fn default_cache_duration() -> i64 {
    DEFAULT_CACHE_DURATION_SECS
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub instances: Vec<Instance>,
    pub urgent_breaks_dnd: bool,
    /// `i64::MAX` means paused indefinitely.
    pub paused_until: Option<i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: default_version(),
            instances: Vec::new(),
            urgent_breaks_dnd: false,
            paused_until: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Instance {
    pub id: String,
    pub url: String,
    pub auth: AuthKind,
    #[serde(default = "default_cache_duration")]
    pub cache_duration_secs: i64,
    pub topics: Vec<Topic>,
}

impl Default for Instance {
    fn default() -> Self {
        Self {
            id: String::new(),
            url: String::new(),
            auth: AuthKind::None,
            cache_duration_secs: DEFAULT_CACHE_DURATION_SECS,
            topics: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthKind {
    #[default]
    None,
    Token,
    Basic {
        username: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Topic {
    pub name: String,
    pub muted: bool,
    pub urgent_breaks_dnd: Option<bool>,
}

impl Settings {
    pub fn instance(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }

    /// Per-topic override wins over the global setting.
    pub fn urgent_allowed(&self, instance_id: &str, topic: &str) -> bool {
        self.instance(instance_id)
            .and_then(|i| i.topics.iter().find(|t| t.name == topic))
            .and_then(|t| t.urgent_breaks_dnd)
            .unwrap_or(self.urgent_breaks_dnd)
    }
}

impl Instance {
    pub fn host(&self) -> String {
        url::Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| self.url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(global: bool, topic: Option<bool>) -> Settings {
        Settings {
            urgent_breaks_dnd: global,
            instances: vec![Instance {
                id: "a".into(),
                topics: vec![Topic {
                    name: "t".into(),
                    muted: false,
                    urgent_breaks_dnd: topic,
                }],
                ..Instance::default()
            }],
            ..Settings::default()
        }
    }

    #[test]
    fn old_json_gets_defaults() {
        let s: Settings = serde_json::from_str(
            r#"{"instances":[{"id":"a","url":"https://x","topics":[{"name":"t"}]}]}"#,
        )
        .unwrap();
        assert_eq!(s.version, 1);
        assert_eq!(s.instances[0].cache_duration_secs, 43200);
        assert_eq!(s.instances[0].auth, AuthKind::None);
        assert!(!s.urgent_breaks_dnd);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s: Settings = serde_json::from_str(r#"{"future":1,"instances":[]}"#).unwrap();
        assert!(s.instances.is_empty());
    }

    #[test]
    fn auth_kind_serde_shape() {
        let a: AuthKind = serde_json::from_str(r#"{"type":"basic","username":"u"}"#).unwrap();
        assert_eq!(
            a,
            AuthKind::Basic {
                username: "u".into()
            }
        );
        assert_eq!(
            serde_json::to_string(&AuthKind::Token).unwrap(),
            r#"{"type":"token"}"#
        );
    }

    #[test]
    fn urgent_allowed_topic_overrides_global() {
        assert!(settings_with(false, Some(true)).urgent_allowed("a", "t"));
        assert!(!settings_with(true, Some(false)).urgent_allowed("a", "t"));
        assert!(settings_with(true, None).urgent_allowed("a", "t"));
        assert!(!settings_with(false, None).urgent_allowed("a", "t"));
        assert!(settings_with(true, None).urgent_allowed("a", "unknown"));
    }

    #[test]
    fn host_extracts_or_falls_back() {
        let mut i = Instance {
            url: "https://ntfy.example.com:8443/path".into(),
            ..Instance::default()
        };
        assert_eq!(i.host(), "ntfy.example.com");
        i.url = "not a url".into();
        assert_eq!(i.host(), "not a url");
    }
}
