use anyhow::{Result, anyhow, bail};

use crate::settings::Settings;

pub fn valid_topic_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The parsed URL's serialization (lowercase host, no default port) without
/// trailing `/`, so equivalent spellings map to the same instance.
pub fn normalize_url(raw: &str) -> Result<String> {
    // Never echo the raw value: it may carry credentials.
    let parsed = url::Url::parse(raw.trim()).map_err(|e| anyhow!("invalid instance url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("instance url must start with http:// or https://");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!(
            "instance url for host {} must not contain credentials; use token or username/password",
            parsed.host_str().unwrap_or_default()
        );
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!(
            "instance url for host {} must not contain a query or fragment",
            parsed.host_str().unwrap_or_default()
        );
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// Whether a stored instance URL (possibly saved in an older, unnormalized
/// form) denotes the normalized import URL.
pub fn same_url(stored: &str, normalized: &str) -> bool {
    match normalize_url(stored) {
        Ok(n) => n == normalized,
        Err(_) => stored.trim_end_matches('/') == normalized,
    }
}

pub fn unique_id(settings: &Settings, base: &str) -> String {
    let taken = |id: &str| settings.instances.iter().any(|i| i.id == id);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .expect("unbounded range")
}
