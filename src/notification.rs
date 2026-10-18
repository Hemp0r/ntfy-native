use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Passive,
    Active,
    Urgent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaRef {
    Remote(String),
    Local(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ActivationPayload {
    Open {
        url: String,
    },
    Http {
        label: String,
        method: String,
        url: String,
        headers: BTreeMap<String, String>,
        body: Option<String>,
    },
}

impl ActivationPayload {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("payload serializes")
    }

    pub fn decode(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub payload: ActivationPayload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub thread: String,
    pub title: String,
    pub body: String,
    pub level: Level,
    pub default_action: Option<ActivationPayload>,
    pub buttons: Vec<Button>,
    pub image: Option<MediaRef>,
    pub icon: Option<MediaRef>,
}

pub trait Notifier: Send + Sync {
    fn show(&self, n: &Notification) -> anyhow::Result<()>;
}

pub type ActivationSink = Arc<dyn Fn(ActivationPayload) + Send + Sync>;

pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn show(&self, n: &Notification) -> anyhow::Result<()> {
        log_notification(n);
        Ok(())
    }
}

/// Message content (title, body) only at debug: never bodies at info.
fn log_notification(n: &Notification) {
    tracing::info!(id = %n.id, "notification");
    tracing::debug!(id = %n.id, title = %n.title, body = %n.body, "notification content");
}

#[derive(Default)]
pub struct RecordingNotifier {
    pub shown: Mutex<Vec<Notification>>,
}

impl Notifier for RecordingNotifier {
    fn show(&self, n: &Notification) -> anyhow::Result<()> {
        self.shown.lock().expect("lock").push(n.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_roundtrip() {
        let p = ActivationPayload::Http {
            label: "Go".into(),
            method: "POST".into(),
            url: "https://x/y".into(),
            headers: BTreeMap::from([("A".into(), "b".into())]),
            body: Some("hi".into()),
        };
        assert_eq!(ActivationPayload::decode(&p.encode()), Some(p));
        let o = ActivationPayload::Open {
            url: "https://a".into(),
        };
        assert_eq!(ActivationPayload::decode(&o.encode()), Some(o));
        assert_eq!(ActivationPayload::decode("x"), None);
    }

    #[test]
    fn recording_notifier_records() {
        let r = RecordingNotifier::default();
        let n = Notification {
            id: "i".into(),
            thread: "t".into(),
            title: "T".into(),
            body: "B".into(),
            level: Level::Active,
            default_action: None,
            buttons: vec![],
            image: None,
            icon: None,
        };
        r.show(&n).unwrap();
        assert_eq!(*r.shown.lock().unwrap(), vec![n]);
    }
}
