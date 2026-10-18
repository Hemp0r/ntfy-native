mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ntfy_native::actions::RecordingOpener;
use ntfy_native::connection::ConnectionConfig;
use ntfy_native::engine::{Command, EngineDeps, EngineHandle, spawn_engine_thread};
use ntfy_native::i18n::EN;
use ntfy_native::notification::{ActivationPayload, Level, Notification, RecordingNotifier};
use ntfy_native::paths::AppPaths;
use ntfy_native::secrets::{MemoryStore, SecretStore};
use ntfy_native::settings::{AuthKind, Instance, Settings, Topic};
use ntfy_native::state::State;
use ntfy_native::status::TrayState;
use ntfy_native::store::{Loaded, load_json};
use support::{FakeServer, Recorded, Reply, Script, unused_url};

const INSTANCE: &str = "inst";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn msg_on(id: &str, topic: &str, time: i64) -> String {
    format!(
        r#"{{"id":"{id}","time":{time},"event":"message","topic":"{topic}","message":"body {id}","expires":{}}}"#,
        time + 43200
    )
}

fn msg(id: &str, time: i64) -> String {
    msg_on(id, "alerts", time)
}

fn open(time: i64) -> String {
    format!(r#"{{"id":"o{time}","time":{time},"event":"open","topic":"alerts,backup"}}"#)
}

/// Polls return nothing, streams send `open` and stay silent.
fn ntfy_script() -> Script {
    Script::with_fallback(|req: &Recorded| {
        if req.is_poll() {
            Reply::lines::<&str>(&[])
        } else {
            Reply::lines(&[open(now())]).keep_open()
        }
    })
}

fn fast(cfg: &mut ConnectionConfig) {
    cfg.keepalive_timeout = Duration::from_secs(5);
    cfg.backoff_initial = Duration::from_millis(50);
    cfg.backoff_max = Duration::from_millis(200);
    cfg.auth_retry = Duration::from_secs(1);
}

fn settings_for(url: &str) -> Settings {
    let topic = |name: &str| Topic {
        name: name.into(),
        ..Topic::default()
    };
    Settings {
        instances: vec![Instance {
            id: INSTANCE.into(),
            url: url.into(),
            topics: vec![topic("alerts"), topic("backup")],
            ..Instance::default()
        }],
        ..Settings::default()
    }
}

fn load<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    match load_json(path) {
        Ok(Loaded::Ok(v)) => Some(v),
        _ => None,
    }
}

/// An engine on its own thread (as in production) with recording fakes.
struct TestEngine {
    handle: EngineHandle,
    thread: Option<std::thread::JoinHandle<()>>,
    notifier: Arc<RecordingNotifier>,
    opener: Arc<RecordingOpener>,
    trays: Arc<Mutex<Vec<TrayState>>>,
    exited: Arc<AtomicBool>,
    paths: AppPaths,
}

impl TestEngine {
    fn start(paths: &AppPaths, settings: Settings, state: State) -> Self {
        Self::start_with(paths, settings, state, Arc::new(MemoryStore::new()))
    }

    fn start_with(
        paths: &AppPaths,
        settings: Settings,
        state: State,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        let notifier = Arc::new(RecordingNotifier::default());
        let opener = Arc::new(RecordingOpener::default());
        let trays = Arc::new(Mutex::new(Vec::new()));
        let exited = Arc::new(AtomicBool::new(false));
        let deps = EngineDeps {
            paths: paths.clone(),
            settings,
            state,
            secrets,
            notifier: notifier.clone(),
            opener: opener.clone(),
            strings: &EN,
            on_tray: {
                let trays = trays.clone();
                Box::new(move |t| trays.lock().unwrap().push(t))
            },
            on_exit: {
                let exited = exited.clone();
                Box::new(move || exited.store(true, Ordering::SeqCst))
            },
            tune: fast,
            enable_triggers: false,
        };
        let (handle, thread) = spawn_engine_thread(deps);
        Self {
            handle,
            thread: Some(thread),
            notifier,
            opener,
            trays,
            exited,
            paths: paths.clone(),
        }
    }

    fn send(&self, cmd: Command) {
        self.handle.tx.send(cmd).expect("engine running");
    }

    fn shown(&self) -> Vec<Notification> {
        self.notifier.shown.lock().unwrap().clone()
    }

    fn shown_ids(&self) -> Vec<String> {
        self.shown().into_iter().map(|n| n.id).collect()
    }

    fn trays(&self) -> Vec<TrayState> {
        self.trays.lock().unwrap().clone()
    }

    fn saved_settings(&self) -> Option<Settings> {
        load(&self.paths.settings_file())
    }

    /// Waits up to 3 s for `cond`.
    async fn wait_for(&self, what: &str, cond: impl Fn(&Self) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !cond(self) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}; shown={:?} trays={:?}",
                self.shown_ids(),
                self.trays()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Sends `Quit` and waits for the engine thread to end.
    async fn quit(mut self) -> Self {
        self.send(Command::Quit);
        let thread = self.thread.take().unwrap();
        let joined = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::task::spawn_blocking(move || thread.join()),
        )
        .await
        .expect("engine thread ends after Quit");
        joined.unwrap().expect("engine thread did not panic");
        self
    }
}

impl Drop for TestEngine {
    fn drop(&mut self) {
        let _ = self.handle.tx.send(Command::Quit);
    }
}

#[tokio::test]
async fn message_reaches_notifier_once() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    // The fallback poll replays m1 after a reconnect: it must not show again.
    let script = Script::with_fallback(|req: &Recorded| {
        if req.is_poll() {
            Reply::lines(&[msg("m1", now())])
        } else {
            Reply::lines(&[open(now())]).keep_open()
        }
    });
    script.push(Reply::lines(&[open(t), msg("m1", t)]).keep_open());
    let server = FakeServer::scripted(&script).await;
    let engine = TestEngine::start(&paths, settings_for(&server.url()), State::default());

    engine.wait_for("m1", |e| !e.shown().is_empty()).await;
    let n = &engine.shown()[0];
    assert_eq!(n.id, "inst:m1");
    assert_eq!(n.body, "body m1");

    engine.send(Command::ReconnectAll);
    let reqs = server.wait_requests(3).await;
    assert!(reqs[1].is_poll(), "reconnect starts with a catch-up poll");
    assert!(!reqs[2].is_poll());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(engine.shown_ids(), ["inst:m1"]);
}

#[tokio::test]
async fn muted_via_command_not_shown() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let script = ntfy_script();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let engine = TestEngine::start(&paths, settings_for(&server.url()), State::default());

    let t = now();
    stream.line(&open(t));
    engine.send(Command::SetTopicMuted {
        instance_id: INSTANCE.into(),
        topic: "alerts".into(),
        muted: true,
    });
    engine
        .wait_for("muted in settings.json", |e| {
            e.saved_settings()
                .is_some_and(|s| s.instances[0].topics[0].muted)
        })
        .await;

    stream.line(&msg_on("m1", "alerts", t));
    stream.line(&msg_on("m2", "backup", t));
    engine.wait_for("m2", |e| !e.shown().is_empty()).await;
    assert_eq!(engine.shown_ids(), ["inst:m2"]);
    let saved = engine.saved_settings().unwrap();
    assert!(!saved.instances[0].topics[1].muted);
}

#[tokio::test]
async fn quit_persists_state_and_calls_exit() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    let script = ntfy_script();
    script.push(Reply::lines(&[open(t), msg("m1", t)]).keep_open());
    let server = FakeServer::scripted(&script).await;
    let engine = TestEngine::start(&paths, settings_for(&server.url()), State::default());

    engine.wait_for("m1", |e| !e.shown().is_empty()).await;
    assert!(!engine.exited.load(Ordering::SeqCst));
    let engine = engine.quit().await;
    assert!(engine.exited.load(Ordering::SeqCst), "on_exit called");

    let state: State = load(&paths.state_file()).expect("state.json written");
    let inst = &state.instances[INSTANCE];
    assert_eq!(inst.watermark, Some(t));
    assert!(inst.seen_ids.contains(&"m1".to_string()));
    assert_eq!(inst.last_message.as_ref().unwrap().id, "m1");
    assert_eq!(state.recent[0].id, "m1");
    assert!(state.gaps.is_empty());
    let settings: Settings = load(&paths.settings_file()).expect("settings.json written");
    assert_eq!(settings, settings_for(&server.url()));
}

#[tokio::test]
async fn restart_with_saved_state_does_not_renotify() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    let script = ntfy_script();
    script.push(Reply::lines(&[open(t), msg("x", t)]).keep_open());
    let server = FakeServer::scripted(&script).await;

    let first = TestEngine::start(&paths, settings_for(&server.url()), State::default());
    first.wait_for("x", |e| !e.shown().is_empty()).await;
    assert_eq!(first.shown_ids(), ["inst:x"]);
    first.quit().await;
    let before_restart = server.requests().len();

    // Second run starts only from what is on disk.
    let settings: Settings = load(&paths.settings_file()).expect("settings.json");
    let state: State = load(&paths.state_file()).expect("state.json");
    script.push(Reply::lines(&[msg("x", t), msg("y", t)]));
    let second = TestEngine::start(&paths, settings, state);

    second.wait_for("y", |e| !e.shown().is_empty()).await;
    let reqs = server.wait_requests(before_restart + 1).await;
    let poll = &reqs[before_restart];
    assert!(poll.is_poll(), "restart begins with a catch-up poll");
    assert_eq!(
        poll.query_param("since"),
        Some((t - 30).to_string().as_str())
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(second.shown_ids(), ["inst:y"]);
}

#[tokio::test]
async fn activated_open_uses_opener() {
    let dir = tempfile::tempdir().unwrap();
    let engine = TestEngine::start(
        &AppPaths::under(dir.path()),
        Settings::default(),
        State::default(),
    );
    engine.send(Command::Activated(ActivationPayload::Open {
        url: "https://x".into(),
    }));
    engine
        .wait_for("opener", |e| !e.opener.opened.lock().unwrap().is_empty())
        .await;
    // The URL arrives normalized (see actions::perform).
    assert_eq!(*engine.opener.opened.lock().unwrap(), ["https://x/"]);
    assert!(engine.shown().is_empty());
}

#[tokio::test]
async fn failed_action_shows_passive_notification() {
    let dir = tempfile::tempdir().unwrap();
    let engine = TestEngine::start(
        &AppPaths::under(dir.path()),
        Settings::default(),
        State::default(),
    );
    engine.send(Command::Activated(ActivationPayload::Open {
        url: "javascript:alert(1)".into(),
    }));
    engine
        .wait_for("failure notification", |e| !e.shown().is_empty())
        .await;
    let n = &engine.shown()[0];
    assert_eq!(n.title, "Action failed: javascript");
    assert_eq!(n.body, "refusing to open URL with scheme javascript");
    assert_eq!(n.level, Level::Passive);
    assert!(n.buttons.is_empty() && n.default_action.is_none());
    assert!(engine.opener.opened.lock().unwrap().is_empty());
}

#[tokio::test]
async fn tray_state_reported() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let server = FakeServer::scripted(&ntfy_script()).await;
    let ok = TestEngine::start(&paths, settings_for(&server.url()), State::default());
    server.wait_requests(1).await;
    ok.wait_for("initial tray", |e| !e.trays().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(ok.trays(), [TrayState::Ok], "reported once, no flapping");
    ok.quit().await;

    let denied = FakeServer::start(|_| Reply::status(401)).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let failed = TestEngine::start(&paths, settings_for(&denied.url()), State::default());
    let auth_failed = TrayState::AuthFailed {
        instance: "127.0.0.1".into(),
    };
    failed
        .wait_for("AuthFailed", |e| e.trays().contains(&auth_failed))
        .await;
    // The auth retry (1 s here) must not flicker back to Ok.
    denied.wait_requests(2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(failed.trays(), [TrayState::Ok, auth_failed]);
}

#[tokio::test]
async fn missing_secret_reports_auth_failed_without_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let server = FakeServer::scripted(&ntfy_script()).await;
    let mut settings = settings_for(&server.url());
    settings.instances[0].auth = AuthKind::Token;
    let engine = TestEngine::start_with(
        &AppPaths::under(dir.path()),
        settings,
        State::default(),
        Arc::new(MemoryStore::new()),
    );
    engine.wait_for("tray", |e| !e.trays().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        engine.trays(),
        [TrayState::AuthFailed {
            instance: "127.0.0.1".into()
        }]
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn pause_is_persisted_and_expired_pause_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    // Unreachable server: within the grace period the tray still shows Ok/Paused.
    let engine = TestEngine::start(&paths, settings_for(&unused_url()), State::default());

    engine.send(Command::PauseAll(Some(i64::MAX)));
    engine
        .wait_for("paused in settings.json", |e| {
            e.saved_settings()
                .is_some_and(|s| s.paused_until == Some(i64::MAX))
        })
        .await;
    engine
        .wait_for("Paused tray", |e| {
            e.trays().last() == Some(&TrayState::Paused)
        })
        .await;

    engine.send(Command::PauseAll(Some(1)));
    engine
        .wait_for("expired pause cleared on disk", |e| {
            e.saved_settings().is_some_and(|s| s.paused_until.is_none())
        })
        .await;
    engine
        .wait_for("Ok tray", |e| e.trays().last() == Some(&TrayState::Ok))
        .await;
}

/// ntfy plus a tarpit under `/file/`: image headers, then a body that never arrives.
fn tarpit_script() -> Script {
    Script::with_fallback(|req: &Recorded| {
        if req.path.starts_with("/file/") {
            Reply::stall().header("Content-Type", "image/png")
        } else if req.is_poll() {
            Reply::lines::<&str>(&[])
        } else {
            Reply::lines(&[open(now())]).keep_open()
        }
    })
}

fn msg_with_image(id: &str, time: i64, image_url: &str) -> String {
    format!(
        r#"{{"id":"{id}","time":{time},"event":"message","topic":"alerts","message":"body {id}","expires":{},"attachment":{{"name":"x.png","type":"image/png","url":"{image_url}"}}}}"#,
        time + 43200
    )
}

/// Starts an engine whose first message has an image stuck in the tarpit and
/// waits until that download is in flight.
async fn engine_with_stalled_image(paths: &AppPaths) -> (FakeServer, TestEngine) {
    let script = tarpit_script();
    let server = FakeServer::scripted(&script).await;
    let t = now();
    let image = format!("{}/file/x.png", server.url());
    script.push(Reply::lines(&[open(t), msg_with_image("m1", t, &image)]).keep_open());
    let engine = TestEngine::start(paths, settings_for(&server.url()), State::default());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !server.requests().iter().any(|r| r.path == "/file/x.png") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "image never requested"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (server, engine)
}

#[tokio::test]
async fn quit_not_blocked_by_tarpit_media() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let (_server, engine) = engine_with_stalled_image(&paths).await;
    assert!(engine.shown().is_empty(), "image still downloading");

    let started = std::time::Instant::now();
    let engine = engine.quit().await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(engine.exited.load(Ordering::SeqCst));
    let shown = engine.shown();
    assert_eq!(shown.len(), 1, "pending notification shown on quit");
    assert_eq!(shown[0].id, "inst:m1");
    assert_eq!(shown[0].image, None);
    let state: State = load(&paths.state_file()).expect("state.json written");
    assert!(
        state.instances[INSTANCE]
            .seen_ids
            .contains(&"m1".to_string())
    );
}

#[tokio::test]
async fn commands_processed_while_media_stalls() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let (_server, engine) = engine_with_stalled_image(&paths).await;

    let started = std::time::Instant::now();
    engine.send(Command::Activated(ActivationPayload::Open {
        url: "https://x".into(),
    }));
    while engine.opener.opened.lock().unwrap().is_empty() {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "activation blocked by media download"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(engine.shown().is_empty(), "image still downloading");
}
