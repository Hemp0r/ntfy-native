//! Error states shown inline in the windows; localized only when drawn.

use crate::i18n::{UiStrings, fill};
use crate::ipc::TestOutcome;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Network failure category from the core (English, no entered values).
    Unreachable(String),
    NotNtfy,
    AuthRejected,
    /// Field/position message from the core's input check (English).
    Invalid(String),
    NoTopics,
    TopicInvalid,
    TopicDuplicate,
    /// Reason from the core (English, secret-free).
    SaveFailed(String),
}

impl Problem {
    /// `None` for a successful test.
    pub fn from_outcome(outcome: &TestOutcome) -> Option<Problem> {
        Some(match outcome {
            TestOutcome::Ok { .. } => return None,
            TestOutcome::InvalidInput { message } => Problem::Invalid(message.clone()),
            TestOutcome::Unreachable { message } => Problem::Unreachable(message.clone()),
            TestOutcome::NotNtfy => Problem::NotNtfy,
            TestOutcome::AuthFailed => Problem::AuthRejected,
        })
    }

    pub fn text(&self, s: &UiStrings) -> String {
        match self {
            Problem::Unreachable(reason) => fill(s.err_unreachable, &[("reason", reason)]),
            Problem::NotNtfy => s.err_not_ntfy.to_owned(),
            Problem::AuthRejected => s.err_auth.to_owned(),
            Problem::Invalid(message) => message.clone(),
            Problem::NoTopics => s.err_no_topics.to_owned(),
            Problem::TopicInvalid => s.err_topic_invalid.to_owned(),
            Problem::TopicDuplicate => s.err_topic_duplicate.to_owned(),
            Problem::SaveFailed(reason) => fill(s.err_save_failed, &[("reason", reason)]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{UI_DE, UI_EN};

    #[test]
    fn outcomes_map_to_localized_texts() {
        let text = |o: TestOutcome| Problem::from_outcome(&o).map(|p| p.text(&UI_EN));
        assert_eq!(text(TestOutcome::Ok { topics: vec![] }), None);
        assert_eq!(
            text(TestOutcome::Unreachable {
                message: "timeout".into()
            })
            .unwrap(),
            "Server unreachable (timeout)"
        );
        assert_eq!(text(TestOutcome::NotNtfy).unwrap(), UI_EN.err_not_ntfy);
        assert_eq!(text(TestOutcome::AuthFailed).unwrap(), UI_EN.err_auth);
        assert_eq!(
            text(TestOutcome::InvalidInput {
                message: "topic 2: duplicate topic".into()
            })
            .unwrap(),
            "topic 2: duplicate topic"
        );
    }

    #[test]
    fn save_failures_are_prefixed() {
        let p = Problem::SaveFailed("could not save settings".into());
        assert_eq!(p.text(&UI_EN), "Saving failed: could not save settings");
        assert_eq!(
            p.text(&UI_DE),
            "Speichern fehlgeschlagen: could not save settings"
        );
    }
}
