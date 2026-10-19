//! Drives `UiHost` against the real binary in `--ui probe` mode.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ntfy_native::cli::{UiArgs, UiMode};
use ntfy_native::engine::{Command, EngineHandle};
use ntfy_native::ipc::{CoreMsg, UiRequest};
use ntfy_native::snapshot::UiSnapshot;
use ntfy_native::ui_host::{HostEvent, UiHost};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const TIMEOUT: Duration = Duration::from_secs(10);

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

fn probe(ignore_close: bool) -> UiArgs {
    UiArgs {
        mode: UiMode::Probe,
        anchor: None,
        monitor: None,
        scale: None,
        probe_ignore_close: ignore_close,
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

    /// Waits for the probe's answer to a snapshot.
    fn expect_test_request(&mut self) {
        match self.command() {
            Command::Ui(UiRequest::SendTestNotification) => {}
            other => panic!("unexpected command {other:?}"),
        }
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
    h.host.open(probe(false));
    assert_eq!(h.host.current_mode(), Some(UiMode::Probe));
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.expect_test_request();
    for _ in 0..2 {
        h.host.deliver(CoreMsg::Snapshot(snapshot()));
        h.expect_test_request();
    }
}

#[test]
fn toggle_closes_open_popover() {
    let mut h = harness();
    h.host.toggle_popover(probe(false));
    assert_eq!(h.host.current_mode(), Some(UiMode::Probe));
    let pid = h.host.pid().expect("pid");
    h.host.toggle_popover(probe(false));
    assert_eq!(h.host.current_mode(), None);
    assert_eq!(h.host.pid(), None);
    assert!(reaped(pid), "child still exists");
    assert_eq!(h.event(), HostEvent::ChildExited);
    // A third click opens a fresh one.
    h.host.toggle_popover(probe(false));
    assert_eq!(h.host.current_mode(), Some(UiMode::Probe));
    assert_ne!(h.host.pid(), Some(pid));
}

#[test]
fn child_exit_is_detected() {
    let mut h = harness();
    h.host.open(probe(false));
    let pid = h.host.pid().expect("pid");
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.expect_test_request();
    h.host.deliver(CoreMsg::Close);
    assert_eq!(h.event(), HostEvent::ChildExited);
    assert_eq!(h.host.current_mode(), None);
    assert!(reaped(pid), "zombie or still running");
    // Nothing reaches a gone child, and the engine never blocks on it.
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.no_command_for(Duration::from_millis(200));
}

#[test]
fn hanging_child_is_killed_on_close() {
    let mut h = harness();
    h.host.open(probe(true));
    let pid = h.host.pid().expect("pid");
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.expect_test_request();
    let start = Instant::now();
    h.host.close();
    let took = start.elapsed();
    assert!(took <= Duration::from_millis(1500), "close took {took:?}");
    assert_eq!(h.host.current_mode(), None);
    assert!(reaped(pid), "hanging child survived close");
    assert_eq!(h.event(), HostEvent::ChildExited);
}

#[test]
fn new_child_gets_last_snapshot() {
    let mut h = harness();
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.host.open(probe(false));
    h.expect_test_request();
    // A replacement child gets it again, the old one is gone.
    let old = h.host.pid().expect("pid");
    h.host.open(probe(false));
    assert!(reaped(old), "replaced child still exists");
    h.expect_test_request();
    h.no_command_for(Duration::from_millis(200));
}

#[test]
fn child_without_snapshot_asks_the_engine() {
    let mut h = harness();
    h.host.open(probe(false));
    assert!(matches!(h.command(), Command::ResendSnapshot));
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.expect_test_request();
}

#[test]
fn messages_before_the_handshake_are_not_sent() {
    // A result for an old child must not precede the hello (the probe would
    // exit with "missing hello" and never answer).
    let mut h = harness();
    h.host.open(probe(false));
    h.host.deliver(CoreMsg::SaveResult {
        request_id: 1,
        result: Ok("a".into()),
    });
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.expect_test_request();
}

#[test]
fn click_right_after_self_close_does_not_reopen() {
    // The click on the tray icon that took focus from the popover closes it
    // before the click arrives; that click must not reopen it.
    let mut h = harness();
    h.host.toggle_popover(probe(false));
    h.host.deliver(CoreMsg::Snapshot(snapshot()));
    h.expect_test_request();
    h.host.deliver(CoreMsg::Close);
    assert_eq!(h.event(), HostEvent::ChildExited);
    h.host.toggle_popover(probe(false));
    assert_eq!(h.host.current_mode(), None);
    // A later click opens it again.
    thread::sleep(Duration::from_millis(600));
    h.host.toggle_popover(probe(false));
    assert_eq!(h.host.current_mode(), Some(UiMode::Probe));
}

#[test]
fn dropping_the_host_closes_the_child() {
    let mut h = harness();
    h.host.open(probe(true));
    let pid = h.host.pid().expect("pid");
    let Harness { host, .. } = h;
    drop(host);
    assert!(reaped(pid), "child outlived the host");
}
