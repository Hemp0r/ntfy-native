//! The eframe app of the UI process: one window per process, kind by mode.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use eframe::egui;

use crate::cli::{UiArgs, UiMode};
use crate::ipc::{CoreMsg, UiMsg, UiRequest};
use crate::snapshot::UiSnapshot;
use crate::ui::focus;
use crate::ui::layout::popover_position;
use crate::ui::popover::{PopoverState, popover_ui};
use crate::ui::text::UiStrings;

pub const POPOVER_SIZE: (f32, f32) = (360.0, 480.0);
pub const SETTINGS_SIZE: (f32, f32) = (560.0, 620.0);
pub const WIZARD_SIZE: (f32, f32) = (520.0, 560.0);
/// Keeps relative ages ("5 min ago") current while the popover is open.
const AGE_REFRESH: Duration = Duration::from_secs(30);

/// Window title and builder for a window mode. `Probe` has no window.
pub fn viewport(args: &UiArgs, strings: &UiStrings) -> Option<egui::ViewportBuilder> {
    let builder = egui::ViewportBuilder::default().with_app_id(crate::paths::APP_ID);
    Some(match args.mode {
        UiMode::Popover => {
            let mut b = builder
                .with_title(strings.popover_title)
                .with_inner_size(POPOVER_SIZE)
                .with_decorations(false)
                .with_always_on_top()
                .with_taskbar(false)
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false);
            if let (Some(anchor), Some(monitor), Some(scale)) =
                (args.anchor, args.monitor, args.scale)
            {
                let (x, y) = popover_position(&anchor, POPOVER_SIZE, scale, monitor);
                b = b.with_position((x, y));
            }
            b
        }
        UiMode::Settings => builder
            .with_title(strings.settings)
            .with_inner_size(SETTINGS_SIZE),
        UiMode::Wizard => builder
            .with_title(strings.wizard_title)
            .with_inner_size(WIZARD_SIZE),
        UiMode::Probe => return None,
    })
}

/// What the window does after handling core messages.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Close,
}

pub struct UiApp {
    mode: UiMode,
    strings: &'static UiStrings,
    inbox: Receiver<CoreMsg>,
    outbox: Sender<UiMsg>,
    snapshot: Option<UiSnapshot>,
    popover: PopoverState,
    next_id: u64,
    /// Frames drawn so far, saturating.
    frames: u8,
    had_focus: bool,
    closing: bool,
}

impl UiApp {
    pub fn new(
        mode: UiMode,
        strings: &'static UiStrings,
        inbox: Receiver<CoreMsg>,
        outbox: Sender<UiMsg>,
    ) -> UiApp {
        UiApp {
            mode,
            strings,
            inbox,
            outbox,
            snapshot: None,
            popover: PopoverState::default(),
            next_id: 1,
            frames: 0,
            had_focus: false,
            closing: false,
        }
    }

    fn drain_inbox(&mut self) -> Flow {
        loop {
            match self.inbox.try_recv() {
                Ok(CoreMsg::Snapshot(s)) => self.snapshot = Some(s),
                Ok(CoreMsg::SaveResult { request_id, result }) => {
                    self.popover.on_save_result(request_id, &result);
                }
                Ok(CoreMsg::Close) => {
                    tracing::info!("core asked the window to close");
                    return Flow::Close;
                }
                Ok(m) => tracing::debug!(kind = crate::ui::process::kind(&m), "ignored"),
                Err(TryRecvError::Empty) => return Flow::Continue,
                Err(TryRecvError::Disconnected) => {
                    tracing::info!("core connection ended, closing window");
                    return Flow::Close;
                }
            }
        }
    }

    /// eframe shows the window only after the first frame, so activate on the second, once.
    fn activation_due(&mut self) -> bool {
        self.frames = self.frames.saturating_add(1);
        self.frames == 2
    }

    fn forward(&self, requests: Vec<UiRequest>) {
        for r in requests {
            // A closed outbox means the core is gone; the inbox notices that too.
            let _ = self.outbox.send(UiMsg::Request(r));
        }
    }

    /// Popover: closes on Esc and once focus is lost after it was focused.
    fn popover_dismissed(&mut self, focused: Option<bool>, escape: bool) -> bool {
        if self.mode != UiMode::Popover {
            return false;
        }
        match focused {
            Some(true) => self.had_focus = true,
            Some(false) if self.had_focus => return true,
            _ => {}
        }
        escape
    }
}

impl eframe::App for UiApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (focused, escape) =
            ctx.input(|i| (i.viewport().focused, i.key_pressed(egui::Key::Escape)));
        if self.closing {
            return;
        }
        if self.drain_inbox() == Flow::Close || self.popover_dismissed(focused, escape) {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.frames == 0 {
            // Make sure a second frame follows, on which the window is visible.
            ui.ctx().request_repaint();
        }
        if self.activation_due() {
            focus::activate(frame);
        }
        if let (UiMode::Popover, Some(snap)) = (self.mode, &self.snapshot) {
            let mut out = Vec::new();
            let now = chrono::Utc::now().timestamp();
            popover_ui(
                ui,
                snap,
                &mut self.popover,
                self.strings,
                now,
                &mut out,
                &mut self.next_id,
            );
            self.forward(out);
            ui.ctx().request_repaint_after(AGE_REFRESH);
            return;
        }
        let title = match self.mode {
            UiMode::Popover => self.strings.popover_title,
            UiMode::Settings => self.strings.settings,
            UiMode::Wizard | UiMode::Probe => self.strings.wizard_title,
        };
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(title);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;
    use crate::i18n::UI_EN;
    use crate::ui::layout::AnchorRect;

    fn app(mode: UiMode) -> (UiApp, Sender<CoreMsg>) {
        let (tx, inbox) = channel();
        let (outbox, _rx) = channel();
        (UiApp::new(mode, &UI_EN, inbox, outbox), tx)
    }

    fn args(mode: UiMode) -> UiArgs {
        UiArgs {
            mode,
            anchor: None,
            monitor: None,
            scale: None,
            probe_ignore_close: false,
        }
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
    fn close_and_disconnect_close_the_window() {
        let (mut a, tx) = app(UiMode::Settings);
        tx.send(CoreMsg::Snapshot(snapshot())).unwrap();
        assert_eq!(a.drain_inbox(), Flow::Continue);
        assert_eq!(a.snapshot, Some(snapshot()));
        tx.send(CoreMsg::Close).unwrap();
        assert_eq!(a.drain_inbox(), Flow::Close);
        drop(tx);
        assert_eq!(a.drain_inbox(), Flow::Close);
    }

    #[test]
    fn save_result_reaches_the_popover() {
        let (mut a, tx) = app(UiMode::Popover);
        a.popover.pending = Some(3);
        a.popover.add_topic = "new".into();
        tx.send(CoreMsg::SaveResult {
            request_id: 3,
            result: Ok("a".into()),
        })
        .unwrap();
        assert_eq!(a.drain_inbox(), Flow::Continue);
        assert_eq!(a.popover.pending, None);
        assert!(a.popover.add_topic.is_empty());
    }

    #[test]
    fn requests_go_to_the_outbox() {
        let (tx, inbox) = channel();
        let (outbox, rx) = channel();
        let a = UiApp::new(UiMode::Popover, &UI_EN, inbox, outbox);
        a.forward(vec![UiRequest::OpenSettings, UiRequest::Quit]);
        let sent: Vec<UiMsg> = rx.try_iter().collect();
        assert_eq!(
            sent,
            [
                UiMsg::Request(UiRequest::OpenSettings),
                UiMsg::Request(UiRequest::Quit)
            ]
        );
        drop(tx);
    }

    #[test]
    fn focus_is_activated_once_on_the_second_frame() {
        let (mut a, _tx) = app(UiMode::Popover);
        assert!(!a.activation_due());
        assert!(a.activation_due());
        assert!(!a.activation_due());
        assert!(!a.activation_due());
    }

    #[test]
    fn popover_closes_on_focus_loss_only_after_focus() {
        let (mut a, _tx) = app(UiMode::Popover);
        assert!(!a.popover_dismissed(None, false));
        assert!(!a.popover_dismissed(Some(false), false));
        assert!(!a.popover_dismissed(Some(true), false));
        assert!(a.popover_dismissed(Some(false), false));
    }

    #[test]
    fn popover_closes_on_escape_settings_does_not() {
        let (mut a, _tx) = app(UiMode::Popover);
        assert!(a.popover_dismissed(None, true));
        let (mut s, _tx) = app(UiMode::Settings);
        assert!(!s.popover_dismissed(Some(true), true));
        assert!(!s.popover_dismissed(Some(false), true));
    }

    #[test]
    fn window_kinds() {
        let p = viewport(&args(UiMode::Popover), &UI_EN).unwrap();
        assert_eq!(p.inner_size, Some(egui::vec2(360.0, 480.0)));
        assert_eq!(p.decorations, Some(false));
        assert_eq!(p.resizable, Some(false));
        assert_eq!(p.taskbar, Some(false));
        assert_eq!(p.window_level, Some(egui::WindowLevel::AlwaysOnTop));
        assert_eq!(p.position, None);

        let s = viewport(&args(UiMode::Settings), &UI_EN).unwrap();
        assert_eq!(s.inner_size, Some(egui::vec2(560.0, 620.0)));
        assert_eq!(s.decorations, None);
        assert_eq!(s.title.as_deref(), Some(UI_EN.settings));
        let w = viewport(&args(UiMode::Wizard), &UI_EN).unwrap();
        assert_eq!(w.inner_size, Some(egui::vec2(520.0, 560.0)));
        assert_eq!(w.title.as_deref(), Some(UI_EN.wizard_title));
        assert!(viewport(&args(UiMode::Probe), &UI_EN).is_none());
    }

    #[test]
    fn popover_is_placed_at_the_anchor() {
        let mut a = args(UiMode::Popover);
        a.anchor = Some(AnchorRect {
            x: 2400.0,
            y: 0.0,
            w: 48.0,
            h: 48.0,
        });
        a.monitor = Some((0.0, 0.0, 2880.0, 1800.0));
        let without_scale = viewport(&a, &UI_EN).unwrap();
        assert_eq!(without_scale.position, None);
        a.scale = Some(2.0);
        let p = viewport(&a, &UI_EN).unwrap();
        assert_eq!(p.position, Some(egui::pos2(1212.0 - 360.0, 28.0)));
    }
}
