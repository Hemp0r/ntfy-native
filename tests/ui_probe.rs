//! Runs the real binary in `--ui probe` mode and talks the IPC protocol.

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

use ntfy_native::ipc::{CoreMsg, PROTOCOL_VERSION, UiMsg, UiRequest, read_msg, write_msg};
use ntfy_native::snapshot::UiSnapshot;

const TIMEOUT: Duration = Duration::from_secs(10);

struct Probe {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Receiver<UiMsg>,
    _home: tempfile::TempDir,
}

impl Probe {
    fn spawn(extra: &[&str]) -> Probe {
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_ntfy-native"))
            .args(["--ui", "probe"])
            .args(extra)
            .env("HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, out) = channel();
        thread::spawn(move || {
            while let Ok(Some(m)) = read_msg::<UiMsg>(&mut stdout) {
                if tx.send(m).is_err() {
                    break;
                }
            }
        });
        Probe {
            child,
            stdin,
            out,
            _home: home,
        }
    }

    fn send(&mut self, m: &CoreMsg) {
        write_msg(self.stdin.as_mut().unwrap(), m).unwrap();
    }

    fn recv(&self) -> UiMsg {
        self.out.recv_timeout(TIMEOUT).expect("message from probe")
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(Instant::now() < deadline, "probe did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn handshake(&mut self) {
        assert_eq!(
            self.recv(),
            UiMsg::Hello {
                version: PROTOCOL_VERSION
            }
        );
        self.send(&CoreMsg::Hello {
            version: PROTOCOL_VERSION,
        });
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

#[test]
fn probe_handshakes_and_exits_on_eof() {
    let mut p = Probe::spawn(&[]);
    p.handshake();
    p.close_stdin();
    assert_eq!(p.wait().code(), Some(0));
}

#[test]
fn probe_rejects_wrong_protocol_version() {
    let mut p = Probe::spawn(&[]);
    assert!(matches!(p.recv(), UiMsg::Hello { .. }));
    p.send(&CoreMsg::Hello {
        version: PROTOCOL_VERSION + 1,
    });
    assert_eq!(p.wait().code(), Some(2));
}

#[test]
fn probe_rejects_missing_hello() {
    let mut p = Probe::spawn(&[]);
    assert!(matches!(p.recv(), UiMsg::Hello { .. }));
    p.send(&CoreMsg::Snapshot(snapshot()));
    assert_eq!(p.wait().code(), Some(2));
}

#[test]
fn probe_answers_snapshot_with_request() {
    let mut p = Probe::spawn(&[]);
    p.handshake();
    for _ in 0..2 {
        p.send(&CoreMsg::Snapshot(snapshot()));
        assert_eq!(p.recv(), UiMsg::Request(UiRequest::SendTestNotification));
    }
    p.send(&CoreMsg::Close);
    assert_eq!(p.wait().code(), Some(0));
}

#[test]
fn probe_flushes_answer_before_exiting() {
    let mut p = Probe::spawn(&[]);
    p.handshake();
    p.send(&CoreMsg::Snapshot(snapshot()));
    p.send(&CoreMsg::Close);
    assert_eq!(p.wait().code(), Some(0));
    assert_eq!(p.recv(), UiMsg::Request(UiRequest::SendTestNotification));
}

#[test]
fn probe_ignore_close_keeps_running() {
    let mut p = Probe::spawn(&["--probe-ignore-close"]);
    p.handshake();
    p.send(&CoreMsg::Close);
    p.send(&CoreMsg::Snapshot(snapshot()));
    assert_eq!(p.recv(), UiMsg::Request(UiRequest::SendTestNotification));
    p.close_stdin();
    thread::sleep(Duration::from_millis(500));
    assert!(p.child.try_wait().unwrap().is_none(), "probe exited");
    assert!(matches!(
        p.out.recv_timeout(Duration::from_millis(1)),
        Err(RecvTimeoutError::Timeout)
    ));
}

#[test]
fn probe_exits_on_eof_before_hello() {
    let mut p = Probe::spawn(&[]);
    assert!(matches!(p.recv(), UiMsg::Hello { .. }));
    p.close_stdin();
    assert_eq!(p.wait().code(), Some(0));
}

#[test]
fn probe_rejects_malformed_line() {
    let mut p = Probe::spawn(&[]);
    p.handshake();
    p.stdin.as_mut().unwrap().write_all(b"not json\n").unwrap();
    p.stdin.as_mut().unwrap().flush().unwrap();
    assert_eq!(p.wait().code(), Some(1));
}
