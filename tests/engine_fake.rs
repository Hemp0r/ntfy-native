mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ntfy_native::actions::RecordingOpener;
use ntfy_native::connection::ConnectionConfig;
use ntfy_native::engine::{Command, EngineDeps, EngineHandle, spawn_engine_thread};
use ntfy_native::i18n::EN;
use ntfy_native::instance_edit::{AuthDraft, InstanceDraft};
use ntfy_native::ipc::{CoreMsg, UiRequest};
use ntfy_native::notification::{ActivationPayload, Level, Notification, RecordingNotifier};
use ntfy_native::paths::AppPaths;
use ntfy_native::secrets::{MemoryStore, SecretStore, account_for};
use ntfy_native::settings::{AuthKind, Instance, Settings, Topic};
use ntfy_native::snapshot::{InstanceStatus, UiSnapshot};
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

type SaveResults = Arc<Mutex<Vec<(u64, Result<String, String>)>>>;

/// An engine on its own thread (as in production) with recording fakes.
struct TestEngine {
    handle: EngineHandle,
    thread: Option<std::thread::JoinHandle<()>>,
    notifier: Arc<RecordingNotifier>,
    opener: Arc<RecordingOpener>,
    trays: Arc<Mutex<Vec<TrayState>>>,
    snapshots: Arc<Mutex<Vec<UiSnapshot>>>,
    save_results: SaveResults,
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
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let save_results = SaveResults::default();
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
            on_ui: {
                let snapshots = snapshots.clone();
                let save_results = save_results.clone();
                Box::new(move |m| match m {
                    CoreMsg::Snapshot(s) => snapshots.lock().unwrap().push(s),
                    CoreMsg::SaveResult { request_id, result } => {
                        save_results.lock().unwrap().push((request_id, result))
                    }
                    _ => {}
                })
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
            snapshots,
            save_results,
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

    fn snapshots(&self) -> Vec<UiSnapshot> {
        self.snapshots.lock().unwrap().clone()
    }

    fn last_snapshot(&self) -> Option<UiSnapshot> {
        self.snapshots().pop()
    }

    /// Sends `SaveInstance` and waits for its result.
    async fn save(&self, request_id: u64, draft: InstanceDraft) -> Result<String, String> {
        self.send(ui(UiRequest::SaveInstance { request_id, draft }));
        let find = |e: &Self| {
            e.save_results
                .lock()
                .unwrap()
                .iter()
                .find(|(id, _)| *id == request_id)
                .map(|(_, r)| r.clone())
        };
        self.wait_for("save result", |e| find(e).is_some()).await;
        find(self).unwrap()
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

fn ui(req: UiRequest) -> Command {
    Command::Ui(req)
}

#[tokio::test]
async fn snapshot_sent_on_start_and_on_change_only() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    let script = ntfy_script();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let engine = TestEngine::start(&paths, settings_for(&server.url()), State::default());

    engine
        .wait_for("first snapshot", |e| !e.snapshots().is_empty())
        .await;
    let first = engine.snapshots().remove(0);
    assert!(first.configured);
    assert!(first.recent.is_empty());

    stream.line(&open(t));
    engine
        .wait_for("connected", |e| {
            e.last_snapshot()
                .is_some_and(|s| s.instances[0].status == InstanceStatus::Connected)
        })
        .await;
    stream.line(&msg("m1", t));
    engine
        .wait_for("recent message", |e| {
            e.last_snapshot().is_some_and(|s| !s.recent.is_empty())
        })
        .await;
    assert_eq!(engine.last_snapshot().unwrap().recent[0].message_id, "m1");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let all = engine.snapshots();
    assert!(
        all.windows(2).all(|w| w[0] != w[1]),
        "unchanged snapshots are not resent"
    );
    let n = all.len();
    // A request that changes nothing still runs a settle: no snapshot.
    engine.send(ui(UiRequest::DismissGap {
        instance_id: INSTANCE.into(),
        topic: "none".into(),
    }));
    engine.send(Command::ResendSnapshot);
    engine.wait_for("resend", |e| e.snapshots().len() > n).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let all = engine.snapshots();
    assert_eq!(all.len(), n + 1, "only the resend was sent");
    assert_eq!(all[n], all[n - 1], "resend repeats the current snapshot");
}

#[tokio::test]
async fn ui_mute_and_urgent_are_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let engine = TestEngine::start(&paths, settings_for(&unused_url()), State::default());
    engine.send(ui(UiRequest::SetTopicMuted {
        instance_id: INSTANCE.into(),
        topic: "alerts".into(),
        muted: true,
    }));
    engine.send(ui(UiRequest::SetTopicUrgent {
        instance_id: INSTANCE.into(),
        topic: "backup".into(),
        urgent: Some(true),
    }));
    engine.send(ui(UiRequest::SetUrgentGlobal { enabled: true }));
    engine
        .wait_for("settings.json", |e| {
            e.saved_settings().is_some_and(|s| {
                s.instances[0].topics[0].muted
                    && s.instances[0].topics[1].urgent_breaks_dnd == Some(true)
                    && s.urgent_breaks_dnd
            })
        })
        .await;
    engine
        .wait_for("snapshot", |e| {
            e.last_snapshot().is_some_and(|s| {
                let topics = &s.instances[0].topics;
                topics[0].muted && topics[1].urgent_breaks_dnd == Some(true) && s.urgent_breaks_dnd
            })
        })
        .await;
}

#[tokio::test]
async fn dismiss_gap_removes_flag() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let gap = |topic: &str| ntfy_native::state::GapFlag {
        instance_id: INSTANCE.into(),
        topic: topic.into(),
        detected_at: 1,
    };
    let state = State {
        gaps: vec![gap("alerts"), gap("backup")],
        ..State::default()
    };
    let engine = TestEngine::start(&paths, settings_for(&unused_url()), state);
    engine
        .wait_for("two gaps", |e| {
            e.last_snapshot().is_some_and(|s| s.gaps.len() == 2)
        })
        .await;
    engine.send(ui(UiRequest::DismissGap {
        instance_id: INSTANCE.into(),
        topic: "alerts".into(),
    }));
    engine
        .wait_for("one gap", |e| {
            e.last_snapshot()
                .is_some_and(|s| s.gaps.len() == 1 && s.gaps[0].topic == "backup")
        })
        .await;
    let engine = engine.quit().await;
    let state: State = load(&engine.paths.state_file()).unwrap();
    assert_eq!(state.gaps.len(), 1);
}

#[tokio::test]
async fn open_recent_uses_stored_click_only() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let recent = |id: &str, click: Option<&str>| ntfy_native::state::RecentMessage {
        instance_id: INSTANCE.into(),
        topic: "alerts".into(),
        id: id.into(),
        time: 1,
        title: String::new(),
        body: String::new(),
        click: click.map(str::to_owned),
    };
    let state = State {
        recent: [
            recent("a", Some("https://example.org/a")),
            recent("b", None),
            recent("c", Some("file:///etc/passwd")),
        ]
        .into(),
        ..State::default()
    };
    let engine = TestEngine::start(&paths, settings_for(&unused_url()), state);
    for id in ["unknown", "b", "c", "a"] {
        engine.send(ui(UiRequest::OpenRecent {
            instance_id: INSTANCE.into(),
            message_id: id.into(),
        }));
    }
    engine
        .wait_for("opener", |e| !e.opener.opened.lock().unwrap().is_empty())
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        *engine.opener.opened.lock().unwrap(),
        ["https://example.org/a"]
    );
}

#[tokio::test]
async fn test_notification_is_shown() {
    let dir = tempfile::tempdir().unwrap();
    let engine = TestEngine::start(
        &AppPaths::under(dir.path()),
        Settings::default(),
        State::default(),
    );
    engine.send(ui(UiRequest::SendTestNotification));
    engine
        .wait_for("test notification", |e| !e.shown().is_empty())
        .await;
    let n = &engine.shown()[0];
    assert_eq!(n.title, "ntfy-native");
    assert_eq!(n.body, "Notifications work.");
    assert_eq!(n.level, Level::Active);
}

fn draft(id: Option<&str>, url: &str, topics: &[&str]) -> InstanceDraft {
    InstanceDraft {
        id: id.map(str::to_owned),
        url: url.into(),
        auth: AuthDraft::None,
        cache_duration_secs: 43200,
        topics: topics
            .iter()
            .map(|name| Topic {
                name: (*name).into(),
                ..Topic::default()
            })
            .collect(),
    }
}

fn streams(server: &FakeServer) -> usize {
    server.requests().iter().filter(|r| !r.is_poll()).count()
}

#[tokio::test]
async fn save_new_instance_connects_live() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    let script = ntfy_script();
    script.push(Reply::lines(&[open(t), msg("m1", t)]).keep_open());
    let server = FakeServer::scripted(&script).await;
    let engine = TestEngine::start(&paths, Settings::default(), State::default());

    let id = engine
        .save(1, draft(None, &server.url(), &["alerts"]))
        .await
        .expect("saved");
    assert_eq!(id, "127.0.0.1");
    let saved = engine
        .saved_settings()
        .expect("settings.json written at once");
    assert_eq!(saved.instances.len(), 1);
    assert_eq!(saved.instances[0].id, id);

    let reqs = server.wait_requests(1).await;
    assert!(!reqs[0].is_poll(), "no history: straight to the stream");
    assert!(reqs[0].path.contains("alerts"));
    engine.wait_for("m1", |e| !e.shown().is_empty()).await;
    assert_eq!(engine.shown_ids(), ["127.0.0.1:m1"]);
    engine
        .wait_for("snapshot with the instance", |e| {
            e.last_snapshot()
                .is_some_and(|s| s.configured && s.instances[0].id == id)
        })
        .await;
}

#[tokio::test]
async fn edit_topics_keeps_state_no_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    // After the restart the catch-up poll replays x.
    let script = Script::with_fallback(|req: &Recorded| {
        if req.is_poll() {
            Reply::lines(&[msg("x", now())])
        } else {
            Reply::lines(&[open(now())]).keep_open()
        }
    });
    script.push(Reply::lines(&[open(t), msg("x", t)]).keep_open());
    let server = FakeServer::scripted(&script).await;
    let engine = TestEngine::start(&paths, settings_for(&server.url()), State::default());
    engine.wait_for("x", |e| !e.shown().is_empty()).await;

    let edited = draft(
        Some(INSTANCE),
        &server.url(),
        &["alerts", "backup", "extra"],
    );
    assert_eq!(engine.save(1, edited).await, Ok(INSTANCE.to_owned()));

    let reqs = server.wait_requests(3).await;
    let poll = &reqs[1];
    assert!(poll.is_poll(), "restart begins with a catch-up poll");
    assert!(poll.path.contains("extra"), "new topic list: {}", poll.path);
    assert_eq!(
        poll.query_param("since"),
        Some((t - 30).to_string().as_str()),
        "catch-up state kept"
    );
    assert!(!reqs[2].is_poll() && reqs[2].path.contains("extra"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(engine.shown_ids(), ["inst:x"]);
    assert_eq!(streams(&server), 2);
    let saved = engine.saved_settings().unwrap();
    assert_eq!(saved.instances[0].topics.len(), 3);
}

#[tokio::test]
async fn mute_only_change_does_not_restart() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let server = FakeServer::scripted(&ntfy_script()).await;
    let engine = TestEngine::start(&paths, settings_for(&server.url()), State::default());
    server.wait_requests(1).await;

    let mut edited = draft(Some(INSTANCE), &server.url(), &["alerts", "backup"]);
    edited.topics[0].muted = true;
    edited.topics[1].urgent_breaks_dnd = Some(true);
    assert_eq!(engine.save(1, edited).await, Ok(INSTANCE.to_owned()));
    let saved = engine.saved_settings().expect("settings.json written");
    assert!(saved.instances[0].topics[0].muted);
    assert_eq!(saved.instances[0].topics[1].urgent_breaks_dnd, Some(true));

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.requests().len(), 1, "no reconnect");
}

#[tokio::test]
async fn remove_instance_stops_connection_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    let script = ntfy_script();
    let (reply, stream) = Reply::stream();
    script.push(reply);
    let server = FakeServer::scripted(&script).await;
    let mut settings = settings_for(&server.url());
    settings.instances[0].auth = AuthKind::Token;
    let secrets = Arc::new(MemoryStore::new());
    secrets.set(&account_for(INSTANCE), "tok").unwrap();
    let state = State {
        gaps: vec![ntfy_native::state::GapFlag {
            instance_id: INSTANCE.into(),
            topic: "alerts".into(),
            detected_at: 1,
        }],
        ..State::default()
    };
    let engine = TestEngine::start_with(&paths, settings, state, secrets.clone());

    stream.line(&open(t));
    stream.line(&msg("m1", t));
    engine.wait_for("m1", |e| !e.shown().is_empty()).await;
    engine
        .wait_for("connected", |e| {
            e.last_snapshot().is_some_and(|s| {
                s.instances[0].status == InstanceStatus::Connected
                    && !s.recent.is_empty()
                    && !s.gaps.is_empty()
            })
        })
        .await;

    engine.send(ui(UiRequest::RemoveInstance {
        instance_id: INSTANCE.into(),
    }));
    engine
        .wait_for("empty snapshot", |e| {
            e.last_snapshot().is_some_and(|s| {
                !s.configured && s.instances.is_empty() && s.recent.is_empty() && s.gaps.is_empty()
            })
        })
        .await;
    let saved = engine.saved_settings().expect("settings.json written");
    assert!(saved.instances.is_empty());
    assert_eq!(secrets.get(&account_for(INSTANCE)).unwrap(), None);

    // The stopped connection no longer delivers.
    stream.line(&msg("late", t));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(engine.shown_ids(), ["inst:m1"]);
    assert_eq!(server.requests().len(), 1, "not restarted");

    let engine = engine.quit().await;
    let state: State = load(&engine.paths.state_file()).unwrap();
    assert!(state.instances.is_empty());
    assert!(state.recent.is_empty());
    assert!(state.gaps.is_empty());
}

#[tokio::test]
async fn invalid_save_returns_error_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let server = FakeServer::scripted(&ntfy_script()).await;
    let secrets = Arc::new(MemoryStore::new());
    let engine = TestEngine::start_with(
        &paths,
        settings_for(&server.url()),
        State::default(),
        secrets.clone(),
    );
    engine
        .wait_for("connected", |e| {
            e.last_snapshot()
                .is_some_and(|s| s.instances[0].status == InstanceStatus::Connected)
        })
        .await;
    let before = engine.last_snapshot().unwrap();

    let mut bad = draft(Some(INSTANCE), &server.url(), &["alerts", "s3cret x"]);
    bad.auth = AuthDraft::Token {
        token: Some("s3cret".into()),
    };
    let err = engine.save(1, bad).await.unwrap_err();
    assert_eq!(
        err,
        "topic 2: invalid topic name (allowed: letters, digits, '-' and '_', 1-64 characters)"
    );
    assert!(!err.contains("s3cret"));

    let unknown = draft(Some("nope"), &server.url(), &["alerts"]);
    assert_eq!(
        engine.save(2, unknown).await,
        Err("instance not found".into())
    );
    let duplicate = draft(None, &server.url(), &["other"]);
    assert_eq!(
        engine.save(3, duplicate).await,
        Err("an instance with this url already exists".into())
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.requests().len(), 1, "no reconnect");
    assert_eq!(secrets.get(&account_for(INSTANCE)).unwrap(), None);
    assert_eq!(engine.saved_settings(), None, "settings.json untouched");
    assert_eq!(engine.last_snapshot(), Some(before));
}

#[tokio::test]
async fn unwritable_settings_reject_save_and_keep_old_settings() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    // A directory in place of settings.json makes every write fail.
    std::fs::create_dir_all(paths.settings_file()).unwrap();
    let server = FakeServer::scripted(&ntfy_script()).await;
    let engine = TestEngine::start(&paths, Settings::default(), State::default());

    assert_eq!(
        engine
            .save(1, draft(None, &server.url(), &["alerts"]))
            .await,
        Err("settings could not be saved".into())
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(server.requests().is_empty(), "not connected");
    let snap = engine.last_snapshot().unwrap();
    assert!(!snap.configured && snap.instances.is_empty());
}

#[tokio::test]
async fn token_change_reconnects_with_new_token() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let server = FakeServer::scripted(&ntfy_script()).await;
    let mut settings = settings_for(&server.url());
    settings.instances[0].auth = AuthKind::Token;
    let secrets = Arc::new(MemoryStore::new());
    secrets.set(&account_for(INSTANCE), "old").unwrap();
    let engine = TestEngine::start_with(&paths, settings, State::default(), secrets.clone());
    let reqs = server.wait_requests(1).await;
    assert_eq!(reqs[0].header("authorization"), Some("Bearer old"));

    let mut edited = draft(Some(INSTANCE), &server.url(), &["alerts", "backup"]);
    edited.auth = AuthDraft::Token {
        token: Some("new".into()),
    };
    assert_eq!(engine.save(1, edited).await, Ok(INSTANCE.to_owned()));
    let reqs = server.wait_requests(2).await;
    assert_eq!(reqs[1].header("authorization"), Some("Bearer new"));
    assert_eq!(
        secrets.get(&account_for(INSTANCE)).unwrap().as_deref(),
        Some("new")
    );
}

#[tokio::test]
async fn url_change_starts_with_fresh_state() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(dir.path());
    let t = now();
    let old_script = ntfy_script();
    old_script.push(Reply::lines(&[open(t), msg("x", t)]).keep_open());
    let old = FakeServer::scripted(&old_script).await;
    let new = FakeServer::scripted(&ntfy_script()).await;
    let engine = TestEngine::start(&paths, settings_for(&old.url()), State::default());
    engine.wait_for("x", |e| !e.shown().is_empty()).await;

    let moved = draft(Some(INSTANCE), &new.url(), &["alerts", "backup"]);
    assert_eq!(engine.save(1, moved).await, Ok(INSTANCE.to_owned()));
    let reqs = new.wait_requests(1).await;
    assert!(!reqs[0].is_poll(), "no catch-up against the new server");
    assert_eq!(reqs[0].query_param("since"), None);
}
