//! Against a real ntfy server; runs only when NTFY_E2E_URL is set:
//! `scripts/ntfy-test-server.sh start && NTFY_E2E_URL=http://127.0.0.1:18080 cargo test --test e2e_ntfy -- --test-threads=1`
//! The gap test restarts that server through the script.

mod support;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ntfy_native::connection::{ConnEvent, ConnectionConfig};
use ntfy_native::ntfy::client::{http_client, poll};
use ntfy_native::state::InstanceState;
use support::conn::{Conn, is_connected, last_state, message_ids};

fn e2e_url() -> Option<String> {
    std::env::var("NTFY_E2E_URL").ok()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn unique_topic() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("e2e-topic-{nanos}")
}

fn config(url: &str, topics: &[&str]) -> ConnectionConfig {
    ConnectionConfig {
        instance_id: "e2e".into(),
        base_url: url.into(),
        topics: topics.iter().map(|t| t.to_string()).collect(),
        auth: None,
        cache_duration: 43200,
        keepalive_timeout: Duration::from_secs(15),
        backoff_initial: Duration::from_millis(100),
        backoff_max: Duration::from_secs(1),
        auth_retry: Duration::from_secs(1),
    }
}

fn start(cfg: ConnectionConfig, state: InstanceState) -> Conn {
    let mut conn = Conn::start(cfg, state);
    conn.patience = Duration::from_secs(10);
    conn
}

/// Publishes `text` and returns the message id.
async fn publish(url: &str, topic: &str, text: &str) -> String {
    let resp = http_client()
        .post(format!("{url}/{topic}"))
        .body(text.to_owned())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    v["id"].as_str().unwrap().to_owned()
}

/// Receives one live message and returns the state saved after it.
async fn receive_one(url: &str, topic: &str) -> (String, InstanceState) {
    let mut conn = start(config(url, &[topic]), InstanceState::default());
    conn.until(is_connected).await;
    let id = publish(url, topic, "first").await;
    let events = conn
        .collect_until(|e| matches!(e, ConnEvent::State(s) if s.seen_ids.contains(&id)))
        .await;
    assert!(events.iter().any(|e| message_ids(e) == [id.clone()]));
    conn.stop().await;
    (id, last_state(&events).unwrap())
}

#[tokio::test]
async fn e2e_poll_since_across_topics_and_date_header() {
    let Some(url) = e2e_url() else { return };
    let a = unique_topic();
    let b = format!("{a}-b");
    let before = unix_now();
    let ia = publish(&url, &a, "x").await;
    let ib = publish(&url, &b, "y").await;

    let topics = [a.clone(), b.clone()];
    let res = poll(&http_client(), &url, &topics, before - 1, None)
        .await
        .unwrap();
    let ids: BTreeSet<_> = res.messages.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, BTreeSet::from([ia, ib]));
    let server_now = res.server_now.expect("Date header");
    assert!((server_now - unix_now()).abs() < 60);
    assert!(!res.truncated);

    let later = poll(&http_client(), &url, &topics, server_now + 5, None)
        .await
        .unwrap();
    assert!(later.messages.is_empty());
}

#[tokio::test]
async fn e2e_live_and_catch_up() {
    let Some(url) = e2e_url() else { return };
    let t1 = unique_topic();
    let t2 = format!("{t1}-b");
    let cfg = config(&url, &[&t1, &t2]);

    let mut conn = start(cfg.clone(), InstanceState::default());
    conn.until(is_connected).await;
    let a = publish(&url, &t1, "A").await;
    match conn
        .until(|e| matches!(e, ConnEvent::Messages { .. }))
        .await
    {
        ConnEvent::Messages { messages, catch_up } => {
            assert_eq!(messages[0].id, a);
            assert!(!catch_up);
        }
        other => panic!("expected Messages, got {other:?}"),
    }
    let state = match conn.next().await {
        ConnEvent::State(s) => s,
        other => panic!("expected State, got {other:?}"),
    };
    conn.stop().await;

    let b = publish(&url, &t1, "B").await;
    let c = publish(&url, &t2, "C").await;

    let mut conn = start(cfg, state);
    let events = conn.collect_until(is_connected).await;
    assert!(!events.contains(&ConnEvent::Gap), "{events:?}");
    let catch_up: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ConnEvent::Messages { messages, catch_up } => Some((messages, *catch_up)),
            _ => None,
        })
        .collect();
    assert_eq!(catch_up.len(), 1, "{events:?}");
    assert!(catch_up[0].1);
    let ids: BTreeSet<_> = catch_up[0].0.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids, BTreeSet::from([b, c]));

    // The stream starts inside the overlap margin and replays A, B and C.
    let rest = conn.collect_for(Duration::from_secs(2)).await;
    assert!(
        !rest.iter().any(|e| matches!(e, ConnEvent::Messages { .. })),
        "{rest:?}"
    );
    conn.stop().await;
}

/// Restores the cached server even if the test fails.
struct RestartWithCache;

impl Drop for RestartWithCache {
    fn drop(&mut self) {
        test_server(&["restart"]);
    }
}

fn test_server(args: &[&str]) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ntfy-test-server.sh");
    let status = Command::new(script).args(args).status().unwrap();
    assert!(status.success(), "ntfy-test-server.sh {args:?} failed");
}

#[tokio::test]
async fn e2e_gap_after_restart_without_cache() {
    let Some(url) = e2e_url() else { return };
    let topic = unique_topic();
    let (_, state) = receive_one(&url, &topic).await;

    let _restore = RestartWithCache;
    test_server(&["restart", "--no-cache"]);

    let mut conn = start(config(&url, &[&topic]), state);
    let events = conn.collect_until(is_connected).await;
    assert_eq!(
        events.iter().filter(|e| **e == ConnEvent::Gap).count(),
        1,
        "{events:?}"
    );
    conn.stop().await;
}

#[tokio::test]
async fn connection_test_against_real_server() {
    let Some(url) = e2e_url() else { return };
    let http = http_client();
    assert!(
        ntfy_native::ntfy::client::health(&http, &url)
            .await
            .unwrap()
    );
    let topic = unique_topic();
    assert!(
        ntfy_native::ntfy::client::topic_readable(&http, &url, &topic, None)
            .await
            .unwrap()
    );
    let draft = ntfy_native::instance_edit::InstanceDraft {
        id: None,
        url: url.clone(),
        auth: ntfy_native::instance_edit::AuthDraft::None,
        cache_duration_secs: 43200,
        topics: vec![ntfy_native::settings::Topic {
            name: topic.clone(),
            ..Default::default()
        }],
    };
    assert_eq!(
        ntfy_native::conntest::test_connection(&http, &draft, None).await,
        ntfy_native::ipc::TestOutcome::Ok {
            topics: vec![ntfy_native::ipc::TopicAccess {
                name: topic,
                readable: true
            }]
        }
    );
}
