//! Drives a `spawn_connection` task and reads its events.

use std::time::Duration;

use ntfy_native::connection::{
    ConnControl, ConnEvent, ConnStatus, ConnectionConfig, spawn_connection,
};
use ntfy_native::ntfy::client::http_client;
use ntfy_native::state::InstanceState;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct Conn {
    pub rx: mpsc::UnboundedReceiver<(String, ConnEvent)>,
    pub ctl: ConnControl,
    pub handle: JoinHandle<()>,
    instance_id: String,
    /// Maximum wait for a single event.
    pub patience: Duration,
}

impl Conn {
    pub fn start(cfg: ConnectionConfig, state: InstanceState) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let ctl = ConnControl::default();
        let instance_id = cfg.instance_id.clone();
        let handle = spawn_connection(cfg, state, http_client(), tx, ctl.clone());
        Self {
            rx,
            ctl,
            handle,
            instance_id,
            patience: Duration::from_secs(2),
        }
    }

    pub async fn next(&mut self) -> ConnEvent {
        let (id, ev) = tokio::time::timeout(self.patience, self.rx.recv())
            .await
            .unwrap_or_else(|_| panic!("no connection event within {:?}", self.patience))
            .expect("connection task alive");
        assert_eq!(id, self.instance_id);
        ev
    }

    /// Skips events until `pred` matches.
    pub async fn until(&mut self, pred: impl Fn(&ConnEvent) -> bool) -> ConnEvent {
        loop {
            let ev = self.next().await;
            if pred(&ev) {
                return ev;
            }
        }
    }

    /// Collects events until `pred` matches (inclusive).
    pub async fn collect_until(&mut self, pred: impl Fn(&ConnEvent) -> bool) -> Vec<ConnEvent> {
        let mut out = Vec::new();
        loop {
            let ev = self.next().await;
            let done = pred(&ev);
            out.push(ev);
            if done {
                return out;
            }
        }
    }

    /// Collects all events arriving within `window`.
    pub async fn collect_for(&mut self, window: Duration) -> Vec<ConnEvent> {
        let deadline = tokio::time::Instant::now() + window;
        let mut out = Vec::new();
        while let Ok(Some((_, ev))) = tokio::time::timeout_at(deadline, self.rx.recv()).await {
            out.push(ev);
        }
        out
    }

    /// Cancels the task and waits for it to end.
    pub async fn stop(self) {
        self.ctl.cancel.cancel();
        self.handle.await.unwrap();
    }
}

pub fn is_connected(ev: &ConnEvent) -> bool {
    *ev == ConnEvent::Status(ConnStatus::Connected)
}

pub fn is_disconnected(ev: &ConnEvent) -> bool {
    matches!(ev, ConnEvent::Status(ConnStatus::Disconnected { .. }))
}

pub fn message_ids(ev: &ConnEvent) -> Vec<String> {
    match ev {
        ConnEvent::Messages { messages, .. } => messages.iter().map(|m| m.id.clone()).collect(),
        _ => vec![],
    }
}

pub fn last_state(events: &[ConnEvent]) -> Option<InstanceState> {
    events.iter().rev().find_map(|e| match e {
        ConnEvent::State(s) => Some(s.clone()),
        _ => None,
    })
}
