//! The eframe app of the UI process: one window per process, kind by mode.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::cli::{UiArgs, UiMode};
use crate::ipc::{CoreMsg, UiMsg, UiRequest};
use crate::snapshot::UiSnapshot;
use crate::ui::focus;
use crate::ui::layout::{AnchorRect, popover_position};
use crate::ui::popover::{PopoverState, popover_ui};
use crate::ui::settings_view::{SettingsState, settings_ui};
use crate::ui::text::UiStrings;
use crate::ui::wizard::{Wizard, WizardStep, wizard_ui};

pub const POPOVER_SIZE: (f32, f32) = (360.0, 480.0);
pub const SETTINGS_SIZE: (f32, f32) = (560.0, 620.0);
pub const WIZARD_SIZE: (f32, f32) = (520.0, 560.0);
/// Keeps relative ages ("5 min ago") current while the popover is open.
const AGE_REFRESH: Duration = Duration::from_secs(30);
/// Activation is cooperative on macOS and may be refused before the core
/// has yielded it: ask again a few times until the window has focus.
const ACTIVATION_RETRY: Duration = Duration::from_millis(150);
const ACTIVATION_RETRIES: u8 = 4;
/// Scale difference above which the popover is placed again.
const SCALE_TOLERANCE: f32 = 0.01;

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

/// Tray anchor, monitor and the scale the core computed the position with.
#[derive(Clone, Copy)]
struct Placement {
    anchor: AnchorRect,
    monitor: (f64, f64, f64, f64),
    scale: f32,
}

pub struct UiApp {
    mode: UiMode,
    strings: &'static UiStrings,
    inbox: Receiver<CoreMsg>,
    outbox: Sender<UiMsg>,
    snapshot: Option<UiSnapshot>,
    popover: PopoverState,
    settings: SettingsState,
    /// The wizard window's state (unused in other modes).
    wizard: Wizard,
    next_id: u64,
    /// Frames drawn so far, saturating.
    frames: u8,
    had_focus: bool,
    activations: u8,
    last_activation: Option<Instant>,
    placement: Option<Placement>,
    focus_requested: bool,
    /// Brings the window to the foreground; replaced in tests.
    activator: fn(&eframe::Frame),
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
            settings: SettingsState::default(),
            wizard: Wizard::new(),
            next_id: 1,
            frames: 0,
            had_focus: false,
            activations: 0,
            last_activation: None,
            placement: None,
            focus_requested: false,
            activator: focus::activate,
            closing: false,
        }
    }

    #[cfg(test)]
    fn with_activator(mut self, activator: fn(&eframe::Frame)) -> UiApp {
        self.activator = activator;
        self
    }

    /// Remembers where the popover was placed, see [`UiApp::position_fix`].
    pub fn with_placement(mut self, args: &UiArgs) -> UiApp {
        self.placement = match (args.anchor, args.monitor, args.scale) {
            (Some(anchor), Some(monitor), Some(scale)) => Some(Placement {
                anchor,
                monitor,
                scale,
            }),
            _ => None,
        };
        self
    }

    /// Mixed DPI: the monitor's real scale (known once the window is shown)
    /// differs from the one passed by the core, so the position is off.
    fn position_fix(&self, native_ppp: Option<f32>) -> Option<egui::Pos2> {
        let Placement {
            anchor,
            monitor,
            scale,
        } = self.placement?;
        let ppp = native_ppp?;
        if self.mode != UiMode::Popover || (ppp - scale).abs() <= SCALE_TOLERANCE {
            return None;
        }
        let (x, y) = popover_position(&anchor, POPOVER_SIZE, ppp, monitor);
        Some(egui::pos2(x, y))
    }

    fn drain_inbox(&mut self) -> Flow {
        loop {
            match self.inbox.try_recv() {
                Ok(CoreMsg::Snapshot(s)) => {
                    self.popover.on_snapshot();
                    self.settings.on_snapshot(&s);
                    self.wizard.urgent_global = s.urgent_breaks_dnd;
                    self.snapshot = Some(s);
                }
                // Request ids come from one counter, so every state only takes its own.
                Ok(CoreMsg::TestResult {
                    request_id,
                    outcome,
                }) => {
                    self.wizard.on_test_result(request_id, &outcome);
                    self.settings.on_test_result(request_id, &outcome);
                }
                Ok(CoreMsg::SaveResult { request_id, result }) => {
                    self.popover.on_save_result(request_id, &result);
                    self.wizard.on_save_result(request_id, &result);
                    self.settings.on_save_result(request_id, &result);
                }
                Ok(CoreMsg::Focus) => self.focus_requested = true,
                Ok(CoreMsg::Close) => {
                    tracing::info!("core asked the window to close");
                    return Flow::Close;
                }
                Ok(m) => tracing::debug!(kind = crate::ui::process::kind(&m), "ignored"),
                Err(TryRecvError::Empty) if self.wizard_finished() => return Flow::Close,
                Err(TryRecvError::Empty) => return Flow::Continue,
                Err(TryRecvError::Disconnected) => {
                    tracing::info!("core connection ended, closing window");
                    return Flow::Close;
                }
            }
        }
    }

    /// The wizard window closes once saved or cancelled.
    fn wizard_finished(&self) -> bool {
        self.mode == UiMode::Wizard
            && (self.wizard.step == WizardStep::Done || self.wizard.cancelled)
    }

    /// eframe shows the window only after the first frame, so activate on
    /// the second, then retry a few times while the window has no focus.
    fn activation_due(&mut self, now: Instant) -> bool {
        self.frames = self.frames.saturating_add(1);
        let due = match (self.frames, self.last_activation) {
            (0 | 1, _) => false,
            (2, _) => true,
            (_, Some(last)) => self.activation_pending() && now - last >= ACTIVATION_RETRY,
            (_, None) => false,
        };
        if due {
            self.activations += 1;
            self.last_activation = Some(now);
        }
        due
    }

    /// Another activation attempt may follow.
    fn activation_pending(&self) -> bool {
        !self.had_focus && self.activations <= ACTIVATION_RETRIES
    }

    fn take_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.focus_requested)
    }

    fn note_focus(&mut self, focused: Option<bool>) {
        if focused == Some(true) {
            self.had_focus = true;
        }
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
    /// Also runs while the window is minimized or hidden (eframe then skips
    /// `ui`), so everything that must reach such a window happens here.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let (focused, escape) =
            ctx.input(|i| (i.viewport().focused, i.key_pressed(egui::Key::Escape)));
        if self.closing {
            return;
        }
        self.note_focus(focused);
        if self.drain_inbox() == Flow::Close || self.popover_dismissed(focused, escape) {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.take_focus_request() {
            // The core yielded (macOS) or allowed (Windows) the foreground.
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            (self.activator)(frame);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.frames == 0 {
            // Make sure a second frame follows, on which the window is visible.
            ui.ctx().request_repaint();
        }
        if self.activation_due(Instant::now()) {
            (self.activator)(frame);
            if self.frames == 2 {
                let ppp = ui.ctx().input(|i| i.viewport().native_pixels_per_point);
                if let Some(pos) = self.position_fix(ppp) {
                    tracing::debug!(?ppp, "monitor scale differs, moving the popover");
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                }
            }
        }
        if self.frames >= 2 && self.activation_pending() {
            ui.ctx().request_repaint_after(ACTIVATION_RETRY);
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
        let mut out = Vec::new();
        match (self.mode, &self.snapshot) {
            (UiMode::Wizard, _) => {
                wizard_ui(
                    ui,
                    &mut self.wizard,
                    self.strings,
                    &mut out,
                    &mut self.next_id,
                );
                if self.wizard_finished() {
                    // `logic` closes the window on the next frame.
                    ui.ctx().request_repaint();
                }
            }
            (UiMode::Settings, Some(snap)) => settings_ui(
                ui,
                snap,
                &mut self.settings,
                self.strings,
                &mut out,
                &mut self.next_id,
            ),
            _ => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| ui.spinner());
                });
            }
        }
        self.forward(out);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use eframe::App as _;
    use std::sync::mpsc::channel;
    use std::time::Instant;

    use super::*;
    use crate::i18n::UI_EN;
    use crate::ipc::TestOutcome;
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
            headless: false,
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

    static ACTIVATIONS: AtomicUsize = AtomicUsize::new(0);

    fn count_activation(_: &eframe::Frame) {
        ACTIVATIONS.fetch_add(1, Ordering::SeqCst);
    }

    /// Commands one logic-only pass (what eframe runs for a minimized or
    /// hidden window) sends to the root viewport.
    fn logic_pass(a: &mut UiApp, ctx: &egui::Context) -> Vec<egui::ViewportCommand> {
        let mut frame = eframe::Frame::_new_kittest();
        let out = ctx.run_logic(&egui::RawInput::default(), |ctx| a.logic(ctx, &mut frame));
        out.viewport_commands
            .get(&egui::ViewportId::ROOT)
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn focus_request_is_handled_without_a_ui_pass() {
        let (a, tx) = app(UiMode::Settings);
        let mut a = a.with_activator(count_activation);
        let ctx = egui::Context::default();
        assert!(logic_pass(&mut a, &ctx).is_empty());
        tx.send(CoreMsg::Focus).unwrap();
        let cmds = logic_pass(&mut a, &ctx);
        assert_eq!(
            cmds,
            [
                egui::ViewportCommand::Minimized(false),
                egui::ViewportCommand::Focus
            ]
        );
        assert_eq!(ACTIVATIONS.load(Ordering::SeqCst), 1);
        // Once per request.
        assert!(logic_pass(&mut a, &ctx).is_empty());
        assert_eq!(ACTIVATIONS.load(Ordering::SeqCst), 1);
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
    fn results_reach_the_wizard_and_done_closes_it() {
        let (mut a, tx) = app(UiMode::Wizard);
        a.wizard.draft.url = "https://ntfy.example.org".into();
        let mut id = 7;
        a.wizard.next(&mut id).unwrap();
        tx.send(CoreMsg::TestResult {
            request_id: 7,
            outcome: TestOutcome::Ok { topics: vec![] },
        })
        .unwrap();
        assert_eq!(a.drain_inbox(), Flow::Continue);
        assert_eq!(a.wizard.step, WizardStep::Auth);

        a.wizard.step = WizardStep::Notifications;
        a.wizard.busy = Some(8);
        tx.send(CoreMsg::SaveResult {
            request_id: 8,
            result: Ok("a".into()),
        })
        .unwrap();
        assert_eq!(a.drain_inbox(), Flow::Close);
        assert_eq!(a.wizard.step, WizardStep::Done);
    }

    #[test]
    fn cancelled_wizard_closes_settings_editor_does_not() {
        let (mut a, _tx) = app(UiMode::Wizard);
        assert_eq!(a.drain_inbox(), Flow::Continue);
        a.wizard.cancelled = true;
        assert_eq!(a.drain_inbox(), Flow::Close);
        let (mut s, _tx) = app(UiMode::Settings);
        s.wizard.cancelled = true;
        assert_eq!(s.drain_inbox(), Flow::Continue);
    }

    #[test]
    fn results_and_snapshots_reach_the_settings() {
        let (mut a, tx) = app(UiMode::Settings);
        a.settings.pending.insert(4, "a".into());
        tx.send(CoreMsg::SaveResult {
            request_id: 4,
            result: Ok("a".into()),
        })
        .unwrap();
        assert_eq!(a.drain_inbox(), Flow::Continue);
        assert!(a.settings.locked.contains("a"));
        a.settings.editor = Some(Wizard::new());
        let mut snap = snapshot();
        snap.urgent_breaks_dnd = true;
        tx.send(CoreMsg::Snapshot(snap)).unwrap();
        assert_eq!(a.drain_inbox(), Flow::Continue);
        assert!(a.settings.locked.is_empty());
        assert!(a.settings.editor.as_ref().unwrap().urgent_global);
        assert!(a.wizard.urgent_global);
    }

    #[test]
    fn snapshot_reenables_the_popover_topic_row() {
        let (mut a, tx) = app(UiMode::Popover);
        a.popover.awaiting_snapshot = true;
        tx.send(CoreMsg::Snapshot(snapshot())).unwrap();
        assert_eq!(a.drain_inbox(), Flow::Continue);
        assert!(!a.popover.awaiting_snapshot);
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
        let t = Instant::now();
        assert!(!a.activation_due(t));
        assert!(a.activation_due(t));
        a.had_focus = true;
        for i in 1..10 {
            assert!(!a.activation_due(t + ACTIVATION_RETRY * i));
        }
    }

    #[test]
    fn activation_is_retried_until_focused_a_few_times() {
        let (mut a, _tx) = app(UiMode::Settings);
        let t = Instant::now();
        assert!(!a.activation_due(t));
        assert!(a.activation_due(t));
        assert!(!a.activation_due(t + ACTIVATION_RETRY / 2), "too soon");
        let mut retries = 0;
        for i in 1..=20 {
            if a.activation_due(t + ACTIVATION_RETRY * i) {
                retries += 1;
            }
        }
        assert_eq!(retries, ACTIVATION_RETRIES);
        assert!(!a.activation_pending());
    }

    #[test]
    fn focus_is_tracked_in_every_mode() {
        let (mut a, _tx) = app(UiMode::Wizard);
        assert!(a.activation_pending());
        a.note_focus(Some(true));
        assert!(a.had_focus && !a.activation_pending());
    }

    fn placed(scale: f32) -> UiArgs {
        let mut a = args(UiMode::Popover);
        a.anchor = Some(AnchorRect {
            x: 2400.0,
            y: 0.0,
            w: 48.0,
            h: 48.0,
        });
        a.monitor = Some((0.0, 0.0, 2880.0, 1800.0));
        a.scale = Some(scale);
        a
    }

    #[test]
    fn popover_is_moved_when_the_real_scale_differs() {
        let (a, _tx) = app(UiMode::Popover);
        let a = a.with_placement(&placed(1.0));
        assert_eq!(a.position_fix(Some(1.005)), None);
        assert_eq!(a.position_fix(None), None);
        let anchor = placed(2.0).anchor.unwrap();
        let (x, y) = popover_position(&anchor, POPOVER_SIZE, 2.0, (0.0, 0.0, 2880.0, 1800.0));
        assert_eq!(a.position_fix(Some(2.0)), Some(egui::pos2(x, y)));
    }

    #[test]
    fn only_a_placed_popover_is_moved() {
        let (a, _tx) = app(UiMode::Popover);
        assert_eq!(a.position_fix(Some(2.0)), None);
        let (s, _tx) = app(UiMode::Settings);
        let s = s.with_placement(&placed(1.0));
        assert_eq!(s.position_fix(Some(2.0)), None);
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
