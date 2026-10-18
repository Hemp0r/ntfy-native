mod support;

use std::collections::BTreeMap;

use ntfy_native::actions::{RecordingOpener, SystemOpener, UrlOpener, is_openable, perform};
use ntfy_native::notification::ActivationPayload;
use ntfy_native::ntfy::client::http_client;
use support::{FakeServer, Reply};

fn http(
    method: &str,
    url: String,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> ActivationPayload {
    ActivationPayload::Http {
        label: "L".into(),
        method: method.into(),
        url,
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        body: body.map(str::to_owned),
    }
}

#[tokio::test]
async fn open_https_calls_opener() {
    let opener = RecordingOpener::default();
    let p = ActivationPayload::Open {
        url: "https://x".into(),
    };
    perform(&http_client(), &p, &opener).await.unwrap();
    assert_eq!(
        *opener.opened.lock().unwrap(),
        vec!["https://x/".to_string()]
    );
}

#[tokio::test]
async fn open_rejects_file_and_javascript() {
    let opener = RecordingOpener::default();
    for url in ["file:///etc/passwd", "javascript:alert(1)", "not a url"] {
        let p = ActivationPayload::Open { url: url.into() };
        let err = perform(&http_client(), &p, &opener).await.unwrap_err();
        let msg = format!("{err:#}");
        for leak in ["passwd", "alert", "not a url"] {
            assert!(!msg.contains(leak), "{msg}");
        }
    }
    assert!(opener.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn http_action_sends_request() {
    let server = FakeServer::start(|_| Reply::status(200)).await;
    let p = http(
        "PUT",
        format!("{}/hook", server.url()),
        &[("X-A", "1")],
        Some("b"),
    );
    perform(&http_client(), &p, &RecordingOpener::default())
        .await
        .unwrap();
    let r = &server.requests()[0];
    assert_eq!(r.method.as_str(), "PUT");
    assert_eq!(r.path, "/hook");
    assert_eq!(r.header("x-a"), Some("1"));
    assert_eq!(&r.body[..], b"b");
}

#[tokio::test]
async fn http_action_error_status() {
    let server = FakeServer::start(|_| Reply::status(500)).await;
    let p = http("POST", server.url(), &[], None);
    assert!(
        perform(&http_client(), &p, &RecordingOpener::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn http_action_rejects_bad_input_without_request() {
    let server = FakeServer::start(|_| Reply::status(200)).await;
    let o = RecordingOpener::default();
    let c = http_client();
    let cases = [
        http("BAD METHOD", server.url(), &[], None),
        http("POST", server.url(), &[("bad name", "v")], None),
        http("POST", server.url(), &[("X", "a\nb")], None),
        http("POST", "file:///etc/passwd".into(), &[], None),
        http("POST", "ftp://x/y".into(), &[], None),
        http("POST", "nonsense".into(), &[], None),
    ];
    for p in &cases {
        assert!(perform(&c, p, &o).await.is_err(), "{p:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn openable_schemes() {
    for u in [
        "http://a",
        "https://a",
        "mailto:a@b.c",
        "HTTPS://a",
        "Mailto:a@b.c",
    ] {
        assert!(is_openable(u), "{u}");
    }
    for u in [
        "ftp://a",
        "file:///x",
        "javascript:alert(1)",
        "ntfy://x",
        "",
        "x",
        " javascript:1",
    ] {
        assert!(!is_openable(u), "{u}");
    }
}

#[test]
fn recording_opener_records_via_trait() {
    let o = RecordingOpener::default();
    (&o as &dyn UrlOpener).open("https://a").unwrap();
    assert_eq!(o.opened.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn http_action_error_hides_url() {
    let p = http(
        "POST",
        format!("{}/hook?token=s3cret", support::unused_url()),
        &[],
        None,
    );
    let err = perform(&http_client(), &p, &RecordingOpener::default())
        .await
        .unwrap_err();
    assert!(!format!("{err:#}").contains("s3cret"));
    assert!(!format!("{err:?}").contains("s3cret"));
}

#[test]
fn system_opener_refuses_dangerous_schemes() {
    for url in ["file:///etc/passwd", "javascript:alert(1)"] {
        let err = SystemOpener.open(url).unwrap_err();
        let msg = format!("{err:#}");
        assert!(!msg.contains("passwd") && !msg.contains("alert"), "{msg}");
    }
}
