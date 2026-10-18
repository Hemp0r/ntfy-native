mod support;

use ntfy_native::conntest::test_connection;
use ntfy_native::instance_edit::{AuthDraft, InstanceDraft};
use ntfy_native::ipc::{TestOutcome, TopicAccess};
use ntfy_native::ntfy::client::{health, http_client, topic_readable};
use ntfy_native::secrets::Auth;
use ntfy_native::settings::Topic;
use support::{FakeServer, Recorded, Reply, unused_url};

const HEALTHY: &str = r#"{"healthy":true}"#;

fn draft(url: &str, topics: &[&str]) -> InstanceDraft {
    InstanceDraft {
        id: None,
        url: url.into(),
        auth: AuthDraft::None,
        cache_duration_secs: 43200,
        topics: topics
            .iter()
            .map(|n| Topic {
                name: (*n).into(),
                ..Topic::default()
            })
            .collect(),
    }
}

fn topic_of(req: &Recorded) -> &str {
    req.path.trim_matches('/').split('/').next().unwrap_or("")
}

fn access(name: &str, readable: bool) -> TopicAccess {
    TopicAccess {
        name: name.into(),
        readable,
    }
}

#[tokio::test]
async fn healthy_server_with_readable_topics() {
    let server = FakeServer::start(|req| match req.path.as_str() {
        "/v1/health" => Reply::bytes(HEALTHY),
        _ => Reply::lines::<&str>(&[]),
    })
    .await;
    let out = test_connection(&http_client(), &draft(&server.url(), &["a", "b"]), None).await;
    assert_eq!(
        out,
        TestOutcome::Ok {
            topics: vec![access("a", true), access("b", true)]
        }
    );
    let reqs = server.requests();
    let poll = reqs.iter().find(|r| r.path == "/a/json").unwrap();
    assert_eq!(poll.query_param("poll"), Some("1"));
    assert_eq!(poll.query_param("since"), Some("1s"));
}

#[tokio::test]
async fn unreachable_server() {
    let url = unused_url();
    let out = test_connection(&http_client(), &draft(&url, &["a"]), None).await;
    let TestOutcome::Unreachable { message } = out else {
        panic!("{out:?}");
    };
    assert_eq!(message, "connection refused");
    assert!(!message.contains("127.0.0.1"));
}

#[tokio::test]
async fn html_server_is_not_ntfy() {
    let server = FakeServer::start(|_| {
        Reply::bytes("<html><body>hello</body></html>").header("content-type", "text/html")
    })
    .await;
    let out = test_connection(&http_client(), &draft(&server.url(), &["a"]), None).await;
    assert_eq!(out, TestOutcome::NotNtfy);
    assert_eq!(server.requests().len(), 1, "no topic probed");
}

#[tokio::test]
async fn other_json_is_not_ntfy() {
    let server = FakeServer::start(|_| Reply::bytes(r#"{"status":"ok"}"#)).await;
    assert!(!health(&http_client(), &server.url()).await.unwrap());
    let server = FakeServer::start(|_| Reply::bytes(r#"{"healthy":false}"#)).await;
    assert!(!health(&http_client(), &server.url()).await.unwrap());
}

#[tokio::test]
async fn old_server_without_health_endpoint_is_detected_by_content_type() {
    let ntfy = FakeServer::start(|req| match req.path.as_str() {
        "/ntfy-native-probe/json" => {
            Reply::lines::<&str>(&[]).header("content-type", "application/x-ndjson; charset=utf-8")
        }
        _ => Reply::status(404),
    })
    .await;
    assert!(health(&http_client(), &ntfy.url()).await.unwrap());
    let probe = &ntfy.requests()[1];
    assert_eq!(probe.query_param("poll"), Some("1"));
    assert_eq!(probe.query_param("since"), Some("1s"));

    let html = FakeServer::start(|req| match req.path.as_str() {
        "/v1/health" => Reply::status(404),
        _ => Reply::bytes("<html></html>").header("content-type", "text/html"),
    })
    .await;
    assert!(!health(&http_client(), &html.url()).await.unwrap());

    let nothing = FakeServer::start(|_| Reply::status(404)).await;
    assert!(!health(&http_client(), &nothing.url()).await.unwrap());
}

fn token_server() -> impl FnMut(&Recorded) -> Reply + Send + 'static {
    |req| {
        if req.path == "/v1/health" {
            return Reply::bytes(HEALTHY);
        }
        match req.header("authorization") {
            Some("Bearer good") => Reply::lines::<&str>(&[]),
            _ => Reply::status(401),
        }
    }
}

#[tokio::test]
async fn bad_token_is_auth_failed() {
    let server = FakeServer::start(token_server()).await;
    let mut d = draft(&server.url(), &["a"]);
    d.auth = AuthDraft::Token {
        token: Some("bad".into()),
    };
    assert_eq!(
        test_connection(&http_client(), &d, None).await,
        TestOutcome::AuthFailed
    );
    d.auth = AuthDraft::Token {
        token: Some("good".into()),
    };
    assert_eq!(
        test_connection(&http_client(), &d, None).await,
        TestOutcome::Ok {
            topics: vec![access("a", true)]
        }
    );
    let health_req = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/v1/health")
        .unwrap();
    assert_eq!(
        health_req.header("authorization"),
        None,
        "no creds to /v1/health"
    );
}

#[tokio::test]
async fn kept_secret_uses_the_stored_credential() {
    let server = FakeServer::start(token_server()).await;
    let mut d = draft(&server.url(), &["a"]);
    d.auth = AuthDraft::Token { token: None };
    let out = test_connection(&http_client(), &d, Some(Auth::Token("good".into()))).await;
    assert!(matches!(out, TestOutcome::Ok { .. }), "{out:?}");
    // Nothing stored, or a stored credential of the other kind.
    for stored in [
        None,
        Some(Auth::Basic {
            username: "u".into(),
            password: "p".into(),
        }),
    ] {
        let out = test_connection(&http_client(), &d, stored).await;
        assert!(matches!(out, TestOutcome::InvalidInput { .. }), "{out:?}");
    }
    d.auth = AuthDraft::Basic {
        username: "u".into(),
        password: None,
    };
    let out = test_connection(&http_client(), &d, Some(Auth::Token("good".into()))).await;
    assert!(matches!(out, TestOutcome::InvalidInput { .. }), "{out:?}");
}

#[tokio::test]
async fn basic_auth_is_sent() {
    let server = FakeServer::start(|req| {
        if req.path == "/v1/health" {
            return Reply::bytes(HEALTHY);
        }
        // u:pw
        match req.header("authorization") {
            Some("Basic dTpwdw==") => Reply::lines::<&str>(&[]),
            _ => Reply::status(401),
        }
    })
    .await;
    let mut d = draft(&server.url(), &["a"]);
    d.auth = AuthDraft::Basic {
        username: "u".into(),
        password: None,
    };
    let stored = Some(Auth::Basic {
        username: "u".into(),
        password: "pw".into(),
    });
    assert!(matches!(
        test_connection(&http_client(), &d, stored).await,
        TestOutcome::Ok { .. }
    ));
}

#[tokio::test]
async fn forbidden_topic_is_not_readable() {
    let server = FakeServer::start(|req| match (req.path.as_str(), topic_of(req)) {
        ("/v1/health", _) => Reply::bytes(HEALTHY),
        (_, "secret") => Reply::status(403),
        _ => Reply::lines::<&str>(&[]),
    })
    .await;
    let out = test_connection(
        &http_client(),
        &draft(&server.url(), &["open", "secret"]),
        None,
    )
    .await;
    assert_eq!(
        out,
        TestOutcome::Ok {
            topics: vec![access("open", true), access("secret", false)]
        }
    );
    let http = http_client();
    assert!(
        !topic_readable(&http, &server.url(), "secret", None)
            .await
            .unwrap()
    );
    assert!(
        topic_readable(&http, &server.url(), "open", None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn no_topics_only_checks_credentials() {
    let server = FakeServer::start(token_server()).await;
    let mut d = draft(&server.url(), &[]);
    d.auth = AuthDraft::Token {
        token: Some("bad".into()),
    };
    assert_eq!(
        test_connection(&http_client(), &d, None).await,
        TestOutcome::AuthFailed
    );
    d.auth = AuthDraft::Token {
        token: Some("good".into()),
    };
    assert_eq!(
        test_connection(&http_client(), &d, None).await,
        TestOutcome::Ok { topics: vec![] }
    );
    let last = server.requests().pop().unwrap();
    assert_eq!(last.path, "/ntfy-native-auth-check/json");

    // A 403 on the probe topic still means the credentials were accepted.
    let forbidden = FakeServer::start(|req| match req.path.as_str() {
        "/v1/health" => Reply::bytes(HEALTHY),
        _ => Reply::status(403),
    })
    .await;
    assert_eq!(
        test_connection(&http_client(), &draft(&forbidden.url(), &[]), None).await,
        TestOutcome::Ok { topics: vec![] }
    );
}

#[tokio::test]
async fn invalid_url_is_invalid_input_without_echo() {
    for url in [
        "s3cret",
        "https://u:s3cret@host.example",
        "ftp://s3cret.example",
        "https://host.example?s3cret=1",
        "",
    ] {
        let out = test_connection(&http_client(), &draft(url, &["a"]), None).await;
        let TestOutcome::InvalidInput { message } = &out else {
            panic!("{url}: {out:?}");
        };
        assert!(!message.contains("s3cret"), "{message}");
    }
    let mut d = draft("https://host.example", &["a", "s3cret/../x"]);
    d.auth = AuthDraft::Token {
        token: Some("s3cret".into()),
    };
    let out = test_connection(&http_client(), &d, None).await;
    let TestOutcome::InvalidInput { message } = &out else {
        panic!("{out:?}");
    };
    assert!(
        message.starts_with("topic 2:") && !message.contains("s3cret"),
        "{message}"
    );
}

#[tokio::test]
async fn server_errors_are_short_unreachable_messages() {
    let server = FakeServer::start(|_| Reply::status(502)).await;
    let out = test_connection(&http_client(), &draft(&server.url(), &["a"]), None).await;
    assert_eq!(
        out,
        TestOutcome::Unreachable {
            message: "HTTP 502".into()
        }
    );
    let server = FakeServer::start(|_| Reply::status(429).header("retry-after", "5")).await;
    let out = test_connection(&http_client(), &draft(&server.url(), &["a"]), None).await;
    assert_eq!(
        out,
        TestOutcome::Unreachable {
            message: "rate limited".into()
        }
    );
}

#[tokio::test(start_paused = true)]
async fn stalled_server_times_out_after_ten_seconds() {
    let server = FakeServer::start(|_| Reply::stall()).await;
    let started = tokio::time::Instant::now();
    let out = test_connection(&http_client(), &draft(&server.url(), &["a"]), None).await;
    assert_eq!(
        out,
        TestOutcome::Unreachable {
            message: "timeout".into()
        }
    );
    assert_eq!(started.elapsed().as_secs(), 10);
}
