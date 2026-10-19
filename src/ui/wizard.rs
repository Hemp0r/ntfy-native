//! Setup wizard: server → login → topics → notifications, each step checked by the core.

use eframe::egui::{self, Align, Key, Label, Layout, RichText, ScrollArea, TextEdit};
use zeroize::Zeroize;

use crate::i18n::UiStrings;
use crate::instance_edit::{AuthDraft, InstanceDraft};
use crate::ipc::{TestOutcome, TopicAccess, UiRequest};
use crate::settings::{Instance, Topic};
use crate::snapshot::{InstanceView, draft_from_view};
use crate::ui::problem::Problem;
use crate::validate::{check_url, same_origin, valid_topic_name};

/// The token/password field; its egui state (with undo history) is dropped after the Auth step.
pub fn secret_field_id() -> egui::Id {
    egui::Id::unique("wizard_secret")
}

/// Drops egui's copy (text and undo history) of what was typed into the secret field.
pub fn forget_secret_field(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(secret_field_id()));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardStep {
    Server,
    Auth,
    Topics,
    Notifications,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthChoice {
    None,
    Token,
    Basic,
}

fn auth_choice(auth: &AuthDraft) -> AuthChoice {
    match auth {
        AuthDraft::None => AuthChoice::None,
        AuthDraft::Token { .. } => AuthChoice::Token,
        AuthDraft::Basic { .. } => AuthChoice::Basic,
    }
}

/// No `Debug`: `secret` holds what the user typed.
pub struct Wizard {
    pub step: WizardStep,
    /// Its secrets are always `None`; the typed one lives in `secret` until sent.
    pub draft: InstanceDraft,
    pub editing: bool,
    /// Request id of the running test or save.
    pub busy: Option<u64>,
    pub error: Option<Problem>,
    pub topic_access: Vec<TopicAccess>,
    /// Global "urgent breaks Do Not Disturb", mirrored from the latest snapshot.
    pub urgent_global: bool,
    /// The user left without saving.
    pub cancelled: bool,
    /// The server answered the Server step with 401/403: show a hint at the login step.
    pub login_required: bool,
    /// Token or password as typed; empty keeps the stored one when editing.
    secret: String,
    new_topic: String,
    original_auth: AuthDraft,
    /// Where the stored secret belongs (editing only).
    original_url: String,
    /// The urgent switch as set in this wizard; sent only once saved.
    urgent_set: Option<bool>,
}

impl Drop for Wizard {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl Default for Wizard {
    fn default() -> Self {
        Self::new()
    }
}

impl Wizard {
    pub fn new() -> Self {
        let draft = InstanceDraft {
            id: None,
            url: String::new(),
            auth: AuthDraft::None,
            cache_duration_secs: Instance::default().cache_duration_secs,
            topics: Vec::new(),
        };
        Self::with_draft(draft, false)
    }

    /// Prefilled from the snapshot; empty secrets keep the stored ones.
    pub fn edit(view: &InstanceView) -> Self {
        Self::with_draft(draft_from_view(view), true)
    }

    fn with_draft(draft: InstanceDraft, editing: bool) -> Self {
        Wizard {
            step: WizardStep::Server,
            original_auth: draft.auth.clone(),
            original_url: draft.url.clone(),
            urgent_set: None,
            login_required: false,
            draft,
            editing,
            busy: None,
            error: None,
            topic_access: Vec::new(),
            urgent_global: false,
            cancelled: false,
            secret: String::new(),
            new_topic: String::new(),
        }
    }

    /// Server/Auth/Topics: a `TestConnection`; Notifications: `SaveInstance`.
    /// `None` while busy or when the step's input is incomplete.
    pub fn next(&mut self, next_id: &mut u64) -> Option<UiRequest> {
        if self.busy.is_some() {
            return None;
        }
        if self.step == WizardStep::Topics && self.draft.topics.is_empty() {
            self.error = Some(Problem::NoTopics);
            return None;
        }
        if !self.can_continue() {
            return None;
        }
        if self.step == WizardStep::Server
            && let Err(e) = check_url(&self.request_url())
        {
            self.error = Some(Problem::Url(e));
            return None;
        }
        let request_id = *next_id;
        *next_id += 1;
        self.busy = Some(request_id);
        self.error = None;
        self.login_required = false;
        let draft = self.request_draft();
        Some(match self.step {
            WizardStep::Notifications => UiRequest::SaveInstance { request_id, draft },
            _ => UiRequest::TestConnection { request_id, draft },
        })
    }

    /// Also abandons a running test (its answer will be ignored), but not a
    /// running save: its late success would make a second Finish a duplicate.
    pub fn back(&mut self) {
        if self.saving() {
            return;
        }
        self.busy = None;
        self.error = None;
        self.login_required = false;
        self.step = match self.step {
            WizardStep::Server | WizardStep::Auth => WizardStep::Server,
            WizardStep::Topics => WizardStep::Auth,
            WizardStep::Notifications => WizardStep::Topics,
            WizardStep::Done => WizardStep::Done,
        };
    }

    pub fn on_test_result(&mut self, request_id: u64, outcome: &TestOutcome) {
        if self.busy != Some(request_id) || self.step == WizardStep::Notifications {
            return;
        }
        self.busy = None;
        if self.step == WizardStep::Server && *outcome == TestOutcome::AuthFailed {
            // Reachable, but only with a login (e.g. a Basic-Auth proxy).
            if self.draft.auth == AuthDraft::None {
                self.set_auth_choice(AuthChoice::Basic);
            }
            self.error = None;
            self.login_required = true;
            self.step = WizardStep::Auth;
            return;
        }
        if let Some(problem) = Problem::from_outcome(outcome) {
            self.error = Some(problem);
            return;
        }
        let TestOutcome::Ok { topics } = outcome else {
            return;
        };
        self.step = match self.step {
            WizardStep::Server => WizardStep::Auth,
            WizardStep::Auth => {
                // Read access may differ with other credentials.
                self.topic_access.clear();
                WizardStep::Topics
            }
            WizardStep::Topics => {
                self.topic_access = topics.clone();
                if self.has_unreadable() {
                    WizardStep::Topics
                } else {
                    WizardStep::Notifications
                }
            }
            other => other,
        };
    }

    /// Once saved, returns the changed urgent switch to send.
    pub fn on_save_result(
        &mut self,
        request_id: u64,
        result: &Result<String, String>,
    ) -> Option<UiRequest> {
        if self.busy != Some(request_id) || self.step != WizardStep::Notifications {
            return None;
        }
        self.busy = None;
        match result {
            Ok(_) => {
                self.error = None;
                self.step = WizardStep::Done;
                self.secret.zeroize();
                self.urgent_set
                    .take()
                    .filter(|enabled| *enabled != self.urgent_global)
                    .map(|enabled| UiRequest::SetUrgentGlobal { enabled })
            }
            Err(reason) => {
                self.error = Some(Problem::SaveFailed(reason.clone()));
                None
            }
        }
    }

    /// Leaves without saving; ignored while a save runs (it would still complete).
    pub fn cancel(&mut self) {
        if self.saving() {
            return;
        }
        self.busy = None;
        self.cancelled = true;
    }

    /// Sets the urgent switch; sent only once saved, so Cancel leaves it unchanged.
    pub fn set_urgent(&mut self, enabled: bool) {
        self.urgent_set = Some(enabled);
    }

    /// The urgent switch as shown: the choice made here, else the current setting.
    pub fn urgent_choice(&self) -> bool {
        self.urgent_set.unwrap_or(self.urgent_global)
    }

    /// Whether "Next" is enabled (ignoring a running request). Topics: an
    /// empty list stays clickable so `next` can say why it doesn't continue.
    pub fn can_continue(&self) -> bool {
        match self.step {
            WizardStep::Server => !self.draft.url.trim().is_empty(),
            WizardStep::Auth => match &self.draft.auth {
                AuthDraft::None => true,
                AuthDraft::Token { .. } => !self.secret.trim().is_empty() || self.can_keep_secret(),
                AuthDraft::Basic { username, .. } => {
                    !username.trim().is_empty()
                        && (!self.secret.trim().is_empty() || self.can_keep_secret())
                }
            },
            WizardStep::Topics => !self.has_unreadable(),
            WizardStep::Notifications => true,
            WizardStep::Done => false,
        }
    }

    /// Editing with the same login kind (and user) on the same origin: an
    /// empty field keeps the stored secret.
    fn can_keep_secret(&self) -> bool {
        self.same_login_kind() && same_origin(&self.original_url, &self.request_url())
    }

    fn same_login_kind(&self) -> bool {
        self.editing
            && match (&self.original_auth, &self.draft.auth) {
                (AuthDraft::Token { .. }, AuthDraft::Token { .. }) => true,
                (AuthDraft::Basic { username: old, .. }, AuthDraft::Basic { username, .. }) => {
                    old == username.trim()
                }
                _ => false,
            }
    }

    /// A `SaveInstance` is in flight.
    pub fn saving(&self) -> bool {
        self.step == WizardStep::Notifications && self.busy.is_some()
    }

    fn has_unreadable(&self) -> bool {
        self.draft.topics.iter().any(|t| {
            self.topic_access
                .iter()
                .any(|a| a.name == t.name && !a.readable)
        })
    }

    fn set_auth_choice(&mut self, choice: AuthChoice) {
        self.secret.zeroize();
        self.error = None;
        let original_user = match &self.original_auth {
            AuthDraft::Basic { username, .. } => username.clone(),
            _ => String::new(),
        };
        self.draft.auth = match choice {
            AuthChoice::None => AuthDraft::None,
            AuthChoice::Token => AuthDraft::Token { token: None },
            AuthChoice::Basic => AuthDraft::Basic {
                username: original_user,
                password: None,
            },
        };
    }

    fn add_topic(&mut self) {
        let name = self.new_topic.trim();
        if name.is_empty() {
            return;
        }
        if !valid_topic_name(name) {
            self.error = Some(Problem::TopicInvalid);
            return;
        }
        if self.draft.topics.iter().any(|t| t.name == name) {
            self.error = Some(Problem::TopicDuplicate);
            return;
        }
        self.draft.topics.push(Topic {
            name: name.to_owned(),
            ..Topic::default()
        });
        self.new_topic.clear();
        self.error = None;
    }

    /// The address as sent: `https://` added when no scheme was typed.
    fn request_url(&self) -> String {
        let url = self.draft.url.trim();
        if url.contains("://") {
            url.to_owned()
        } else {
            format!("https://{url}")
        }
    }

    /// What goes to the core: the typed secret filled in, topics only from the Topics step on.
    fn request_draft(&self) -> InstanceDraft {
        let mut draft = self.draft.clone();
        draft.url = self.request_url();
        // Blank keeps the stored secret; a password keeps its own spaces.
        let secret = (!self.secret.trim().is_empty()).then(|| self.secret.clone());
        let unusable = secret.is_none() && !self.can_keep_secret();
        draft.auth = match &self.draft.auth {
            AuthDraft::None => AuthDraft::None,
            // Only reachability is checked here; the login step asks for the secret.
            _ if unusable && self.step == WizardStep::Server => AuthDraft::None,
            AuthDraft::Token { .. } => AuthDraft::Token {
                token: secret.map(|t| t.trim().to_owned()),
            },
            AuthDraft::Basic { username, .. } => AuthDraft::Basic {
                username: username.trim().to_owned(),
                password: secret,
            },
        };
        if matches!(self.step, WizardStep::Server | WizardStep::Auth) {
            draft.topics.clear();
        }
        draft
    }
}

/// Draws the current step and collects the user's requests in `out`.
pub fn wizard_ui(
    ui: &mut egui::Ui,
    w: &mut Wizard,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    ui.style_mut().interaction.selectable_labels = false;
    if w.step != WizardStep::Auth {
        forget_secret_field(ui.ctx());
    }
    if w.step == WizardStep::Done {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading(s.wizard_done);
            });
        });
        return;
    }
    egui::Panel::bottom("wizard_buttons").show(ui, |ui| buttons(ui, w, s, out, next_id));
    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut submit = false;
            ui.add_enabled_ui(w.busy.is_none(), |ui| match w.step {
                WizardStep::Server => submit = server_step(ui, w, s),
                WizardStep::Auth => submit = auth_step(ui, w, s),
                WizardStep::Topics => topics_step(ui, w, s),
                WizardStep::Notifications => notifications_step(ui, w, s, out),
                WizardStep::Done => {}
            });
            ui.add_space(8.0);
            if w.busy.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(s.testing);
                });
            }
            if let Some(e) = &w.error {
                ui.add(
                    Label::new(RichText::new(e.text(s)).color(ui.visuals().error_fg_color)).wrap(),
                );
            }
            if submit {
                out.extend(w.next(next_id));
            }
        });
    });
}

fn buttons(
    ui: &mut egui::Ui,
    w: &mut Wizard,
    s: &UiStrings,
    out: &mut Vec<UiRequest>,
    next_id: &mut u64,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!w.saving(), egui::Button::new(s.cancel))
            .clicked()
        {
            w.cancel();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if w.step == WizardStep::Notifications {
                s.finish
            } else {
                s.next
            };
            let ready = w.busy.is_none() && w.can_continue();
            if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                out.extend(w.next(next_id));
            }
            if w.step != WizardStep::Server
                && ui
                    .add_enabled(!w.saving(), egui::Button::new(s.back))
                    .clicked()
            {
                w.back();
            }
        });
    });
    ui.add_space(4.0);
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.heading(text);
    ui.add_space(8.0);
}

/// A text field with a visible label above it; `true` when Enter was pressed in it.
fn field(ui: &mut egui::Ui, label: &str, edit: TextEdit<'_>) -> (egui::Response, bool) {
    let label = ui.label(label);
    let response = ui
        .add(edit.desired_width(f32::INFINITY))
        .labelled_by(label.id);
    let entered = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    ui.add_space(6.0);
    (response, entered)
}

fn server_step(ui: &mut egui::Ui, w: &mut Wizard, s: &UiStrings) -> bool {
    heading(ui, s.topic_server);
    let (response, entered) = field(
        ui,
        s.server_url,
        TextEdit::singleline(&mut w.draft.url).hint_text("https://"),
    );
    if response.changed() {
        w.error = None;
    }
    ui.label(RichText::new(s.server_hint).small().weak());
    entered
}

fn auth_step(ui: &mut egui::Ui, w: &mut Wizard, s: &UiStrings) -> bool {
    heading(ui, s.auth_heading);
    if w.login_required {
        ui.label(s.login_required);
        ui.add_space(4.0);
    }
    let current = auth_choice(&w.draft.auth);
    for (choice, label) in [
        (AuthChoice::None, s.auth_none),
        (AuthChoice::Token, s.auth_token),
        (AuthChoice::Basic, s.auth_basic),
    ] {
        if ui.radio(current == choice, label).clicked() && current != choice {
            w.set_auth_choice(choice);
        }
    }
    ui.add_space(8.0);
    let hint = if w.can_keep_secret() {
        s.keep_secret_hint
    } else if w.same_login_kind() {
        s.reenter_secret_hint
    } else {
        ""
    };
    let mut entered = false;
    let mut changed = false;
    match &mut w.draft.auth {
        AuthDraft::None => {}
        AuthDraft::Token { .. } => {
            let (r, e) = field(
                ui,
                s.auth_token,
                TextEdit::singleline(&mut w.secret)
                    .id(secret_field_id())
                    .password(true)
                    .hint_text(hint),
            );
            (changed, entered) = (r.changed(), e);
        }
        AuthDraft::Basic { username, .. } => {
            let (r, _) = field(ui, s.username, TextEdit::singleline(username));
            let (p, e) = field(
                ui,
                s.password,
                TextEdit::singleline(&mut w.secret)
                    .id(secret_field_id())
                    .password(true)
                    .hint_text(hint),
            );
            (changed, entered) = (r.changed() || p.changed(), e);
        }
    }
    if !hint.is_empty() {
        ui.label(RichText::new(hint).small().weak());
    }
    if changed {
        w.error = None;
    }
    entered
}

fn topics_step(ui: &mut egui::Ui, w: &mut Wizard, s: &UiStrings) {
    heading(ui, s.topics_heading);
    let mut remove = None;
    for (i, t) in w.draft.topics.iter().enumerate() {
        let unreadable = w
            .topic_access
            .iter()
            .any(|a| a.name == t.name && !a.readable);
        ui.horizontal(|ui| {
            ui.label(&t.name);
            if unreadable {
                ui.label(RichText::new(s.topic_not_readable).color(ui.visuals().error_fg_color));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(s.remove).clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        w.draft.topics.remove(i);
        w.error = None;
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let clicked = ui.button(s.add_topic).clicked();
            let edit = ui
                .add(
                    TextEdit::singleline(&mut w.new_topic)
                        .hint_text(s.topic_name)
                        .desired_width(f32::INFINITY),
                )
                .accessible_name(s.topic_name);
            if edit.changed() {
                w.error = None;
            }
            let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if clicked || entered {
                w.add_topic();
                edit.request_focus();
            }
        });
    });
}

fn notifications_step(ui: &mut egui::Ui, w: &mut Wizard, s: &UiStrings, out: &mut Vec<UiRequest>) {
    heading(ui, s.notifications_heading);
    if ui.button(s.send_test).clicked() {
        out.push(UiRequest::SendTestNotification);
    }
    let hint = if cfg!(target_os = "windows") {
        s.notifications_hint_windows
    } else {
        s.notifications_hint_macos
    };
    ui.add(Label::new(RichText::new(hint).small().weak()).wrap());
    ui.add_space(12.0);
    let mut urgent = w.urgent_choice();
    if ui.checkbox(&mut urgent, s.urgent_global).changed() {
        w.set_urgent(urgent);
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::i18n::UI_EN;
    use crate::snapshot::{AuthView, InstanceStatus};
    use crate::validate::UrlError;

    fn ok(names: &[(&str, bool)]) -> TestOutcome {
        TestOutcome::Ok {
            topics: names
                .iter()
                .map(|(n, readable)| TopicAccess {
                    name: (*n).into(),
                    readable: *readable,
                })
                .collect(),
        }
    }

    fn topic(name: &str) -> Topic {
        Topic {
            name: name.into(),
            ..Topic::default()
        }
    }

    fn test_draft(r: Option<UiRequest>) -> (u64, InstanceDraft) {
        match r {
            Some(UiRequest::TestConnection { request_id, draft }) => (request_id, draft),
            other => panic!("{other:?}"),
        }
    }

    /// Answers the pending request with `outcome`.
    fn answer(w: &mut Wizard, outcome: TestOutcome) {
        let id = w.busy.expect("pending request");
        w.on_test_result(id, &outcome);
    }

    fn at_topics(id: &mut u64) -> Wizard {
        let mut w = Wizard::new();
        w.draft.url = "https://ntfy.example.org".into();
        w.next(id).unwrap();
        answer(&mut w, ok(&[]));
        w.next(id).unwrap();
        answer(&mut w, ok(&[]));
        assert_eq!(w.step, WizardStep::Topics);
        w
    }

    fn at_notifications(id: &mut u64) -> Wizard {
        let mut w = at_topics(id);
        w.draft.topics = vec![topic("alerts")];
        w.next(id).unwrap();
        answer(&mut w, ok(&[("alerts", true)]));
        assert_eq!(w.step, WizardStep::Notifications);
        w
    }

    fn view(auth: AuthView) -> InstanceView {
        InstanceView {
            id: "a".into(),
            host: "ntfy.example.org".into(),
            url: "https://ntfy.example.org".into(),
            status: InstanceStatus::Connected,
            auth,
            cache_duration_secs: 86_400,
            topics: vec![Topic {
                name: "alerts".into(),
                muted: true,
                urgent_breaks_dnd: Some(true),
            }],
        }
    }

    #[test]
    fn server_step_requires_test_ok() {
        let mut id = 1;
        let mut w = Wizard::new();
        assert_eq!(w.step, WizardStep::Server);
        assert!(!w.can_continue());
        assert!(w.next(&mut id).is_none(), "no url yet");
        assert_eq!(id, 1);

        w.draft.url = " ntfy.example.org ".into();
        let (rid, draft) = test_draft(w.next(&mut id));
        assert_eq!((rid, id, w.busy), (1, 2, Some(1)));
        assert_eq!(draft.url, "https://ntfy.example.org");
        assert_eq!(draft.auth, AuthDraft::None);
        assert!(draft.topics.is_empty());
        assert!(w.next(&mut id).is_none(), "busy");

        w.on_test_result(1, &TestOutcome::NotNtfy);
        assert_eq!(w.step, WizardStep::Server);
        assert_eq!(w.busy, None);
        assert_eq!(w.error, Some(Problem::NotNtfy));

        for (outcome, problem) in [
            (
                TestOutcome::Unreachable {
                    message: "DNS".into(),
                },
                Problem::Unreachable("DNS".into()),
            ),
            (
                TestOutcome::InvalidInput {
                    message: "bad url".into(),
                },
                Problem::Invalid("bad url".into()),
            ),
        ] {
            w.next(&mut id).unwrap();
            answer(&mut w, outcome);
            assert_eq!(w.step, WizardStep::Server);
            assert_eq!(w.error, Some(problem));
        }

        w.next(&mut id).unwrap();
        assert_eq!(w.error, None, "cleared when retrying");
        answer(&mut w, ok(&[]));
        assert_eq!(w.step, WizardStep::Auth);
        assert_eq!(w.error, None);
    }

    #[test]
    fn auth_step_tests_chosen_auth_without_topics() {
        let mut id = 1;
        let mut w = Wizard::new();
        w.draft.url = "https://ntfy.example.org".into();
        w.next(&mut id).unwrap();
        answer(&mut w, ok(&[]));

        w.set_auth_choice(AuthChoice::Token);
        assert!(!w.can_continue(), "new server needs a token");
        assert!(w.next(&mut id).is_none());
        w.secret = "tk_secret".into();
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(
            draft.auth,
            AuthDraft::Token {
                token: Some("tk_secret".into())
            }
        );
        assert!(draft.topics.is_empty());
        assert_eq!(w.draft.auth, AuthDraft::Token { token: None });

        answer(&mut w, TestOutcome::AuthFailed);
        assert_eq!(w.step, WizardStep::Auth);
        assert_eq!(w.error, Some(Problem::AuthRejected));

        w.set_auth_choice(AuthChoice::Basic);
        assert!(w.secret.is_empty(), "switching clears the typed secret");
        w.secret = "pw".into();
        assert!(!w.can_continue(), "username missing");
        w.draft.auth = AuthDraft::Basic {
            username: " alice ".into(),
            password: None,
        };
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(
            draft.auth,
            AuthDraft::Basic {
                username: "alice".into(),
                password: Some("pw".into())
            }
        );
        answer(&mut w, ok(&[]));
        assert_eq!(w.step, WizardStep::Topics);
    }

    #[test]
    fn stale_test_results_are_ignored() {
        let mut id = 1;
        let mut w = Wizard::new();
        w.draft.url = "https://ntfy.example.org".into();
        w.next(&mut id).unwrap();
        w.on_test_result(99, &ok(&[]));
        assert_eq!(w.step, WizardStep::Server);
        assert_eq!(w.busy, Some(1));
        answer(&mut w, ok(&[]));
        assert_eq!(w.step, WizardStep::Auth);

        // Going back abandons the running test; its late answer changes nothing.
        let (late, _) = test_draft(w.next(&mut id));
        w.back();
        assert_eq!(w.step, WizardStep::Server);
        assert_eq!(w.busy, None);
        w.on_test_result(late, &ok(&[]));
        assert_eq!(w.step, WizardStep::Server);
        w.on_test_result(late, &TestOutcome::NotNtfy);
        assert_eq!(w.error, None);

        // A newer request replaces an older one.
        let (first, _) = test_draft(w.next(&mut id));
        w.back();
        let (second, _) = test_draft(w.next(&mut id));
        assert_ne!(first, second);
        w.on_test_result(first, &TestOutcome::NotNtfy);
        assert_eq!(w.error, None);
        assert_eq!(w.busy, Some(second));
        w.on_save_result(second, &Ok("a".into()));
        assert_eq!(w.busy, Some(second), "a save result is not a test result");
    }

    #[test]
    fn topics_step_blocks_unreadable_and_invalid() {
        let mut id = 1;
        let mut w = at_topics(&mut id);
        let before = id;
        assert!(w.next(&mut id).is_none());
        assert_eq!(w.error, Some(Problem::NoTopics));
        assert_eq!(id, before);

        w.new_topic = "bad topic".into();
        w.add_topic();
        assert_eq!(w.error, Some(Problem::TopicInvalid));
        assert!(w.draft.topics.is_empty());
        assert_eq!(w.new_topic, "bad topic", "kept for correction");

        for name in [" alerts ", "secret"] {
            w.new_topic = name.into();
            w.add_topic();
            assert_eq!(w.error, None);
            assert!(w.new_topic.is_empty());
        }
        w.new_topic = "alerts".into();
        w.add_topic();
        assert_eq!(w.error, Some(Problem::TopicDuplicate));
        assert_eq!(w.draft.topics, [topic("alerts"), topic("secret")]);

        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(draft.topics, [topic("alerts"), topic("secret")]);
        answer(&mut w, ok(&[("alerts", true), ("secret", false)]));
        assert_eq!(w.step, WizardStep::Topics, "unreadable topic blocks");
        assert!(!w.can_continue());
        assert!(w.next(&mut id).is_none());

        w.draft.topics.retain(|t| t.name != "secret");
        assert!(w.can_continue());
        w.next(&mut id).unwrap();
        answer(&mut w, ok(&[("alerts", true)]));
        assert_eq!(w.step, WizardStep::Notifications);
    }

    #[test]
    fn finish_sends_save_and_done_on_ok() {
        let mut id = 1;
        let mut w = at_notifications(&mut id);
        let r = w.next(&mut id);
        let Some(UiRequest::SaveInstance { request_id, draft }) = r else {
            panic!("{r:?}");
        };
        assert_eq!(w.busy, Some(request_id));
        assert_eq!(draft.id, None);
        assert_eq!(draft.url, "https://ntfy.example.org");
        assert_eq!(draft.cache_duration_secs, 43_200);
        assert_eq!(draft.topics, [topic("alerts")]);

        w.on_save_result(request_id, &Err("could not save settings".into()));
        assert_eq!(w.step, WizardStep::Notifications);
        assert_eq!(
            w.error,
            Some(Problem::SaveFailed("could not save settings".into()))
        );

        let Some(UiRequest::SaveInstance { request_id, .. }) = w.next(&mut id) else {
            panic!("retry");
        };
        w.on_save_result(request_id + 1, &Ok("x".into()));
        assert_eq!(w.step, WizardStep::Notifications);
        w.on_save_result(request_id, &Ok("ntfy.example.org".into()));
        assert_eq!(w.step, WizardStep::Done);
        assert_eq!(w.busy, None);
        assert_eq!(w.error, None);
        assert!(w.next(&mut id).is_none());
    }

    #[test]
    fn edit_mode_keeps_secrets_empty() {
        let mut id = 1;
        let mut w = Wizard::edit(&view(AuthView::Token));
        assert!(w.editing);
        assert_eq!(w.step, WizardStep::Server);
        assert_eq!(w.draft.id.as_deref(), Some("a"));
        assert_eq!(w.draft.url, "https://ntfy.example.org");
        assert_eq!(w.draft.auth, AuthDraft::Token { token: None });

        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(draft.auth, AuthDraft::Token { token: None });
        assert!(draft.topics.is_empty());
        answer(&mut w, ok(&[]));
        assert!(w.can_continue(), "stored token is kept");
        w.next(&mut id).unwrap();
        answer(&mut w, ok(&[]));
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(draft.topics, view(AuthView::Token).topics);
        answer(&mut w, ok(&[("alerts", true)]));
        let Some(UiRequest::SaveInstance { draft, .. }) = w.next(&mut id) else {
            panic!("save");
        };
        assert_eq!(draft.id.as_deref(), Some("a"));
        assert_eq!(draft.auth, AuthDraft::Token { token: None });
        assert_eq!(draft.cache_duration_secs, 86_400);
        assert!(draft.topics[0].muted, "topic settings survive");

        let mut w = Wizard::edit(&view(AuthView::Basic {
            username: "alice".into(),
        }));
        w.step = WizardStep::Auth;
        assert!(w.can_continue());
        w.draft.auth = AuthDraft::Basic {
            username: "bob".into(),
            password: None,
        };
        assert!(!w.can_continue(), "a new user needs a password");
        w.set_auth_choice(AuthChoice::Token);
        assert!(!w.can_continue(), "no stored token to keep");
    }

    struct T {
        w: Wizard,
        out: Vec<UiRequest>,
        next_id: u64,
    }

    fn harness(w: Wizard) -> Harness<'static, T> {
        Harness::builder()
            .with_size(egui::vec2(520.0, 560.0))
            .allow_missing_glyphs()
            .build_ui_state(
                |ui, t: &mut T| wizard_ui(ui, &mut t.w, &UI_EN, &mut t.out, &mut t.next_id),
                T {
                    w,
                    out: Vec::new(),
                    next_id: 1,
                },
            )
    }

    fn click(h: &mut Harness<'static, T>, label: &str) {
        h.get_by_label(label).click();
        h.run_steps(4);
    }

    fn take(h: &mut Harness<'static, T>) -> Vec<UiRequest> {
        std::mem::take(&mut h.state_mut().out)
    }

    fn type_into(h: &mut Harness<'static, T>, role: egui::Role, label: &str, text: &str) {
        let field = h.get_by_role_and_label(role, label);
        field.focus();
        field.type_text(text);
        h.run_steps(4);
    }

    #[test]
    fn ui_walks_through_the_steps() {
        let mut h = harness(Wizard::new());
        h.run_steps(4);
        type_into(
            &mut h,
            egui::Role::TextInput,
            UI_EN.server_url,
            "https://ntfy.example.org",
        );
        h.run_steps(4);
        click(&mut h, "Next");
        let out = take(&mut h);
        assert!(
            matches!(out[..], [UiRequest::TestConnection { .. }]),
            "{out:?}"
        );
        h.get_by_label(UI_EN.testing);
        h.state_mut().w.on_test_result(1, &TestOutcome::NotNtfy);
        h.run_steps(4);
        h.get_by_label(UI_EN.err_not_ntfy);
        click(&mut h, "Next");
        assert_eq!(take(&mut h).len(), 1);
        h.state_mut().w.on_test_result(2, &ok(&[]));
        h.run_steps(4);

        h.get_by_label(UI_EN.auth_heading);
        click(&mut h, UI_EN.auth_token);
        click(&mut h, "Next");
        assert!(take(&mut h).is_empty(), "token missing");
        type_into(&mut h, egui::Role::PasswordInput, UI_EN.auth_token, "tk_1");
        h.run_steps(4);
        click(&mut h, "Next");
        let out = take(&mut h);
        let [UiRequest::TestConnection { request_id, draft }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(
            draft.auth,
            AuthDraft::Token {
                token: Some("tk_1".into())
            }
        );
        h.state_mut().w.on_test_result(*request_id, &ok(&[]));
        h.run_steps(4);

        h.get_by_label(UI_EN.topics_heading);
        click(&mut h, "Next");
        h.get_by_label(UI_EN.err_no_topics);
        assert!(take(&mut h).is_empty());
        type_into(&mut h, egui::Role::TextInput, UI_EN.topic_name, "alerts");
        h.run_steps(4);
        click(&mut h, "+ Topic");
        h.get_by_label("alerts");
        click(&mut h, "Next");
        let out = take(&mut h);
        let [UiRequest::TestConnection { request_id, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        h.state_mut()
            .w
            .on_test_result(*request_id, &ok(&[("alerts", false)]));
        h.run_steps(4);
        h.get_by_label(UI_EN.topic_not_readable);
        click(&mut h, "Remove");
        assert!(h.query_by_label("alerts").is_none());

        type_into(&mut h, egui::Role::TextInput, UI_EN.topic_name, "news");
        h.run_steps(4);
        click(&mut h, "+ Topic");
        click(&mut h, "Next");
        let out = take(&mut h);
        let [UiRequest::TestConnection { request_id, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        h.state_mut()
            .w
            .on_test_result(*request_id, &ok(&[("news", true)]));
        h.run_steps(4);

        h.get_by_label(UI_EN.notifications_heading);
        click(&mut h, UI_EN.send_test);
        click(&mut h, UI_EN.urgent_global);
        assert_eq!(
            take(&mut h),
            [UiRequest::SendTestNotification],
            "the switch waits for Finish"
        );
        click(&mut h, "Finish");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { request_id, draft }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(draft.topics, [topic("news")]);
        h.state_mut()
            .w
            .on_save_result(*request_id, &Err("disk full".into()));
        h.run_steps(4);
        h.get_by_label("Saving failed: disk full");
        click(&mut h, "Back");
        assert_eq!(h.state().w.step, WizardStep::Topics);
    }

    #[test]
    fn back_is_blocked_while_saving() {
        let mut id = 1;
        let mut w = at_notifications(&mut id);
        let Some(UiRequest::SaveInstance { request_id, .. }) = w.next(&mut id) else {
            panic!("save");
        };
        w.back();
        assert_eq!(w.step, WizardStep::Notifications);
        assert_eq!(w.busy, Some(request_id));

        let mut h = harness(w);
        h.run_steps(4);
        click(&mut h, "Back");
        assert_eq!(h.state().w.step, WizardStep::Notifications);
        h.state_mut()
            .w
            .on_save_result(request_id, &Ok("ntfy.example.org".into()));
        assert_eq!(h.state().w.step, WizardStep::Done);
    }

    #[test]
    fn blank_secret_keeps_the_stored_one() {
        let mut id = 1;
        let mut w = Wizard::edit(&view(AuthView::Token));
        w.step = WizardStep::Auth;
        w.secret = "  \t ".into();
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(draft.auth, AuthDraft::Token { token: None });

        let mut w = Wizard::edit(&view(AuthView::Basic {
            username: "alice".into(),
        }));
        w.step = WizardStep::Auth;
        w.secret = "   ".into();
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(
            draft.auth,
            AuthDraft::Basic {
                username: "alice".into(),
                password: None
            }
        );
        // A real password keeps its spaces.
        w.on_test_result(w.busy.unwrap(), &TestOutcome::AuthFailed);
        w.secret = " pw ".into();
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(
            draft.auth,
            AuthDraft::Basic {
                username: "alice".into(),
                password: Some(" pw ".into())
            }
        );

        let mut w = Wizard::new();
        w.step = WizardStep::Auth;
        w.set_auth_choice(AuthChoice::Basic);
        w.draft.auth = AuthDraft::Basic {
            username: "bob".into(),
            password: None,
        };
        w.secret = "  ".into();
        assert!(!w.can_continue(), "a blank password is no password");
    }

    #[test]
    fn secret_field_is_forgotten_after_the_auth_step() {
        let mut w = Wizard::new();
        w.step = WizardStep::Auth;
        w.set_auth_choice(AuthChoice::Token);
        let mut h = harness(w);
        h.run_steps(4);
        type_into(&mut h, egui::Role::PasswordInput, UI_EN.auth_token, "tk_1");
        assert!(egui::text_edit::TextEditState::load(&h.ctx, secret_field_id()).is_some());
        click(&mut h, "Next");
        let id = h.state().w.busy.unwrap();
        h.state_mut().w.on_test_result(id, &ok(&[]));
        h.run_steps(4);
        assert_eq!(h.state().w.step, WizardStep::Topics);
        assert!(egui::text_edit::TextEditState::load(&h.ctx, secret_field_id()).is_none());
        assert_eq!(
            h.state().w.secret,
            "tk_1",
            "still needed for the next steps"
        );
    }

    #[test]
    fn secret_is_wiped_once_saved() {
        let mut id = 1;
        let mut w = at_notifications(&mut id);
        w.secret = "tk_1".into();
        let request = w.next(&mut id);
        let Some(UiRequest::SaveInstance { request_id, .. }) = request else {
            panic!("save");
        };
        w.on_save_result(request_id, &Ok("a".into()));
        assert!(w.secret.is_empty());
    }

    #[test]
    fn ui_cancel_and_edit_hint() {
        let mut h = harness(Wizard::edit(&view(AuthView::Token)));
        h.state_mut().w.step = WizardStep::Auth;
        h.run_steps(4);
        h.get_by_label(UI_EN.keep_secret_hint);
        click(&mut h, "Cancel");
        assert!(h.state().w.cancelled);
        assert!(take(&mut h).is_empty());
    }

    #[test]
    fn changing_the_host_asks_for_the_secret_again() {
        let mut id = 1;
        let mut w = Wizard::edit(&view(AuthView::Token));
        w.step = WizardStep::Auth;
        assert!(w.can_keep_secret());
        for same in [
            "https://ntfy.example.org/sub",
            "ntfy.example.org",
            "https://NTFY.example.org:443/",
        ] {
            w.draft.url = same.into();
            assert!(w.can_keep_secret(), "{same}");
        }
        for other in [
            "https://other.example",
            "http://ntfy.example.org",
            "https://ntfy.example.org:8443",
        ] {
            w.draft.url = other.into();
            assert!(!w.can_keep_secret(), "{other}");
            assert!(!w.can_continue(), "{other}");
        }
        w.secret = "tk_new".into();
        assert!(w.can_continue());

        // The Server step tests the new address without the old secret.
        let mut w = Wizard::edit(&view(AuthView::Token));
        w.draft.url = "https://other.example".into();
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(draft.auth, AuthDraft::None);
        let mut w = Wizard::edit(&view(AuthView::Token));
        w.draft.url = "https://ntfy.example.org/sub".into();
        let (_, draft) = test_draft(w.next(&mut id));
        assert_eq!(draft.auth, AuthDraft::Token { token: None });

        let mut w = Wizard::edit(&view(AuthView::Basic {
            username: "alice".into(),
        }));
        w.step = WizardStep::Auth;
        assert!(w.can_continue());
        w.draft.url = "https://other.example".into();
        assert!(!w.can_keep_secret());
        assert!(
            !w.can_continue(),
            "a password is needed for the new address"
        );
    }

    #[test]
    fn ui_hint_asks_again_for_a_new_address() {
        let mut w = Wizard::edit(&view(AuthView::Token));
        w.draft.url = "https://other.example".into();
        w.step = WizardStep::Auth;
        let mut h = harness(w);
        h.run_steps(4);
        h.get_by_label(UI_EN.reenter_secret_hint);
        assert!(h.query_by_label(UI_EN.keep_secret_hint).is_none());
    }

    #[test]
    fn login_required_at_the_server_step_moves_on_to_login() {
        let mut id = 1;
        let mut w = Wizard::new();
        w.draft.url = "https://proxy.example".into();
        w.next(&mut id).unwrap();
        answer(&mut w, TestOutcome::AuthFailed);
        assert_eq!(w.step, WizardStep::Auth);
        assert_eq!(w.error, None);
        assert!(w.login_required);
        assert_eq!(
            w.draft.auth,
            AuthDraft::Basic {
                username: String::new(),
                password: None
            }
        );

        // At the login step a rejection is an error again.
        w.draft.auth = AuthDraft::Basic {
            username: "alice".into(),
            password: None,
        };
        w.secret = "pw".into();
        w.next(&mut id).unwrap();
        assert!(!w.login_required);
        answer(&mut w, TestOutcome::AuthFailed);
        assert_eq!(w.step, WizardStep::Auth);
        assert_eq!(w.error, Some(Problem::AuthRejected));
        w.back();
        assert!(!w.login_required);

        // A chosen login kind is kept.
        let mut w = Wizard::edit(&view(AuthView::Token));
        w.next(&mut id).unwrap();
        answer(&mut w, TestOutcome::AuthFailed);
        assert_eq!(w.step, WizardStep::Auth);
        assert_eq!(w.draft.auth, AuthDraft::Token { token: None });
    }

    #[test]
    fn ui_shows_the_login_required_hint() {
        let mut w = Wizard::new();
        w.draft.url = "https://proxy.example".into();
        let mut h = harness(w);
        h.run_steps(4);
        click(&mut h, "Next");
        let id = h.state().w.busy.unwrap();
        h.state_mut().w.on_test_result(id, &TestOutcome::AuthFailed);
        h.run_steps(4);
        h.get_by_label(UI_EN.login_required);
        assert!(h.query_by_label(UI_EN.err_auth).is_none());
        assert_eq!(h.state().w.step, WizardStep::Auth);
    }

    #[test]
    fn urgent_switch_is_sent_only_after_saving() {
        let mut id = 1;
        let mut w = at_notifications(&mut id);
        let mut h = harness(w);
        h.run_steps(4);
        click(&mut h, UI_EN.urgent_global);
        assert!(take(&mut h).is_empty(), "nothing sent before Finish");
        // A snapshot meanwhile does not undo the choice.
        h.state_mut().w.urgent_global = false;
        h.run_steps(4);
        assert!(h.state().w.urgent_choice());
        click(&mut h, "Finish");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { request_id, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        let request_id = *request_id;
        assert_eq!(
            h.state_mut()
                .w
                .on_save_result(request_id, &Err("disk full".into())),
            None
        );
        click(&mut h, "Finish");
        let out = take(&mut h);
        let [UiRequest::SaveInstance { request_id, .. }] = out.as_slice() else {
            panic!("{out:?}");
        };
        assert_eq!(
            h.state_mut().w.on_save_result(*request_id, &Ok("a".into())),
            Some(UiRequest::SetUrgentGlobal { enabled: true })
        );

        // Unchanged switch: nothing extra.
        w = at_notifications(&mut id);
        let Some(UiRequest::SaveInstance { request_id, .. }) = w.next(&mut id) else {
            panic!("save");
        };
        assert_eq!(w.on_save_result(request_id, &Ok("a".into())), None);

        // Cancelled: nothing is sent.
        let mut h = harness(at_notifications(&mut id));
        h.run_steps(4);
        click(&mut h, UI_EN.urgent_global);
        click(&mut h, "Cancel");
        assert!(h.state().w.cancelled);
        assert!(take(&mut h).is_empty());
    }

    #[test]
    fn cancel_is_blocked_while_saving() {
        let mut id = 1;
        let mut w = at_notifications(&mut id);
        let Some(UiRequest::SaveInstance { request_id, .. }) = w.next(&mut id) else {
            panic!("save");
        };
        w.cancel();
        assert!(!w.cancelled);
        assert_eq!(w.busy, Some(request_id));
        let mut h = harness(w);
        h.run_steps(4);
        click(&mut h, "Cancel");
        assert!(!h.state().w.cancelled);
        assert_eq!(h.state().w.busy, Some(request_id));

        // While a test runs, Cancel still works.
        let mut w = Wizard::new();
        w.draft.url = "https://ntfy.example.org".into();
        w.next(&mut id).unwrap();
        w.cancel();
        assert!(w.cancelled);
        assert_eq!(w.busy, None);
    }

    #[test]
    fn invalid_addresses_are_caught_before_testing() {
        let mut id = 1;
        for (url, problem) in [
            ("ftp://ntfy.example.org", UrlError::Scheme),
            ("https://u:p@ntfy.example.org", UrlError::Credentials),
            ("ntfy.example.org/?x=1", UrlError::QueryOrFragment),
            ("https://", UrlError::Invalid),
        ] {
            let mut w = Wizard::new();
            w.draft.url = url.into();
            assert!(w.next(&mut id).is_none(), "{url}");
            assert_eq!(w.busy, None);
            assert_eq!(w.error, Some(Problem::Url(problem)), "{url}");
        }
        assert_eq!(id, 1, "no request sent");
    }
}
