//! Executes notification actions: opening URLs and sending HTTP requests.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::notification::ActivationPayload;

const HTTP_ACTION_TIMEOUT: Duration = Duration::from_secs(15);

pub trait UrlOpener: Send + Sync {
    fn open(&self, url: &str) -> anyhow::Result<()>;
}

pub struct SystemOpener;

impl UrlOpener for SystemOpener {
    fn open(&self, url: &str) -> anyhow::Result<()> {
        let parsed = openable_url(url)?;
        open::that_detached(parsed.as_str()).context("failed to open URL")
    }
}

#[derive(Default)]
pub struct RecordingOpener {
    pub opened: Mutex<Vec<String>>,
}

impl UrlOpener for RecordingOpener {
    fn open(&self, url: &str) -> anyhow::Result<()> {
        self.opened.lock().expect("lock").push(url.to_owned());
        Ok(())
    }
}

/// Only http, https and mailto may be handed to the OS.
pub fn is_openable(url: &str) -> bool {
    openable_url(url).is_ok()
}

/// Parsed URL (the parser lowercases the scheme), or an error that names
/// only the scheme.
fn openable_url(url: &str) -> anyhow::Result<url::Url> {
    match url::Url::parse(url) {
        Ok(u) if matches!(u.scheme(), "http" | "https" | "mailto") => Ok(u),
        _ => bail!("refusing to open URL with scheme {}", scheme_of(url)),
    }
}

fn scheme_of(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| u.scheme().to_owned())
        .unwrap_or_else(|_| "invalid".to_owned())
}

pub async fn perform(
    http: &reqwest::Client,
    payload: &ActivationPayload,
    opener: &dyn UrlOpener,
) -> anyhow::Result<()> {
    match payload {
        ActivationPayload::Open { url } => {
            let parsed = openable_url(url)?;
            opener.open(parsed.as_str())
        }
        ActivationPayload::Http {
            method,
            url,
            headers,
            body,
            ..
        } => {
            let method = Method::from_bytes(method.as_bytes()).context("invalid HTTP method")?;
            let parsed = url::Url::parse(url).context("invalid action URL")?;
            if !matches!(parsed.scheme(), "http" | "https") {
                bail!(
                    "HTTP action must use http or https, got {}",
                    parsed.scheme()
                );
            }
            let mut map = HeaderMap::new();
            for (k, v) in headers {
                let name = HeaderName::from_bytes(k.as_bytes()).context("invalid header name")?;
                let value = HeaderValue::from_str(v).context("invalid header value")?;
                map.append(name, value);
            }
            tracing::info!(method = %method, host = parsed.host_str().unwrap_or(""), "http action");
            let mut req = http
                .request(method, parsed)
                .headers(map)
                .timeout(HTTP_ACTION_TIMEOUT);
            if let Some(b) = body {
                req = req.body(b.clone());
            }
            let status = req
                .send()
                .await
                .map_err(|e| e.without_url())
                .context("HTTP action request failed")?
                .status();
            if !status.is_success() {
                bail!("HTTP action returned status {status}");
            }
            Ok(())
        }
    }
}
