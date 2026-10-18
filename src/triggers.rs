//! Reconnect triggers: wake from sleep and network changes.

use std::time::{Duration, SystemTime};

use futures_util::{Stream, StreamExt};
use if_watch::IfEvent;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};

use crate::engine::Command;

const WAKE_TICK: Duration = Duration::from_secs(5);
const WAKE_THRESHOLD: Duration = Duration::from_secs(30);
/// The watcher reports every existing address at startup.
const NET_STARTUP_IGNORE: Duration = Duration::from_secs(5);
const NET_DEBOUNCE: Duration = Duration::from_secs(2);

/// True if the wall clock advanced by more than `tick + threshold`.
pub fn is_wake_jump(
    prev: SystemTime,
    now: SystemTime,
    tick: Duration,
    threshold: Duration,
) -> bool {
    now.duration_since(prev)
        .is_ok_and(|elapsed| elapsed > tick + threshold)
}

/// Sends `ReconnectAll` after the machine woke from sleep. The tokio timer
/// does not advance while asleep, the wall clock does.
pub fn spawn_wake_detector(tx: mpsc::UnboundedSender<Command>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(WAKE_TICK);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut prev = SystemTime::now();
        loop {
            ticker.tick().await;
            let now = SystemTime::now();
            if is_wake_jump(prev, now, WAKE_TICK, WAKE_THRESHOLD) {
                tracing::info!("wake from sleep detected");
                if tx.send(Command::ReconnectAll).is_err() {
                    return;
                }
            }
            prev = now;
        }
    })
}

/// Sends `ReconnectAll` once the network configuration has settled after a change.
pub fn spawn_network_watcher(tx: mpsc::UnboundedSender<Command>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        match if_watch::tokio::IfWatcher::new() {
            Ok(watcher) => watch_network(watcher, tx, NET_STARTUP_IGNORE, NET_DEBOUNCE).await,
            Err(e) => tracing::warn!(error = %e, "network watcher unavailable"),
        }
    })
}

/// Ignores events during `ignore_for`, then sends `ReconnectAll` once no
/// further event arrived for `quiet`. Stops on the first error, since the
/// watcher may keep failing immediately; the keepalive watchdog still covers
/// lost connections.
async fn watch_network<S, E>(
    mut events: S,
    tx: mpsc::UnboundedSender<Command>,
    ignore_for: Duration,
    quiet: Duration,
) where
    S: Stream<Item = Result<IfEvent, E>> + Unpin,
    E: std::fmt::Display,
{
    let ignore_until = Instant::now() + ignore_for;
    let mut deadline: Option<Instant> = None;
    loop {
        let settled = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(ev)) => {
                    if Instant::now() >= ignore_until {
                        tracing::debug!(?ev, "network change");
                        deadline = Some(Instant::now() + quiet);
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "network watcher failed, stopping it");
                    return;
                }
                None => return,
            },
            _ = settled => {
                deadline = None;
                tracing::info!("network changed");
                if tx.send(Command::ReconnectAll).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::pin::Pin;

    use if_watch::IpNet;

    use super::*;

    #[test]
    fn wake_jump_detection() {
        let prev = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let check = |secs| {
            is_wake_jump(
                prev,
                prev + Duration::from_secs(secs),
                WAKE_TICK,
                WAKE_THRESHOLD,
            )
        };
        assert!(!check(5));
        assert!(!check(35));
        assert!(check(36));
        assert!(check(40));
        // Clock set backwards: no wake.
        assert!(!is_wake_jump(
            prev,
            prev - Duration::from_secs(3600),
            WAKE_TICK,
            WAKE_THRESHOLD
        ));
    }

    type Events = Pin<Box<dyn Stream<Item = io::Result<IfEvent>> + Send>>;

    fn events() -> (mpsc::UnboundedSender<io::Result<IfEvent>>, Events) {
        let (tx, rx) = mpsc::unbounded_channel();
        let stream =
            futures_util::stream::unfold(
                rx,
                |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
            );
        (tx, Box::pin(stream))
    }

    fn net() -> IpNet {
        "192.168.1.2/24".parse().unwrap()
    }

    fn up() -> IfEvent {
        IfEvent::Up(net())
    }

    fn reconnects(rx: &mut mpsc::UnboundedReceiver<Command>) -> usize {
        let mut n = 0;
        while let Ok(cmd) = rx.try_recv() {
            assert!(matches!(cmd, Command::ReconnectAll), "{cmd:?}");
            n += 1;
        }
        n
    }

    #[tokio::test(start_paused = true)]
    async fn network_changes_ignored_at_startup_then_debounced() {
        let (ev_tx, events) = events();
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(watch_network(events, tx, NET_STARTUP_IGNORE, NET_DEBOUNCE));
        let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));

        sleep(1000).await;
        ev_tx.send(Ok(up())).unwrap();
        sleep(5000).await; // t = 6 s
        assert_eq!(reconnects(&mut rx), 0, "startup events are ignored");

        ev_tx.send(Ok(up())).unwrap();
        sleep(1000).await; // t = 7 s
        ev_tx.send(Ok(IfEvent::Down(net()))).unwrap();
        sleep(1900).await; // t = 8.9 s
        assert_eq!(reconnects(&mut rx), 0, "still settling");
        sleep(200).await; // t = 9.1 s
        assert_eq!(reconnects(&mut rx), 1);
        sleep(10_000).await;
        assert_eq!(reconnects(&mut rx), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn network_watcher_stops_on_error() {
        let (ev_tx, events) = events();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(watch_network(events, tx, Duration::ZERO, NET_DEBOUNCE));
        ev_tx.send(Err(io::Error::other("gone"))).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("watcher ends")
            .unwrap();
        assert_eq!(reconnects(&mut rx), 0);
    }
}
