use std::collections::BTreeMap;

use crate::connection::ConnStatus;
use crate::settings::Settings;
use crate::snapshot::InstanceStatus;

pub const PROBLEM_GRACE_SECS: i64 = 15;

#[derive(Clone, Debug, PartialEq)]
pub enum TrayState {
    Ok,
    Problem { instance: String },
    AuthFailed { instance: String },
    Paused,
    NotConfigured,
}

/// `now` is local Unix time. Raw error text from `ConnStatus` never leaves this function.
pub fn aggregate(
    settings: &Settings,
    statuses: &BTreeMap<String, (ConnStatus, i64)>,
    paused: bool,
    now: i64,
) -> TrayState {
    let active: Vec<_> = settings
        .instances
        .iter()
        .filter(|i| !i.topics.is_empty())
        .collect();
    if active.is_empty() {
        return TrayState::NotConfigured;
    }
    let connecting = (ConnStatus::Connecting, now);
    let lookup = |id: &str| statuses.get(id).unwrap_or(&connecting);

    if let Some(i) = active
        .iter()
        .find(|i| matches!(lookup(&i.id).0, ConnStatus::AuthFailed))
    {
        return TrayState::AuthFailed { instance: i.host() };
    }
    if let Some(i) = active.iter().find(|i| {
        let (status, since) = lookup(&i.id);
        !matches!(status, ConnStatus::Connected) && now - since >= PROBLEM_GRACE_SECS
    }) {
        return TrayState::Problem { instance: i.host() };
    }
    if paused {
        TrayState::Paused
    } else {
        TrayState::Ok
    }
}

/// Same grace period as `aggregate`; raw error text never leaves this function.
pub fn instance_status(status: Option<&(ConnStatus, i64)>, now: i64) -> InstanceStatus {
    match status {
        None => InstanceStatus::Connecting,
        Some((ConnStatus::AuthFailed, _)) => InstanceStatus::AuthFailed,
        Some((ConnStatus::Connected, _)) => InstanceStatus::Connected,
        Some((_, since)) if now - since >= PROBLEM_GRACE_SECS => InstanceStatus::Problem,
        Some(_) => InstanceStatus::Connecting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Instance, Topic};

    const NOW: i64 = 1_000_000;

    fn settings() -> Settings {
        Settings {
            instances: vec![Instance {
                id: "a".into(),
                url: "https://ntfy.example.org".into(),
                topics: vec![Topic {
                    name: "t".into(),
                    ..Topic::default()
                }],
                ..Instance::default()
            }],
            ..Settings::default()
        }
    }

    fn st(s: ConnStatus, since: i64) -> BTreeMap<String, (ConnStatus, i64)> {
        BTreeMap::from([("a".to_string(), (s, since))])
    }

    #[test]
    fn aggregate_not_configured() {
        let none = Settings::default();
        assert_eq!(
            aggregate(&none, &BTreeMap::new(), false, NOW),
            TrayState::NotConfigured
        );
        let mut empty_topics = settings();
        empty_topics.instances[0].topics.clear();
        assert_eq!(
            aggregate(&empty_topics, &BTreeMap::new(), false, NOW),
            TrayState::NotConfigured
        );
    }

    #[test]
    fn aggregate_grace_period() {
        let s = settings();
        assert_eq!(
            aggregate(&s, &st(ConnStatus::Connecting, NOW - 10), false, NOW),
            TrayState::Ok
        );
        assert_eq!(
            aggregate(&s, &st(ConnStatus::Connecting, NOW - 20), false, NOW),
            TrayState::Problem {
                instance: "ntfy.example.org".into()
            }
        );
        assert_eq!(aggregate(&s, &BTreeMap::new(), false, NOW), TrayState::Ok);
    }

    #[test]
    fn aggregate_disconnected_hides_error_text() {
        let s = settings();
        let t = aggregate(
            &s,
            &st(
                ConnStatus::Disconnected {
                    error: "connection refused".into(),
                },
                NOW - 30,
            ),
            false,
            NOW,
        );
        assert_eq!(
            t,
            TrayState::Problem {
                instance: "ntfy.example.org".into()
            }
        );
        assert!(!format!("{t:?}").contains("refused"));
    }

    #[test]
    fn aggregate_auth_failed_immediate_and_precedence() {
        let s = settings();
        assert_eq!(
            aggregate(&s, &st(ConnStatus::AuthFailed, NOW), true, NOW),
            TrayState::AuthFailed {
                instance: "ntfy.example.org".into()
            }
        );
    }

    #[test]
    fn aggregate_paused() {
        let s = settings();
        assert_eq!(
            aggregate(&s, &st(ConnStatus::Connected, NOW - 100), true, NOW),
            TrayState::Paused
        );
        assert_eq!(
            aggregate(&s, &st(ConnStatus::Connected, NOW - 100), false, NOW),
            TrayState::Ok
        );
    }

    #[test]
    fn aggregate_problem_beats_paused() {
        let s = settings();
        assert!(matches!(
            aggregate(&s, &st(ConnStatus::Connecting, NOW - 60), true, NOW),
            TrayState::Problem { .. }
        ));
    }

    #[test]
    fn instance_status_mapping() {
        let c = |s: ConnStatus, since: i64| instance_status(Some(&(s, since)), NOW);
        assert_eq!(
            c(ConnStatus::Connecting, NOW - 10),
            InstanceStatus::Connecting
        );
        assert_eq!(c(ConnStatus::Connecting, NOW - 20), InstanceStatus::Problem);
        let d = || ConnStatus::Disconnected { error: "x".into() };
        assert_eq!(c(d(), NOW - 5), InstanceStatus::Connecting);
        assert_eq!(c(d(), NOW - 30), InstanceStatus::Problem);
        assert_eq!(c(ConnStatus::AuthFailed, NOW), InstanceStatus::AuthFailed);
        assert_eq!(
            c(ConnStatus::Connected, NOW - 100),
            InstanceStatus::Connected
        );
        assert_eq!(instance_status(None, NOW), InstanceStatus::Connecting);
    }
}
