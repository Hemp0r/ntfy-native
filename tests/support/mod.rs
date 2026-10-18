//! Small fake HTTP server for integration tests (ntfy, media, actions).
#![allow(dead_code)]

pub mod conn;

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use futures_util::StreamExt;
use tokio::sync::{Notify, mpsc};

/// One request as seen by the server.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: Method,
    pub path: String,
    pub query: Option<String>,
    pub headers: HeaderMap,
    pub body: Bytes,
    /// When the request arrived.
    pub at: Instant,
}

impl Recorded {
    /// Raw (not percent-decoded) value of a query parameter.
    pub fn query_param(&self, key: &str) -> Option<&str> {
        self.query.as_deref()?.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (k == key).then_some(v)
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// ntfy: `?poll=1` marks a poll request, anything else is a stream.
    pub fn is_poll(&self) -> bool {
        self.query_param("poll") == Some("1")
    }
}

enum ReplyBody {
    Full(Bytes),
    /// Sends the bytes, then stays open without ending.
    FullThenStall(Bytes),
    Channel(mpsc::UnboundedReceiver<Bytes>),
    Stall,
}

/// Response to send for one request.
pub struct Reply {
    status: StatusCode,
    headers: Vec<(String, String)>,
    body: ReplyBody,
}

impl Reply {
    /// Empty body with the given status.
    pub fn status(code: u16) -> Self {
        Self::bytes(Bytes::new()).with_status(code)
    }

    /// 200 with a fixed body.
    pub fn bytes(body: impl Into<Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: ReplyBody::Full(body.into()),
        }
    }

    /// 200 with each line terminated by `\n`.
    pub fn lines<S: AsRef<str>>(lines: &[S]) -> Self {
        let body: String = lines.iter().map(|l| format!("{}\n", l.as_ref())).collect();
        Self::bytes(body)
    }

    /// 200 whose body is fed through the returned sender; the body ends when
    /// the sender is dropped, and stays open (silent) while it is held.
    pub fn stream() -> (Self, StreamTx) {
        let (tx, rx) = mpsc::unbounded_channel();
        let reply = Self {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: ReplyBody::Channel(rx),
        };
        (reply, StreamTx(tx))
    }

    /// 200 whose body never yields and never ends.
    pub fn stall() -> Self {
        Self {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: ReplyBody::Stall,
        }
    }

    /// Keeps the connection open after a fixed body instead of ending it.
    pub fn keep_open(mut self) -> Self {
        if let ReplyBody::Full(b) = self.body {
            self.body = ReplyBody::FullThenStall(b);
        }
        self
    }

    pub fn with_status(mut self, code: u16) -> Self {
        self.status = StatusCode::from_u16(code).expect("valid status code");
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    fn into_response(self) -> Response {
        let body = match self.body {
            ReplyBody::Full(b) => Body::from(b),
            ReplyBody::FullThenStall(b) => Body::from_stream(
                futures_util::stream::iter([Ok::<_, Infallible>(b)])
                    .chain(futures_util::stream::pending()),
            ),
            ReplyBody::Channel(rx) => {
                Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.map(|b| (Ok::<_, Infallible>(b), rx))
                }))
            }
            ReplyBody::Stall => {
                Body::from_stream(futures_util::stream::pending::<Result<Bytes, Infallible>>())
            }
        };
        let mut resp = Response::new(body);
        *resp.status_mut() = self.status;
        for (k, v) in self.headers {
            resp.headers_mut().append(
                HeaderName::from_bytes(k.as_bytes()).expect("header name"),
                HeaderValue::from_str(&v).expect("header value"),
            );
        }
        resp
    }
}

/// Feeds a [`Reply::stream`] body.
#[derive(Clone)]
pub struct StreamTx(mpsc::UnboundedSender<Bytes>);

impl StreamTx {
    /// Sends `line` followed by `\n`.
    pub fn line(&self, line: &str) {
        self.raw(&format!("{line}\n"));
    }

    /// Sends bytes as they are (one body chunk).
    pub fn raw(&self, data: &str) {
        let _ = self.0.send(Bytes::from(data.to_owned()));
    }
}

/// Replies in push order; when the queue is empty, the fallback answers
/// (default: [`Reply::stall`]).
#[derive(Clone)]
pub struct Script {
    queue: Arc<Mutex<VecDeque<Reply>>>,
    fallback: fn(&Recorded) -> Reply,
}

impl Default for Script {
    fn default() -> Self {
        Self::with_fallback(|_| Reply::stall())
    }
}

impl Script {
    pub fn with_fallback(fallback: fn(&Recorded) -> Reply) -> Self {
        Self {
            queue: Arc::default(),
            fallback,
        }
    }

    pub fn push(&self, reply: Reply) {
        self.queue.lock().unwrap().push_back(reply);
    }

    pub fn handler(&self) -> impl FnMut(&Recorded) -> Reply + Send + 'static {
        let queue = self.queue.clone();
        let fallback = self.fallback;
        move |req| {
            let next = queue.lock().unwrap().pop_front();
            next.unwrap_or_else(|| fallback(req))
        }
    }
}

type Handler = Box<dyn FnMut(&Recorded) -> Reply + Send>;

#[derive(Clone)]
struct Shared {
    handler: Arc<Mutex<Handler>>,
    log: Arc<Mutex<Vec<Recorded>>>,
    arrived: Arc<Notify>,
}

pub struct FakeServer {
    pub addr: SocketAddr,
    shared: Shared,
}

impl FakeServer {
    /// Serves every method and path through `handler` on 127.0.0.1.
    pub async fn start(handler: impl FnMut(&Recorded) -> Reply + Send + 'static) -> Self {
        let shared = Shared {
            handler: Arc::new(Mutex::new(Box::new(handler))),
            log: Arc::default(),
            arrived: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().fallback(handle).with_state(shared.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { addr, shared }
    }

    /// Serves replies from `script` in order.
    pub async fn scripted(script: &Script) -> Self {
        Self::start(script.handler()).await
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.shared.log.lock().unwrap().clone()
    }

    /// Waits until at least `n` requests arrived (panics after 2 s).
    pub async fn wait_requests(&self, n: usize) -> Vec<Recorded> {
        let wait = async {
            loop {
                let arrived = self.shared.arrived.notified();
                let log = self.requests();
                if log.len() >= n {
                    return log;
                }
                arrived.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), wait)
            .await
            .unwrap_or_else(|_| panic!("expected {n} requests, got {:?}", self.requests()))
    }
}

async fn handle(State(shared): State<Shared>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX)
        .await
        .unwrap_or_default();
    let recorded = Recorded {
        method: parts.method,
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().map(str::to_owned),
        headers: parts.headers,
        body,
        at: Instant::now(),
    };
    let reply = (shared.handler.lock().unwrap())(&recorded);
    shared.log.lock().unwrap().push(recorded);
    shared.arrived.notify_waiters();
    reply.into_response()
}

/// Upper bound for one refused connect to [`unused_url`]: immediate on Unix,
/// but Windows retries a refused loopback connect for about 1-2 s.
pub const REFUSED_CONNECT: Duration = if cfg!(windows) {
    Duration::from_secs(3)
} else {
    Duration::ZERO
};

/// Base URL of a local port with nothing listening.
pub fn unused_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}
