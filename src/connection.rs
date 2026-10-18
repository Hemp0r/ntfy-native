use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::ntfy::client::{self, EventStream, NetworkKind, NtfyError, PollResult};
use crate::ntfy::model::{Event, Message};
use crate::secrets::Auth;
use crate::settings::Instance;
use crate::state::InstanceState;
use crate::sync::catch_up_since;

/// Minimum interval between `State` events caused by keepalives alone.
const KEEPALIVE_STATE_INTERVAL: Duration = Duration::from_secs(60);
/// Upper bound for a server-requested `Retry-After` wait (lower bound:
/// `backoff_initial`, so `Retry-After: 0` cannot cause a tight loop).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(600);

#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    pub instance_id: String,
    pub base_url: String,
    pub topics: Vec<String>,
    pub auth: Option<Auth>,
    pub cache_duration: i64,
    /// The stream counts as dead if nothing arrives for this long.
    pub keepalive_timeout: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub auth_retry: Duration,
}

impl ConnectionConfig {
    pub fn production(inst: &Instance, auth: Option<Auth>) -> Self {
        Self {
            instance_id: inst.id.clone(),
            base_url: inst.url.clone(),
            topics: inst.topics.iter().map(|t| t.name.clone()).collect(),
            auth,
            cache_duration: inst.cache_duration_secs,
            keepalive_timeout: Duration::from_secs(120),
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            auth_retry: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnStatus {
    Connecting,
    Connected,
    Disconnected { error: String },
    AuthFailed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnEvent {
    Status(ConnStatus),
    Messages {
        messages: Vec<Message>,
        catch_up: bool,
    },
    /// Messages may have been missed (reported at most once per poll).
    Gap,
    State(InstanceState),
}

#[derive(Clone, Default)]
pub struct ConnControl {
    pub reconnect: Arc<tokio::sync::Notify>,
    pub cancel: CancellationToken,
}

/// Runs the connection for one instance until `ctl.cancel` fires or the
/// receiver of `tx` is dropped. Events are tagged with `cfg.instance_id`.
pub fn spawn_connection(
    cfg: ConnectionConfig,
    state: InstanceState,
    http: reqwest::Client,
    tx: mpsc::UnboundedSender<(String, ConnEvent)>,
    ctl: ConnControl,
) -> tokio::task::JoinHandle<()> {
    let backoff = cfg.backoff_initial;
    let conn = Connection {
        cfg,
        state,
        http,
        tx,
        ctl,
        backoff,
    };
    tokio::spawn(conn.run())
}

/// The task must end (cancelled or nobody listening).
struct Stop;

/// `None`: interrupted by a reconnect request.
type Interruptible<T> = Result<Option<T>, Stop>;

enum StreamEnd {
    Reconnect,
    Lost(String),
}

struct Connection {
    cfg: ConnectionConfig,
    state: InstanceState,
    http: reqwest::Client,
    tx: mpsc::UnboundedSender<(String, ConnEvent)>,
    ctl: ConnControl,
    backoff: Duration,
}

impl Connection {
    async fn run(mut self) {
        while self.cycle().await.is_ok() {}
    }

    /// One connection attempt including the wait before the next one.
    async fn cycle(&mut self) -> Result<(), Stop> {
        self.emit(ConnEvent::Status(ConnStatus::Connecting))?;
        let mut since = None;
        if let Some(poll_since) = catch_up_since(&self.state) {
            let polled = client::poll(
                &self.http,
                &self.cfg.base_url,
                &self.cfg.topics,
                poll_since,
                self.cfg.auth.as_ref(),
            );
            match self.interruptible(polled).await? {
                None => return Ok(()),
                Some(Err(e)) => return self.handle_error(e).await,
                Some(Ok(polled)) => self.apply_poll(polled)?,
            }
            since = catch_up_since(&self.state);
        }

        let subscribed = tokio::time::timeout(
            self.cfg.keepalive_timeout,
            client::subscribe(
                &self.http,
                &self.cfg.base_url,
                &self.cfg.topics,
                since,
                self.cfg.auth.as_ref(),
            ),
        );
        let stream = match self.interruptible(subscribed).await? {
            None => return Ok(()),
            Some(Err(_elapsed)) => {
                let e = NtfyError::Network {
                    kind: NetworkKind::Timeout,
                    message: "timed out waiting for the server to respond".into(),
                };
                return self.handle_error(e).await;
            }
            Some(Ok(Err(e))) => return self.handle_error(e).await,
            Some(Ok(Ok(stream))) => stream,
        };

        match self.read_stream(stream, since.is_none()).await? {
            StreamEnd::Reconnect => Ok(()),
            StreamEnd::Lost(error) => {
                tracing::warn!(instance = %self.cfg.instance_id, %error, "connection lost");
                self.emit(ConnEvent::Status(ConnStatus::Disconnected { error }))?;
                let delay = self.next_backoff();
                self.wait(delay).await
            }
        }
    }

    fn apply_poll(&mut self, polled: PollResult) -> Result<(), Stop> {
        let result =
            self.state
                .apply_catch_up(polled.messages, polled.server_now, self.cfg.cache_duration);
        if result.gap || polled.truncated {
            tracing::warn!(
                instance = %self.cfg.instance_id,
                truncated = polled.truncated,
                "messages may have been missed"
            );
            self.emit(ConnEvent::Gap)?;
        }
        if !result.accepted.is_empty() {
            self.emit(ConnEvent::Messages {
                messages: result.accepted,
                catch_up: true,
            })?;
        }
        self.emit(ConnEvent::State(self.state.clone()))
    }

    /// Reports `Connected` (and resets the backoff) only once the first event
    /// parsed, so a server that answers 200 and closes, or sends something
    /// other than ntfy JSON, keeps backing off. Without `since` the stream
    /// replays nothing, so its `open` time is a safe catch-up starting point.
    async fn read_stream(
        &mut self,
        mut stream: EventStream,
        without_since: bool,
    ) -> Result<StreamEnd, Stop> {
        let mut connected = false;
        let mut last_state: Option<Instant> = None;
        loop {
            let next = tokio::time::timeout(self.cfg.keepalive_timeout, stream.next());
            let event = match self.interruptible(next).await? {
                None => return Ok(StreamEnd::Reconnect),
                Some(Err(_elapsed)) => {
                    let secs = self.cfg.keepalive_timeout.as_secs_f64();
                    return Ok(StreamEnd::Lost(format!("no data from server for {secs}s")));
                }
                Some(Ok(None)) => return Ok(StreamEnd::Lost("server closed the stream".into())),
                Some(Ok(Some(Err(e)))) => return Ok(StreamEnd::Lost(e.to_string())),
                Some(Ok(Some(Ok(event)))) => event,
            };
            if !connected {
                connected = true;
                self.emit(ConnEvent::Status(ConnStatus::Connected))?;
                tracing::info!(instance = %self.cfg.instance_id, "connected");
                self.backoff = self.cfg.backoff_initial;
            }
            match event {
                Event::Open { time } if without_since => {
                    self.state.observe_time(time);
                    self.emit(ConnEvent::State(self.state.clone()))?;
                    last_state = Some(Instant::now());
                }
                Event::Keepalive { time } => {
                    self.state.observe_time(time);
                    if last_state.is_none_or(|t| t.elapsed() >= KEEPALIVE_STATE_INTERVAL) {
                        self.emit(ConnEvent::State(self.state.clone()))?;
                        last_state = Some(Instant::now());
                    }
                }
                Event::Message(m) => {
                    if self.state.accept(&m) {
                        self.emit(ConnEvent::Messages {
                            messages: vec![m],
                            catch_up: false,
                        })?;
                        self.emit(ConnEvent::State(self.state.clone()))?;
                        last_state = Some(Instant::now());
                    }
                }
                Event::Open { .. } | Event::Other { .. } => {}
            }
        }
    }

    async fn handle_error(&mut self, e: NtfyError) -> Result<(), Stop> {
        if let NtfyError::Auth(code) = e {
            tracing::warn!(instance = %self.cfg.instance_id, code, "authentication failed");
            self.emit(ConnEvent::Status(ConnStatus::AuthFailed))?;
            return self.wait(self.cfg.auth_retry).await;
        }
        let error = e.to_string();
        tracing::warn!(instance = %self.cfg.instance_id, %error, "connection failed");
        self.emit(ConnEvent::Status(ConnStatus::Disconnected { error }))?;
        let delay = match e {
            NtfyError::RateLimited { retry_after } => retry_after
                .map_or(self.cfg.backoff_max, |d| {
                    d.clamp(self.cfg.backoff_initial, MAX_RETRY_AFTER)
                }),
            _ => self.next_backoff(),
        };
        self.wait(delay).await
    }

    fn next_backoff(&mut self) -> Duration {
        let (delay, next) = backoff_step(self.backoff, self.cfg.backoff_max);
        self.backoff = next;
        delay
    }

    /// Sleeps; a reconnect request ends the wait early.
    async fn wait(&self, delay: Duration) -> Result<(), Stop> {
        self.interruptible(tokio::time::sleep(delay))
            .await
            .map(drop)
    }

    async fn interruptible<F: Future>(&self, fut: F) -> Interruptible<F::Output> {
        tokio::select! {
            biased;
            _ = self.ctl.cancel.cancelled() => Err(Stop),
            _ = self.ctl.reconnect.notified() => Ok(None),
            out = fut => Ok(Some(out)),
        }
    }

    fn emit(&self, event: ConnEvent) -> Result<(), Stop> {
        self.tx
            .send((self.cfg.instance_id.clone(), event))
            .map_err(|_| Stop)
    }
}

/// Returns the delay to wait now (`current` ±20 % jitter) and the next
/// backoff (doubled, capped at `max`).
fn backoff_step(current: Duration, max: Duration) -> (Duration, Duration) {
    let factor: f64 = rand::random_range(0.8..=1.2);
    let delay =
        Duration::try_from_secs_f64(current.as_secs_f64() * factor).unwrap_or(Duration::MAX);
    (delay, current.saturating_mul(2).min(max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{AuthKind, Topic};

    #[test]
    fn backoff_doubles_up_to_max_with_jitter() {
        let ms = Duration::from_millis;
        let max = ms(400);
        let mut current = ms(50);
        let mut nexts = Vec::new();
        for _ in 0..5 {
            let (delay, next) = backoff_step(current, max);
            assert!(delay >= current.mul_f64(0.8) && delay <= current.mul_f64(1.2));
            nexts.push(next);
            current = next;
        }
        assert_eq!(nexts, [ms(100), ms(200), ms(400), ms(400), ms(400)]);
        assert_eq!(backoff_step(Duration::MAX, max).1, max);
    }

    #[test]
    fn backoff_jitter_bounds() {
        let base = Duration::from_millis(1000);
        let delays: Vec<_> = (0..1000).map(|_| backoff_step(base, base).0).collect();
        let (lo, hi) = (base.mul_f64(0.8), base.mul_f64(1.2));
        assert!(delays.iter().all(|d| (lo..=hi).contains(d)));
        // Actually spread out, not a constant factor.
        assert!(delays.iter().any(|d| *d < base.mul_f64(0.9)));
        assert!(delays.iter().any(|d| *d > base.mul_f64(1.1)));
    }

    #[test]
    fn production_config_values() {
        let inst = Instance {
            id: "i".into(),
            url: "https://ntfy.example".into(),
            auth: AuthKind::Token,
            cache_duration_secs: 3600,
            topics: vec![
                Topic {
                    name: "a".into(),
                    ..Topic::default()
                },
                Topic {
                    name: "b".into(),
                    muted: true,
                    ..Topic::default()
                },
            ],
        };
        let cfg = ConnectionConfig::production(&inst, Some(Auth::Token("t".into())));
        assert_eq!(cfg.instance_id, "i");
        assert_eq!(cfg.base_url, "https://ntfy.example");
        assert_eq!(cfg.topics, ["a", "b"]);
        assert_eq!(cfg.auth, Some(Auth::Token("t".into())));
        assert_eq!(cfg.cache_duration, 3600);
        assert_eq!(cfg.keepalive_timeout, Duration::from_secs(120));
        assert_eq!(cfg.backoff_initial, Duration::from_secs(1));
        assert_eq!(cfg.backoff_max, Duration::from_secs(30));
        assert_eq!(cfg.auth_retry, Duration::from_secs(60));
    }
}
