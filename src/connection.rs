use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::ntfy::client::{self, EventStream, NtfyError, PollResult};
use crate::ntfy::model::{Event, Message};
use crate::secrets::Auth;
use crate::settings::Instance;
use crate::state::InstanceState;
use crate::sync::catch_up_since;

/// Minimum interval between `State` events caused by keepalives alone.
const KEEPALIVE_STATE_INTERVAL: Duration = Duration::from_secs(60);

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
                let e = NtfyError::Network("timed out waiting for the server to respond".into());
                return self.handle_error(e).await;
            }
            Some(Ok(Err(e))) => return self.handle_error(e).await,
            Some(Ok(Ok(stream))) => stream,
        };
        self.emit(ConnEvent::Status(ConnStatus::Connected))?;
        tracing::info!(instance = %self.cfg.instance_id, "connected");
        self.backoff = self.cfg.backoff_initial;

        match self.read_stream(stream).await? {
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

    async fn read_stream(&mut self, mut stream: EventStream) -> Result<StreamEnd, Stop> {
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
            match event {
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
        let delay = self.next_backoff();
        self.wait(delay).await
    }

    /// Current backoff with ±20 % jitter; doubles the next one up to the maximum.
    fn next_backoff(&mut self) -> Duration {
        let current = self.backoff;
        self.backoff = (current * 2).min(self.cfg.backoff_max);
        current.mul_f64(rand::random_range(0.8..=1.2))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{AuthKind, Topic};

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
