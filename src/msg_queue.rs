//! Queue from the core to one UI child's writer thread. A snapshot still
//! waiting to be written is replaced by a newer one, so a child that reads
//! slowly never piles up stale snapshots.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};

use crate::ipc::CoreMsg;

#[derive(Default)]
pub struct MsgQueue {
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Default)]
struct State {
    items: VecDeque<CoreMsg>,
    closed: bool,
}

impl MsgQueue {
    /// `false` once closed. A queued snapshot is dropped and the new one
    /// appended, so it still follows everything queued before it.
    pub fn push(&self, msg: CoreMsg) -> bool {
        let mut s = self.lock();
        if s.closed {
            return false;
        }
        if matches!(msg, CoreMsg::Snapshot(_)) {
            s.items.retain(|m| !matches!(m, CoreMsg::Snapshot(_)));
        }
        s.items.push_back(msg);
        self.ready.notify_one();
        true
    }

    /// Ends the queue; what is queued is still handed out.
    pub fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// Blocks until a message is queued; `None` once closed and empty.
    pub fn pop(&self) -> Option<CoreMsg> {
        let mut s = self.lock();
        loop {
            if let Some(m) = s.items.pop_front() {
                return Some(m);
            }
            if s.closed {
                return None;
            }
            s = self.ready.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::snapshot::UiSnapshot;

    fn snap(version: &str) -> CoreMsg {
        CoreMsg::Snapshot(UiSnapshot {
            app_version: version.into(),
            configured: true,
            instances: vec![],
            recent: vec![],
            gaps: vec![],
            paused_until: None,
            urgent_breaks_dnd: false,
            autostart: true,
        })
    }

    fn save(id: u64) -> CoreMsg {
        CoreMsg::SaveResult {
            request_id: id,
            result: Ok("a".into()),
        }
    }

    fn drain(q: &MsgQueue) -> Vec<CoreMsg> {
        q.close();
        std::iter::from_fn(|| q.pop()).collect()
    }

    #[test]
    fn keeps_order() {
        let q = MsgQueue::default();
        for m in [
            CoreMsg::Hello { version: 1 },
            snap("1"),
            save(1),
            CoreMsg::Close,
        ] {
            assert!(q.push(m));
        }
        assert_eq!(
            drain(&q),
            [
                CoreMsg::Hello { version: 1 },
                snap("1"),
                save(1),
                CoreMsg::Close
            ]
        );
    }

    #[test]
    fn unsent_snapshot_is_replaced_by_the_newest_at_the_end() {
        // The newest snapshot must still follow a result queued after the
        // stale one (the UI unlocks on the snapshot after a save).
        let q = MsgQueue::default();
        q.push(snap("1"));
        q.push(save(1));
        q.push(snap("2"));
        q.push(snap("3"));
        assert_eq!(drain(&q), [save(1), snap("3")]);
    }

    #[test]
    fn written_snapshot_is_not_replaced() {
        let q = MsgQueue::default();
        q.push(snap("1"));
        assert_eq!(q.pop(), Some(snap("1")));
        q.push(snap("2"));
        assert_eq!(drain(&q), [snap("2")]);
    }

    #[test]
    fn closed_queue_drains_then_ends_and_rejects() {
        let q = MsgQueue::default();
        q.push(save(1));
        q.close();
        assert!(!q.push(save(2)));
        assert_eq!(q.pop(), Some(save(1)));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn pop_waits_for_a_message_or_close() {
        let q = Arc::new(MsgQueue::default());
        let reader = {
            let q = q.clone();
            thread::spawn(move || std::iter::from_fn(|| q.pop()).collect::<Vec<_>>())
        };
        thread::sleep(Duration::from_millis(50));
        q.push(save(1));
        thread::sleep(Duration::from_millis(50));
        q.close();
        assert_eq!(reader.join().unwrap(), [save(1)]);
    }
}
