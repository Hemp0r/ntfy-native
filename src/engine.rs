//! The engine: runs all connections, dispatches their messages to the
//! notifier, executes actions and persists settings and state.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::actions::{self, UrlOpener};
use crate::autostart::{Autostart, AutostartState};
use crate::connection::{ConnControl, ConnEvent, ConnStatus, ConnectionConfig, spawn_connection};
use crate::conntest::test_connection;
use crate::dispatcher::Dispatcher;
use crate::i18n::{Strings, fill};
use crate::instance_edit::{AuthDraft, InstanceDraft, apply_draft, remove_instance};
use crate::ipc::{CoreMsg, TestOutcome, UiRequest};
use crate::media::MediaFetcher;
use crate::notification::{ActivationPayload, Level, MediaRef, Notification, Notifier};
use crate::ntfy::client::{NetworkKind, http_client};
use crate::paths::AppPaths;
use crate::secrets::{Auth, SecretStore, resolve_auth};
use crate::settings::{Instance, Settings};
use crate::snapshot::{UiSnapshot, build_snapshot};
use crate::state::{InstanceState, State};
use crate::status::{TrayState, aggregate};
use crate::store::save_json;
use crate::triggers::{spawn_network_watcher, spawn_wake_detector};
use crate::validate::{same_origin, same_url};

const TICK: Duration = Duration::from_secs(5);
const MEDIA_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MEDIA_CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const STATE_VERSION: u32 = 1;
/// Upper bound for a whole connection test.
const TEST_DEADLINE: Duration = Duration::from_secs(15);
/// On quit, pending notifications are shown without media within this time.
const FLUSH_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum Command {
    Activated(ActivationPayload),
    ReconnectAll,
    PauseAll(Option<i64>),
    SetTopicMuted {
        instance_id: String,
        topic: String,
        muted: bool,
    },
    /// Request from the UI process.
    Ui(UiRequest),
    /// Sends the current snapshot even if unchanged (a UI just connected).
    ResendSnapshot,
    Quit,
}

pub struct EngineDeps {
    pub paths: AppPaths,
    pub settings: Settings,
    pub state: State,
    pub secrets: Arc<dyn SecretStore>,
    pub notifier: Arc<dyn Notifier>,
    pub opener: Arc<dyn UrlOpener>,
    pub strings: &'static Strings,
    /// Called on the engine thread; must not block (enqueue onto a channel).
    pub on_tray: Box<dyn Fn(TrayState) + Send>,
    pub on_exit: Box<dyn FnOnce() + Send>,
    /// Snapshots and test/save results for the UI process. Called on the
    /// engine thread; must not block (enqueue onto a channel).
    pub on_ui: Box<dyn Fn(CoreMsg) + Send>,
    /// Start at login; read at start and on every tick, changed on request.
    pub autostart: Arc<dyn Autostart>,
    /// Adjusts each connection's config; production: `|_| {}`.
    pub tune: fn(&mut ConnectionConfig),
    /// Wake/network reconnect triggers and Ctrl-C/SIGTERM handling.
    pub enable_triggers: bool,
}

#[derive(Clone)]
pub struct EngineHandle {
    pub tx: mpsc::UnboundedSender<Command>,
}

/// Runs the engine on a new thread with a tokio current-thread runtime.
pub fn spawn_engine_thread(deps: EngineDeps) -> (EngineHandle, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = EngineHandle { tx: tx.clone() };
    let thread = std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!(error = %e, "cannot start engine runtime");
                    (deps.on_exit)();
                    return;
                }
            };
            if let Err(e) = rt.block_on(run_engine(deps, tx, rx)) {
                tracing::error!(error = %format!("{e:#}"), "engine stopped with error");
            }
        })
        .expect("spawn engine thread");
    (handle, thread)
}

/// Runs until `Quit`, a shutdown signal (with `enable_triggers`) or until all
/// command senders are gone. Calls `on_exit` after the final save.
pub async fn run_engine(
    deps: EngineDeps,
    tx: mpsc::UnboundedSender<Command>,
    mut rx: mpsc::UnboundedReceiver<Command>,
) -> anyhow::Result<()> {
    // `stop_connection` relies on a single-threaded runtime.
    debug_assert_eq!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::CurrentThread
    );
    let EngineDeps {
        paths,
        settings,
        state,
        secrets,
        notifier,
        opener,
        strings,
        on_tray,
        on_exit,
        on_ui,
        autostart,
        tune,
        enable_triggers,
    } = deps;

    let http = http_client();
    let media = Arc::new(MediaFetcher::new(http.clone(), paths.media_dir()));

    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (test_tx, test_results) = mpsc::unbounded_channel();
    let (delivery_tx, delivery_rx) = mpsc::unbounded_channel();
    let stop_media = CancellationToken::new();
    let mut worker = tokio::spawn(delivery_worker(
        delivery_rx,
        media.clone(),
        notifier.clone(),
        stop_media.clone(),
    ));
    let mut engine = Engine {
        saved_settings: settings.clone(),
        dispatcher: Dispatcher::new(settings, state.recent, state.gaps, strings),
        instances: state.instances,
        connections: HashMap::new(),
        statuses: BTreeMap::new(),
        state_dirty: false,
        tray: None,
        settings_save: SaveHealth::default(),
        state_save: SaveHealth::default(),
        media,
        media_cleanup: Periodic::starting_now(MEDIA_CLEANUP_INTERVAL),
        paths,
        http,
        deliveries: Some(delivery_tx),
        notifier,
        opener,
        strings,
        on_tray,
        on_ui,
        autostart_state: autostart.state(),
        autostart,
        snapshot: None,
        secrets,
        tune,
        events_tx: ev_tx,
        events: ev_rx,
        test_tx,
        test_results,
        test_task: None,
    };
    engine.start_connections();

    let mut triggers = Vec::new();
    if enable_triggers {
        triggers.push(spawn_wake_detector(tx.clone()));
        triggers.push(spawn_network_watcher(tx.clone()));
    }
    // Without other senders, `rx` ends once every `EngineHandle` is dropped.
    drop(tx);

    let shutdown = async {
        if enable_triggers {
            shutdown_signal().await
        } else {
            std::future::pending().await
        }
    };
    tokio::pin!(shutdown);
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    engine.settle(unix_now());
    loop {
        // Connection events first: a connection sends `Messages` and its
        // `State` back to back, so a `Quit` is never handled between them.
        // Event handling never awaits (media and display run in the delivery
        // worker), so a command waits at most for the events already queued.
        tokio::select! {
            biased;
            Some((id, ev)) = engine.events.recv() => engine.on_conn_event(&id, ev),
            Some((request_id, outcome)) = engine.test_results.recv() => {
                (engine.on_ui)(CoreMsg::TestResult { request_id, outcome });
            }
            cmd = rx.recv() => {
                if engine.on_command(cmd).is_break() {
                    break;
                }
            }
            _ = ticker.tick() => engine.on_tick(),
            _ = &mut shutdown => break,
        }
        engine.settle(unix_now());
    }

    for t in triggers {
        t.abort();
    }
    engine.stop_connections();
    // Closing the queue lets the worker finish; pending items skip media.
    engine.deliveries = None;
    stop_media.cancel();
    if tokio::time::timeout(FLUSH_DEADLINE, &mut worker)
        .await
        .is_err()
    {
        worker.abort();
        tracing::warn!("notifications still pending at exit were dropped");
    }
    let result = engine.save_all();
    on_exit();
    result
}

/// A notification queued for media download and display.
struct Delivery {
    n: Notification,
    base_url: String,
    auth: Option<Auth>,
}

/// Shows queued notifications in order, after fetching their media. Once
/// `stop_media` fires, remaining and in-flight items are shown without
/// remote media.
async fn delivery_worker(
    mut rx: mpsc::UnboundedReceiver<Delivery>,
    media: Arc<MediaFetcher>,
    notifier: Arc<dyn Notifier>,
    stop_media: CancellationToken,
) {
    while let Some(d) = rx.recv().await {
        let n = tokio::select! {
            biased;
            _ = stop_media.cancelled() => without_remote_media(d.n),
            n = media.resolve(d.n.clone(), &d.base_url, d.auth.as_ref()) => n,
        };
        show(notifier.as_ref(), &n);
    }
}

fn without_remote_media(mut n: Notification) -> Notification {
    let local = |m: Option<MediaRef>| m.filter(|m| matches!(m, MediaRef::Local(_)));
    n.image = local(n.image.take());
    n.icon = local(n.icon.take());
    n
}

struct Running {
    ctl: ConnControl,
    base_url: String,
    auth: Option<Auth>,
    /// `None` once its unexpected end has been reported.
    task: Option<JoinHandle<()>>,
}

/// A connection task only ends when cancelled or when the engine is gone, so
/// one that finished while the engine runs has panicked. It is reported once
/// as disconnected (the tray then shows a connection problem) and not
/// restarted: a panic caused by server data would likely repeat.
fn reap_crashed_connections(
    connections: &mut HashMap<String, Running>,
    statuses: &mut BTreeMap<String, (ConnStatus, i64)>,
    now: i64,
) {
    for (id, r) in connections.iter_mut() {
        if r.ctl.cancel.is_cancelled() || !r.task.as_ref().is_some_and(|t| t.is_finished()) {
            continue;
        }
        r.task = None;
        tracing::error!(instance = %id, "connection task stopped unexpectedly");
        let error = "connection task panicked".to_owned();
        track_status(statuses, id, ConnStatus::Disconnected { error }, now);
    }
}

/// Fires once per `every`, starting immediately.
struct Periodic {
    next: Instant,
    every: Duration,
}

impl Periodic {
    fn starting_now(every: Duration) -> Self {
        Self {
            next: Instant::now(),
            every,
        }
    }

    fn due(&mut self, now: Instant) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + self.every;
        true
    }
}

struct Engine {
    paths: AppPaths,
    http: reqwest::Client,
    /// `None` once closed for shutdown.
    deliveries: Option<mpsc::UnboundedSender<Delivery>>,
    notifier: Arc<dyn Notifier>,
    opener: Arc<dyn UrlOpener>,
    strings: &'static Strings,
    on_tray: Box<dyn Fn(TrayState) + Send>,
    on_ui: Box<dyn Fn(CoreMsg) + Send>,
    autostart: Arc<dyn Autostart>,
    autostart_state: AutostartState,
    /// Last snapshot sent to the UI.
    snapshot: Option<UiSnapshot>,
    dispatcher: Dispatcher,
    /// Last successfully written `settings.json` content.
    saved_settings: Settings,
    instances: BTreeMap<String, InstanceState>,
    connections: HashMap<String, Running>,
    statuses: BTreeMap<String, (ConnStatus, i64)>,
    state_dirty: bool,
    tray: Option<TrayState>,
    settings_save: SaveHealth,
    state_save: SaveHealth,
    media: Arc<MediaFetcher>,
    media_cleanup: Periodic,
    secrets: Arc<dyn SecretStore>,
    tune: fn(&mut ConnectionConfig),
    events_tx: mpsc::UnboundedSender<(String, ConnEvent)>,
    events: mpsc::UnboundedReceiver<(String, ConnEvent)>,
    /// Outcomes of connection tests running in their own tasks.
    test_tx: mpsc::UnboundedSender<(u64, TestOutcome)>,
    test_results: mpsc::UnboundedReceiver<(u64, TestOutcome)>,
    /// The running connection test; a new request replaces it.
    test_task: Option<tokio::task::AbortHandle>,
}

impl Engine {
    fn start_connections(&mut self) {
        let instances = self.dispatcher.settings.instances.clone();
        for inst in &instances {
            self.start_connection(inst);
        }
    }

    /// Starts the connection with the stored `InstanceState` (default if none).
    fn start_connection(&mut self, inst: &Instance) {
        if inst.topics.is_empty() {
            return;
        }
        let auth = match resolve_auth(self.secrets.as_ref(), inst) {
            Ok(auth) => auth,
            Err(e) => {
                tracing::error!(instance = %inst.id, error = %format!("{e:#}"), "cannot load credentials");
                self.statuses
                    .insert(inst.id.clone(), (ConnStatus::AuthFailed, unix_now()));
                return;
            }
        };
        let mut cfg = ConnectionConfig::production(inst, auth.clone());
        (self.tune)(&mut cfg);
        let ctl = ConnControl::default();
        let state = self.instances.get(&inst.id).cloned().unwrap_or_default();
        let task = spawn_connection(
            cfg,
            state,
            self.http.clone(),
            self.events_tx.clone(),
            ctl.clone(),
        );
        self.connections.insert(
            inst.id.clone(),
            Running {
                ctl,
                base_url: inst.url.clone(),
                auth,
                task: Some(task),
            },
        );
    }

    /// Cancels the instance's connection and applies the events it already
    /// queued (all instances'), so its `InstanceState` is current and no
    /// stale event follows.
    /// Relies on the current-thread runtime: a cancelled connection task
    /// cannot run before it is next polled, and then stops at its first
    /// await without emitting anything.
    fn stop_connection(&mut self, id: &str) {
        if let Some(r) = self.connections.get(id) {
            r.ctl.cancel.cancel();
        }
        while let Ok((ev_id, ev)) = self.events.try_recv() {
            self.on_conn_event(&ev_id, ev);
        }
        self.connections.remove(id);
        self.statuses.remove(id);
    }

    fn save_instance(&mut self, draft: &InstanceDraft) -> Result<String, String> {
        let (settings, id, secret_change) =
            match apply_draft(&self.dispatcher.settings, draft, self.secrets.as_ref()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "instance not saved");
                    return Err(e.to_string());
                }
            };
        let old = self.dispatcher.settings.instance(&id).cloned();
        let previous = std::mem::replace(&mut self.dispatcher.settings, settings);
        if self.write_settings().is_err() {
            // The secret store may already hold a newly entered secret for an
            // instance that is not saved: accepted, the next successful save
            // or entering it again fixes it.
            tracing::error!(instance = %id, "instance not saved: settings.json could not be written");
            self.dispatcher.settings = previous;
            return Err("settings could not be saved".into());
        }
        if let Err(e) = secret_change.commit(self.secrets.as_ref()) {
            tracing::warn!(instance = %id, error = %format!("{e:#}"), "stale secret not deleted");
        }
        let new = self
            .dispatcher
            .settings
            .instance(&id)
            .cloned()
            .expect("apply_draft returns the id of an instance it contains");
        let secret_entered = matches!(
            draft.auth,
            AuthDraft::Token { token: Some(_) }
                | AuthDraft::Basic {
                    password: Some(_),
                    ..
                }
        );
        self.reconcile(old.as_ref(), &new, secret_entered);
        tracing::info!(instance = %id, "instance saved");
        Ok(id)
    }

    /// Restarts the connection if anything it uses changed (or none runs).
    /// The `InstanceState` survives unless the server changed.
    fn reconcile(&mut self, old: Option<&Instance>, new: &Instance, secret_entered: bool) {
        let same_server = old.is_some_and(|o| same_url(&o.url, &new.url));
        let unchanged = same_server
            && old.is_some_and(|o| {
                o.auth == new.auth
                    && o.cache_duration_secs == new.cache_duration_secs
                    && topic_names(o) == topic_names(new)
            })
            && !secret_entered;
        let running = self
            .connections
            .get(&new.id)
            .is_some_and(|r| r.task.is_some());
        let gaps = self.dispatcher.gaps.len();
        self.dispatcher
            .gaps
            .retain(|g| g.instance_id != new.id || new.topics.iter().any(|t| t.name == g.topic));
        self.state_dirty |= self.dispatcher.gaps.len() != gaps;
        if unchanged && running {
            return;
        }
        self.stop_connection(&new.id);
        if !same_server {
            self.instances.remove(&new.id);
            self.dispatcher.gaps.retain(|g| g.instance_id != new.id);
        }
        self.state_dirty = true;
        self.start_connection(new);
    }

    fn remove_instance(&mut self, id: &str) {
        let (settings, secret_change) = match remove_instance(&self.dispatcher.settings, id) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(instance = %id, error = %format!("{e:#}"), "instance not removed");
                return;
            }
        };
        let previous = std::mem::replace(&mut self.dispatcher.settings, settings);
        if self.write_settings().is_err() {
            tracing::error!(instance = %id, "instance not removed: settings.json could not be written");
            self.dispatcher.settings = previous;
            return;
        }
        if let Err(e) = secret_change.commit(self.secrets.as_ref()) {
            tracing::warn!(instance = %id, error = %format!("{e:#}"), "stale secret not deleted");
        }
        self.stop_connection(id);
        self.instances.remove(id);
        self.dispatcher.gaps.retain(|g| g.instance_id != id);
        self.dispatcher.recent.retain(|m| m.instance_id != id);
        self.state_dirty = true;
        tracing::info!(instance = %id, "instance removed");
    }

    /// Writes `settings.json` from the current settings.
    fn write_settings(&mut self) -> anyhow::Result<()> {
        let result = save_json(&self.paths.settings_file(), &self.dispatcher.settings);
        if result.is_ok() {
            self.saved_settings = self.dispatcher.settings.clone();
        }
        self.settings_save.record("settings", &result);
        result
    }

    /// Notifications are queued before the connection's following `State`
    /// event is applied. Events still queued from a removed instance are
    /// dropped.
    fn on_conn_event(&mut self, id: &str, ev: ConnEvent) {
        if self.dispatcher.settings.instance(id).is_none() {
            return;
        }
        let now = unix_now();
        match ev {
            ConnEvent::Status(status) => track_status(&mut self.statuses, id, status, now),
            ConnEvent::Messages { messages, catch_up } => {
                let notifications = self.dispatcher.on_messages(id, &messages, catch_up, now);
                self.state_dirty = true;
                for n in notifications {
                    self.enqueue(id, n);
                }
            }
            ConnEvent::Gap => {
                self.state_dirty = true;
                if let Some(n) = self.dispatcher.on_gap(id, now) {
                    self.enqueue(id, n);
                }
            }
            ConnEvent::State(state) => {
                self.instances.insert(id.to_owned(), state);
                self.state_dirty = true;
            }
        }
    }

    fn enqueue(&self, id: &str, n: Notification) {
        let (base_url, auth) = self
            .connections
            .get(id)
            .map(|r| (r.base_url.clone(), r.auth.clone()))
            .unwrap_or_default();
        let sent = self
            .deliveries
            .as_ref()
            .is_some_and(|tx| tx.send(Delivery { n, base_url, auth }).is_ok());
        if !sent {
            tracing::error!("notification worker stopped");
        }
    }

    fn on_command(&mut self, cmd: Option<Command>) -> ControlFlow<()> {
        match cmd {
            None | Some(Command::Quit) => return ControlFlow::Break(()),
            Some(Command::Activated(payload)) => self.spawn_action(payload),
            Some(Command::ReconnectAll) => {
                tracing::info!("reconnecting all instances");
                for r in self.connections.values() {
                    r.ctl.reconnect.notify_one();
                }
            }
            Some(Command::PauseAll(until)) => self.dispatcher.pause_all(until),
            Some(Command::SetTopicMuted {
                instance_id,
                topic,
                muted,
            }) => {
                self.dispatcher.set_topic_muted(&instance_id, &topic, muted);
            }
            Some(Command::Ui(req)) => self.on_ui_request(req),
            Some(Command::ResendSnapshot) => {
                let snap = self.build_snapshot(unix_now());
                (self.on_ui)(CoreMsg::Snapshot(snap.clone()));
                self.snapshot = Some(snap);
            }
        }
        ControlFlow::Continue(())
    }

    fn on_ui_request(&mut self, req: UiRequest) {
        match req {
            UiRequest::PauseAll { until } => self.dispatcher.pause_all(until),
            UiRequest::SetTopicMuted {
                instance_id,
                topic,
                muted,
            } => {
                self.dispatcher.set_topic_muted(&instance_id, &topic, muted);
            }
            UiRequest::SetTopicUrgent {
                instance_id,
                topic,
                urgent,
            } => {
                self.dispatcher
                    .set_topic_urgent(&instance_id, &topic, urgent);
            }
            UiRequest::SetUrgentGlobal { enabled } => {
                self.dispatcher.set_urgent_global(enabled);
            }
            UiRequest::DismissGap { instance_id, topic } => {
                if self.dispatcher.dismiss_gap(&instance_id, &topic) {
                    self.state_dirty = true;
                }
            }
            UiRequest::OpenRecent {
                instance_id,
                message_id,
            } => {
                let click = self
                    .dispatcher
                    .recent
                    .iter()
                    .find(|m| m.instance_id == instance_id && m.id == message_id)
                    .and_then(|m| m.click.clone());
                match click {
                    Some(url) if actions::is_openable(&url) => {
                        self.spawn_action(ActivationPayload::Open { url });
                    }
                    _ => tracing::debug!("recent message has no openable click URL"),
                }
            }
            UiRequest::SendTestNotification => self.enqueue_test_notification(),
            UiRequest::SaveInstance { request_id, draft } => {
                let result = self.save_instance(&draft);
                (self.on_ui)(CoreMsg::SaveResult { request_id, result });
            }
            UiRequest::RemoveInstance { instance_id } => self.remove_instance(&instance_id),
            UiRequest::SetAutostart {
                request_id,
                enabled,
            } => {
                let result = match self.autostart.set(enabled) {
                    Ok(state) => {
                        self.autostart_state = state;
                        Ok(state)
                    }
                    Err(e) => {
                        tracing::warn!(error = %format!("{e:#}"), "changing autostart failed");
                        self.autostart_state = self.autostart.state();
                        Err(autostart_error_text(&e, home_dir().as_deref()))
                    }
                };
                (self.on_ui)(CoreMsg::AutostartResult { request_id, result });
            }
            UiRequest::TestConnection { request_id, draft } => {
                self.spawn_connection_test(request_id, draft);
            }
            other => tracing::debug!(request = ?other, "UI request not handled by the engine"),
        }
    }

    /// Runs off the event loop: a hanging server must not delay anything.
    fn spawn_connection_test(&mut self, request_id: u64, mut draft: InstanceDraft) {
        if let Some(previous) = self.test_task.take() {
            previous.abort();
        }
        if keeps_secret(&draft.auth) && self.moved_instance(&draft) {
            // The stored secret belongs to the old address; test without one.
            draft.auth = AuthDraft::None;
        }
        let stored = self.stored_auth_for_test(&draft);
        let http = self.http.clone();
        let results = self.test_tx.clone();
        let task = tokio::spawn(async move {
            let outcome =
                tokio::time::timeout(TEST_DEADLINE, test_connection(&http, &draft, stored))
                    .await
                    .unwrap_or_else(|_| TestOutcome::Unreachable {
                        message: NetworkKind::Timeout.label().into(),
                    });
            let _ = results.send((request_id, outcome));
        });
        self.test_task = Some(task.abort_handle());
    }

    /// The draft edits an instance whose stored URL has another origin.
    fn moved_instance(&self, draft: &InstanceDraft) -> bool {
        draft
            .id
            .as_deref()
            .and_then(|id| self.dispatcher.settings.instance(id))
            .is_some_and(|inst| !same_origin(&inst.url, &draft.url))
    }

    /// The saved credential of the edited instance, for drafts that keep it
    /// on the same origin (scheme, host, port).
    fn stored_auth_for_test(&self, draft: &InstanceDraft) -> Option<Auth> {
        let inst = self
            .dispatcher
            .settings
            .instance(draft.id.as_deref()?)
            .filter(|i| keeps_secret(&draft.auth) && same_origin(&i.url, &draft.url))?;
        match resolve_auth(self.secrets.as_ref(), inst) {
            Ok(auth) => auth,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "stored credential unavailable for connection test");
                None
            }
        }
    }

    fn enqueue_test_notification(&self) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        self.enqueue(
            "",
            Notification {
                id: format!("test:{stamp}"),
                thread: "test".into(),
                title: self.strings.test_title.to_owned(),
                body: self.strings.test_body.to_owned(),
                level: Level::Active,
                default_action: None,
                buttons: Vec::new(),
                image: None,
                icon: None,
            },
        );
    }

    fn build_snapshot(&self, now: i64) -> UiSnapshot {
        build_snapshot(
            &self.dispatcher.settings,
            &self.statuses,
            &self.dispatcher.recent,
            &self.dispatcher.gaps,
            self.autostart_state,
            now,
        )
    }

    /// Runs the action off the event loop so a slow HTTP action cannot delay
    /// message delivery.
    fn spawn_action(&self, payload: ActivationPayload) {
        let http = self.http.clone();
        let opener = self.opener.clone();
        let notifier = self.notifier.clone();
        let strings = self.strings;
        tokio::spawn(async move {
            if let Err(err) = actions::perform(&http, &payload, opener.as_ref()).await {
                tracing::warn!(error = %format!("{err:#}"), "action failed");
                show(
                    notifier.as_ref(),
                    &action_failed(&payload, &err.to_string(), strings),
                );
            }
        });
    }

    fn on_tick(&mut self) {
        self.autostart_state = self.autostart.state();
        reap_crashed_connections(&mut self.connections, &mut self.statuses, unix_now());
        if self.media_cleanup.due(Instant::now()) {
            match self.media.cleanup(MEDIA_MAX_AGE) {
                Ok(n) if n > 0 => tracing::debug!(removed = n, "media cache cleaned"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "media cache cleanup failed"),
            }
        }
        if self.state_dirty {
            let result = save_json(&self.paths.state_file(), &self.state());
            self.state_dirty = result.is_err();
            self.state_save.record("state", &result);
        }
    }

    /// Clears an expired pause, persists changed settings and reports a changed
    /// tray state.
    fn settle(&mut self, now: i64) {
        let paused = self.dispatcher.is_paused(now);
        if self.dispatcher.settings != self.saved_settings {
            let _ = self.write_settings();
        }
        let tray = aggregate(&self.dispatcher.settings, &self.statuses, paused, now);
        if self.tray.as_ref() != Some(&tray) {
            (self.on_tray)(tray.clone());
            self.tray = Some(tray);
        }
        let snap = self.build_snapshot(now);
        if self.snapshot.as_ref() != Some(&snap) {
            (self.on_ui)(CoreMsg::Snapshot(snap.clone()));
            self.snapshot = Some(snap);
        }
    }

    fn state(&self) -> State {
        State {
            version: STATE_VERSION,
            instances: self.instances.clone(),
            recent: self.dispatcher.recent.clone(),
            gaps: self.dispatcher.gaps.clone(),
        }
    }

    fn stop_connections(&self) {
        for r in self.connections.values() {
            r.ctl.cancel.cancel();
        }
    }

    /// Final write of settings and state.
    fn save_all(&self) -> anyhow::Result<()> {
        let settings = save_json(&self.paths.settings_file(), &self.dispatcher.settings);
        let state = save_json(&self.paths.state_file(), &self.state());
        for e in [&settings, &state]
            .into_iter()
            .filter_map(|r| r.as_ref().err())
        {
            tracing::error!(error = %format!("{e:#}"), "saving on exit failed");
        }
        settings.and(state)
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// The full error chain for the UI; an error that mentions the user's
/// home directory becomes a generic message (details stay in the log).
fn autostart_error_text(e: &anyhow::Error, home: Option<&std::path::Path>) -> String {
    let text = format!("{e:#}");
    let leaks_home = home
        .and_then(|h| h.to_str())
        .is_some_and(|h| !h.is_empty() && text.contains(h));
    if leaks_home {
        "autostart could not be changed".to_owned()
    } else {
        text
    }
}

/// Logs a failing save once per failure streak instead of on every attempt.
#[derive(Default)]
struct SaveHealth {
    failing: bool,
}

impl SaveHealth {
    /// Returns true if the result started or ended a failure streak.
    fn record(&mut self, what: &str, result: &anyhow::Result<()>) -> bool {
        match result {
            Err(e) if !self.failing => {
                tracing::warn!(error = %format!("{e:#}"), "saving {what} failed");
            }
            Ok(()) if self.failing => tracing::info!("saving {what} works again"),
            _ => return false,
        }
        self.failing = result.is_err();
        true
    }
}

fn topic_names(inst: &Instance) -> BTreeSet<&str> {
    inst.topics.iter().map(|t| t.name.as_str()).collect()
}

/// Records a connection status. `since` marks when the instance entered its
/// condition (connected, without connection, auth failed): retries after a
/// failure keep the failure and its time, so the tray's grace period can run
/// out and `AuthFailed` does not flicker during auth retries.
fn track_status(
    statuses: &mut BTreeMap<String, (ConnStatus, i64)>,
    id: &str,
    status: ConnStatus,
    now: i64,
) {
    fn condition(s: &ConnStatus) -> u8 {
        match s {
            ConnStatus::Connected => 0,
            ConnStatus::Connecting | ConnStatus::Disconnected { .. } => 1,
            ConnStatus::AuthFailed => 2,
        }
    }
    match statuses.get_mut(id) {
        Some((old, _))
            if status == ConnStatus::Connecting && !matches!(old, ConnStatus::Connected) => {}
        Some((old, _)) if condition(old) == condition(&status) => *old = status,
        _ => {
            statuses.insert(id.to_owned(), (status, now));
        }
    }
}

/// Never panics: a panicking notifier must not stop further deliveries.
fn show(notifier: &dyn Notifier, n: &Notification) {
    tracing::debug!(id = %n.id, title = %n.title, "showing notification");
    match catch_unwind(AssertUnwindSafe(|| notifier.show(n))) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(id = %n.id, error = %format!("{e:#}"), "showing notification failed")
        }
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            tracing::error!(id = %n.id, panic = %msg, "notifier panicked");
        }
    }
}

fn action_failed(payload: &ActivationPayload, error: &str, strings: &Strings) -> Notification {
    let label = match payload {
        ActivationPayload::Http { label, .. } => label.clone(),
        ActivationPayload::Open { url } => url::Url::parse(url)
            .map(|u| u.scheme().to_owned())
            .unwrap_or_else(|_| "link".into()),
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Notification {
        id: format!("action-failed:{stamp}"),
        thread: "action-failed".into(),
        title: fill(strings.action_failed_title, &[("label", &label)]),
        body: error.to_owned(),
        level: Level::Passive,
        default_action: None,
        buttons: Vec::new(),
        image: None,
        icon: None,
    }
}

/// Resolves on Ctrl-C or (Unix) SIGTERM; never if no handler can be installed.
async fn shutdown_signal() {
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    let interrupt = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "cannot listen for Ctrl-C");
            std::future::pending::<()>().await
        }
    };
    tokio::select! {
        _ = terminate => tracing::info!("SIGTERM received, quitting"),
        _ = interrupt => tracing::info!("Ctrl-C received, quitting"),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The draft keeps the stored token or password (its secret is `None`).
fn keeps_secret(auth: &AuthDraft) -> bool {
    matches!(
        auth,
        AuthDraft::Token { token: None } | AuthDraft::Basic { password: None, .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;
    use crate::settings::{Instance, Topic};

    fn disconnected() -> ConnStatus {
        ConnStatus::Disconnected {
            error: "refused".into(),
        }
    }

    #[test]
    fn retries_keep_the_time_the_connection_was_lost() {
        let mut st = BTreeMap::new();
        track_status(&mut st, "a", ConnStatus::Connecting, 0);
        track_status(&mut st, "a", disconnected(), 10);
        track_status(&mut st, "a", ConnStatus::Connecting, 12);
        track_status(&mut st, "a", disconnected(), 20);
        assert_eq!(st["a"], (disconnected(), 0));

        let settings = Settings {
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
        };
        assert!(matches!(
            aggregate(&settings, &st, false, 20),
            TrayState::Problem { .. }
        ));

        track_status(&mut st, "a", ConnStatus::Connected, 30);
        track_status(&mut st, "a", ConnStatus::Connected, 40);
        assert_eq!(st["a"], (ConnStatus::Connected, 30));
        track_status(&mut st, "a", ConnStatus::Connecting, 100);
        assert_eq!(st["a"], (ConnStatus::Connecting, 100));
    }

    #[test]
    fn auth_retry_keeps_auth_failed() {
        let mut st = BTreeMap::new();
        track_status(&mut st, "a", ConnStatus::AuthFailed, 200);
        track_status(&mut st, "a", ConnStatus::Connecting, 260);
        assert_eq!(st["a"], (ConnStatus::AuthFailed, 200));
        track_status(&mut st, "a", disconnected(), 261);
        assert_eq!(st["a"], (disconnected(), 261));
    }

    #[test]
    fn save_failures_logged_once_per_streak() {
        let mut h = SaveHealth::default();
        let failed = || Err(anyhow::anyhow!("disk full"));
        assert!(!h.record("x", &Ok(())));
        assert!(h.record("x", &failed()), "first failure logged");
        assert!(!h.record("x", &failed()), "repeat is quiet");
        assert!(h.record("x", &Ok(())), "recovery logged");
        assert!(!h.record("x", &Ok(())));
        assert!(h.record("x", &failed()), "new streak logged");
    }

    #[test]
    fn remote_media_stripped_local_kept() {
        let n = Notification {
            id: "i".into(),
            thread: "t".into(),
            title: String::new(),
            body: String::new(),
            level: Level::Active,
            default_action: None,
            buttons: Vec::new(),
            image: Some(MediaRef::Remote("https://x/a.png".into())),
            icon: Some(MediaRef::Local("/tmp/i.png".into())),
        };
        let n = without_remote_media(n);
        assert_eq!(n.image, None);
        assert_eq!(n.icon, Some(MediaRef::Local("/tmp/i.png".into())));
    }

    fn plain(id: &str) -> Notification {
        Notification {
            id: id.into(),
            thread: "t".into(),
            title: String::new(),
            body: String::new(),
            level: Level::Active,
            default_action: None,
            buttons: Vec::new(),
            image: None,
            icon: None,
        }
    }

    /// Panics on the first notification, records the rest.
    #[derive(Default)]
    struct PanicOnce {
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl Notifier for PanicOnce {
        fn show(&self, n: &Notification) -> anyhow::Result<()> {
            let first = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(n.id.clone());
                calls.len() == 1
            };
            assert!(!first, "notifier exploded");
            Ok(())
        }
    }

    #[tokio::test]
    async fn panicking_notifier_does_not_stop_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let media = Arc::new(MediaFetcher::new(http_client(), dir.path().into()));
        let notifier = Arc::new(PanicOnce::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(delivery_worker(
            rx,
            media,
            notifier.clone(),
            CancellationToken::new(),
        ));
        for id in ["a", "b"] {
            let n = plain(id);
            tx.send(Delivery {
                n,
                base_url: String::new(),
                auth: None,
            })
            .unwrap();
        }
        drop(tx);
        worker.await.expect("worker survives the panic");
        assert_eq!(*notifier.calls.lock().unwrap(), ["a", "b"]);
    }

    #[tokio::test]
    async fn crashed_connection_reported_once() {
        let running = |task| Running {
            ctl: ConnControl::default(),
            base_url: String::new(),
            auth: None,
            task: Some(task),
        };
        let crashed = tokio::spawn(async { panic!("connection bug") });
        let alive = tokio::spawn(std::future::pending::<()>());
        let stopped = running(tokio::spawn(async {}));
        stopped.ctl.cancel.cancel();
        let mut conns = HashMap::from([
            ("crashed".to_owned(), running(crashed)),
            ("alive".to_owned(), running(alive)),
            ("stopped".to_owned(), stopped),
        ]);
        let finished = |c: &HashMap<String, Running>, id: &str| {
            c[id].task.as_ref().is_some_and(|t| t.is_finished())
        };
        while !(finished(&conns, "crashed") && finished(&conns, "stopped")) {
            tokio::task::yield_now().await;
        }
        let mut st = BTreeMap::from([("crashed".to_owned(), (ConnStatus::Connected, 5))]);
        reap_crashed_connections(&mut conns, &mut st, 10);
        assert!(matches!(
            st["crashed"],
            (ConnStatus::Disconnected { .. }, 10)
        ));
        assert_eq!(st.len(), 1, "alive and cancelled connections untouched");
        assert!(conns["crashed"].task.is_none());

        st.insert("crashed".into(), (ConnStatus::Connected, 20));
        reap_crashed_connections(&mut conns, &mut st, 30);
        assert_eq!(st["crashed"], (ConnStatus::Connected, 20), "reported once");
    }

    #[tokio::test]
    async fn reconcile_applies_queued_events_before_restart() {
        use crate::ntfy::model::parse_event;
        use crate::state::SeenMessage;

        let dir = tempfile::tempdir().unwrap();
        let topic = |n: &str| Topic {
            name: n.into(),
            ..Topic::default()
        };
        let inst = Instance {
            id: "a".into(),
            url: "https://ntfy.example.org".into(),
            topics: vec![topic("t")],
            ..Instance::default()
        };
        let settings = Settings {
            instances: vec![inst.clone()],
            ..Settings::default()
        };
        let notifier = Arc::new(crate::notification::RecordingNotifier::default());
        let media = Arc::new(MediaFetcher::new(http_client(), dir.path().into()));
        let (delivery_tx, delivery_rx) = mpsc::unbounded_channel();
        let worker = tokio::spawn(delivery_worker(
            delivery_rx,
            media.clone(),
            notifier.clone(),
            CancellationToken::new(),
        ));
        let (events_tx, events) = mpsc::unbounded_channel();
        let (test_tx, test_results) = mpsc::unbounded_channel();
        let mut engine = Engine {
            paths: AppPaths::under(dir.path()),
            http: http_client(),
            deliveries: Some(delivery_tx),
            notifier: notifier.clone(),
            opener: Arc::new(crate::actions::RecordingOpener::default()),
            strings: &EN,
            on_tray: Box::new(|_| {}),
            on_ui: Box::new(|_| {}),
            autostart: Arc::new(crate::autostart::MemoryAutostart::new(
                AutostartState::Disabled,
            )),
            autostart_state: AutostartState::Disabled,
            snapshot: None,
            dispatcher: Dispatcher::new(settings.clone(), Default::default(), Vec::new(), &EN),
            saved_settings: settings,
            instances: BTreeMap::new(),
            connections: HashMap::from([(
                "a".to_owned(),
                Running {
                    ctl: ConnControl::default(),
                    base_url: inst.url.clone(),
                    auth: None,
                    task: Some(tokio::spawn(std::future::pending::<()>())),
                },
            )]),
            statuses: BTreeMap::new(),
            state_dirty: false,
            tray: None,
            settings_save: SaveHealth::default(),
            state_save: SaveHealth::default(),
            media,
            media_cleanup: Periodic::starting_now(MEDIA_CLEANUP_INTERVAL),
            secrets: Arc::new(crate::secrets::MemoryStore::new()),
            tune: |_| {},
            events_tx,
            events,
            test_tx,
            test_results,
            test_task: None,
        };

        let message = match parse_event(
            r#"{"id":"m1","time":100,"event":"message","topic":"t","message":"hi"}"#,
        )
        .unwrap()
        {
            crate::ntfy::model::Event::Message(m) => m,
            other => panic!("{other:?}"),
        };
        let new_state = InstanceState {
            watermark: Some(100),
            last_message: Some(SeenMessage {
                id: "m1".into(),
                time: 100,
                expires: None,
            }),
            seen_ids: ["m1".to_owned()].into(),
        };
        let send = |ev| engine.events_tx.send(("a".to_owned(), ev)).unwrap();
        send(ConnEvent::Messages {
            messages: vec![message],
            catch_up: false,
        });
        send(ConnEvent::State(new_state.clone()));

        // A topic change restarts the connection (no topics: nothing starts).
        let edited = Instance {
            topics: Vec::new(),
            ..inst.clone()
        };
        engine.reconcile(Some(&inst), &edited, false);

        assert_eq!(engine.instances["a"], new_state);
        assert!(engine.events.try_recv().is_err(), "queue drained");
        assert!(engine.connections.is_empty());
        engine.deliveries = None;
        worker.await.unwrap();
        let shown = notifier.shown.lock().unwrap();
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0].id, "a:m1");
    }

    #[test]
    fn periodic_fires_immediately_then_every_interval() {
        let hour = Duration::from_secs(3600);
        let mut p = Periodic::starting_now(hour);
        let t0 = Instant::now();
        assert!(p.due(t0));
        assert!(!p.due(t0 + hour - Duration::from_secs(1)));
        assert!(p.due(t0 + hour));
        assert!(!p.due(t0 + hour + Duration::from_secs(1)));
    }

    #[test]
    fn action_failed_label_and_body() {
        let http = ActivationPayload::Http {
            label: "Restart".into(),
            method: "POST".into(),
            url: "https://h/x?token=s".into(),
            headers: Default::default(),
            body: None,
        };
        let n = action_failed(&http, "HTTP action returned status 500", &EN);
        assert_eq!(n.title, "Action failed: Restart");
        assert_eq!(n.body, "HTTP action returned status 500");
        assert_eq!(n.level, Level::Passive);

        let bad = ActivationPayload::Open {
            url: "not a url".into(),
        };
        assert_eq!(action_failed(&bad, "e", &EN).title, "Action failed: link");
    }
}

#[cfg(test)]
mod autostart_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn error_text_hides_home_path() {
        let home = Path::new("/Users/jörg");
        let plain = anyhow::anyhow!("autostart needs the app bundle");
        assert_eq!(
            autostart_error_text(&plain, Some(home)),
            "autostart needs the app bundle"
        );
        let with_path =
            anyhow::anyhow!("boom").context("writing /Users/jörg/Library/LaunchAgents/x.plist");
        let text = autostart_error_text(&with_path, Some(home));
        assert!(!text.contains("jörg"), "{text}");
        assert!(!text.is_empty());

        let chained = anyhow::anyhow!("permission denied").context("registering the login item");
        assert_eq!(
            autostart_error_text(&chained, Some(home)),
            "registering the login item: permission denied"
        );
        let leaking_cause = anyhow::anyhow!("cannot write /Users/jörg/x.plist").context("failed");
        let text = autostart_error_text(&leaking_cause, Some(home));
        assert!(!text.contains("jörg"), "{text}");
    }
}
