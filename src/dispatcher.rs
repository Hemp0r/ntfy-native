use std::collections::VecDeque;

use crate::i18n::{Strings, fill};
use crate::notification::Notification;
use crate::ntfy::model::Message;
use crate::render::render;
use crate::settings::Settings;
use crate::state::{GapFlag, RECENT_MAX, RecentMessage};

pub const BUNDLE_THRESHOLD: usize = 3;

pub struct Dispatcher {
    pub settings: Settings,
    pub recent: VecDeque<RecentMessage>,
    pub gaps: Vec<GapFlag>,
    strings: &'static Strings,
}

impl Dispatcher {
    pub fn new(
        settings: Settings,
        recent: VecDeque<RecentMessage>,
        gaps: Vec<GapFlag>,
        strings: &'static Strings,
    ) -> Self {
        Self {
            settings,
            recent,
            gaps,
            strings,
        }
    }

    fn topic_muted(&self, instance_id: &str, topic: &str) -> bool {
        self.settings
            .instance(instance_id)
            .and_then(|i| i.topics.iter().find(|t| t.name == topic))
            .is_some_and(|t| t.muted)
    }

    /// `msgs` are expected oldest first; unknown topics are treated as not muted.
    pub fn on_messages(
        &mut self,
        instance_id: &str,
        msgs: &[Message],
        catch_up: bool,
        now: i64,
    ) -> Vec<Notification> {
        let paused = self.is_paused(now);
        let mut out = Vec::new();
        let mut topics: Vec<&str> = Vec::new();
        for m in msgs {
            if !topics.contains(&m.topic.as_str()) {
                topics.push(&m.topic);
            }
        }

        for topic in topics {
            let group: Vec<&Message> = msgs.iter().filter(|m| m.topic == topic).collect();
            let rendered: Vec<Notification> = group
                .iter()
                .map(|m| {
                    render(
                        instance_id,
                        m,
                        self.settings.urgent_allowed(instance_id, topic),
                        self.strings,
                    )
                })
                .collect();

            for (m, n) in group.iter().zip(&rendered) {
                self.recent.push_front(RecentMessage {
                    instance_id: instance_id.to_string(),
                    topic: m.topic.clone(),
                    id: m.id.clone(),
                    time: m.time,
                    title: n.title.clone(),
                    body: n.body.clone(),
                    click: m.click.clone(),
                });
            }
            self.recent.truncate(RECENT_MAX);

            if paused || self.topic_muted(instance_id, topic) {
                continue;
            }

            if catch_up && group.len() > BUNDLE_THRESHOLD {
                let latest = (0..group.len())
                    .max_by_key(|&i| group[i].time)
                    .expect("group is non-empty");
                let l = &rendered[latest];
                let body = [l.title.as_str(), l.body.as_str()]
                    .iter()
                    .filter(|p| !p.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push(Notification {
                    id: format!("{instance_id}:bundle:{topic}:{}", group[latest].id),
                    thread: l.thread.clone(),
                    title: fill(
                        self.strings.bundle_title,
                        &[("topic", topic), ("n", &group.len().to_string())],
                    ),
                    body,
                    level: rendered.iter().map(|n| n.level).max().expect("non-empty"),
                    default_action: None,
                    buttons: Vec::new(),
                    image: None,
                    icon: None,
                });
            } else {
                out.extend(rendered);
            }
        }
        out
    }

    pub fn on_gap(&mut self, instance_id: &str, now: i64) -> Option<Notification> {
        let (host, topics) = {
            let inst = self.settings.instance(instance_id)?;
            let topics: Vec<(String, bool)> = inst
                .topics
                .iter()
                .map(|t| (t.name.clone(), t.muted))
                .collect();
            (inst.host(), topics)
        };
        for (name, _) in &topics {
            self.gaps
                .retain(|g| !(g.instance_id == instance_id && &g.topic == name));
            self.gaps.push(GapFlag {
                instance_id: instance_id.to_string(),
                topic: name.clone(),
                detected_at: now,
            });
        }
        let audible: Vec<&str> = topics
            .iter()
            .filter(|(_, muted)| !muted)
            .map(|(n, _)| n.as_str())
            .collect();
        if audible.is_empty() || self.is_paused(now) {
            return None;
        }
        Some(Notification {
            id: format!("{instance_id}:gap:{now}"),
            thread: format!("{instance_id}/gap"),
            title: self.strings.gap_title.to_string(),
            body: fill(
                self.strings.gap_body,
                &[("topics", &audible.join(", ")), ("instance", &host)],
            ),
            level: crate::notification::Level::Active,
            default_action: None,
            buttons: Vec::new(),
            image: None,
            icon: None,
        })
    }

    pub fn pause_all(&mut self, until: Option<i64>) {
        self.settings.paused_until = until;
    }

    pub fn set_topic_muted(&mut self, instance_id: &str, topic: &str, muted: bool) -> bool {
        let Some(t) = self
            .settings
            .instances
            .iter_mut()
            .find(|i| i.id == instance_id)
            .and_then(|i| i.topics.iter_mut().find(|t| t.name == topic))
        else {
            return false;
        };
        let changed = t.muted != muted;
        t.muted = muted;
        changed
    }

    /// `None` falls back to the global setting.
    pub fn set_topic_urgent(
        &mut self,
        instance_id: &str,
        topic: &str,
        urgent: Option<bool>,
    ) -> bool {
        let Some(t) = self
            .settings
            .instances
            .iter_mut()
            .find(|i| i.id == instance_id)
            .and_then(|i| i.topics.iter_mut().find(|t| t.name == topic))
        else {
            return false;
        };
        let changed = t.urgent_breaks_dnd != urgent;
        t.urgent_breaks_dnd = urgent;
        changed
    }

    pub fn set_urgent_global(&mut self, enabled: bool) -> bool {
        let changed = self.settings.urgent_breaks_dnd != enabled;
        self.settings.urgent_breaks_dnd = enabled;
        changed
    }

    pub fn set_autostart(&mut self, enabled: bool) -> bool {
        let changed = self.settings.autostart != enabled;
        self.settings.autostart = enabled;
        changed
    }

    /// Returns whether a flag was removed.
    pub fn dismiss_gap(&mut self, instance_id: &str, topic: &str) -> bool {
        let before = self.gaps.len();
        self.gaps
            .retain(|g| !(g.instance_id == instance_id && g.topic == topic));
        self.gaps.len() != before
    }

    pub fn is_paused(&mut self, now: i64) -> bool {
        match self.settings.paused_until {
            Some(t) if t > now => true,
            Some(_) => {
                self.settings.paused_until = None;
                false
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;
    use crate::notification::Level;
    use crate::settings::{Instance, Topic};

    const NOW: i64 = 1_000;

    fn settings(topics: &[(&str, bool)]) -> Settings {
        Settings {
            instances: vec![Instance {
                id: "i".into(),
                url: "https://ntfy.example.org".into(),
                topics: topics
                    .iter()
                    .map(|(n, m)| Topic {
                        name: (*n).into(),
                        muted: *m,
                        urgent_breaks_dnd: None,
                    })
                    .collect(),
                ..Instance::default()
            }],
            ..Settings::default()
        }
    }

    fn disp(topics: &[(&str, bool)]) -> Dispatcher {
        Dispatcher::new(settings(topics), VecDeque::new(), Vec::new(), &EN)
    }

    #[test]
    fn urgent_and_gap_setters_report_changes() {
        let mut d = disp(&[("t", false)]);
        assert!(d.set_topic_urgent("i", "t", Some(true)));
        assert!(!d.set_topic_urgent("i", "t", Some(true)));
        assert!(!d.set_topic_urgent("i", "nope", Some(true)));
        assert!(d.settings.urgent_allowed("i", "t"));
        assert!(d.set_topic_urgent("i", "t", None));
        assert!(d.set_urgent_global(true));
        assert!(!d.set_urgent_global(true));
        assert!(d.set_autostart(false));
        assert!(!d.set_autostart(false));

        d.on_gap("i", 5);
        assert!(d.dismiss_gap("i", "t"));
        assert!(!d.dismiss_gap("i", "t"));
        assert!(d.gaps.is_empty());
    }

    fn msg(id: &str, topic: &str, time: i64) -> Message {
        Message {
            id: id.into(),
            time,
            topic: topic.into(),
            title: None,
            message: Some(format!("body {id}")),
            tags: vec![],
            priority: 3,
            click: None,
            icon: None,
            actions: vec![],
            attachment: None,
            expires: None,
        }
    }

    #[test]
    fn message_notifies_and_is_recent() {
        let mut d = disp(&[("a", false)]);
        let m = msg("m1", "a", 5);
        let n = d.on_messages("i", std::slice::from_ref(&m), false, NOW);
        assert_eq!(n.len(), 1);
        assert_eq!(d.recent[0].id, "m1");
        assert_eq!(d.recent[0].body, "body m1");
    }

    #[test]
    fn muted_topic_recorded_not_notified() {
        let mut d = disp(&[("a", true)]);
        assert!(
            d.on_messages("i", &[msg("m1", "a", 5)], false, NOW)
                .is_empty()
        );
        assert_eq!(d.recent.len(), 1);
    }

    #[test]
    fn unknown_topic_still_notifies() {
        let mut d = disp(&[("a", true)]);
        assert_eq!(
            d.on_messages("i", &[msg("m1", "gone", 5)], false, NOW)
                .len(),
            1
        );
        assert_eq!(d.recent.len(), 1);
    }

    #[test]
    fn pause_until_suppresses_then_expires() {
        let mut d = disp(&[("a", false)]);
        d.pause_all(Some(NOW + 60));
        assert!(
            d.on_messages("i", &[msg("m1", "a", 5)], false, NOW)
                .is_empty()
        );
        assert_eq!(d.recent.len(), 1);
        assert_eq!(
            d.on_messages("i", &[msg("m2", "a", 6)], false, NOW + 61)
                .len(),
            1
        );
        assert_eq!(d.settings.paused_until, None);
    }

    #[test]
    fn pause_indefinitely() {
        let mut d = disp(&[("a", false)]);
        d.pause_all(Some(i64::MAX));
        assert!(d.is_paused(NOW + 1_000_000_000));
        d.pause_all(None);
        assert!(!d.is_paused(NOW));
    }

    #[test]
    fn catch_up_bundles_above_threshold() {
        let mut d = disp(&[("a", false), ("b", false)]);
        let mut msgs: Vec<Message> = (0..4).map(|i| msg(&format!("a{i}"), "a", i)).collect();
        msgs.push(msg("b0", "b", 10));
        msgs.push(msg("b1", "b", 11));
        msgs[3].priority = 4;
        let n = d.on_messages("i", &msgs, true, NOW);
        assert_eq!(n.len(), 3);
        let bundle = &n[0];
        assert_eq!(bundle.id, "i:bundle:a:a3");
        assert_eq!(bundle.title, "a: 4 new messages");
        assert_eq!(bundle.body, "a\nbody a3");
        assert_eq!(bundle.level, Level::Active);
        assert!(bundle.default_action.is_none() && bundle.buttons.is_empty());
        assert_eq!(bundle.thread, "i/a");
        assert_eq!(d.recent.len(), 6);
    }

    #[test]
    fn bundle_threshold_is_exclusive() {
        let mut d = disp(&[("a", false)]);
        let msgs: Vec<Message> = (0..3).map(|i| msg(&format!("a{i}"), "a", i)).collect();
        assert_eq!(d.on_messages("i", &msgs, true, NOW).len(), 3);
    }

    #[test]
    fn live_burst_not_bundled() {
        let mut d = disp(&[("a", false)]);
        let msgs: Vec<Message> = (0..5).map(|i| msg(&format!("a{i}"), "a", i)).collect();
        assert_eq!(d.on_messages("i", &msgs, false, NOW).len(), 5);
    }

    #[test]
    fn recent_capped_newest_first() {
        let mut d = disp(&[("a", false)]);
        let msgs: Vec<Message> = (0..25).map(|i| msg(&format!("a{i}"), "a", i)).collect();
        d.on_messages("i", &msgs, false, NOW);
        assert_eq!(d.recent.len(), RECENT_MAX);
        assert_eq!(d.recent[0].id, "a24");
    }

    #[test]
    fn urgent_respects_overrides() {
        let mut m = msg("m", "a", 1);
        m.priority = 5;
        let mut d = disp(&[("a", false)]);
        d.settings.urgent_breaks_dnd = false;
        d.settings.instances[0].topics[0].urgent_breaks_dnd = Some(true);
        assert_eq!(
            d.on_messages("i", &[m.clone()], false, NOW)[0].level,
            Level::Urgent
        );
        d.settings.urgent_breaks_dnd = true;
        d.settings.instances[0].topics[0].urgent_breaks_dnd = Some(false);
        assert_eq!(d.on_messages("i", &[m], false, NOW)[0].level, Level::Active);
    }

    #[test]
    fn gap_flags_and_notification() {
        let mut d = disp(&[("a", false), ("b", true)]);
        d.gaps.push(GapFlag {
            instance_id: "i".into(),
            topic: "a".into(),
            detected_at: 1,
        });
        let n = d.on_gap("i", NOW).expect("notification");
        assert_eq!(d.gaps.len(), 2);
        assert!(d.gaps.iter().all(|g| g.detected_at == NOW));
        assert_eq!(n.level, Level::Active);
        assert_eq!(n.body, "a on ntfy.example.org");
    }

    #[test]
    fn gap_suppressed_when_paused_but_flagged() {
        let mut d = disp(&[("a", false)]);
        d.pause_all(Some(i64::MAX));
        assert!(d.on_gap("i", NOW).is_none());
        assert_eq!(d.gaps.len(), 1);
    }

    #[test]
    fn gap_suppressed_when_all_muted_but_flagged() {
        let mut d = disp(&[("a", true)]);
        assert!(d.on_gap("i", NOW).is_none());
        assert_eq!(d.gaps.len(), 1);
    }

    #[test]
    fn set_topic_muted_reports_change() {
        let mut d = disp(&[("a", false)]);
        assert!(d.set_topic_muted("i", "a", true));
        assert!(!d.set_topic_muted("i", "a", true));
        assert!(!d.set_topic_muted("i", "zzz", true));
    }
}
