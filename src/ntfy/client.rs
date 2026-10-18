use std::collections::VecDeque;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::Stream;
use reqwest::header::{DATE, HeaderMap, RETRY_AFTER};

use crate::ntfy::model::{Event, Message, parse_event};
use crate::secrets::Auth;

const POLL_TIMEOUT: Duration = Duration::from_secs(30);
/// Longer lines (no newline from a broken server) are dropped instead of buffered.
const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(thiserror::Error, Debug)]
pub enum NtfyError {
    #[error("authentication failed (HTTP {0})")]
    Auth(u16),
    #[error("unexpected HTTP status {0}")]
    Status(u16),
    /// HTTP 429, with the server's `Retry-After` if it sent a usable one.
    #[error("rate limited by server (HTTP 429)")]
    RateLimited { retry_after: Option<Duration> },
    #[error("network error: {0}")]
    Network(String),
}

#[derive(Debug)]
pub struct PollResult {
    pub messages: Vec<Message>,
    /// Server time from the `Date` header; `None` if missing or unparseable.
    pub server_now: Option<i64>,
    /// `X-Messages-Truncated: 1`: the server dropped the oldest messages.
    pub truncated: bool,
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event, NtfyError>> + Send>>;

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent(concat!("ntfy-native/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("TLS backend initializes")
}

pub fn topics_url(base: &str, topics: &[String]) -> String {
    format!("{}/{}/json", base.trim_end_matches('/'), topics.join(","))
}

pub async fn poll(
    http: &reqwest::Client,
    base: &str,
    topics: &[String],
    since: i64,
    auth: Option<&Auth>,
) -> Result<PollResult, NtfyError> {
    let url = format!("{}?poll=1&since={since}", topics_url(base, topics));
    let resp = request(http, &url, auth).timeout(POLL_TIMEOUT).send().await;
    let resp = check_status(resp.map_err(network)?)?;
    let server_now = server_time(resp.headers());
    if server_now.is_none() {
        tracing::warn!(
            base,
            "poll response without usable Date header, gap check skipped"
        );
    }
    let truncated = resp
        .headers()
        .get("x-messages-truncated")
        .is_some_and(|v| v.as_bytes().trim_ascii() == b"1");
    let body = resp.bytes().await.map_err(network)?;
    let messages = String::from_utf8_lossy(&body)
        .lines()
        .filter_map(parse_line)
        .filter_map(|ev| match ev {
            Event::Message(m) => Some(m),
            _ => None,
        })
        .collect();
    Ok(PollResult {
        messages,
        server_now,
        truncated,
    })
}

pub async fn subscribe(
    http: &reqwest::Client,
    base: &str,
    topics: &[String],
    since: Option<i64>,
    auth: Option<&Auth>,
) -> Result<EventStream, NtfyError> {
    let mut url = topics_url(base, topics);
    if let Some(since) = since {
        url.push_str(&format!("?since={since}"));
    }
    let resp = request(http, &url, auth).send().await;
    let resp = check_status(resp.map_err(network)?)?;
    let stream = futures_util::stream::unfold(
        (Some(resp), LineBuffer::default(), VecDeque::new()),
        |(mut resp, mut lines, mut pending)| async move {
            loop {
                if let Some(ev) = pending.pop_front() {
                    return Some((Ok(ev), (resp, lines, pending)));
                }
                match resp.as_mut()?.chunk().await {
                    Ok(Some(chunk)) => {
                        pending.extend(lines.push(&chunk).iter().filter_map(|l| parse_line(l)))
                    }
                    Ok(None) => {
                        resp = None;
                        pending.extend(lines.finish().as_deref().and_then(parse_line));
                    }
                    Err(e) => return Some((Err(network(e)), (None, lines, pending))),
                }
            }
        },
    );
    Ok(Box::pin(stream))
}

fn request(http: &reqwest::Client, url: &str, auth: Option<&Auth>) -> reqwest::RequestBuilder {
    let req = http.get(url);
    match auth {
        None => req,
        Some(Auth::Token(token)) => req.bearer_auth(token),
        Some(Auth::Basic { username, password }) => req.basic_auth(username, Some(password)),
    }
}

fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, NtfyError> {
    let status = resp.status();
    match status.as_u16() {
        401 | 403 => Err(NtfyError::Auth(status.as_u16())),
        429 => Err(NtfyError::RateLimited {
            retry_after: retry_after(resp.headers()),
        }),
        _ if !status.is_success() => Err(NtfyError::Status(status.as_u16())),
        _ => Ok(resp),
    }
}

/// `Retry-After` as delay-seconds or HTTP date.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(SystemTime::now()).unwrap_or_default())
}

/// Includes the error's source chain, which carries the actual cause.
fn network(e: reqwest::Error) -> NtfyError {
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    NtfyError::Network(text)
}

fn server_time(headers: &HeaderMap) -> Option<i64> {
    let date = headers.get(DATE)?.to_str().ok()?;
    let time = httpdate::parse_http_date(date).ok()?;
    Some(time.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

fn parse_line(line: &str) -> Option<Event> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    parse_event(line)
        .inspect_err(
            |e| tracing::warn!(error = %e, len = line.len(), "skipping unparseable ntfy line"),
        )
        .ok()
}

/// Splits a byte stream into lines.
#[derive(Default)]
struct LineBuffer {
    buf: Vec<u8>,
    overlong: bool,
}

impl LineBuffer {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for part in chunk.split_inclusive(|b| *b == b'\n') {
            let (data, complete) = match part.strip_suffix(b"\n") {
                Some(data) => (data, true),
                None => (part, false),
            };
            if !self.overlong {
                self.buf.extend_from_slice(data);
                if self.buf.len() > MAX_LINE_BYTES {
                    tracing::warn!("dropping overlong ntfy line");
                    self.buf.clear();
                    self.overlong = true;
                }
            }
            if complete {
                if !self.overlong {
                    lines.push(String::from_utf8_lossy(&self.buf).into_owned());
                }
                self.buf.clear();
                self.overlong = false;
            }
        }
        lines
    }

    /// Returns the unterminated rest at end of stream.
    fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        (!self.overlong && !rest.is_empty()).then(|| String::from_utf8_lossy(&rest).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topics_url_joins_topics_and_trims_slash() {
        let topics = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(
            topics_url("https://n.example/", &topics),
            "https://n.example/a,b/json"
        );
        assert_eq!(
            topics_url("https://n.example", &topics[..1]),
            "https://n.example/a/json"
        );
    }

    #[test]
    fn line_buffer_splits_across_chunks() {
        let mut lb = LineBuffer::default();
        assert!(lb.push(b"ab").is_empty());
        assert_eq!(lb.push(b"c\r\nd\n\ne"), ["abc\r", "d", ""]);
        assert_eq!(lb.finish().as_deref(), Some("e"));
        assert_eq!(lb.finish(), None);
    }

    #[test]
    fn line_buffer_drops_overlong_line() {
        let mut lb = LineBuffer::default();
        let big = vec![b'x'; MAX_LINE_BYTES + 1];
        assert!(lb.push(&big).is_empty());
        assert_eq!(lb.push(b"tail\nok\n"), ["ok"]);
    }

    #[test]
    fn parse_line_skips_blank_and_garbage() {
        assert_eq!(parse_line("  \r"), None);
        assert_eq!(parse_line("garbage"), None);
        assert_eq!(
            parse_line(r#"{"id":"k","time":5,"event":"keepalive"}"#),
            Some(Event::Keepalive { time: 5 })
        );
    }

    #[test]
    fn retry_after_seconds_or_date() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(RETRY_AFTER, " 120 ".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(120)));
        h.insert(RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(retry_after(&h), None);
        h.insert(
            RETRY_AFTER,
            "Sun, 18 Oct 2020 10:00:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&h), Some(Duration::ZERO));
        let later = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(90));
        h.insert(RETRY_AFTER, later.parse().unwrap());
        let d = retry_after(&h).unwrap();
        assert!(
            d > Duration::from_secs(80) && d <= Duration::from_secs(90),
            "{d:?}"
        );
    }

    #[test]
    fn server_time_from_date_header() {
        let mut h = HeaderMap::new();
        assert_eq!(server_time(&h), None);
        h.insert(DATE, "nonsense".parse().unwrap());
        assert_eq!(server_time(&h), None);
        h.insert(DATE, "Sun, 18 Oct 2026 10:00:00 GMT".parse().unwrap());
        assert_eq!(server_time(&h), Some(1_792_317_600));
    }
}
