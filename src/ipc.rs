use std::io::{self, BufRead, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::instance_edit::InstanceDraft;
use crate::snapshot::UiSnapshot;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_LINE: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum CoreMsg {
    Hello {
        version: u32,
    },
    Snapshot(UiSnapshot),
    TestResult {
        request_id: u64,
        outcome: TestOutcome,
    },
    SaveResult {
        request_id: u64,
        result: Result<String, String>,
    },
    Close,
    /// Bring the open window to the front (it was requested again).
    Focus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum UiMsg {
    Hello { version: u32 },
    Request(UiRequest),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum UiRequest {
    PauseAll {
        until: Option<i64>,
    },
    SetTopicMuted {
        instance_id: String,
        topic: String,
        muted: bool,
    },
    SetTopicUrgent {
        instance_id: String,
        topic: String,
        urgent: Option<bool>,
    },
    SetUrgentGlobal {
        enabled: bool,
    },
    SetAutostart {
        enabled: bool,
    },
    DismissGap {
        instance_id: String,
        topic: String,
    },
    OpenRecent {
        instance_id: String,
        message_id: String,
    },
    SendTestNotification,
    TestConnection {
        request_id: u64,
        draft: InstanceDraft,
    },
    SaveInstance {
        request_id: u64,
        draft: InstanceDraft,
    },
    RemoveInstance {
        instance_id: String,
    },
    OpenConfigDir,
    OpenLogs,
    OpenSettings,
    OpenWizard,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum TestOutcome {
    Ok { topics: Vec<TopicAccess> },
    InvalidInput { message: String },
    Unreachable { message: String },
    NotNtfy,
    AuthFailed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopicAccess {
    pub name: String,
    pub readable: bool,
}

/// One JSON object per line, flushed.
pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// `None` on EOF. Lines over `MAX_LINE` bytes or undecodable lines are `InvalidData`.
pub fn read_msg<T: DeserializeOwned>(r: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut buf = Vec::new();
    let n = r.take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    if buf.len() > MAX_LINE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ipc line too long",
        ));
    }
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "malformed ipc message"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_edit::AuthDraft;
    use crate::settings::Topic;
    use crate::snapshot::{AuthView, InstanceStatus, InstanceView};

    fn draft() -> InstanceDraft {
        InstanceDraft {
            id: Some("a".into()),
            url: "https://ntfy.example.org".into(),
            auth: AuthDraft::Basic {
                username: "u".into(),
                password: Some("p".into()),
            },
            cache_duration_secs: 43200,
            topics: vec![Topic {
                name: "t".into(),
                muted: true,
                urgent_breaks_dnd: Some(false),
            }],
        }
    }

    fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(m: T) {
        let mut out = Vec::new();
        write_msg(&mut out, &m).unwrap();
        assert_eq!(out.iter().filter(|b| **b == b'\n').count(), 1);
        assert_eq!(out.last(), Some(&b'\n'));
        let back: T = read_msg(&mut out.as_slice()).unwrap().unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn messages_roundtrip_as_single_lines() {
        let snapshot = UiSnapshot {
            app_version: "1.0".into(),
            configured: true,
            instances: vec![InstanceView {
                id: "a".into(),
                host: "h".into(),
                url: "https://h".into(),
                status: InstanceStatus::Problem,
                auth: AuthView::Basic {
                    username: "u\nv".into(),
                },
                cache_duration_secs: 3600,
                topics: vec![Topic::default()],
            }],
            recent: vec![],
            gaps: vec![],
            paused_until: Some(i64::MAX),
            urgent_breaks_dnd: true,
            autostart: false,
        };
        let core = vec![
            CoreMsg::Hello {
                version: PROTOCOL_VERSION,
            },
            CoreMsg::Snapshot(snapshot),
            CoreMsg::TestResult {
                request_id: 1,
                outcome: TestOutcome::Ok {
                    topics: vec![TopicAccess {
                        name: "t".into(),
                        readable: false,
                    }],
                },
            },
            CoreMsg::TestResult {
                request_id: 2,
                outcome: TestOutcome::InvalidInput {
                    message: "m".into(),
                },
            },
            CoreMsg::TestResult {
                request_id: 3,
                outcome: TestOutcome::Unreachable {
                    message: "m".into(),
                },
            },
            CoreMsg::TestResult {
                request_id: 4,
                outcome: TestOutcome::NotNtfy,
            },
            CoreMsg::TestResult {
                request_id: 5,
                outcome: TestOutcome::AuthFailed,
            },
            CoreMsg::SaveResult {
                request_id: 6,
                result: Ok("a".into()),
            },
            CoreMsg::SaveResult {
                request_id: 7,
                result: Err("bad".into()),
            },
            CoreMsg::Close,
            CoreMsg::Focus,
        ];
        for m in core {
            roundtrip(m);
        }
        let s = |a: &str| a.to_string();
        let requests = vec![
            UiRequest::PauseAll { until: None },
            UiRequest::PauseAll { until: Some(5) },
            UiRequest::SetTopicMuted {
                instance_id: s("a"),
                topic: s("t"),
                muted: true,
            },
            UiRequest::SetTopicUrgent {
                instance_id: s("a"),
                topic: s("t"),
                urgent: Some(true),
            },
            UiRequest::SetUrgentGlobal { enabled: true },
            UiRequest::SetAutostart { enabled: false },
            UiRequest::DismissGap {
                instance_id: s("a"),
                topic: s("t"),
            },
            UiRequest::OpenRecent {
                instance_id: s("a"),
                message_id: s("m"),
            },
            UiRequest::SendTestNotification,
            UiRequest::TestConnection {
                request_id: 1,
                draft: draft(),
            },
            UiRequest::SaveInstance {
                request_id: 2,
                draft: draft(),
            },
            UiRequest::RemoveInstance {
                instance_id: s("a"),
            },
            UiRequest::OpenConfigDir,
            UiRequest::OpenLogs,
            UiRequest::OpenSettings,
            UiRequest::OpenWizard,
            UiRequest::Quit,
        ];
        roundtrip(UiMsg::Hello {
            version: PROTOCOL_VERSION,
        });
        for r in requests {
            roundtrip(UiMsg::Request(r));
        }
    }

    #[test]
    fn eof_is_none_and_garbage_is_invalid_data() {
        let none: Option<UiMsg> = read_msg(&mut &b""[..]).unwrap();
        assert!(none.is_none());
        for bad in [
            &b"not json\n"[..],
            b"{\"t\":\"nope\"}\n",
            b"{\"t\":\"hello\"",
        ] {
            let e = read_msg::<UiMsg>(&mut &bad[..]).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn oversized_line_rejected() {
        let mut big = vec![b'x'; MAX_LINE + 10];
        big.push(b'\n');
        let e = read_msg::<UiMsg>(&mut big.as_slice()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        // no newline at all
        let e = read_msg::<UiMsg>(&mut vec![b'x'; MAX_LINE + 10].as_slice()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn two_messages_read_in_order() {
        let mut out = Vec::new();
        write_msg(&mut out, &CoreMsg::Close).unwrap();
        write_msg(&mut out, &CoreMsg::Hello { version: 1 }).unwrap();
        let mut r = out.as_slice();
        assert_eq!(read_msg::<CoreMsg>(&mut r).unwrap(), Some(CoreMsg::Close));
        assert_eq!(
            read_msg::<CoreMsg>(&mut r).unwrap(),
            Some(CoreMsg::Hello { version: 1 })
        );
        assert_eq!(read_msg::<CoreMsg>(&mut r).unwrap(), None);
    }
}
