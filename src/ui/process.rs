//! UI process entry (`ntfy-native --ui <mode>`): stdin/stdout bridge to the
//! core, handshake, and the window or probe loop.

use std::io::{self, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::anyhow;

use crate::cli::{UiArgs, UiMode};
use crate::ipc::{CoreMsg, PROTOCOL_VERSION, UiMsg, UiRequest, read_msg, write_msg};
use crate::paths::AppPaths;
use crate::ui::app::{UiApp, viewport};
use crate::ui::fonts;
use crate::ui::text::{UiStrings, ui_strings};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound for flushing queued messages to the core at exit.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const LOG_PREFIX: &str = "ui";

pub struct Bridge {
    pub inbox: Receiver<CoreMsg>,
    pub outbox: Sender<UiMsg>,
    tail: BridgeTail,
}

/// What stays with `run_ui` once the channels are handed out.
struct BridgeTail {
    writer_done: Receiver<()>,
    read_failed: Arc<AtomicBool>,
}

impl BridgeTail {
    /// Waits (bounded) until every queued message is written. Call after
    /// all `outbox` senders are dropped.
    fn finish(self) -> anyhow::Result<()> {
        // The writer only ever drops its sender: `Disconnected` means done.
        if let Err(RecvTimeoutError::Timeout) = self.writer_done.recv_timeout(FLUSH_TIMEOUT) {
            tracing::warn!("ipc writer did not finish in time");
        }
        if self.read_failed.load(Ordering::SeqCst) {
            return Err(anyhow!("invalid message from the core"));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("protocol version mismatch (core {core}, ui {PROTOCOL_VERSION})")]
    VersionMismatch { core: u32 },
    #[error("core did not start with a hello")]
    MissingHello,
}

/// Process exit code for a `run_ui` result.
pub fn exit_code(result: &anyhow::Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) if e.downcast_ref::<UiError>().is_some() => 2,
        Err(_) => 1,
    }
}

/// Reader thread on stdin (calls `on_message` after every message and once
/// at EOF or on a read error), writer thread on stdout. The inbox
/// disconnects at EOF; an invalid line is fatal and also disconnects it.
pub fn start_bridge(on_message: impl Fn() + Send + 'static) -> Bridge {
    start_bridge_with(io::stdin(), io::stdout(), on_message)
}

fn start_bridge_with(
    input: impl Read + Send + 'static,
    mut output: impl Write + Send + 'static,
    on_message: impl Fn() + Send + 'static,
) -> Bridge {
    let (in_tx, inbox) = mpsc::channel();
    let (outbox, out_rx) = mpsc::channel::<UiMsg>();
    let (done_tx, writer_done) = mpsc::channel();
    let read_failed = Arc::new(AtomicBool::new(false));
    let failed = read_failed.clone();
    let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
        thread::Builder::new()
            .name(name.into())
            .spawn(f)
            .expect("spawning ipc thread");
    };
    spawn(
        "ui-ipc-read",
        Box::new(move || {
            let mut input = BufReader::new(input);
            loop {
                match read_msg::<CoreMsg>(&mut input) {
                    Ok(Some(m)) => {
                        tracing::debug!(kind = kind(&m), "from core");
                        if in_tx.send(m).is_err() {
                            break;
                        }
                        on_message();
                    }
                    Ok(None) => {
                        tracing::info!("stdin closed");
                        break;
                    }
                    Err(e) => {
                        // The rest of the line may still be unread: stop.
                        tracing::error!(error = %e, "ipc read failed");
                        failed.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
            drop(in_tx);
            on_message();
        }),
    );
    spawn(
        "ui-ipc-write",
        Box::new(move || {
            for m in out_rx {
                if let Err(e) = write_msg(&mut output, &m) {
                    tracing::warn!(error = %e, "ipc write failed");
                    break;
                }
            }
            drop(done_tx);
        }),
    );
    Bridge {
        inbox,
        outbox,
        tail: BridgeTail {
            writer_done,
            read_failed,
        },
    }
}

/// Message kind for logs; never the contents.
pub fn kind(m: &CoreMsg) -> &'static str {
    match m {
        CoreMsg::Hello { .. } => "hello",
        CoreMsg::Snapshot(_) => "snapshot",
        CoreMsg::TestResult { .. } => "test_result",
        CoreMsg::SaveResult { .. } => "save_result",
        CoreMsg::AutostartResult { .. } => "autostart_result",
        CoreMsg::Close => "close",
        CoreMsg::Focus => "focus",
    }
}

pub fn run_ui(args: UiArgs) -> anyhow::Result<()> {
    let _log_guard = match init_logging() {
        Ok(g) => Some(g),
        Err(e) => {
            // The window is more useful than the log; carry on without it.
            eprintln!("ntfy-native ui: logging unavailable: {e:#}");
            None
        }
    };
    tracing::info!(mode = ?args.mode, "ui starting");
    let result = run_logged(args);
    match &result {
        Ok(()) => tracing::info!("ui finished"),
        Err(e) => tracing::error!(error = %format!("{e:#}"), "ui failed"),
    }
    result
}

fn init_logging() -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    let paths = AppPaths::platform()?;
    std::fs::create_dir_all(&paths.log_dir)?;
    crate::logging::init_with_prefix(&paths, LOG_PREFIX)
}

fn run_logged(args: UiArgs) -> anyhow::Result<()> {
    if args.mode == UiMode::Probe || args.headless {
        let bridge = start_bridge(|| {});
        return run_probe(bridge, args.probe_ignore_close);
    }
    let ctx = eframe::egui::Context::default();
    let repaint = ctx.clone();
    let bridge = start_bridge(move || repaint.request_repaint());
    let connected = handshake(&bridge)?;
    let Bridge {
        inbox,
        outbox,
        tail,
    } = bridge;
    if connected {
        run_window(&args, ui_strings(), ctx, inbox, outbox)?;
    } else {
        drop(outbox);
    }
    tail.finish()
}

/// `Ok(false)`: the core went away before answering.
fn handshake(bridge: &Bridge) -> anyhow::Result<bool> {
    let _ = bridge.outbox.send(UiMsg::Hello {
        version: PROTOCOL_VERSION,
    });
    match bridge.inbox.recv_timeout(HANDSHAKE_TIMEOUT) {
        Ok(CoreMsg::Hello { version }) if version == PROTOCOL_VERSION => {
            tracing::info!("handshake ok");
            Ok(true)
        }
        Ok(CoreMsg::Hello { version }) => Err(UiError::VersionMismatch { core: version }.into()),
        Ok(_) => Err(UiError::MissingHello.into()),
        Err(RecvTimeoutError::Disconnected) => Ok(false),
        Err(RecvTimeoutError::Timeout) => Err(anyhow!("no handshake from the core")),
    }
}

/// Test mode: answers each snapshot with a test-notification request.
fn run_probe(bridge: Bridge, ignore_close: bool) -> anyhow::Result<()> {
    let connected = match handshake(&bridge) {
        Ok(c) => c,
        Err(e) if !ignore_close => return Err(e),
        Err(_) => false,
    };
    if connected {
        loop {
            match bridge.inbox.recv() {
                Ok(CoreMsg::Snapshot(_)) => {
                    let _ = bridge
                        .outbox
                        .send(UiMsg::Request(UiRequest::SendTestNotification));
                }
                Ok(CoreMsg::Close) if !ignore_close => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
    if ignore_close {
        tracing::info!("probe ignoring close, sleeping until killed");
        loop {
            thread::park();
        }
    }
    let Bridge { outbox, tail, .. } = bridge;
    drop(outbox);
    tail.finish()
}

/// Windows without a computed position (no anchor, monitor or scale) are centred.
fn window_options(viewport: eframe::egui::ViewportBuilder) -> eframe::NativeOptions {
    eframe::NativeOptions {
        centered: viewport.position.is_none(),
        viewport,
        ..Default::default()
    }
}

fn run_window(
    args: &UiArgs,
    strings: &'static UiStrings,
    ctx: eframe::egui::Context,
    inbox: Receiver<CoreMsg>,
    outbox: Sender<UiMsg>,
) -> anyhow::Result<()> {
    let viewport = viewport(args, strings).ok_or_else(|| anyhow!("mode has no window"))?;
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut options = window_options(viewport);
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        let popover = args.mode == UiMode::Popover;
        options.event_loop_builder = Some(Box::new(move |b| {
            // The popover must not add a Dock icon or menu bar.
            if popover {
                b.with_activation_policy(ActivationPolicy::Accessory)
                    .with_default_menu(false);
            }
        }));
    }
    let mode = args.mode;
    let placement = args.clone();
    eframe::run_native_ext(
        strings.popover_title,
        options,
        Some(ctx),
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(
                UiApp::new(mode, strings, inbox, outbox).with_placement(&placement),
            ))
        }),
    )
    .map_err(|e| anyhow!("ui window failed: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn lines(msgs: &[CoreMsg]) -> Vec<u8> {
        let mut v = Vec::new();
        for m in msgs {
            write_msg(&mut v, m).unwrap();
        }
        v
    }

    #[test]
    fn bridge_delivers_in_order_and_disconnects_at_eof() {
        let (calls_tx, calls) = mpsc::channel();
        let calls_tx = Mutex::new(calls_tx);
        let input = lines(&[CoreMsg::Hello { version: 1 }, CoreMsg::Close]);
        let b = start_bridge_with(io::Cursor::new(input), io::sink(), move || {
            let _ = calls_tx.lock().unwrap().send(());
        });
        assert_eq!(b.inbox.recv().unwrap(), CoreMsg::Hello { version: 1 });
        assert_eq!(b.inbox.recv().unwrap(), CoreMsg::Close);
        assert!(b.inbox.recv().is_err());
        // two messages + EOF; the last call may follow the disconnect
        for _ in 0..3 {
            calls
                .recv_timeout(Duration::from_secs(5))
                .expect("on_message call");
        }
        assert!(calls.recv_timeout(Duration::from_millis(50)).is_err());
        drop(b.outbox);
        b.tail.finish().unwrap();
    }

    #[test]
    fn invalid_line_is_fatal() {
        let mut input = b"garbage\n".to_vec();
        input.extend(lines(&[CoreMsg::Close]));
        let b = start_bridge_with(io::Cursor::new(input), io::sink(), || {});
        assert!(b.inbox.recv().is_err(), "nothing after an invalid line");
        drop(b.outbox);
        assert!(b.tail.finish().is_err());
    }

    #[test]
    fn writer_flushes_queued_messages() {
        let out = SharedBuf::default();
        let b = start_bridge_with(io::empty(), out.clone(), || {});
        for _ in 0..100 {
            b.outbox
                .send(UiMsg::Request(UiRequest::SendTestNotification))
                .unwrap();
        }
        let Bridge { outbox, tail, .. } = b;
        drop(outbox);
        tail.finish().unwrap();
        let written = out.0.lock().unwrap().clone();
        let mut r = written.as_slice();
        let mut n = 0;
        while let Some(m) = read_msg::<UiMsg>(&mut r).unwrap() {
            assert_eq!(m, UiMsg::Request(UiRequest::SendTestNotification));
            n += 1;
        }
        assert_eq!(n, 100);
    }

    #[test]
    fn popover_without_position_is_centered() {
        use crate::ui::layout::AnchorRect;
        let mut args = UiArgs {
            mode: UiMode::Popover,
            anchor: Some(AnchorRect {
                x: 10.0,
                y: 0.0,
                w: 20.0,
                h: 20.0,
            }),
            monitor: Some((0.0, 0.0, 1000.0, 800.0)),
            scale: None,
            probe_ignore_close: false,
            headless: false,
        };
        let strings = crate::i18n::ui_strings_for("en");
        assert!(window_options(viewport(&args, strings).unwrap()).centered);
        args.scale = Some(1.0);
        assert!(!window_options(viewport(&args, strings).unwrap()).centered);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(
            exit_code(&Err(UiError::VersionMismatch { core: 9 }.into())),
            2
        );
        assert_eq!(exit_code(&Err(UiError::MissingHello.into())), 2);
        assert_eq!(exit_code(&Err(anyhow!("x"))), 1);
    }
}
