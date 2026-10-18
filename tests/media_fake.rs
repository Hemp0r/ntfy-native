mod support;

use std::fs;
use std::time::{Duration, SystemTime};

use ntfy_native::media::{MAX_IMAGE_BYTES, MediaFetcher};
use ntfy_native::notification::{Level, MediaRef, Notification};
use ntfy_native::ntfy::client::http_client;
use ntfy_native::secrets::Auth;
use support::{FakeServer, Reply};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png-bytes";

fn fetcher(dir: &std::path::Path) -> MediaFetcher {
    MediaFetcher::new(http_client(), dir.to_path_buf())
}

fn dir_entries(dir: &std::path::Path) -> Vec<String> {
    match fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn notification(id: &str, image: Option<&str>, icon: Option<&str>) -> Notification {
    Notification {
        id: id.into(),
        thread: "t".into(),
        title: "title".into(),
        body: "body".into(),
        level: Level::Active,
        default_action: None,
        buttons: Vec::new(),
        image: image.map(|u| MediaRef::Remote(u.into())),
        icon: icon.map(|u| MediaRef::Remote(u.into())),
    }
}

#[tokio::test]
async fn fetches_png() {
    let server = FakeServer::start(|_| Reply::bytes(PNG).header("content-type", "image/png")).await;
    let dir = tempfile::tempdir().unwrap();
    let path = fetcher(dir.path())
        .fetch(&format!("{}/a", server.url()), "abc", 1024, None)
        .await
        .expect("fetched");
    assert_eq!(path.extension().unwrap(), "png");
    assert_eq!(fs::read(&path).unwrap(), PNG);
    assert_eq!(dir_entries(dir.path()), vec!["abc.png".to_string()]);
}

#[tokio::test]
async fn rejects_non_image() {
    let server =
        FakeServer::start(|_| Reply::bytes("<html>").header("content-type", "text/html")).await;
    let dir = tempfile::tempdir().unwrap();
    let got = fetcher(dir.path())
        .fetch(&server.url(), "abc", 1024, None)
        .await;
    assert!(got.is_none());
    assert!(dir_entries(dir.path()).is_empty());
}

#[tokio::test]
async fn rejects_large_content_length() {
    let server = FakeServer::start(|_| {
        Reply::stall()
            .header("content-type", "image/png")
            .header("content-length", &(6 * 1024 * 1024).to_string())
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let got = fetcher(dir.path())
        .fetch(&server.url(), "abc", MAX_IMAGE_BYTES, None)
        .await;
    assert!(got.is_none());
    assert!(dir_entries(dir.path()).is_empty());
}

#[tokio::test]
async fn rejects_oversized_stream_without_length() {
    let (reply, tx) = Reply::stream();
    let reply = reply.header("content-type", "image/png");
    let mut once = Some(reply);
    let server =
        FakeServer::start(move |_| once.take().unwrap_or_else(|| Reply::status(500))).await;
    let chunk = "x".repeat(1024 * 1024);
    for _ in 0..6 {
        tx.raw(&chunk);
    }
    let dir = tempfile::tempdir().unwrap();
    let got = fetcher(dir.path())
        .fetch(&server.url(), "abc", MAX_IMAGE_BYTES, None)
        .await;
    assert!(got.is_none());
    assert!(
        dir_entries(dir.path()).is_empty(),
        "{:?}",
        dir_entries(dir.path())
    );
    drop(tx);
}

#[tokio::test]
async fn rejects_file_scheme() {
    let server = FakeServer::start(|_| Reply::bytes(PNG).header("content-type", "image/png")).await;
    let dir = tempfile::tempdir().unwrap();
    let f = fetcher(dir.path());
    assert!(
        f.fetch("file:///etc/passwd", "abc", 1024, None)
            .await
            .is_none()
    );
    assert!(
        f.fetch("javascript:alert(1)", "abc", 1024, None)
            .await
            .is_none()
    );
    assert!(server.requests().is_empty());
    assert!(dir_entries(dir.path()).is_empty());
}

#[tokio::test]
async fn auth_only_same_origin() {
    let ntfy = FakeServer::start(|_| Reply::bytes(PNG).header("content-type", "image/png")).await;
    let other = FakeServer::start(|_| Reply::bytes(PNG).header("content-type", "image/png")).await;
    let dir = tempfile::tempdir().unwrap();
    let auth = Auth::Token("tk_secret".into());
    let n = notification(
        "m1",
        Some(&format!("{}/file/a.png", ntfy.url())),
        Some(&format!("{}/icon.png", other.url())),
    );
    let out = fetcher(dir.path())
        .resolve(n, &ntfy.url(), Some(&auth))
        .await;
    assert!(matches!(out.image, Some(MediaRef::Local(_))));
    assert!(matches!(out.icon, Some(MediaRef::Local(_))));
    assert_eq!(
        ntfy.requests()[0].header("authorization"),
        Some("Bearer tk_secret")
    );
    assert_eq!(other.requests()[0].header("authorization"), None);
}

#[tokio::test]
async fn resolve_drops_failed_refs() {
    let server = FakeServer::start(|req| {
        if req.path == "/icon.png" {
            Reply::bytes(PNG).header("content-type", "image/png")
        } else {
            Reply::status(404)
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let n = notification(
        "a/b",
        Some(&format!("{}/missing.png", server.url())),
        Some(&format!("{}/icon.png", server.url())),
    );
    let out = fetcher(dir.path())
        .resolve(n.clone(), &server.url(), None)
        .await;
    assert_eq!(out.image, None);
    let Some(MediaRef::Local(icon)) = &out.icon else {
        panic!("icon should resolve");
    };
    assert_eq!(icon.file_name().unwrap(), "a_b-icon.png");
    assert_eq!(out.title, n.title);
    assert_eq!(out.body, n.body);
    assert_eq!(out.id, n.id);
}

#[test]
fn cleanup_removes_old_files() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.png");
    let new = dir.path().join("new.png");
    fs::write(&old, b"x").unwrap();
    fs::write(&new, b"x").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let past = SystemTime::now() - Duration::from_secs(3 * 86400);
    filetime::set_file_mtime(&old, filetime::FileTime::from_system_time(past)).unwrap();
    let f = fetcher(dir.path());
    assert_eq!(f.cleanup(Duration::from_secs(86400)).unwrap(), 1);
    assert!(!old.exists());
    assert!(new.exists());
    assert!(dir.path().join("sub").exists());
    let missing = fetcher(&dir.path().join("nope"));
    assert_eq!(missing.cleanup(Duration::ZERO).unwrap(), 0);
}
