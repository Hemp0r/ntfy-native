use crate::ntfy::model::Message;
use crate::state::{InstanceState, SEEN_RING, SeenMessage};

pub const DEDUP_MARGIN_SECS: i64 = 30;

/// Server-time `since` for the catch-up poll; `None` without history.
pub fn catch_up_since(st: &InstanceState) -> Option<i64> {
    let last = st.last_message.as_ref().map(|m| m.time);
    let oldest = match (st.watermark, last) {
        (Some(w), Some(l)) => w.min(l),
        (Some(t), None) | (None, Some(t)) => t,
        (None, None) => return None,
    };
    Some(oldest.saturating_sub(DEDUP_MARGIN_SECS))
}

pub fn stream_since(server_now: i64) -> i64 {
    server_now.saturating_sub(DEDUP_MARGIN_SECS)
}

impl InstanceState {
    /// Returns true if the message id has not been seen and should be delivered.
    /// Deduplication is by id only, so a server clock jump cannot drop messages.
    /// Only ever raises the watermark (clock regression is handled solely in
    /// `observe_time`); `last_message` is replaced only by an equal or newer message.
    pub fn accept(&mut self, msg: &Message) -> bool {
        if self.seen_ids.contains(&msg.id) {
            return false;
        }
        self.seen_ids.push_back(msg.id.clone());
        while self.seen_ids.len() > SEEN_RING {
            self.seen_ids.pop_front();
        }
        self.watermark = Some(self.watermark.map_or(msg.time, |w| w.max(msg.time)));
        if self
            .last_message
            .as_ref()
            .is_none_or(|l| msg.time >= l.time)
        {
            self.last_message = Some(SeenMessage {
                id: msg.id.clone(),
                time: msg.time,
                expires: msg.expires,
            });
        }
        true
    }

    /// Raises the watermark. A time more than the margin behind the watermark
    /// means the server clock went backwards: follow it and cap `last_message`.
    pub fn observe_time(&mut self, server_time: i64) {
        match self.watermark {
            Some(w) if server_time < w.saturating_sub(DEDUP_MARGIN_SECS) => {
                self.watermark = Some(server_time);
                if let Some(l) = self.last_message.as_mut() {
                    l.time = l.time.min(server_time);
                }
            }
            Some(w) => self.watermark = Some(w.max(server_time)),
            None => self.watermark = Some(server_time),
        }
    }

    /// Call after `detect_gap` returned true and before accepting the polled
    /// messages, so the "possibly missed" hint fires only once. Catch-up callers
    /// should use `apply_catch_up`, which enforces this order. Sets the
    /// watermark to `server_now` (also when the server clock went backwards)
    /// and forgets `last_message`; accepting older polled messages afterwards
    /// does not lower the watermark.
    pub fn rebaseline_after_gap(&mut self, server_now: i64) {
        self.watermark = Some(server_now);
        self.last_message = None;
    }
}

/// Result of `InstanceState::apply_catch_up`.
#[derive(Debug, PartialEq)]
pub struct CatchUpResult {
    pub gap: bool,
    pub accepted: Vec<Message>,
}

impl InstanceState {
    /// Processes one catch-up poll in the only correct order: gap detection on
    /// the pre-poll state, rebaseline on a gap, accept the polled messages, then
    /// observe the server time (which pulls the watermark back if pre-correction
    /// messages raised it past a corrected server clock). Without a server time
    /// (no `Date` header) no gap can be computed and nothing time-related is
    /// touched; the local clock is never used.
    pub fn apply_catch_up(
        &mut self,
        polled: Vec<Message>,
        server_now: Option<i64>,
        cache_duration: i64,
    ) -> CatchUpResult {
        let mut gap = false;
        if let Some(now) = server_now {
            gap = detect_gap(self, &polled, now, cache_duration);
            if gap {
                self.rebaseline_after_gap(now);
            }
        }
        let accepted = polled.into_iter().filter(|m| self.accept(m)).collect();
        if let Some(now) = server_now {
            self.observe_time(now);
        }
        CatchUpResult { gap, accepted }
    }
}

/// Detects possibly missed messages. Call with the state from before `accept`
/// was applied to the poll results. A gap exists if any of
/// (a) we were offline longer than the server caches
/// (`server_now - watermark > cache_duration`),
/// (b) the last known message carries an `expires` still in the future (so the
/// server cache must still hold it) but the poll does not return it, i.e. the
/// server lost its cache (restart without `cache-file`), or
/// (c) the server clock is behind our watermark by more than the margin (it was
/// turned back while we were offline), so our timeline is no longer comparable.
/// Catch-up callers should use `InstanceState::apply_catch_up` instead of calling
/// this directly.
/// Messages without `expires` (`Cache: no`, or state from an older app version)
/// are never sentinels. Only server times are compared. Not detecting a gap is
/// no proof of completeness: without a recent message in the server cache a
/// lost cache is undetectable.
pub fn detect_gap(
    st: &InstanceState,
    polled: &[Message],
    server_now: i64,
    cache_duration: i64,
) -> bool {
    if st.watermark.is_some_and(|w| {
        server_now.saturating_sub(w) > cache_duration
            || server_now < w.saturating_sub(DEDUP_MARGIN_SECS)
    }) {
        return true;
    }
    st.last_message.as_ref().is_some_and(|l| {
        l.expires.is_some_and(|e| server_now < e) && !polled.iter().any(|m| m.id == l.id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, time: i64) -> Message {
        Message {
            id: id.into(),
            time,
            topic: "t".into(),
            title: None,
            message: None,
            tags: vec![],
            priority: 3,
            click: None,
            icon: None,
            actions: vec![],
            attachment: None,
            expires: None,
        }
    }

    fn st(watermark: Option<i64>, last: Option<(&str, i64)>) -> InstanceState {
        InstanceState {
            watermark,
            last_message: last.map(|(id, time)| SeenMessage {
                id: id.into(),
                time,
                expires: Some(time + 43_200),
            }),
            seen_ids: Default::default(),
        }
    }

    #[test]
    fn no_history_no_catch_up() {
        assert_eq!(catch_up_since(&InstanceState::default()), None);
    }

    #[test]
    fn since_uses_min_minus_margin() {
        assert_eq!(catch_up_since(&st(Some(1000), Some(("a", 900)))), Some(870));
        assert_eq!(catch_up_since(&st(Some(1000), None)), Some(970));
        assert_eq!(catch_up_since(&st(None, Some(("a", 900)))), Some(870));
    }

    #[test]
    fn stream_since_subtracts_margin() {
        assert_eq!(stream_since(1000), 970);
    }

    #[test]
    fn duplicate_id_rejected() {
        let mut s = InstanceState::default();
        let m = msg("a", 100);
        assert!(s.accept(&m));
        assert!(!s.accept(&m));
    }

    #[test]
    fn old_unseen_message_accepted() {
        let mut s = st(Some(1000), None);
        assert!(s.accept(&msg("old", 900)));
        assert!(s.seen_ids.contains(&"old".to_string()));
        assert_eq!(s.watermark, Some(1000));
    }

    #[test]
    fn seen_ring_capped() {
        let mut s = InstanceState::default();
        for i in 0..=SEEN_RING {
            assert!(s.accept(&msg(&format!("id{i}"), 100)));
        }
        assert_eq!(s.seen_ids.len(), SEEN_RING);
        assert!(!s.seen_ids.contains(&"id0".to_string()));
        assert!(s.seen_ids.contains(&format!("id{SEEN_RING}")));
    }

    #[test]
    fn watermark_monotonic_within_margin() {
        let mut s = InstanceState::default();
        s.observe_time(100);
        s.observe_time(80);
        assert_eq!(s.watermark, Some(100));
    }

    #[test]
    fn watermark_follows_clock_regression() {
        let mut s = st(None, Some(("a", 1000)));
        s.observe_time(1000);
        s.observe_time(100);
        assert_eq!(s.watermark, Some(100));
        assert_eq!(s.last_message.as_ref().unwrap().time, 100);
    }

    #[test]
    fn accept_raises_watermark() {
        let mut s = InstanceState::default();
        s.accept(&msg("a", 10));
        assert_eq!(s.watermark, Some(10));
    }

    #[test]
    fn last_message_is_latest() {
        let mut s = InstanceState::default();
        s.accept(&msg("a", 10));
        s.accept(&msg("b", 5));
        assert_eq!(s.last_message.as_ref().unwrap().time, 10);
        assert_eq!(s.last_message.as_ref().unwrap().id, "a");
    }

    #[test]
    fn gap_when_offline_longer_than_cache() {
        assert!(detect_gap(&st(Some(0), None), &[], 50_000, 43_200));
    }

    #[test]
    fn no_gap_without_history() {
        assert!(!detect_gap(&InstanceState::default(), &[], 50_000, 43_200));
    }

    #[test]
    fn no_gap_at_exact_cache_duration() {
        assert!(!detect_gap(
            &st(Some(1000), None),
            &[],
            1000 + 43_200,
            43_200
        ));
    }

    #[test]
    fn no_gap_when_last_message_in_poll() {
        let now = 1_000_000;
        let s = st(Some(now - 60), Some(("a", now - 100)));
        assert!(!detect_gap(&s, &[msg("a", now - 100)], now, 43_200));
    }

    #[test]
    fn gap_when_unexpired_last_message_missing() {
        let now = 1_000_000;
        let s = st(Some(now - 60), Some(("a", now - 100)));
        assert!(detect_gap(&s, &[], now, 43_200));
    }

    #[test]
    fn no_gap_when_last_message_expired_normally() {
        let now = 1_000_000;
        let s = st(Some(now - 60), Some(("a", now - 100_000)));
        assert!(!detect_gap(&s, &[], now, 43_200));
    }

    #[test]
    fn no_gap_when_last_message_has_no_expires() {
        let now = 1_000_000;
        let mut s = st(Some(now - 60), Some(("a", now - 100)));
        s.last_message.as_mut().unwrap().expires = None;
        assert!(!detect_gap(&s, &[], now, 43_200));
    }

    fn msg_exp(id: &str, time: i64, expires: i64) -> Message {
        Message {
            expires: Some(expires),
            ..msg(id, time)
        }
    }

    #[test]
    fn gap_fires_once_after_rebaseline() {
        let now = 1_000_000;
        let mut s = st(Some(now - 60), Some(("a", now - 100)));
        let c = msg_exp("c", now - 50, now + 1000);
        let r = s.apply_catch_up(vec![c.clone()], Some(now), 43_200);
        assert!(r.gap);
        assert_eq!(r.accepted, vec![c.clone()]);
        assert_eq!(s.watermark, Some(now));
        let r = s.apply_catch_up(vec![c], Some(now + 5), 43_200);
        assert!(!r.gap);
        assert!(r.accepted.is_empty());
    }

    #[test]
    fn gap_on_server_clock_regression_while_offline() {
        let mut s = st(Some(10_000), Some(("a", 10_000)));
        s.last_message.as_mut().unwrap().expires = Some(20_000);
        s.seen_ids.push_back("a".into());
        let r = s.apply_catch_up(vec![msg_exp("a", 10_000, 20_000)], Some(7_000), 43_200);
        assert!(r.gap);
        assert!(r.accepted.is_empty());
        assert_eq!(s.watermark, Some(7_000));
        assert_eq!(catch_up_since(&s), Some(6_970));
    }

    #[test]
    fn clock_regression_with_unseen_pre_correction_message() {
        let mut s = st(Some(10_000), Some(("a", 10_000)));
        s.last_message.as_mut().unwrap().expires = Some(20_000);
        s.seen_ids.push_back("a".into());
        let a = msg_exp("a", 10_000, 20_000);
        let b = msg_exp("b", 10_010, 20_010);
        let r = s.apply_catch_up(vec![a.clone(), b.clone()], Some(7_000), 43_200);
        assert!(r.gap);
        assert_eq!(r.accepted, vec![b.clone()]);
        assert!(catch_up_since(&s) <= Some(7_000 - DEDUP_MARGIN_SECS));
        let r = s.apply_catch_up(vec![a, b], Some(7_010), 43_200);
        assert!(!r.gap);
        assert!(r.accepted.is_empty());
    }

    #[test]
    fn cache_loss_hint_fires_once() {
        let now = 1_000_000;
        let mut s = st(Some(now - 60), Some(("a", now - 100)));
        assert!(s.last_message.as_ref().unwrap().expires > Some(now + 5));
        assert!(s.apply_catch_up(vec![], Some(now), 43_200).gap);
        assert!(!s.apply_catch_up(vec![], Some(now + 5), 43_200).gap);
    }

    #[test]
    fn catch_up_without_server_time_skips_gap_detection() {
        let mut s = st(Some(0), Some(("a", 0)));
        let r = s.apply_catch_up(vec![msg("b", 5)], None, 43_200);
        assert!(!r.gap);
        assert_eq!(r.accepted, vec![msg("b", 5)]);
        assert_eq!(s.watermark, Some(5));
    }

    #[test]
    fn poll_then_overlapping_stream_replay_accepts_each_once() {
        let mut s = InstanceState::default();
        let polled = [msg("a", 100), msg("b", 101), msg("c", 102)];
        assert!(polled.iter().all(|m| s.accept(m)));
        let replay = [msg("b", 101), msg("c", 102), msg("d", 103)];
        let accepted: Vec<_> = replay
            .iter()
            .filter(|m| s.accept(m))
            .map(|m| &m.id)
            .collect();
        assert_eq!(accepted, ["d"]);
    }
}
