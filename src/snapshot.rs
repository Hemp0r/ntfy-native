use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::autostart::AutostartState;
use crate::connection::ConnStatus;
use crate::instance_edit::{AuthDraft, InstanceDraft};
use crate::settings::{AuthKind, Instance, Settings, Topic};
use crate::state::{GapFlag, RecentMessage};
use crate::status::instance_status;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub app_version: String,
    pub configured: bool,
    pub instances: Vec<InstanceView>,
    pub recent: Vec<RecentView>,
    pub gaps: Vec<GapView>,
    pub paused_until: Option<i64>,
    pub urgent_breaks_dnd: bool,
    pub autostart: AutostartState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceView {
    pub id: String,
    pub host: String,
    pub url: String,
    pub status: InstanceStatus,
    pub auth: AuthView,
    pub cache_duration_secs: i64,
    pub topics: Vec<Topic>,
}

/// Never carries a secret, only whether one is expected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AuthView {
    None,
    Token,
    Basic { username: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceStatus {
    Connecting,
    Connected,
    Problem,
    AuthFailed,
}

/// The click URL is deliberately not exposed, only whether one exists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentView {
    pub instance_id: String,
    pub host: String,
    pub topic: String,
    pub message_id: String,
    pub time: i64,
    pub title: String,
    pub body: String,
    pub has_click: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GapView {
    pub instance_id: String,
    pub host: String,
    pub topic: String,
    pub detected_at: i64,
}

fn host_of(settings: &Settings, instance_id: &str) -> String {
    settings
        .instance(instance_id)
        .map(Instance::host)
        .unwrap_or_else(|| instance_id.to_owned())
}

pub fn build_snapshot(
    settings: &Settings,
    statuses: &BTreeMap<String, (ConnStatus, i64)>,
    recent: &VecDeque<RecentMessage>,
    gaps: &[GapFlag],
    autostart: AutostartState,
    now: i64,
) -> UiSnapshot {
    UiSnapshot {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        configured: settings.instances.iter().any(|i| !i.topics.is_empty()),
        instances: settings
            .instances
            .iter()
            .map(|i| InstanceView {
                id: i.id.clone(),
                host: i.host(),
                url: i.url.clone(),
                status: instance_status(statuses.get(&i.id), now),
                auth: match &i.auth {
                    AuthKind::None => AuthView::None,
                    AuthKind::Token => AuthView::Token,
                    AuthKind::Basic { username } => AuthView::Basic {
                        username: username.clone(),
                    },
                },
                cache_duration_secs: i.cache_duration_secs,
                topics: i.topics.clone(),
            })
            .collect(),
        recent: recent
            .iter()
            .map(|m| RecentView {
                instance_id: m.instance_id.clone(),
                host: host_of(settings, &m.instance_id),
                topic: m.topic.clone(),
                message_id: m.id.clone(),
                time: m.time,
                title: m.title.clone(),
                body: m.body.clone(),
                has_click: m.click.is_some(),
            })
            .collect(),
        gaps: gaps
            .iter()
            .map(|g| GapView {
                instance_id: g.instance_id.clone(),
                host: host_of(settings, &g.instance_id),
                topic: g.topic.clone(),
                detected_at: g.detected_at,
            })
            .collect(),
        paused_until: settings.paused_until,
        urgent_breaks_dnd: settings.urgent_breaks_dnd,
        autostart,
    }
}

/// Starting point for editing an existing instance; secrets stay unchanged (`None`).
pub fn draft_from_view(v: &InstanceView) -> InstanceDraft {
    InstanceDraft {
        id: Some(v.id.clone()),
        url: v.url.clone(),
        auth: match &v.auth {
            AuthView::None => AuthDraft::None,
            AuthView::Token => AuthDraft::Token { token: None },
            AuthView::Basic { username } => AuthDraft::Basic {
                username: username.clone(),
                password: None,
            },
        },
        cache_duration_secs: v.cache_duration_secs,
        topics: v.topics.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{MemoryStore, SecretStore, account_for};

    const NOW: i64 = 1_000_000;

    fn settings(auth: AuthKind) -> Settings {
        Settings {
            instances: vec![Instance {
                id: "a".into(),
                url: "https://ntfy.example.org".into(),
                auth,
                topics: vec![Topic {
                    name: "t".into(),
                    ..Topic::default()
                }],
                ..Instance::default()
            }],
            ..Settings::default()
        }
    }

    fn snap(s: &Settings, st: Option<(ConnStatus, i64)>) -> UiSnapshot {
        let statuses = st.map(|v| ("a".to_string(), v)).into_iter().collect();
        build_snapshot(
            s,
            &statuses,
            &VecDeque::new(),
            &[],
            AutostartState::Disabled,
            NOW,
        )
    }

    #[test]
    fn snapshot_never_contains_secrets() {
        let store = MemoryStore::new();
        store.set(&account_for("a"), "s3cret").unwrap();
        let s = settings(AuthKind::Token);
        let snapshot = snap(&s, Some((ConnStatus::Connected, NOW)));
        assert_eq!(snapshot.instances[0].auth, AuthView::Token);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("s3cret"), "{json}");
        let s = settings(AuthKind::Basic {
            username: "alice".into(),
        });
        let snapshot = snap(&s, None);
        assert_eq!(
            snapshot.instances[0].auth,
            AuthView::Basic {
                username: "alice".into()
            }
        );
    }

    #[test]
    fn snapshot_status_uses_grace() {
        let s = settings(AuthKind::None);
        let status = |st| snap(&s, Some(st)).instances[0].status;
        assert_eq!(
            status((ConnStatus::Connecting, NOW - 10)),
            InstanceStatus::Connecting
        );
        assert_eq!(
            status((ConnStatus::Connecting, NOW - 20)),
            InstanceStatus::Problem
        );
        assert_eq!(
            status((ConnStatus::AuthFailed, NOW)),
            InstanceStatus::AuthFailed
        );
        assert_eq!(
            status((ConnStatus::Connected, NOW - 100)),
            InstanceStatus::Connected
        );
        assert_eq!(
            snap(&s, None).instances[0].status,
            InstanceStatus::Connecting
        );
    }

    #[test]
    fn snapshot_recent_has_click_flag_not_url() {
        let s = settings(AuthKind::None);
        let recent = VecDeque::from([
            RecentMessage {
                instance_id: "a".into(),
                topic: "t".into(),
                id: "m1".into(),
                time: 5,
                title: "T".into(),
                body: "B".into(),
                click: Some("https://secret.example/click?x=1".into()),
            },
            RecentMessage {
                instance_id: "gone".into(),
                id: "m2".into(),
                ..RecentMessage::default()
            },
        ]);
        let gaps = [GapFlag {
            instance_id: "a".into(),
            topic: "t".into(),
            detected_at: 9,
        }];
        let snapshot = build_snapshot(
            &s,
            &BTreeMap::new(),
            &recent,
            &gaps,
            AutostartState::Disabled,
            NOW,
        );
        assert!(snapshot.recent[0].has_click && !snapshot.recent[1].has_click);
        assert_eq!(snapshot.recent[0].host, "ntfy.example.org");
        assert_eq!(snapshot.recent[0].message_id, "m1");
        assert_eq!(snapshot.recent[1].host, "gone");
        assert_eq!(snapshot.gaps[0].host, "ntfy.example.org");
        assert_eq!(snapshot.gaps[0].detected_at, 9);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("secret.example"), "{json}");
    }

    #[test]
    fn snapshot_hides_disconnect_error_text() {
        let s = settings(AuthKind::None);
        let snapshot = snap(
            &s,
            Some((
                ConnStatus::Disconnected {
                    error: "connection refused".into(),
                },
                NOW - 30,
            )),
        );
        assert_eq!(snapshot.instances[0].status, InstanceStatus::Problem);
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("refused")
        );
    }

    #[test]
    fn snapshot_copies_global_fields() {
        let mut s = settings(AuthKind::None);
        s.paused_until = Some(42);
        s.urgent_breaks_dnd = true;
        let snapshot = snap(&s, None);
        assert!(snapshot.configured && snapshot.urgent_breaks_dnd);
        assert_eq!(snapshot.paused_until, Some(42));
        assert!(
            !build_snapshot(
                &Settings::default(),
                &BTreeMap::new(),
                &VecDeque::new(),
                &[],
                AutostartState::Disabled,
                NOW
            )
            .configured
        );
    }

    #[test]
    fn draft_from_view_keeps_secrets() {
        let view = |auth| InstanceView {
            id: "a".into(),
            host: "h".into(),
            url: "https://h".into(),
            status: InstanceStatus::Connected,
            auth,
            cache_duration_secs: 3600,
            topics: vec![Topic {
                name: "t".into(),
                muted: true,
                urgent_breaks_dnd: None,
            }],
        };
        let d = draft_from_view(&view(AuthView::Token));
        assert_eq!(d.id.as_deref(), Some("a"));
        assert_eq!(d.url, "https://h");
        assert_eq!(d.cache_duration_secs, 3600);
        assert!(d.topics[0].muted);
        assert_eq!(d.auth, AuthDraft::Token { token: None });
        assert_eq!(draft_from_view(&view(AuthView::None)).auth, AuthDraft::None);
        assert_eq!(
            draft_from_view(&view(AuthView::Basic {
                username: "u".into()
            }))
            .auth,
            AuthDraft::Basic {
                username: "u".into(),
                password: None
            }
        );
    }
}
