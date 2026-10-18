use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Interface fixed by the plan; events are short-lived, so boxing buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Open { time: i64 },
    Keepalive { time: i64 },
    Message(Message),
    Other { event: String, time: i64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub time: i64,
    pub topic: String,
    pub title: Option<String>,
    pub message: Option<String>,
    pub tags: Vec<String>,
    /// Always within 1..=5.
    pub priority: u8,
    pub click: Option<String>,
    pub icon: Option<String>,
    pub actions: Vec<Action>,
    pub attachment: Option<Attachment>,
    /// Server time at which the cache drops this message; `None` if unset or not a number.
    pub expires: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    View {
        label: String,
        url: String,
    },
    Http {
        label: String,
        url: String,
        method: String,
        headers: BTreeMap<String, String>,
        body: Option<String>,
    },
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime: Option<String>,
    pub size: Option<u64>,
    pub url: String,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        if let Some(mime) = &self.mime
            && mime.trim().to_ascii_lowercase().starts_with("image/")
        {
            return true;
        }
        self.name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .is_some_and(|ext| matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp"))
    }
}

/// Parses one line of the ntfy JSON stream. Only a non-object line or a
/// missing `event`/`time`/`id` (or `topic` on messages) is an error; malformed
/// optional fields degrade to their defaults.
pub fn parse_event(line: &str) -> Result<Event, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    let obj = value
        .as_object()
        .ok_or_else(|| serde_json::Error::custom("expected a JSON object"))?;
    let event =
        str_field(obj, "event").ok_or_else(|| serde_json::Error::custom("missing event"))?;
    let time = obj
        .get("time")
        .and_then(Value::as_i64)
        .ok_or_else(|| serde_json::Error::custom("missing time"))?;
    let id = str_field(obj, "id").ok_or_else(|| serde_json::Error::custom("missing id"))?;
    match event.as_str() {
        "open" => Ok(Event::Open { time }),
        "keepalive" => Ok(Event::Keepalive { time }),
        "message" => {
            let topic = str_field(obj, "topic")
                .ok_or_else(|| serde_json::Error::custom("missing topic"))?;
            Ok(Event::Message(Message {
                id,
                time,
                topic,
                title: str_field(obj, "title"),
                message: str_field(obj, "message"),
                tags: tags(obj.get("tags")),
                priority: priority(obj.get("priority")),
                click: str_field(obj, "click"),
                icon: str_field(obj, "icon"),
                actions: obj
                    .get("actions")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(action).collect())
                    .unwrap_or_default(),
                attachment: obj.get("attachment").and_then(attachment),
                expires: obj.get("expires").and_then(Value::as_i64),
            }))
        }
        _ => Ok(Event::Other { event, time }),
    }
}

type Obj = serde_json::Map<String, Value>;

fn str_field(obj: &Obj, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn tags(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn priority(v: Option<&Value>) -> u8 {
    match v.and_then(Value::as_i64) {
        Some(p) if p >= 1 => p.min(5) as u8,
        _ => 3,
    }
}

/// Returns `None` for entries that are unusable (not an object, or a known
/// action type missing its required fields).
fn action(v: &Value) -> Option<Action> {
    let obj = v.as_object()?;
    match obj.get("action").and_then(Value::as_str)? {
        "view" => Some(Action::View {
            label: str_field(obj, "label")?,
            url: str_field(obj, "url")?,
        }),
        "http" => Some(Action::Http {
            label: str_field(obj, "label")?,
            url: str_field(obj, "url")?,
            method: str_field(obj, "method")
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "POST".into()),
            headers: obj
                .get("headers")
                .and_then(Value::as_object)
                .map(|h| {
                    h.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            body: str_field(obj, "body"),
        }),
        _ => Some(Action::Unsupported),
    }
}

fn attachment(v: &Value) -> Option<Attachment> {
    let obj = v.as_object()?;
    let url = str_field(obj, "url")?;
    let name = str_field(obj, "name")
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            let path = url.split(['?', '#']).next().unwrap_or(&url);
            path.rsplit('/').next().unwrap_or(path).to_owned()
        });
    Some(Attachment {
        name,
        mime: str_field(obj, "type"),
        size: obj.get("size").and_then(Value::as_u64),
        url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(line: &str) -> Message {
        let Event::Message(m) = parse_event(line).unwrap() else {
            panic!("not a message")
        };
        m
    }

    const BASE: &str = r#""id":"x","time":5,"event":"message","topic":"t""#;

    #[test]
    fn parses_full_message() {
        let l = r#"{"id":"sPs71M8A2T","time":1700000000,"expires":1700043200,"event":"message","topic":"backups","title":"Backup","message":"OK","priority":4,"tags":["white_check_mark","nas"],"click":"https://x/y","icon":"https://x/i.png","actions":[{"action":"view","label":"Open","url":"https://x"},{"action":"http","label":"Ack","url":"https://x/ack"}],"attachment":{"name":"a.png","type":"image/png","size":10,"expires":1,"url":"https://x/a.png"}}"#;
        let m = msg(l);
        assert_eq!(m.priority, 4);
        assert_eq!(m.tags, vec!["white_check_mark", "nas"]);
        assert_eq!(
            m.actions[1],
            Action::Http {
                label: "Ack".into(),
                url: "https://x/ack".into(),
                method: "POST".into(),
                headers: BTreeMap::new(),
                body: None
            }
        );
        assert_eq!(m.expires, Some(1700043200));
        assert!(m.attachment.unwrap().is_image());
    }

    #[test]
    fn non_numeric_expires_is_none() {
        let l = format!(r#"{{{BASE},"expires":"soon"}}"#);
        assert_eq!(msg(&l).expires, None);
        assert_eq!(msg(&format!(r#"{{{BASE}}}"#)).expires, None);
    }

    #[test]
    fn parses_keepalive_and_open() {
        let k = r#"{"id":"x","time":5,"event":"keepalive","topic":"a,b"}"#;
        assert_eq!(parse_event(k).unwrap(), Event::Keepalive { time: 5 });
        let o = r#"{"id":"x","time":6,"event":"open","topic":"a,b"}"#;
        assert_eq!(parse_event(o).unwrap(), Event::Open { time: 6 });
    }

    #[test]
    fn unknown_event_is_other() {
        for ev in ["poll_request", "message_delete"] {
            let l = format!(r#"{{"id":"x","time":7,"event":"{ev}","topic":"a"}}"#);
            assert_eq!(
                parse_event(&l).unwrap(),
                Event::Other {
                    event: ev.into(),
                    time: 7
                }
            );
        }
    }

    #[test]
    fn priority_defaults_and_clamps() {
        assert_eq!(msg(&format!("{{{BASE}}}")).priority, 3);
        assert_eq!(msg(&format!(r#"{{{BASE},"priority":0}}"#)).priority, 3);
        assert_eq!(msg(&format!(r#"{{{BASE},"priority":9}}"#)).priority, 5);
        assert_eq!(msg(&format!(r#"{{{BASE},"priority":-2}}"#)).priority, 3);
        assert_eq!(msg(&format!(r#"{{{BASE},"priority":"5"}}"#)).priority, 3);
        assert_eq!(msg(&format!(r#"{{{BASE},"priority":null}}"#)).priority, 3);
        assert_eq!(msg(&format!(r#"{{{BASE},"priority":1}}"#)).priority, 1);
    }

    #[test]
    fn broadcast_action_is_unsupported() {
        let l = format!(
            r#"{{{BASE},"actions":[{{"action":"broadcast","label":"x","extras":{{"a":"b"}}}}]}}"#
        );
        assert_eq!(msg(&l).actions, vec![Action::Unsupported]);
    }

    #[test]
    fn http_action_details() {
        let l = format!(
            r#"{{{BASE},"actions":[{{"action":"http","label":"L","url":"https://x","method":"PUT","headers":{{"A":"b","N":1}},"body":"hi"}}]}}"#
        );
        let mut headers = BTreeMap::new();
        headers.insert("A".to_string(), "b".to_string());
        assert_eq!(
            msg(&l).actions,
            vec![Action::Http {
                label: "L".into(),
                url: "https://x".into(),
                method: "PUT".into(),
                headers,
                body: Some("hi".into())
            }]
        );
    }

    #[test]
    fn action_missing_url_is_dropped_message_still_parses() {
        let l = format!(
            r#"{{{BASE},"message":"hi","actions":[{{"action":"view","label":"x"}},{{"action":"http","label":"y"}},{{"action":"view","label":"ok","url":"https://x"}},"junk"]}}"#
        );
        let m = msg(&l);
        assert_eq!(m.message.as_deref(), Some("hi"));
        assert_eq!(
            m.actions,
            vec![Action::View {
                label: "ok".into(),
                url: "https://x".into()
            }]
        );
    }

    #[test]
    fn attachment_without_url_is_dropped() {
        let l = format!(r#"{{{BASE},"attachment":{{"name":"a.png"}}}}"#);
        assert_eq!(msg(&l).attachment, None);
    }

    #[test]
    fn attachment_without_name_falls_back_to_url() {
        let l = format!(r#"{{{BASE},"attachment":{{"url":"https://x/dir/f.png?a=1"}}}}"#);
        let a = msg(&l).attachment.unwrap();
        assert_eq!(a.name, "f.png");
        assert!(a.is_image());
    }

    #[test]
    fn malformed_optional_fields_degrade() {
        let l = format!(
            r#"{{{BASE},"tags":"nope","title":5,"message":{{}},"click":[],"icon":1,"actions":"x","attachment":7}}"#
        );
        let m = msg(&l);
        assert!(m.tags.is_empty());
        assert_eq!(m.title, None);
        assert_eq!(m.message, None);
        assert_eq!(m.click, None);
        assert_eq!(m.icon, None);
        assert!(m.actions.is_empty());
        assert_eq!(m.attachment, None);
    }

    #[test]
    fn non_string_tags_are_skipped() {
        let l = format!(r#"{{{BASE},"tags":["a",1,null,"b"]}}"#);
        assert_eq!(msg(&l).tags, vec!["a", "b"]);
    }

    #[test]
    fn is_image_by_mime_or_extension() {
        let a = |name: &str, mime: Option<&str>| Attachment {
            name: name.into(),
            mime: mime.map(Into::into),
            size: None,
            url: "https://x/f".into(),
        };
        assert!(a("f", Some("image/webp")).is_image());
        assert!(a("F.JPEG", None).is_image());
        assert!(!a("f.pdf", Some("application/pdf")).is_image());
        assert!(!a("f.png.exe", None).is_image());
        assert!(!a("f", None).is_image());
    }

    #[test]
    fn unknown_fields_ignored() {
        assert_eq!(msg(&format!(r#"{{{BASE},"foo":1}}"#)).topic, "t");
    }

    #[test]
    fn message_without_topic_is_err() {
        assert!(parse_event(r#"{"id":"x","time":5,"event":"message"}"#).is_err());
    }

    #[test]
    fn missing_required_fields_is_err() {
        assert!(parse_event(r#"{"id":"x","time":5}"#).is_err());
        assert!(parse_event(r#"{"id":"x","event":"open"}"#).is_err());
        assert!(parse_event(r#"{"time":5,"event":"open"}"#).is_err());
    }

    #[test]
    fn invalid_json_is_err() {
        assert!(parse_event("garbage").is_err());
        assert!(parse_event("[1,2]").is_err());
        assert!(parse_event("").is_err());
    }
}
