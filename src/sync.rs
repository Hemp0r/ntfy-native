use crate::ntfy::model::Message;
use crate::state::{InstanceState, SEEN_RING, SeenMessage};

pub const DEDUP_MARGIN_SECS: i64 = 10;

/// Server-time `since` for the catch-up poll; `None` without history.
pub fn catch_up_since(st: &InstanceState) -> Option<i64> {
    let last = st.last_message.as_ref().map(|m| m.time);
    let oldest = match (st.watermark, last) {
        (Some(w), Some(l)) => w.min(l),
        (Some(t), None) | (None, Some(t)) => t,
        (None, None) => return None,
    };
    Some(oldest - DEDUP_MARGIN_SECS)
}

pub fn stream_since(server_now: i64) -> i64 {
    server_now - DEDUP_MARGIN_SECS
}

impl InstanceState {
    /// Returns true if the message is new and should be delivered.
    pub fn accept(&mut self, msg: &Message) -> bool {
        if self.seen_ids.contains(&msg.id) {
            return false;
        }
        if let Some(w) = self.watermark
            && msg.time < w - DEDUP_MARGIN_SECS
        {
            return false;
        }
        self.seen_ids.push_back(msg.id.clone());
        while self.seen_ids.len() > SEEN_RING {
            self.seen_ids.pop_front();
        }
        self.observe_time(msg.time);
        if self
            .last_message
            .as_ref()
            .is_none_or(|l| msg.time >= l.time)
        {
            self.last_message = Some(SeenMessage {
                id: msg.id.clone(),
                time: msg.time,
            });
        }
        true
    }

    pub fn observe_time(&mut self, server_time: i64) {
        self.watermark = Some(self.watermark.map_or(server_time, |w| w.max(server_time)));
    }
}

/// Detects possibly missed messages. Call with the state from before `accept`
/// was applied to the poll results. A gap exists if either
/// (a) we were offline longer than the server caches (`server_now - watermark
/// > cache_duration`), or
/// (b) our last known message is still within the cache window but the poll
/// no longer returns it, which means the server lost its cache (e.g. restart
/// without `cache-file`). Only server times are compared.
pub fn detect_gap(
    st: &InstanceState,
    polled: &[Message],
    server_now: i64,
    cache_duration: i64,
) -> bool {
    if st
        .watermark
        .is_some_and(|w| server_now - w > cache_duration)
    {
        return true;
    }
    st.last_message.as_ref().is_some_and(|l| {
        server_now - l.time < cache_duration && !polled.iter().any(|m| m.id == l.id)
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
        }
    }

    fn st(watermark: Option<i64>, last: Option<(&str, i64)>) -> InstanceState {
        InstanceState {
            watermark,
            last_message: last.map(|(id, time)| SeenMessage {
                id: id.into(),
                time,
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
        assert_eq!(catch_up_since(&st(Some(1000), Some(("a", 900)))), Some(890));
        assert_eq!(catch_up_since(&st(Some(1000), None)), Some(990));
        assert_eq!(catch_up_since(&st(None, Some(("a", 900)))), Some(890));
    }

    #[test]
    fn stream_since_subtracts_margin() {
        assert_eq!(stream_since(1000), 990);
    }

    #[test]
    fn duplicate_id_rejected() {
        let mut s = InstanceState::default();
        let m = msg("a", 100);
        assert!(s.accept(&m));
        assert!(!s.accept(&m));
    }

    #[test]
    fn old_message_rejected_recent_unseen_accepted() {
        let mut s = st(Some(1000), None);
        assert!(!s.accept(&msg("old", 900)));
        assert!(s.accept(&msg("new", 995)));
        assert!(!s.seen_ids.contains(&"old".to_string()));
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
    fn watermark_monotonic() {
        let mut s = InstanceState::default();
        s.observe_time(100);
        s.observe_time(50);
        assert_eq!(s.watermark, Some(100));
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
    fn no_gap_when_last_message_in_poll() {
        let now = 1_000_000;
        let s = st(Some(now - 60), Some(("a", now - 100)));
        assert!(!detect_gap(&s, &[msg("a", now - 100)], now, 43_200));
    }

    #[test]
    fn gap_when_recent_last_message_missing() {
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
}
