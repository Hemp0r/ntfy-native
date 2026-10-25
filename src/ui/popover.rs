//! Tray popover: status, topic switches, gap hints, recent messages and the footer.

use chrono::{Local, TimeZone};
use eframe::egui::{
    self, Align, Button, Color32, ComboBox, CornerRadius, Frame, Key, Label, Layout, Margin,
    RichText, ScrollArea, Sense, Shape, TextEdit, TextStyle, TextWrapMode, UiBuilder, WidgetInfo,
    WidgetText, text::LayoutJob,
};

use crate::i18n::{UiStrings, fill};
use crate::ipc::UiRequest;
use crate::settings::Topic;
use crate::snapshot::{InstanceStatus, InstanceView, RecentView, UiSnapshot, draft_from_view};
use crate::state::RECENT_MAX;
use crate::ui::problem::Problem;
use crate::ui::time::{PauseChoice, format_age, format_paused, pause_until};
use crate::validate::valid_topic_name;

const GREEN: Color32 = Color32::from_rgb(52, 199, 89);
const RED: Color32 = Color32::from_rgb(255, 59, 48);
const GREY: Color32 = Color32::from_rgb(142, 142, 147);

/// What the popover keeps between frames: the "+ Topic" field and its pending save.
#[derive(Debug, Default)]
pub struct PopoverState {
    pub add_topic: String,
    pub add_instance: Option<String>,
    pub pending: Option<u64>,
    /// Saved, but the snapshot with the new topic has not arrived yet.
    pub awaiting_snapshot: bool,
    pub error: Option<Problem>,
}

impl PopoverState {
    /// The result arrives just before the snapshot with the new topic: clear
    /// the field now, keep the row disabled until `on_snapshot`.
    pub fn on_save_result(&mut self, request_id: u64, result: &Result<String, String>) {
        if self.pending != Some(request_id) {
            return;
        }
        self.pending = None;
        match result {
            Ok(_) => {
                self.add_topic.clear();
                self.error = None;
                self.awaiting_snapshot = true;
            }
            Err(e) => self.error = Some(Problem::SaveFailed(e.clone())),
        }
    }

    pub fn on_snapshot(&mut self) {
        self.awaiting_snapshot = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overall {
    Connected,
    Connecting,
    Problem,
    AuthFailed,
    Paused(i64),
}

/// Same precedence as the tray: problems outrank a pause. Instances without topics don't count.
pub fn overall_status(snap: &UiSnapshot, now: i64) -> Overall {
    let active = || snap.instances.iter().filter(|i| !i.topics.is_empty());
    let any = |st| active().any(|i| i.status == st);
    if any(InstanceStatus::AuthFailed) {
        Overall::AuthFailed
    } else if any(InstanceStatus::Problem) {
        Overall::Problem
    } else if let Some(until) = snap.paused_until.filter(|u| *u > now) {
        Overall::Paused(until)
    } else if any(InstanceStatus::Connecting) {
        Overall::Connecting
    } else {
        Overall::Connected
    }
}

fn status_text(st: InstanceStatus, s: &UiStrings) -> &'static str {
    match st {
        InstanceStatus::Connected => s.status_connected,
        InstanceStatus::Connecting => s.status_connecting,
        InstanceStatus::Problem => s.status_problem,
        InstanceStatus::AuthFailed => s.status_auth_failed,
    }
}

fn status_color(st: InstanceStatus) -> Color32 {
    match st {
        InstanceStatus::Connected => GREEN,
        InstanceStatus::Connecting => GREY,
        InstanceStatus::Problem | InstanceStatus::AuthFailed => RED,
    }
}

/// Draws the popover and collects the user's requests in `out`; no other side effects.
pub fn popover_ui(
    ui: &mut egui::Ui,
    snap: &UiSnapshot,
    state: &mut PopoverState,
    s: &UiStrings,
    now: i64,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    ui.style_mut().interaction.selectable_labels = false;
    if !snap.configured {
        not_configured(ui, s, out);
        return;
    }
    egui::Panel::top("popover_header").show(ui, |ui| header(ui, snap, s, now));
    egui::Panel::bottom("popover_footer").show(ui, |ui| {
        footer(ui, snap, state, s, now, out, next_id);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for inst in &snap.instances {
                instance(ui, inst, s, out);
            }
            gaps(ui, snap, s, out);
            ui.separator();
            recent(ui, snap, s, now, out);
        });
    });
}

fn not_configured(ui: &mut egui::Ui, s: &UiStrings, out: &mut Vec<UiRequest>) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.heading(s.popover_title);
            ui.add_space(8.0);
            ui.label(s.not_configured);
            ui.add_space(8.0);
            if ui.button(s.setup).clicked() {
                out.push(UiRequest::OpenWizard);
            }
        });
    });
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let size = ui.spacing().interact_size.y * 0.5;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size * 0.4, color);
}

fn header(ui: &mut egui::Ui, snap: &UiSnapshot, s: &UiStrings, now: i64) {
    let (text, color) = match overall_status(snap, now) {
        Overall::Connected => (s.status_connected.to_owned(), GREEN),
        Overall::Connecting => (s.status_connecting.to_owned(), GREY),
        Overall::Problem => (s.status_problem.to_owned(), RED),
        Overall::AuthFailed => (s.status_auth_failed.to_owned(), RED),
        Overall::Paused(until) => (format_paused(until, now, s), GREY),
    };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.heading(s.popover_title);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(text);
            dot(ui, color);
        });
    });
    ui.add_space(2.0);
}

fn instance(ui: &mut egui::Ui, inst: &InstanceView, s: &UiStrings, out: &mut Vec<UiRequest>) {
    ui.horizontal(|ui| {
        dot(ui, status_color(inst.status));
        ui.label(RichText::new(&inst.host).strong());
        if inst.status != InstanceStatus::Connected {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(status_text(inst.status, s)).small().weak());
            });
        }
    });
    for t in &inst.topics {
        let mut on = !t.muted;
        if topic_switch(ui, &t.name, &mut on).changed() {
            out.push(UiRequest::SetTopicMuted {
                instance_id: inst.id.clone(),
                topic: t.name.clone(),
                muted: !on,
            });
        }
    }
    ui.add_space(6.0);
}

/// One row: topic name on the left, an on/off switch on the right; the whole row is clickable.
fn topic_switch(ui: &mut egui::Ui, name: &str, on: &mut bool) -> egui::Response {
    let height = ui.spacing().interact_size.y;
    let width = ui.available_width();
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, name));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, *on);
        let switch_size = egui::vec2(height * 1.6, height * 0.8);
        let switch = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - switch_size.x - 2.0,
                rect.center().y - switch_size.y / 2.0,
            ),
            switch_size,
        );
        let indent = ui.spacing().interact_size.y * 0.5 + ui.spacing().item_spacing.x;
        let text_width = (switch.left() - rect.left() - indent - 8.0).max(0.0);
        let galley = WidgetText::from(name).into_galley(
            ui,
            Some(TextWrapMode::Truncate),
            text_width,
            TextStyle::Body,
        );
        let text_pos = egui::pos2(
            rect.left() + indent,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter()
            .galley(text_pos, galley, ui.visuals().text_color());

        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let radius = switch.height() / 2.0;
        let fill = if *on {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        ui.painter()
            .rect_filled(switch, CornerRadius::same(radius as u8), fill);
        let knob_x = egui::lerp((switch.left() + radius)..=(switch.right() - radius), t);
        ui.painter().circle(
            egui::pos2(knob_x, switch.center().y),
            radius * 0.8,
            Color32::WHITE,
            visuals.bg_stroke,
        );
    }
    response
}

fn gaps(ui: &mut egui::Ui, snap: &UiSnapshot, s: &UiStrings, out: &mut Vec<UiRequest>) {
    let multi = snap.instances.len() > 1;
    for g in &snap.gaps {
        let topic = if multi {
            format!("{}/{}", g.host, g.topic)
        } else {
            g.topic.clone()
        };
        let warn = ui.visuals().warn_fg_color;
        Frame::new()
            .fill(warn.gamma_multiply(0.12))
            .corner_radius(6)
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(s.dismiss).clicked() {
                            out.push(UiRequest::DismissGap {
                                instance_id: g.instance_id.clone(),
                                topic: g.topic.clone(),
                            });
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let text = fill(s.gap_hint, &[("topic", &topic)]);
                            ui.add(Label::new(RichText::new(text).color(warn)).wrap());
                        });
                    })
                });
            });
        ui.add_space(4.0);
    }
}

fn recent(ui: &mut egui::Ui, snap: &UiSnapshot, s: &UiStrings, now: i64, out: &mut Vec<UiRequest>) {
    ui.label(RichText::new(s.recent_heading).strong());
    if snap.recent.is_empty() {
        ui.label(RichText::new(s.recent_empty).weak());
    }
    for m in snap.recent.iter().take(RECENT_MAX) {
        recent_entry(ui, m, s, now, out);
    }
}

fn recent_entry(
    ui: &mut egui::Ui,
    m: &RecentView,
    s: &UiStrings,
    now: i64,
    out: &mut Vec<UiRequest>,
) {
    let sense = if m.has_click {
        Sense::click()
    } else {
        Sense::hover()
    };
    let background = ui.painter().add(Shape::Noop);
    let builder = UiBuilder::new()
        .id_salt(("recent", &m.instance_id, &m.message_id))
        .sense(sense);
    let response = ui
        .scope_builder(builder, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let meta = format!("{} · {}", m.topic, format_age(now, m.time, s));
                    ui.label(RichText::new(meta).small().weak());
                    if !m.title.is_empty() {
                        ui.add(Label::new(RichText::new(&m.title).strong()).truncate());
                    }
                    if !m.body.is_empty() {
                        body(ui, &m.body);
                    }
                });
        })
        .response;
    if !m.has_click {
        return;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let fill = ui.visuals().widgets.hovered.weak_bg_fill;
        ui.painter().set(
            background,
            egui::epaint::RectShape::filled(response.rect, 6, fill),
        );
    }
    if response.clicked() {
        out.push(UiRequest::OpenRecent {
            instance_id: m.instance_id.clone(),
            message_id: m.message_id.clone(),
        });
    }
}

/// At most two lines, the rest elided.
fn body(ui: &mut egui::Ui, text: &str) {
    let font = TextStyle::Body.resolve(ui.style());
    let mut job = LayoutJob::simple(
        text.to_owned(),
        font,
        ui.visuals().text_color(),
        ui.available_width(),
    );
    job.wrap.max_rows = 2;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    ui.add(Label::new(galley));
}

fn footer(
    ui: &mut egui::Ui,
    snap: &UiSnapshot,
    state: &mut PopoverState,
    s: &UiStrings,
    now: i64,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    ui.add_space(6.0);
    if !snap.instances.is_empty() {
        add_topic(ui, snap, state, s, out, next_id);
    }
    if let Some(e) = &state.error {
        ui.add(Label::new(RichText::new(e.text(s)).color(ui.visuals().error_fg_color)).wrap());
    }
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        pause_menu(ui, snap, s, now, out);
        if ui.button(s.settings).clicked() {
            out.push(UiRequest::OpenSettings);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button(s.quit).clicked() {
                out.push(UiRequest::Quit);
            }
        });
    });
    ui.add_space(4.0);
}

fn add_topic(
    ui: &mut egui::Ui,
    snap: &UiSnapshot,
    state: &mut PopoverState,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    let target = state
        .add_instance
        .as_deref()
        .and_then(|id| snap.instances.iter().find(|i| i.id == id))
        .or(snap.instances.first());
    let Some(target) = target else { return };
    ui.add_enabled_ui(state.pending.is_none() && !state.awaiting_snapshot, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let clicked = ui.button(s.add_topic).clicked();
                if snap.instances.len() > 1 {
                    let picker = ComboBox::from_id_salt("add_topic_instance")
                        .selected_text(&target.host)
                        .width(110.0)
                        .truncate()
                        .show_ui(ui, |ui| {
                            for i in &snap.instances {
                                ui.selectable_value(
                                    &mut state.add_instance,
                                    Some(i.id.clone()),
                                    &i.host,
                                );
                            }
                        })
                        .response;
                    ui.ctx().accesskit_node_builder(picker.id, |node| {
                        node.set_label(s.topic_server);
                    });
                }
                let edit = ui.add(
                    TextEdit::singleline(&mut state.add_topic)
                        .hint_text(s.topic_name)
                        .desired_width(f32::INFINITY),
                );
                if edit.changed() {
                    state.error = None;
                }
                let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if clicked || entered {
                    if state.add_topic.trim().is_empty() {
                        edit.request_focus();
                    } else {
                        submit_topic(target, state, out, next_id);
                    }
                }
            })
        });
    });
}

fn submit_topic(
    target: &InstanceView,
    state: &mut PopoverState,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    let name = state.add_topic.trim();
    if !valid_topic_name(name) {
        state.error = Some(Problem::TopicInvalid);
        return;
    }
    if target.topics.iter().any(|t| t.name == name) {
        state.error = Some(Problem::TopicDuplicate);
        return;
    }
    let mut draft = draft_from_view(target);
    draft.topics.push(Topic {
        name: name.to_owned(),
        ..Topic::default()
    });
    let request_id = *next_id;
    *next_id += 1;
    state.pending = Some(request_id);
    state.error = None;
    out.push(UiRequest::SaveInstance { request_id, draft });
}

fn pause_menu(
    ui: &mut egui::Ui,
    snap: &UiSnapshot,
    s: &UiStrings,
    now: i64,
    out: &mut Vec<UiRequest>,
) {
    let paused = snap.paused_until.is_some_and(|u| u > now);
    ui.menu_button(format!("{} ▾", s.pause), |ui| {
        let choices = [
            (PauseChoice::Minutes15, s.pause_15m),
            (PauseChoice::Hour1, s.pause_1h),
            (PauseChoice::UntilTomorrow, s.pause_tomorrow),
            (PauseChoice::Indefinitely, s.pause_forever),
            (PauseChoice::Resume, s.resume),
        ];
        for (choice, label) in choices {
            let enabled = choice != PauseChoice::Resume || paused;
            if ui.add_enabled(enabled, Button::new(label)).clicked() {
                let local = Local
                    .timestamp_opt(now, 0)
                    .single()
                    .unwrap_or_else(Local::now);
                out.push(UiRequest::PauseAll {
                    until: pause_until(choice, local),
                });
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::i18n::UI_EN;
    use crate::instance_edit::AuthDraft;
    use crate::settings::Topic;
    use crate::snapshot::{AuthView, GapView, InstanceView, RecentView};

    const NOW: i64 = 1_800_000_000;

    struct T {
        snap: UiSnapshot,
        state: PopoverState,
        out: Vec<UiRequest>,
        next_id: u64,
    }

    fn topic(name: &str, muted: bool) -> Topic {
        Topic {
            name: name.into(),
            muted,
            urgent_breaks_dnd: None,
        }
    }

    fn instance(id: &str, host: &str, topics: Vec<Topic>) -> InstanceView {
        InstanceView {
            id: id.into(),
            host: host.into(),
            url: format!("https://{host}"),
            status: InstanceStatus::Connected,
            auth: AuthView::Token,
            cache_duration_secs: 43_200,
            topics,
        }
    }

    fn recent(id: &str, topic: &str, title: &str, has_click: bool, age: i64) -> RecentView {
        RecentView {
            instance_id: "a".into(),
            host: "ntfy.example.org".into(),
            topic: topic.into(),
            message_id: id.into(),
            time: NOW - age,
            title: title.into(),
            body: format!("body of {title}"),
            has_click,
        }
    }

    fn snapshot() -> UiSnapshot {
        UiSnapshot {
            app_version: "1".into(),
            configured: true,
            instances: vec![
                instance(
                    "a",
                    "ntfy.example.org",
                    vec![topic("alerts", false), topic("backups", false)],
                ),
                instance("b", "push.example.net", vec![topic("home", true)]),
            ],
            recent: vec![
                recent("m1", "alerts", "Disk full", true, 30),
                recent("m2", "backups", "Backup done", false, 600),
                recent("m3", "alerts", "Door opened", false, 7200),
            ],
            gaps: vec![],
            paused_until: None,
            urgent_breaks_dnd: false,
            autostart: crate::autostart::AutostartState::Disabled,
        }
    }

    fn harness(snap: UiSnapshot) -> Harness<'static, T> {
        harness_sized(snap, egui::vec2(360.0, 900.0))
    }

    fn harness_sized(snap: UiSnapshot, size: egui::Vec2) -> Harness<'static, T> {
        Harness::builder()
            .with_size(size)
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, t: &mut T| {
                    popover_ui(
                        ui,
                        &t.snap,
                        &mut t.state,
                        &UI_EN,
                        NOW,
                        &mut t.out,
                        &mut t.next_id,
                    )
                },
                T {
                    snap,
                    state: PopoverState::default(),
                    out: Vec::new(),
                    next_id: 1,
                },
            )
    }

    fn click(h: &mut Harness<'static, T>, label: &str) {
        h.get_by_label(label).click();
        h.run();
    }

    fn take(h: &mut Harness<'static, T>) -> Vec<UiRequest> {
        std::mem::take(&mut h.state_mut().out)
    }

    #[test]
    fn shows_status_topics_and_recent() {
        let mut h = harness(snapshot());
        h.run();
        for label in [
            "ntfy",
            "Connected",
            "ntfy.example.org",
            "push.example.net",
            "alerts",
            "backups",
            "home",
            "Recent",
            "Disk full",
            "Backup done",
            "Door opened",
            "alerts · just now",
            "backups · 10 min ago",
            "body of Disk full",
        ] {
            h.get_by_label(label);
        }
        assert!(take(&mut h).is_empty());
    }

    #[test]
    fn toggling_topic_sends_mute_request() {
        let mut h = harness(snapshot());
        h.run();
        click(&mut h, "backups");
        assert_eq!(
            take(&mut h),
            [UiRequest::SetTopicMuted {
                instance_id: "a".into(),
                topic: "backups".into(),
                muted: true,
            }]
        );
        click(&mut h, "home");
        assert_eq!(
            take(&mut h),
            [UiRequest::SetTopicMuted {
                instance_id: "b".into(),
                topic: "home".into(),
                muted: false,
            }]
        );
    }

    #[test]
    fn pause_menu_sends_until() {
        let mut h = harness(snapshot());
        h.run();
        click(&mut h, "Pause ▾");
        click(&mut h, "1 hour");
        assert_eq!(
            take(&mut h),
            [UiRequest::PauseAll {
                until: Some(NOW + 3600)
            }]
        );
        click(&mut h, "Pause ▾");
        click(&mut h, "Until I resume");
        assert_eq!(
            take(&mut h),
            [UiRequest::PauseAll {
                until: Some(i64::MAX)
            }]
        );
    }

    #[test]
    fn resume_only_while_paused() {
        let mut h = harness(snapshot());
        h.run();
        click(&mut h, "Pause ▾");
        click(&mut h, "Resume");
        assert!(take(&mut h).is_empty());

        let mut snap = snapshot();
        snap.paused_until = Some(i64::MAX);
        let mut h = harness(snap);
        h.run();
        h.get_by_label("Paused");
        click(&mut h, "Pause ▾");
        click(&mut h, "Resume");
        assert_eq!(take(&mut h), [UiRequest::PauseAll { until: None }]);
    }

    #[test]
    fn unconfigured_shows_setup_button() {
        let mut snap = snapshot();
        snap.configured = false;
        let mut h = harness(snap);
        h.run();
        h.get_by_label("Not set up yet");
        assert!(h.query_by_label("Pause ▾").is_none());
        assert!(h.query_by_label("alerts").is_none());
        click(&mut h, "Set up …");
        assert_eq!(take(&mut h), [UiRequest::OpenWizard]);
    }

    #[test]
    fn recent_without_click_is_not_clickable() {
        let mut h = harness(snapshot());
        h.run();
        click(&mut h, "Backup done");
        click(&mut h, "body of Door opened");
        assert!(take(&mut h).is_empty());
        click(&mut h, "Disk full");
        assert_eq!(
            take(&mut h),
            [UiRequest::OpenRecent {
                instance_id: "a".into(),
                message_id: "m1".into(),
            }]
        );
    }

    #[test]
    fn empty_recent_shows_hint() {
        let mut snap = snapshot();
        snap.recent.clear();
        let mut h = harness(snap);
        h.run();
        h.get_by_label("No messages yet");
    }

    #[test]
    fn gap_hint_can_be_dismissed() {
        let mut snap = snapshot();
        snap.gaps = vec![GapView {
            instance_id: "b".into(),
            host: "push.example.net".into(),
            topic: "home".into(),
            detected_at: NOW,
        }];
        let mut h = harness(snap);
        h.run();
        h.get_by_label("Messages may have been missed: push.example.net/home");
        click(&mut h, "Dismiss");
        assert_eq!(
            take(&mut h),
            [UiRequest::DismissGap {
                instance_id: "b".into(),
                topic: "home".into(),
            }]
        );
    }

    #[test]
    fn footer_buttons() {
        let mut h = harness(snapshot());
        h.run();
        click(&mut h, "Settings");
        click(&mut h, "Quit");
        assert_eq!(take(&mut h), [UiRequest::OpenSettings, UiRequest::Quit]);
    }

    #[test]
    fn footer_stays_inside_the_popover_window() {
        let mut snap = snapshot();
        let more = snap.recent.clone();
        snap.recent.extend(
            more.into_iter()
                .cycle()
                .take(17)
                .enumerate()
                .map(|(i, mut m)| {
                    m.message_id = format!("x{i}");
                    m
                }),
        );
        let size = crate::ui::app::POPOVER_SIZE;
        let mut h = harness_sized(snap, egui::vec2(size.0, size.1));
        h.run();
        assert!(h.get_by_label("Quit").rect().bottom() <= size.1);
        click(&mut h, "Settings");
        assert_eq!(take(&mut h), [UiRequest::OpenSettings]);
    }

    #[test]
    fn add_topic_saves_instance_with_new_topic() {
        let mut h = harness(snapshot());
        h.state_mut().state.add_topic = " new-topic ".into();
        h.state_mut().state.add_instance = Some("b".into());
        h.run();
        click(&mut h, "+ Topic");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { request_id, draft }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(*request_id, 1);
        assert_eq!(h.state().next_id, 2);
        assert_eq!(h.state().state.pending, Some(1));
        assert_eq!(draft.id.as_deref(), Some("b"));
        assert_eq!(draft.auth, AuthDraft::Token { token: None });
        assert_eq!(
            draft.topics,
            [topic("home", true), topic("new-topic", false)]
        );

        // Blocked while the save is in flight.
        click(&mut h, "+ Topic");
        assert!(take(&mut h).is_empty());

        h.state_mut().state.on_save_result(7, &Ok("b".into()));
        assert_eq!(h.state().state.pending, Some(1));
        h.state_mut().state.on_save_result(1, &Ok("b".into()));
        assert_eq!(h.state().state.pending, None);
        assert_eq!(h.state().state.add_topic, "");
        assert_eq!(h.state().state.error, None);

        // The snapshot with the new topic is still on its way: no draft from stale topics.
        h.state_mut().state.add_topic = "second".into();
        h.run();
        click(&mut h, "+ Topic");
        assert!(take(&mut h).is_empty());
        h.state_mut().state.on_snapshot();
        h.run();
        click(&mut h, "+ Topic");
        assert_eq!(take(&mut h).len(), 1);
    }

    #[test]
    fn add_topic_defaults_to_first_instance_and_shows_save_error() {
        let mut h = harness(snapshot());
        h.state_mut().state.add_topic = "extra".into();
        h.run();
        click(&mut h, "+ Topic");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { draft, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(draft.id.as_deref(), Some("a"));
        h.state_mut()
            .state
            .on_save_result(1, &Err("could not save settings".into()));
        h.run();
        h.get_by_label("Saving failed: could not save settings");
        assert_eq!(h.state().state.add_topic, "extra");
        assert_eq!(h.state().state.pending, None);
    }

    #[test]
    fn enter_in_topic_field_submits() {
        let mut h = harness(snapshot());
        h.run();
        let field = h.get_by_role(egui::Role::TextInput);
        field.focus();
        field.type_text("via-enter");
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        let out = take(&mut h);
        let [UiRequest::SaveInstance { draft, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(draft.topics.last().unwrap().name, "via-enter");
    }

    #[test]
    fn add_topic_is_validated_locally() {
        let mut h = harness(snapshot());
        h.state_mut().state.add_topic = "no spaces".into();
        h.run();
        click(&mut h, "+ Topic");
        assert!(take(&mut h).is_empty());
        h.get_by_label(UI_EN.err_topic_invalid);

        h.state_mut().state.add_topic = "alerts".into();
        click(&mut h, "+ Topic");
        assert!(take(&mut h).is_empty());
        h.get_by_label(UI_EN.err_topic_duplicate);

        h.state_mut().state.add_topic = String::new();
        click(&mut h, "+ Topic");
        assert!(take(&mut h).is_empty());
        assert_eq!(h.state().next_id, 1);

        // Typing again clears the message.
        h.get_by_role(egui::Role::TextInput).type_text("x");
        h.run();
        assert_eq!(h.state().state.add_topic, "x");
        assert_eq!(h.state().state.error, None);
    }

    #[test]
    fn overall_status_colours() {
        let now = NOW;
        let mut snap = snapshot();
        assert_eq!(overall_status(&snap, now), Overall::Connected);
        snap.instances[1].status = InstanceStatus::Connecting;
        assert_eq!(overall_status(&snap, now), Overall::Connecting);
        snap.paused_until = Some(now + 60);
        assert_eq!(overall_status(&snap, now), Overall::Paused(now + 60));
        snap.instances[0].status = InstanceStatus::Problem;
        assert_eq!(overall_status(&snap, now), Overall::Problem);
        snap.instances[1].status = InstanceStatus::AuthFailed;
        assert_eq!(overall_status(&snap, now), Overall::AuthFailed);
        // An expired pause no longer counts; instances without topics are ignored.
        let mut snap = snapshot();
        snap.paused_until = Some(now - 1);
        snap.instances.push(instance("c", "idle.example", vec![]));
        snap.instances[2].status = InstanceStatus::Problem;
        assert_eq!(overall_status(&snap, now), Overall::Connected);
    }
}
