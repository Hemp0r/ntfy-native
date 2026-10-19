//! Core side of the UI process: spawns, replaces and closes the `--ui`
//! child and bridges its JSON-lines protocol to the engine. Nothing here
//! blocks the engine: messages go through a channel to a writer thread, and
//! a reader thread handles everything the child sends.

use std::ffi::{OsStr, OsString};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::actions::open_local_dir;
use crate::activation;
use crate::cli::{UiArgs, UiMode};
use crate::engine::{Command, EngineHandle};
use crate::ipc::{CoreMsg, PROTOCOL_VERSION, UiMsg, UiRequest, read_msg, write_msg};
use crate::snapshot::UiSnapshot;

/// How long a child may take to exit after `Close` before it is killed.
const CLOSE_GRACE: Duration = Duration::from_secs(1);
/// A click on the tray icon arriving this soon after the popover closed on
/// its own is the click that closed it (focus moved to the icon).
const REOPEN_DEBOUNCE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    /// The UI asked for another window (settings, wizard).
    Open(UiMode),
    Quit,
    ChildExited,
}

pub type EventSink = Arc<dyn Fn(HostEvent) + Send + Sync>;

/// What a request from the UI turns into.
#[derive(Debug, PartialEq)]
enum Route {
    Event(HostEvent),
    OpenConfigDir,
    OpenLogs,
    Engine(UiRequest),
}

fn route(req: UiRequest) -> Route {
    match req {
        UiRequest::OpenSettings => Route::Event(HostEvent::Open(UiMode::Settings)),
        UiRequest::OpenWizard => Route::Event(HostEvent::Open(UiMode::Wizard)),
        UiRequest::Quit => Route::Event(HostEvent::Quit),
        UiRequest::OpenConfigDir => Route::OpenConfigDir,
        UiRequest::OpenLogs => Route::OpenLogs,
        other => Route::Engine(other),
    }
}

/// The current child's input, shared with `deliver` and the reader thread.
struct Link {
    id: u64,
    writer: Sender<CoreMsg>,
    /// Hello exchanged; before that nothing but the hello may be written.
    connected: bool,
}

#[derive(Default)]
struct Outlet {
    snapshot: Option<UiSnapshot>,
    link: Option<Link>,
    /// Mode and time of the last child that closed on its own.
    self_closed: Option<(UiMode, Instant)>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Delivers core messages to whichever child is connected. Cheap to clone;
/// never blocks on the child.
#[derive(Clone)]
pub struct UiSink {
    outlet: Arc<Mutex<Outlet>>,
}

impl UiSink {
    pub fn deliver(&self, msg: CoreMsg) {
        let mut o = lock(&self.outlet);
        if let CoreMsg::Snapshot(s) = &msg {
            o.snapshot = Some(s.clone());
        }
        if let Some(link) = o.link.as_ref().filter(|l| l.connected) {
            let _ = link.writer.send(msg);
        }
    }
}

struct Running {
    id: u64,
    mode: UiMode,
    pid: u32,
    process: Arc<Mutex<Child>>,
    exited: Arc<AtomicBool>,
    /// Set when the host closes the child (not a close on its own).
    closing: Arc<AtomicBool>,
}

impl Running {
    fn alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Default)]
struct Folders {
    config: Option<PathBuf>,
    logs: Option<PathBuf>,
}

pub struct UiHost {
    exe: PathBuf,
    engine: EngineHandle,
    on_event: EventSink,
    envs: Vec<(OsString, OsString)>,
    folders: Folders,
    sink: UiSink,
    current: Option<Running>,
    next_id: u64,
}

impl UiHost {
    pub fn new(exe: PathBuf, engine: EngineHandle, on_event: EventSink) -> Self {
        UiHost {
            exe,
            engine,
            on_event,
            envs: Vec::new(),
            folders: Folders::default(),
            sink: UiSink {
                outlet: Arc::default(),
            },
            current: None,
            next_id: 1,
        }
    }

    /// An environment variable for every child (tests point `HOME` elsewhere).
    pub fn with_env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.envs
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Folders opened for `OpenConfigDir` and `OpenLogs`.
    pub fn with_folders(mut self, config: PathBuf, logs: PathBuf) -> Self {
        self.folders = Folders {
            config: Some(config),
            logs: Some(logs),
        };
        self
    }

    /// A handle for the engine's `on_ui`.
    pub fn sink(&self) -> UiSink {
        self.sink.clone()
    }

    /// Replaces a running child (close, wait up to a second, then kill).
    pub fn open(&mut self, args: UiArgs) {
        self.close();
        activation::allow_child_foreground();
        let mut cmd = Process::new(&self.exe);
        cmd.args(args.to_args())
            .envs(self.envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, mode = ?args.mode, "starting the UI process failed");
                return;
            }
        };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            tracing::error!("UI process without pipes");
            kill_and_wait(&mut child);
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        let pid = child.id();
        let (writer, inbox) = mpsc::channel();
        let running = Running {
            id,
            mode: args.mode,
            pid,
            process: Arc::new(Mutex::new(child)),
            exited: Arc::default(),
            closing: Arc::default(),
        };
        lock(&self.sink.outlet).link = Some(Link {
            id,
            writer,
            connected: false,
        });
        let reader = Reader {
            id,
            mode: args.mode,
            process: running.process.clone(),
            exited: running.exited.clone(),
            closing: running.closing.clone(),
            outlet: self.sink.outlet.clone(),
            engine: self.engine.clone(),
            on_event: self.on_event.clone(),
            folders: self.folders.clone(),
        };
        let started = spawn_thread("ui-host-write", move || write_loop(inbox, stdin))
            .and_then(|_| spawn_thread("ui-host-read", move || reader.run(stdout)));
        if let Err(e) = started {
            tracing::error!(error = %e, "starting the UI threads failed");
            lock(&self.sink.outlet).link = None;
            kill_and_wait(&mut lock(&running.process));
            return;
        }
        tracing::info!(mode = ?args.mode, pid, "UI process started");
        self.current = Some(running);
    }

    /// An open window of `args.mode` is closed, otherwise one is opened.
    pub fn toggle_popover(&mut self, args: UiArgs) {
        if self.current_mode() == Some(args.mode) {
            self.close();
            return;
        }
        let just_closed = {
            let mut o = lock(&self.sink.outlet);
            match o.self_closed.take() {
                Some((mode, at)) => mode == args.mode && at.elapsed() < REOPEN_DEBOUNCE,
                None => false,
            }
        };
        if just_closed {
            tracing::debug!("click closed the popover, not reopening");
            return;
        }
        self.open(args);
    }

    /// Sends to the child once it is connected; a snapshot is also kept for
    /// the next child.
    pub fn deliver(&self, msg: CoreMsg) {
        self.sink.deliver(msg);
    }

    /// Asks the child to close and kills it if it has not exited after a
    /// second. Returns once it is gone.
    pub fn close(&mut self) {
        let Some(run) = self.current.take() else {
            return;
        };
        run.closing.store(true, Ordering::SeqCst);
        {
            let mut o = lock(&self.sink.outlet);
            if let Some(link) = o.link.take_if(|l| l.id == run.id)
                && link.connected
            {
                let _ = link.writer.send(CoreMsg::Close);
            }
            // Dropping the link closes the child's stdin after `Close`.
        }
        if run.alive() {
            tracing::info!(mode = ?run.mode, pid = run.pid, "closing UI process");
        }
        reap(&run.process, CLOSE_GRACE);
    }

    pub fn current_mode(&self) -> Option<UiMode> {
        self.current.as_ref().filter(|r| r.alive()).map(|r| r.mode)
    }

    pub fn pid(&self) -> Option<u32> {
        self.current.as_ref().filter(|r| r.alive()).map(|r| r.pid)
    }
}

impl Drop for UiHost {
    fn drop(&mut self) {
        self.close();
    }
}

fn spawn_thread(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map(|_| ())
}

fn kill_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits up to `grace` for the child to exit, then kills it; always reaps.
fn reap(process: &Mutex<Child>, grace: Duration) {
    let deadline = Instant::now() + grace;
    loop {
        match lock(process).try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => {}
            Ok(None) => {
                tracing::warn!("UI process did not exit in time, killing it");
                break;
            }
            Err(e) => {
                tracing::warn!(error = %e, "checking the UI process failed, killing it");
                break;
            }
        }
        thread::sleep(POLL);
    }
    kill_and_wait(&mut lock(process));
}

/// Ends when the link is dropped (closing stdin) or the child stops reading.
fn write_loop(inbox: Receiver<CoreMsg>, stdin: ChildStdin) {
    let mut out = BufWriter::new(stdin);
    for m in inbox {
        if let Err(e) = write_msg(&mut out, &m) {
            tracing::debug!(error = %e, "writing to the UI process failed");
            break;
        }
    }
}

struct Reader {
    id: u64,
    mode: UiMode,
    process: Arc<Mutex<Child>>,
    exited: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
    outlet: Arc<Mutex<Outlet>>,
    engine: EngineHandle,
    on_event: EventSink,
    folders: Folders,
}

impl Reader {
    fn run(self, stdout: ChildStdout) {
        let mut input = BufReader::new(stdout);
        let clean = loop {
            match read_msg::<UiMsg>(&mut input) {
                Ok(Some(UiMsg::Hello { version })) => self.on_hello(version),
                Ok(Some(UiMsg::Request(req))) => self.on_request(req),
                Ok(None) => break true,
                Err(e) => {
                    // The rest of a bad line is still unread: give up on the child.
                    tracing::warn!(error = %e, "invalid message from the UI process");
                    break false;
                }
            }
        };
        {
            let mut o = lock(&self.outlet);
            o.link.take_if(|l| l.id == self.id);
            if clean && !self.closing.load(Ordering::SeqCst) {
                o.self_closed = Some((self.mode, Instant::now()));
            }
        }
        if clean {
            reap(&self.process, CLOSE_GRACE);
        } else {
            kill_and_wait(&mut lock(&self.process));
        }
        let status = lock(&self.process).try_wait().ok().flatten();
        tracing::info!(mode = ?self.mode, ?status, "UI process ended");
        self.exited.store(true, Ordering::SeqCst);
        (self.on_event)(HostEvent::ChildExited);
    }

    fn on_hello(&self, version: u32) {
        if version != PROTOCOL_VERSION {
            // The child sees our version and exits.
            tracing::warn!(
                ui = version,
                core = PROTOCOL_VERSION,
                "UI protocol mismatch"
            );
        }
        let resend = {
            let mut o = lock(&self.outlet);
            let Outlet { snapshot, link, .. } = &mut *o;
            let Some(link) = link.as_mut().filter(|l| l.id == self.id) else {
                return;
            };
            let _ = link.writer.send(CoreMsg::Hello {
                version: PROTOCOL_VERSION,
            });
            if let Some(s) = snapshot {
                let _ = link.writer.send(CoreMsg::Snapshot(s.clone()));
            }
            link.connected = true;
            snapshot.is_none()
        };
        if resend {
            let _ = self.engine.tx.send(Command::ResendSnapshot);
        }
    }

    fn on_request(&self, req: UiRequest) {
        match route(req) {
            Route::Event(e) => (self.on_event)(e),
            Route::OpenConfigDir => open_folder(self.folders.config.as_deref()),
            Route::OpenLogs => open_folder(self.folders.logs.as_deref()),
            Route::Engine(req) => {
                if self.engine.tx.send(Command::Ui(req)).is_err() {
                    tracing::warn!("engine gone, UI request dropped");
                }
            }
        }
    }
}

fn open_folder(dir: Option<&Path>) {
    let Some(dir) = dir else {
        tracing::debug!("no folder configured");
        return;
    };
    if let Err(e) = open_local_dir(dir) {
        tracing::warn!(error = %format!("{e:#}"), "opening a folder failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_routed() {
        assert_eq!(
            route(UiRequest::OpenSettings),
            Route::Event(HostEvent::Open(UiMode::Settings))
        );
        assert_eq!(
            route(UiRequest::OpenWizard),
            Route::Event(HostEvent::Open(UiMode::Wizard))
        );
        assert_eq!(route(UiRequest::Quit), Route::Event(HostEvent::Quit));
        assert_eq!(route(UiRequest::OpenConfigDir), Route::OpenConfigDir);
        assert_eq!(route(UiRequest::OpenLogs), Route::OpenLogs);
        for r in [
            UiRequest::SendTestNotification,
            UiRequest::PauseAll { until: Some(1) },
            UiRequest::RemoveInstance {
                instance_id: "a".into(),
            },
        ] {
            assert_eq!(route(r.clone()), Route::Engine(r));
        }
    }

    fn sink_with_link(connected: bool) -> (UiSink, Receiver<CoreMsg>) {
        let sink = UiSink {
            outlet: Arc::default(),
        };
        let (writer, rx) = mpsc::channel();
        lock(&sink.outlet).link = Some(Link {
            id: 1,
            writer,
            connected,
        });
        (sink, rx)
    }

    fn snapshot() -> UiSnapshot {
        UiSnapshot {
            app_version: "1".into(),
            configured: true,
            instances: vec![],
            recent: vec![],
            gaps: vec![],
            paused_until: None,
            urgent_breaks_dnd: false,
        }
    }

    #[test]
    fn sink_holds_messages_until_connected_but_keeps_the_snapshot() {
        let (sink, rx) = sink_with_link(false);
        sink.deliver(CoreMsg::Snapshot(snapshot()));
        sink.deliver(CoreMsg::Close);
        assert!(rx.try_recv().is_err());
        assert_eq!(lock(&sink.outlet).snapshot, Some(snapshot()));

        let (sink, rx) = sink_with_link(true);
        sink.deliver(CoreMsg::Snapshot(snapshot()));
        sink.deliver(CoreMsg::Close);
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(got, [CoreMsg::Snapshot(snapshot()), CoreMsg::Close]);
    }
}
