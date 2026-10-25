//! Drives `UiHost` against the real binary; children run the windowless
//! probe loop (`--ui probe`, or any mode with the hidden `--headless`).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ntfy_native::cli::{UiArgs, UiMode};
use ntfy_native::engine::{Command, EngineHandle};
use ntfy_native::ipc::{CoreMsg, UiRequest};
use ntfy_native::snapshot::UiSnapshot;
use ntfy_native::ui_host::{HostEvent, Outcome, UiHost};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const TIMEOUT: Duration = Duration::from_secs(10);
/// `close()` and replacing a child must not wait for it.
const NON_BLOCKING: Duration = Duration::from_millis(50);

struct Harness {
    host: UiHost,
    commands: UnboundedReceiver<Command>,
    events: Receiver<HostEvent>,
    _home: tempfile::TempDir,
}

fn harness() -> Harness {
    let home = tempfile::tempdir().unwrap();
    let (tx, commands) = unbounded_channel();
    let (ev_tx, events) = channel();
    let ev_tx = Mutex::new(ev_tx);
    let host = UiHost::new(
        PathBuf::from(env!("CARGO_BIN_EXE_ntfy-native")),
        EngineHandle { tx },
        Arc::new(move |e| {
            let _ = ev_tx.lock().unwrap().send(e);
        }),
    )
    .with_env("HOME", home.path())
    // Children inherit stderr, where debug builds also log.
    .with_env("NTFY_NATIVE_LOG", "off");
    Harness {
        host,
        commands,
        events,
        _home: home,
    }
}

fn args(mode: UiMode) -> UiArgs {
    UiArgs {
        headless: true,
        ..UiArgs::window(mode)
    }
}

fn probe(ignore_close: bool) -> UiArgs {
    UiArgs {
        probe_ignore_close: ignore_close,
        ..args(UiMode::Probe)
    }
}

fn snapshot() -> UiSnapshot {
    UiSnapshot {
        app_version: "1.0".into(),
        configured: false,
        instances: vec![],
        recent: vec![],
        gaps: vec![],
        paused_until: None,
        urgent_breaks_dnd: false,
        autostart: true,
    }
}

fn started(o: Outcome) -> u32 {
    match o {
        Outcome::Started(pid) => pid,
        other => panic!("expected a new child, got {other:?}"),
    }
}

impl Harness {
    /// Next command the host sent to the engine.
    fn command(&mut self) -> Command {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Ok(c) = self.commands.try_recv() {
                return c;
            }
            assert!(Instant::now() < deadline, "no command from the host");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits for the probe's answer to a snapshot. A `ResendSnapshot`
    /// (the hello won the race against `deliver`) is skipped.
    fn expect_test_request(&mut self) {
        loop {
            match self.command() {
                Command::ResendSnapshot => {}
                Command::Ui(UiRequest::SendTestNotification) => return,
                other => panic!("unexpected command {other:?}"),
            }
        }
    }

    /// Delivers a snapshot and waits for the answer: the child is connected.
    fn connect(&mut self) {
        self.host.deliver(CoreMsg::Snapshot(snapshot()));
        self.expect_test_request();
    }

    fn no_command_for(&mut self, d: Duration) {
        thread::sleep(d);
        if let Ok(c) = self.commands.try_recv() {
            panic!("unexpected command {c:?}");
        }
    }

    fn event(&self) -> HostEvent {
        self.events.recv_timeout(TIMEOUT).expect("host event")
    }

    fn child_exited(&self) {
        assert_eq!(self.event(), HostEvent::ChildExited);
    }
}

/// Neither running nor a zombie: the pid no longer exists.
#[cfg(unix)]
fn reaped(pid: u32) -> bool {
    !std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[cfg(not(unix))]
fn reaped(_pid: u32) -> bool {
    true
}

#[test]
fn opens_child_and_forwards_requests() {
    let mut h = harness();
    started(h.host.open(probe(false)));
    assert_eq!(h.host.current_mode(), Some(UiMode::Probe));
    for _ in 0..3 {
        h.connect();
    }
}

#[test]
fn toggle_closes_open_popover() {
    let mut h = harness();
    let pid = started(h.host.toggle_popover(args(UiMode::Popover)));
    assert_eq!(h.host.current_mode(), Some(UiMode::Popover));
    assert_eq!(
        h.host.toggle_popover(args(UiMode::Popover)),
        Outcome::Closed
    );
    assert_eq!(h.host.current_mode(), None);
    assert_eq!(h.host.pid(), None);
    h.child_exited();
    assert!(reaped(pid), "child still exists");
    // A third click opens a fresh one.
    let again = started(h.host.toggle_popover(args(UiMode::Popover)));
    assert_ne!(again, pid);
    assert_eq!(h.host.current_mode(), Some(UiMode::Popover));
}

#[test]
fn child_exit_is_detected() {
    let mut h = harness();
    let pid = started(h.host.open(probe(false)));
    h.connect();
    h.host.deliver(CoreMsg::Close);
    h.child_exited();
    assert_eq!(h.host.current_mode(), None);
    assert!(reaped(pid), "zombie or still running");
    // Nothing reaches a gone child, and the engine never blocks on it.
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.no_command_for(Duration::from_millis(200));
}

#[test]
fn hanging_child_is_killed_on_close() {
    let mut h = harness();
    let pid = started(h.host.open(probe(true)));
    h.connect();
    let start = Instant::now();
    h.host.close();
    assert!(start.elapsed() < NON_BLOCKING, "close blocked");
    assert_eq!(h.host.current_mode(), None);
    h.child_exited();
    let took = start.elapsed();
    // It ignored `Close`, so only the kill after the grace period ended it.
    assert!(took >= Duration::from_millis(900), "not killed: {took:?}");
    assert!(took <= Duration::from_millis(2000), "killed late: {took:?}");
    assert!(reaped(pid), "hanging child survived close");
}

#[test]
fn replacing_a_hanging_child_does_not_wait() {
    let mut h = harness();
    let old = started(h.host.open(probe(true)));
    h.connect();
    let start = Instant::now();
    let new = started(h.host.open(probe(false)));
    assert!(start.elapsed() < NON_BLOCKING, "replace blocked");
    assert_ne!(new, old);
    h.child_exited();
    assert!(reaped(old), "replaced child survived");
    h.connect();
}

#[test]
fn new_child_gets_last_snapshot() {
    let mut h = harness();
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    let old = started(h.host.open(probe(false)));
    h.expect_test_request();
    // A replacement child gets it again, the old one is gone.
    started(h.host.open(probe(false)));
    h.expect_test_request();
    h.child_exited();
    assert!(reaped(old), "replaced child still exists");
    h.no_command_for(Duration::from_millis(200));
}

#[test]
fn child_without_snapshot_asks_the_engine() {
    let mut h = harness();
    started(h.host.open(probe(false)));
    assert!(matches!(h.command(), Command::ResendSnapshot));
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.expect_test_request();
}

#[test]
fn messages_before_the_handshake_are_not_sent() {
    // A result for an old child must not precede the hello (the probe would
    // exit with "missing hello" and never answer).
    let mut h = harness();
    started(h.host.open(probe(false)));
    h.host.deliver(CoreMsg::SaveResult {
        request_id: 1,
        result: Ok("a".into()),
    });
    h.connect();
}

#[test]
fn click_right_after_self_close_does_not_reopen() {
    // The click on the tray icon that took focus from the popover closes it
    // before the click arrives; that click must not reopen it.
    let mut h = harness();
    started(h.host.toggle_popover(args(UiMode::Popover)));
    h.connect();
    h.host.deliver(CoreMsg::Close);
    h.child_exited();
    assert_eq!(
        h.host.toggle_popover(args(UiMode::Popover)),
        Outcome::Nothing
    );
    assert_eq!(h.host.current_mode(), None);
    // A later click opens it again.
    thread::sleep(Duration::from_millis(600));
    started(h.host.toggle_popover(args(UiMode::Popover)));
    assert_eq!(h.host.current_mode(), Some(UiMode::Popover));
}

#[test]
fn open_settings_window_is_focused_not_replaced() {
    let mut h = harness();
    let pid = started(h.host.open(args(UiMode::Settings)));
    h.connect();
    assert_eq!(h.host.open(args(UiMode::Settings)), Outcome::Focused(pid));
    assert_eq!(h.host.open(args(UiMode::Wizard)), Outcome::Focused(pid));
    // A tray click does not open a popover over it either.
    assert_eq!(
        h.host.toggle_popover(args(UiMode::Popover)),
        Outcome::Focused(pid)
    );
    assert_eq!(h.host.current_mode(), Some(UiMode::Settings));
    assert_eq!(h.host.pid(), Some(pid));
    // The window ignores the focus request (no window), but stays.
    h.host.focus_window();
    // A new child would have answered the stored snapshot.
    h.no_command_for(Duration::from_millis(300));
    assert!(h.events.try_recv().is_err(), "a child exited");
}

#[test]
fn open_wizard_is_focused_not_replaced() {
    let mut h = harness();
    let pid = started(h.host.open(args(UiMode::Wizard)));
    h.connect();
    assert_eq!(h.host.open(args(UiMode::Settings)), Outcome::Focused(pid));
    assert_eq!(h.host.current_mode(), Some(UiMode::Wizard));
}

#[test]
fn popover_is_replaced_by_settings() {
    let mut h = harness();
    let popover = started(h.host.toggle_popover(args(UiMode::Popover)));
    h.connect();
    let settings = started(h.host.open(args(UiMode::Settings)));
    assert_ne!(settings, popover);
    assert_eq!(h.host.current_mode(), Some(UiMode::Settings));
    h.child_exited();
    assert!(reaped(popover), "popover still exists");
    h.connect();
}

#[test]
fn dropping_the_host_closes_the_child() {
    let mut h = harness();
    let pid = started(h.host.open(probe(true)));
    let Harness { host, .. } = h;
    drop(host);
    assert!(reaped(pid), "child outlived the host");
}

#[test]
fn close_and_wait_also_waits_for_earlier_closes() {
    // Quitting right after closing a hanging window must not leave it behind.
    let mut h = harness();
    let pid = started(h.host.open(probe(true)));
    h.connect();
    h.host.close();
    h.host.close_and_wait();
    assert!(reaped(pid), "hanging child survived the quit");
}
