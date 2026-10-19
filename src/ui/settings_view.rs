//! Settings window: servers with their topics, global switches, folders, version.

use std::collections::BTreeSet;

use eframe::egui::{self, Align, Color32, ComboBox, Frame, Label, Layout, RichText, ScrollArea};

use crate::i18n::{UiStrings, fill};
use crate::ipc::{TestOutcome, UiRequest};
use crate::settings::Topic;
use crate::snapshot::{InstanceStatus, InstanceView, UiSnapshot, draft_from_view};
use crate::ui::problem::Problem;
use crate::ui::wizard::{Wizard, WizardStep, wizard_ui};

/// 12 h, 24 h, 72 h, 7 days.
pub const CACHE_CHOICES: [i64; 4] = [43_200, 86_400, 259_200, 604_800];

const GREEN: Color32 = Color32::from_rgb(52, 199, 89);
const RED: Color32 = Color32::from_rgb(255, 59, 48);
const GREY: Color32 = Color32::from_rgb(142, 142, 147);

/// What the settings window keeps between frames.
#[derive(Default)]
pub struct SettingsState {
    /// The embedded wizard for adding or editing a server.
    pub editor: Option<Wizard>,
    /// Instance whose removal awaits confirmation.
    pub confirm_remove: Option<String>,
    /// Running save (request id, instance id) from this list.
    pub pending: Option<(u64, String)>,
    /// Instances changed by a confirmed request, disabled until the next snapshot shows the change.
    pub locked: BTreeSet<String>,
    pub error: Option<Problem>,
}

impl SettingsState {
    pub fn on_test_result(&mut self, request_id: u64, outcome: &TestOutcome) {
        if let Some(w) = &mut self.editor {
            w.on_test_result(request_id, outcome);
        }
    }

    /// The result arrives just before the snapshot with the change, so the
    /// changed server stays disabled until `on_snapshot`.
    pub fn on_save_result(&mut self, request_id: u64, result: &Result<String, String>) {
        if let Some(w) = &mut self.editor {
            w.on_save_result(request_id, result);
            if w.step == WizardStep::Done {
                if let Ok(id) = result {
                    self.locked.insert(id.clone());
                }
                self.editor = None;
                return;
            }
        }
        if self
            .pending
            .as_ref()
            .is_none_or(|(id, _)| *id != request_id)
        {
            return;
        }
        let Some((_, instance_id)) = self.pending.take() else {
            return;
        };
        match result {
            Ok(_) => {
                self.locked.insert(instance_id);
                self.error = None;
            }
            Err(reason) => self.error = Some(Problem::SaveFailed(reason.clone())),
        }
    }

    pub fn on_snapshot(&mut self, snap: &UiSnapshot) {
        self.locked.clear();
        if let Some(w) = &mut self.editor {
            w.urgent_global = snap.urgent_breaks_dnd;
        }
    }

    fn enabled(&self, instance_id: &str) -> bool {
        !self.locked.contains(instance_id)
            && self
                .pending
                .as_ref()
                .is_none_or(|(_, id)| id != instance_id)
    }
}

pub fn cache_label(secs: i64, s: &UiStrings) -> String {
    let hours = |h: i64| fill(s.cache_hours, &[("n", &h.to_string())]);
    match secs {
        604_800 => fill(s.cache_days, &[("n", "7")]),
        _ if CACHE_CHOICES.contains(&secs) => hours(secs / 3600),
        _ => {
            let v = if secs % 3600 == 0 {
                hours(secs / 3600)
            } else {
                format!("{secs} s")
            };
            fill(s.cache_custom, &[("v", &v)])
        }
    }
}

fn status_look(st: InstanceStatus, s: &UiStrings) -> (&'static str, Color32) {
    match st {
        InstanceStatus::Connected => (s.status_connected, GREEN),
        InstanceStatus::Connecting => (s.status_connecting, GREY),
        InstanceStatus::Problem => (s.status_problem, RED),
        InstanceStatus::AuthFailed => (s.status_auth_failed, RED),
    }
}

/// Draws the settings (or the embedded wizard) and collects the user's requests in `out`.
pub fn settings_ui(
    ui: &mut egui::Ui,
    snap: &UiSnapshot,
    state: &mut SettingsState,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    if let Some(w) = &mut state.editor {
        wizard_ui(ui, w, s, out, next_id);
        if w.cancelled {
            state.editor = None;
            ui.ctx().request_repaint();
        }
        return;
    }
    ui.style_mut().interaction.selectable_labels = false;
    egui::Panel::bottom("settings_footer").show(ui, |ui| footer(ui, snap, s, out));
    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.heading(s.servers_heading);
            ui.add_space(6.0);
            for inst in &snap.instances {
                instance(ui, inst, state, s, out, next_id);
                ui.add_space(6.0);
            }
            if let Some(e) = &state.error {
                ui.add(
                    Label::new(RichText::new(e.text(s)).color(ui.visuals().error_fg_color)).wrap(),
                );
            }
            if ui.button(s.add_server).clicked() {
                state.editor = Some(Wizard::new());
            }
            if let Some(w) = &mut state.editor {
                w.urgent_global = snap.urgent_breaks_dnd;
                state.confirm_remove = None;
            }
            ui.add_space(12.0);
            ui.separator();
            ui.heading(s.general_heading);
            ui.add_space(6.0);
            let mut urgent = snap.urgent_breaks_dnd;
            if ui.checkbox(&mut urgent, s.urgent_global).changed() {
                out.push(UiRequest::SetUrgentGlobal { enabled: urgent });
            }
        });
    });
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let size = ui.spacing().interact_size.y * 0.5;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), size * 0.4, color);
}

fn instance(
    ui: &mut egui::Ui,
    inst: &InstanceView,
    state: &mut SettingsState,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    let enabled = state.enabled(&inst.id);
    let confirming = state.confirm_remove.as_deref() == Some(inst.id.as_str());
    Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                let (text, color) = status_look(inst.status, s);
                dot(ui, color);
                ui.label(RichText::new(&inst.host).strong());
                ui.label(RichText::new(text).small().weak());
                if !confirming {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(s.remove).clicked() {
                            state.confirm_remove = Some(inst.id.clone());
                        }
                        if ui.button(s.edit).clicked() {
                            state.editor = Some(Wizard::edit(inst));
                        }
                    });
                }
            });
            if confirming {
                confirm_remove(ui, inst, state, s, out);
            }
            ui.add_space(4.0);
            cache_duration(ui, inst, state, s, out, next_id);
            ui.add_space(4.0);
            for t in &inst.topics {
                topic_row(ui, inst, t, s, out);
            }
        });
    });
}

fn confirm_remove(
    ui: &mut egui::Ui,
    inst: &InstanceView,
    state: &mut SettingsState,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
) {
    ui.horizontal(|ui| {
        let warn = ui.visuals().warn_fg_color;
        ui.label(RichText::new(fill(s.remove_confirm, &[("host", &inst.host)])).color(warn));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button(s.cancel).clicked() {
                state.confirm_remove = None;
            }
            if ui.button(s.remove).clicked() {
                state.confirm_remove = None;
                state.locked.insert(inst.id.clone());
                out.push(UiRequest::RemoveInstance {
                    instance_id: inst.id.clone(),
                });
            }
        });
    });
}

fn cache_duration(
    ui: &mut egui::Ui,
    inst: &InstanceView,
    state: &mut SettingsState,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    let current = inst.cache_duration_secs;
    let mut chosen = current;
    ui.horizontal(|ui| {
        let label = ui.label(s.cache_duration);
        ComboBox::from_id_salt(("cache_duration", &inst.id))
            .selected_text(cache_label(current, s))
            .show_ui(ui, |ui| {
                for secs in CACHE_CHOICES {
                    ui.selectable_value(&mut chosen, secs, cache_label(secs, s));
                }
            })
            .response
            .labelled_by(label.id);
    });
    ui.label(RichText::new(s.cache_hint).small().weak());
    if chosen != current {
        let mut draft = draft_from_view(inst);
        draft.cache_duration_secs = chosen;
        let request_id = *next_id;
        *next_id += 1;
        state.pending = Some((request_id, inst.id.clone()));
        state.error = None;
        out.push(UiRequest::SaveInstance { request_id, draft });
    }
}

fn topic_row(
    ui: &mut egui::Ui,
    inst: &InstanceView,
    t: &Topic,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
) {
    ui.horizontal(|ui| {
        ui.label(&t.name);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let mut muted = t.muted;
            if ui.checkbox(&mut muted, s.muted).changed() {
                out.push(UiRequest::SetTopicMuted {
                    instance_id: inst.id.clone(),
                    topic: t.name.clone(),
                    muted,
                });
            }
            ui.add_space(8.0);
            let mut urgent = t.urgent_breaks_dnd;
            // Right to left: drawn as Default | On | Off.
            for (value, label) in [
                (Some(false), s.urgent_off),
                (Some(true), s.urgent_on),
                (None, s.urgent_default),
            ] {
                ui.selectable_value(&mut urgent, value, label);
            }
            ui.label(RichText::new(s.urgent_short).weak());
            if urgent != t.urgent_breaks_dnd {
                out.push(UiRequest::SetTopicUrgent {
                    instance_id: inst.id.clone(),
                    topic: t.name.clone(),
                    urgent,
                });
            }
        });
    });
}

fn footer(ui: &mut egui::Ui, snap: &UiSnapshot, s: &UiStrings, out: &mut Vec<UiRequest>) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(s.open_config).clicked() {
            out.push(UiRequest::OpenConfigDir);
        }
        if ui.button(s.open_logs).clicked() {
            out.push(UiRequest::OpenLogs);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let version = fill(s.version, &[("v", &snap.app_version)]);
            ui.label(RichText::new(version).small().weak());
        });
    });
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::i18n::UI_EN;
    use crate::instance_edit::AuthDraft;
    use crate::snapshot::AuthView;

    struct T {
        snap: UiSnapshot,
        state: SettingsState,
        out: Vec<UiRequest>,
        next_id: u64,
    }

    fn topic(name: &str, urgent: Option<bool>) -> Topic {
        Topic {
            name: name.into(),
            muted: false,
            urgent_breaks_dnd: urgent,
        }
    }

    fn snapshot() -> UiSnapshot {
        UiSnapshot {
            app_version: "1.2.3".into(),
            configured: true,
            instances: vec![InstanceView {
                id: "a".into(),
                host: "ntfy.example.org".into(),
                url: "https://ntfy.example.org".into(),
                status: InstanceStatus::Connected,
                auth: AuthView::Token,
                cache_duration_secs: 43_200,
                topics: vec![topic("alerts", None), topic("backups", Some(false))],
            }],
            recent: vec![],
            gaps: vec![],
            paused_until: None,
            urgent_breaks_dnd: false,
        }
    }

    fn harness(snap: UiSnapshot) -> Harness<'static, T> {
        let mut h = Harness::builder()
            .with_size(egui::vec2(560.0, 900.0))
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, t: &mut T| {
                    settings_ui(
                        ui,
                        &t.snap,
                        &mut t.state,
                        &UI_EN,
                        &mut t.out,
                        &mut t.next_id,
                    )
                },
                T {
                    snap,
                    state: SettingsState::default(),
                    out: Vec::new(),
                    next_id: 1,
                },
            );
        h.run_steps(4);
        h
    }

    fn click(h: &mut Harness<'static, T>, label: &str) {
        h.get_by_label(label).click();
        h.run_steps(4);
    }

    fn open_cache_menu(h: &mut Harness<'static, T>) {
        h.get_by_role_and_label(egui::Role::ComboBox, UI_EN.cache_duration)
            .click();
        h.run_steps(4);
    }

    fn cache_value(h: &Harness<'static, T>) -> String {
        h.get_by_role_and_label(egui::Role::ComboBox, UI_EN.cache_duration)
            .value()
            .unwrap_or_default()
    }

    fn take(h: &mut Harness<'static, T>) -> Vec<UiRequest> {
        std::mem::take(&mut h.state_mut().out)
    }

    #[test]
    fn shows_servers_topics_and_footer() {
        let mut h = harness(snapshot());
        for label in [
            "Servers",
            "ntfy.example.org",
            "Connected",
            "alerts",
            "backups",
            "Version 1.2.3",
            UI_EN.cache_hint,
            UI_EN.urgent_global,
        ] {
            h.get_by_label(label);
        }
        assert_eq!(cache_value(&h), "12 h");
        click(&mut h, UI_EN.open_config);
        click(&mut h, UI_EN.open_logs);
        click(&mut h, UI_EN.urgent_global);
        assert_eq!(
            take(&mut h),
            [
                UiRequest::OpenConfigDir,
                UiRequest::OpenLogs,
                UiRequest::SetUrgentGlobal { enabled: true }
            ]
        );
    }

    #[test]
    fn settings_remove_needs_confirmation() {
        let mut h = harness(snapshot());
        click(&mut h, "Remove");
        assert!(take(&mut h).is_empty());
        h.get_by_label("Really remove ntfy.example.org?");
        click(&mut h, "Cancel");
        assert!(
            h.query_by_label("Really remove ntfy.example.org?")
                .is_none()
        );
        assert!(take(&mut h).is_empty());

        click(&mut h, "Remove");
        click(&mut h, "Remove");
        assert_eq!(
            take(&mut h),
            [UiRequest::RemoveInstance {
                instance_id: "a".into()
            }]
        );
        // Disabled until the snapshot without the server arrives.
        click(&mut h, "Remove");
        assert!(take(&mut h).is_empty());
        let mut snap = snapshot();
        snap.instances.clear();
        h.state_mut().state.on_snapshot(&snap);
        h.state_mut().snap = snap;
        h.run_steps(4);
        assert!(h.query_by_label("ntfy.example.org").is_none());
    }

    #[test]
    fn settings_urgent_tristate() {
        let mut h = harness(snapshot());
        // alerts: Default selected → On
        h.get_all_by_label("On").next().unwrap().click();
        h.run_steps(4);
        // backups: Off selected → Default
        h.get_all_by_label("Default").nth(1).unwrap().click();
        h.run_steps(4);
        // alerts: Default again is no change
        h.get_all_by_label("Default").next().unwrap().click();
        h.run_steps(4);
        h.get_all_by_label("Off").next().unwrap().click();
        h.run_steps(4);
        h.get_all_by_label(UI_EN.muted).nth(1).unwrap().click();
        h.run_steps(4);
        let urgent = |topic: &str, urgent| UiRequest::SetTopicUrgent {
            instance_id: "a".into(),
            topic: topic.into(),
            urgent,
        };
        assert_eq!(
            take(&mut h),
            [
                urgent("alerts", Some(true)),
                urgent("backups", None),
                urgent("alerts", Some(false)),
                UiRequest::SetTopicMuted {
                    instance_id: "a".into(),
                    topic: "backups".into(),
                    muted: true,
                }
            ]
        );
    }

    #[test]
    fn cache_duration_saves_and_locks_until_snapshot() {
        let mut h = harness(snapshot());
        open_cache_menu(&mut h);
        click(&mut h, "7 days");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { request_id, draft }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(draft.id.as_deref(), Some("a"));
        assert_eq!(draft.cache_duration_secs, 604_800);
        assert_eq!(draft.auth, AuthDraft::Token { token: None });
        assert_eq!(draft.topics, snapshot().instances[0].topics);
        let request_id = *request_id;
        assert_eq!(h.state().state.pending, Some((request_id, "a".into())));

        click(&mut h, "Remove");
        assert!(
            h.query_by_label("Really remove ntfy.example.org?")
                .is_none()
        );

        h.state_mut()
            .state
            .on_save_result(request_id, &Err("could not save settings".into()));
        h.run_steps(4);
        h.get_by_label("Saving failed: could not save settings");
        assert_eq!(h.state().state.pending, None);

        open_cache_menu(&mut h);
        click(&mut h, "24 h");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { request_id, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        h.state_mut()
            .state
            .on_save_result(*request_id, &Ok("a".into()));
        h.run_steps(4);
        assert_eq!(h.state().state.error, None);
        click(&mut h, "Remove");
        assert!(
            h.query_by_label("Really remove ntfy.example.org?")
                .is_none(),
            "locked until the snapshot"
        );
        let snap = snapshot();
        h.state_mut().state.on_snapshot(&snap);
        h.run_steps(4);
        click(&mut h, "Remove");
        h.get_by_label("Really remove ntfy.example.org?");
    }

    #[test]
    fn cache_labels() {
        assert_eq!(cache_label(43_200, &UI_EN), "12 h");
        assert_eq!(cache_label(259_200, &UI_EN), "72 h");
        assert_eq!(cache_label(604_800, &UI_EN), "7 days");
        assert_eq!(cache_label(7_200, &UI_EN), "Custom (2 h)");
        assert_eq!(cache_label(5_000, &UI_EN), "Custom (5000 s)");
        let mut snap = snapshot();
        snap.instances[0].cache_duration_secs = 7_200;
        let h = harness(snap);
        assert_eq!(cache_value(&h), "Custom (2 h)");
    }

    #[test]
    fn edit_opens_wizard_and_returns_after_save() {
        let mut h = harness(snapshot());
        click(&mut h, "Edit");
        h.get_by_label(UI_EN.server_url);
        assert!(h.query_by_label("Servers").is_none());
        click(&mut h, "Next");
        let out = take(&mut h);
        let [UiRequest::TestConnection { request_id, draft }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(draft.id.as_deref(), Some("a"));
        let id = *request_id;
        h.state_mut()
            .state
            .on_test_result(id + 5, &TestOutcome::NotNtfy);
        h.state_mut()
            .state
            .on_test_result(id, &TestOutcome::Ok { topics: vec![] });
        h.run_steps(4);
        assert_eq!(
            h.state().state.editor.as_ref().unwrap().step,
            WizardStep::Auth
        );
        // Jump to the end and finish.
        h.state_mut().state.editor.as_mut().unwrap().step = WizardStep::Notifications;
        h.run_steps(4);
        click(&mut h, "Finish");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { request_id, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        h.state_mut()
            .state
            .on_save_result(*request_id, &Ok("a".into()));
        h.run_steps(4);
        assert!(h.state().state.editor.is_none());
        h.get_by_label("Servers");
        click(&mut h, "Remove");
        assert!(
            h.query_by_label("Really remove ntfy.example.org?")
                .is_none(),
            "edited server locked until the snapshot"
        );
    }

    #[test]
    fn add_server_and_cancel() {
        let mut h = harness(snapshot());
        h.state_mut().snap.urgent_breaks_dnd = true;
        click(&mut h, UI_EN.add_server);
        let w = h.state().state.editor.as_ref().unwrap();
        assert!(!w.editing);
        assert!(w.urgent_global);
        click(&mut h, "Cancel");
        assert!(h.state().state.editor.is_none());
        h.get_by_label("Servers");
        assert!(take(&mut h).is_empty());
    }
}
