mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ntfy_native::connection::{ConnEvent, ConnStatus, ConnectionConfig};
use ntfy_native::secrets::Auth;
use ntfy_native::state::{InstanceState, SeenMessage};
use support::conn::{Conn, is_connected, is_disconnected, last_state, message_ids};
use support::{FakeServer, Reply, Script, unused_url};

const INSTANCE: &str = "inst-1";

fn config(base_url: &str, auth: Option<Auth>) -> ConnectionConfig {
    ConnectionConfig {
        instance_id: INSTANCE.into(),
        base_url: base_url.into(),
        topics: vec!["alerts".into(), "backup".into()],
        auth,
        cache_duration: 43200,
        keepalive_timeout: Duration::from_millis(300),
        backoff_initial: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        auth_retry: Duration::from_secs(1),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn http_date(unix: i64) -> String {
    httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(unix as u64))
}

fn msg(id: &str, time: i64) -> String {
    format!(
        r#"{{"id":"{id}","time":{time},"event":"message","topic":"alerts","message":"body {id}","expires":{}}}"#,
        time + 43200
    )
}

fn keepalive(time: i64) -> String {
    format!(r#"{{"id":"k{time}","time":{time},"event":"keepalive","topic":"alerts,backup"}}"#)
}

fn open(time: i64) -> String {
    format!(r#"{{"id":"o{time}","time":{time},"event":"open","topic":"alerts,backup"}}"#)
}

/// State with one delivered message `id` at `time`.
fn history(id: &str, time: i64, expires: Option<i64>) -> InstanceState {
    InstanceState {
        watermark: Some(time),
        last_message: Some(SeenMessage {
            id: id.into(),
            time,
            expires,
        }),
        seen_ids: [id.to_owned()].into(),
    }
}

#[tokio::test]
async fn first_connect_without_history_streams_without_since() {
    let script = Script::default();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    assert_eq!(conn.next().await, ConnEvent::Status(ConnStatus::Connecting));
    assert!(is_connected(&conn.next().await));
    let req = &server.requests()[0];
    assert_eq!(req.path, "/alerts,backup/json");
    assert!(!req.is_poll());
    assert_eq!(req.query_param("since"), None);

    let t = now();
    stream.line(&open(t));
    stream.line(&msg("m1", t));
    match conn.next().await {
        ConnEvent::Messages { messages, catch_up } => {
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].id, "m1");
            assert!(!catch_up);
        }
        other => panic!("expected Messages, got {other:?}"),
    }
    match conn.next().await {
        ConnEvent::State(st) => {
            assert_eq!(st.watermark, Some(t));
            assert_eq!(st.last_message.unwrap().id, "m1");
        }
        other => panic!("expected State, got {other:?}"),
    }
}

#[tokio::test]
async fn keepalive_timeout_triggers_catch_up_poll() {
    let script = Script::default();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    script.push(Reply::lines::<&str>(&[]));
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    conn.until(is_connected).await;
    let t = now() - 5;
    stream.line(&msg("m1", t));
    conn.until(|e| message_ids(e) == ["m1"]).await;
    // `stream` stays alive: the connection hangs silently.
    let started = tokio::time::Instant::now();
    conn.until(is_disconnected).await;
    assert!(started.elapsed() >= Duration::from_millis(250));

    let reqs = server.wait_requests(2).await;
    assert!(reqs[1].is_poll());
    assert_eq!(
        reqs[1].query_param("since"),
        Some((t - 30).to_string().as_str())
    );
    drop(stream);
}

#[tokio::test]
async fn stream_end_reconnects_with_catch_up() {
    let script = Script::default();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    conn.until(is_connected).await;
    let t = now();
    stream.line(&msg("m1", t));
    conn.until(|e| message_ids(e) == ["m1"]).await;
    drop(stream);
    conn.until(is_disconnected).await;
    let reqs = server.wait_requests(2).await;
    assert!(reqs[1].is_poll());
    assert_eq!(
        reqs[1].query_param("since"),
        Some((t - 30).to_string().as_str())
    );
}

#[tokio::test]
async fn duplicate_between_poll_and_stream_delivered_once() {
    let t = now();
    let script = Script::default();
    script.push(Reply::lines(&[msg("x", t - 50)]).header("Date", &http_date(t)));
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), history("old", t - 100, None));

    let events = conn.collect_until(is_connected).await;
    assert!(events.contains(&ConnEvent::Status(ConnStatus::Connecting)));
    let catch_up: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ConnEvent::Messages { messages, catch_up } => Some((messages.clone(), *catch_up)),
            _ => None,
        })
        .collect();
    assert_eq!(catch_up.len(), 1);
    assert_eq!(catch_up[0].0[0].id, "x");
    assert!(catch_up[0].1);

    let reqs = server.requests();
    assert!(reqs[0].is_poll());
    assert_eq!(
        reqs[0].query_param("since"),
        Some((t - 130).to_string().as_str())
    );
    // Stream resumes from the catch-up position derived from event times.
    assert!(!reqs[1].is_poll());
    assert_eq!(
        reqs[1].query_param("since"),
        Some((t - 80).to_string().as_str())
    );

    stream.line(&msg("x", t - 50));
    stream.line(&msg("y", t));
    let rest = conn.collect_until(|e| message_ids(e) == ["y"]).await;
    let xs = rest
        .iter()
        .filter(|e| message_ids(e).contains(&"x".to_owned()));
    assert_eq!(xs.count(), 0, "x was delivered twice: {rest:?}");
}

#[tokio::test]
async fn gap_event_on_missing_last_message() {
    let t = now();
    let script = Script::default();
    script.push(Reply::lines::<&str>(&[]).header("Date", &http_date(t)));
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(
        config(&server.url(), None),
        history("lost", t - 5, Some(t + 3600)),
    );

    let events = conn.collect_until(is_connected).await;
    let gaps = events.iter().filter(|e| **e == ConnEvent::Gap).count();
    assert_eq!(gaps, 1, "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ConnEvent::Messages { .. }))
    );
    let state = last_state(&events);
    // Rebaselined: the hint is not repeated on the next reconnect.
    assert_eq!(state.unwrap().last_message, None);
}

#[tokio::test]
async fn truncated_poll_reports_gap_once() {
    let t = now();
    let script = Script::default();
    script.push(
        Reply::lines(&[msg("n1", t - 2)])
            .header("Date", &http_date(t))
            .header("X-Messages-Truncated", "1"),
    );
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(
        config(&server.url(), None),
        history("lost", t - 5, Some(t + 3600)),
    );

    let events = conn.collect_until(is_connected).await;
    let gaps = events.iter().filter(|e| **e == ConnEvent::Gap).count();
    assert_eq!(gaps, 1, "{events:?}");
    assert!(events.iter().any(|e| message_ids(e) == ["n1"]));
}

#[tokio::test]
async fn truncated_header_alone_reports_gap() {
    let t = now();
    let script = Script::default();
    script.push(
        Reply::lines(&[msg("seen", t - 5), msg("n1", t - 2)])
            .header("Date", &http_date(t))
            .header("X-Messages-Truncated", "1"),
    );
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(
        config(&server.url(), None),
        history("seen", t - 5, Some(t + 3600)),
    );

    let events = conn.collect_until(is_connected).await;
    assert_eq!(events.iter().filter(|e| **e == ConnEvent::Gap).count(), 1);
    assert!(events.iter().any(|e| message_ids(e) == ["n1"]));
}

#[tokio::test]
async fn unauthorized_reports_auth_failed_and_waits() {
    let script = Script::default();
    script.push(Reply::status(401));
    let server = FakeServer::scripted(&script).await;
    let started = tokio::time::Instant::now();
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    conn.until(|e| *e == ConnEvent::Status(ConnStatus::AuthFailed))
        .await;
    tokio::time::sleep_until(started + Duration::from_millis(500)).await;
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn forbidden_poll_reports_auth_failed() {
    let script = Script::default();
    script.push(Reply::status(403));
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), history("a", now() - 10, None));

    conn.until(|e| *e == ConnEvent::Status(ConnStatus::AuthFailed))
        .await;
    assert!(server.requests()[0].is_poll());
}

#[tokio::test]
async fn server_error_reports_disconnected_and_retries() {
    let script = Script::default();
    script.push(Reply::status(500));
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    match conn.until(is_disconnected).await {
        ConnEvent::Status(ConnStatus::Disconnected { error }) => assert!(error.contains("500")),
        _ => unreachable!(),
    }
    conn.until(is_connected).await;
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn server_down_retries_with_backoff() {
    let mut conn = Conn::start(config(&unused_url(), None), InstanceState::default());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    let mut disconnected = 0;
    while let Ok(Some((_, ev))) = tokio::time::timeout_at(deadline, conn.rx.recv()).await {
        if is_disconnected(&ev) {
            disconnected += 1;
        }
    }
    assert!(disconnected >= 3, "only {disconnected} attempts");
    assert!(!conn.handle.is_finished());
}

#[tokio::test]
async fn reconnect_notify_forces_new_connection() {
    let server = FakeServer::scripted(&Script::default()).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    conn.until(is_connected).await;
    conn.ctl.reconnect.notify_one();
    conn.until(|e| *e == ConnEvent::Status(ConnStatus::Connecting))
        .await;
    conn.until(is_connected).await;
    let reqs = server.wait_requests(2).await;
    assert!(!reqs[1].is_poll());
}

#[tokio::test]
async fn invalid_json_line_is_skipped() {
    let script = Script::default();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    conn.until(is_connected).await;
    let t = now();
    stream.line("garbage");
    stream.line(r#"{"event":"message","time":1}"#);
    stream.line(&format!(
        r#"{{"id":"e","time":{t},"event":"poll_request"}}"#
    ));
    stream.line("");
    // Split across two chunks.
    let line = msg("ok", t);
    let (a, b) = line.split_at(10);
    stream.raw(a);
    stream.raw(&format!("{b}\n"));
    let ev = conn.next().await;
    assert_eq!(message_ids(&ev), ["ok"], "{ev:?}");
}

#[tokio::test]
async fn keepalive_updates_state_throttled() {
    let script = Script::default();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());

    conn.until(is_connected).await;
    let t = now();
    stream.line(&open(t - 100));
    stream.line(&keepalive(t));
    match conn.next().await {
        ConnEvent::State(st) => assert_eq!(st.watermark, Some(t)),
        other => panic!("expected State, got {other:?}"),
    }
    stream.line(&keepalive(t + 1));
    stream.line(&msg("m", t + 1));
    // No State for the second keepalive (throttled); next is the message.
    assert_eq!(message_ids(&conn.next().await), ["m"]);
    match conn.next().await {
        ConnEvent::State(st) => assert_eq!(st.watermark, Some(t + 1)),
        other => panic!("expected State, got {other:?}"),
    }
}

#[tokio::test]
async fn sends_auth_headers() {
    let script = Script::default();
    script.push(Reply::lines::<&str>(&[]));
    let server = FakeServer::scripted(&script).await;
    let mut conn = Conn::start(
        config(&server.url(), Some(Auth::Token("tk_x".into()))),
        history("a", now() - 10, None),
    );
    conn.until(is_connected).await;
    let reqs = server.wait_requests(2).await;
    assert!(reqs[0].is_poll());
    for r in &reqs {
        assert_eq!(r.header("authorization"), Some("Bearer tk_x"));
    }

    let server = FakeServer::scripted(&Script::default()).await;
    let auth = Auth::Basic {
        username: "u".into(),
        password: "pw".into(),
    };
    let mut conn = Conn::start(config(&server.url(), Some(auth)), InstanceState::default());
    conn.until(is_connected).await;
    assert_eq!(
        server.requests()[0].header("authorization"),
        Some("Basic dTpwdw==")
    );
}

#[tokio::test]
async fn cancel_stops_task() {
    let server = FakeServer::scripted(&Script::default()).await;
    let mut conn = Conn::start(config(&server.url(), None), InstanceState::default());
    conn.until(is_connected).await;
    conn.ctl.cancel.cancel();
    tokio::time::timeout(Duration::from_millis(200), conn.handle)
        .await
        .expect("task ends within 200 ms")
        .unwrap();

    // Also while waiting for the next attempt.
    let mut conn = Conn::start(config(&unused_url(), None), InstanceState::default());
    conn.until(is_disconnected).await;
    conn.ctl.cancel.cancel();
    tokio::time::timeout(Duration::from_millis(200), conn.handle)
        .await
        .expect("task ends within 200 ms")
        .unwrap();
}
