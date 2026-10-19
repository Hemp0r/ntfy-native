use anyhow::{Result, anyhow, bail};

use crate::settings::{Settings, Topic};

pub fn valid_topic_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Errors name the position, never the entered value.
pub fn validate_topics(topics: &[Topic]) -> Result<()> {
    for (i, topic) in topics.iter().enumerate() {
        let pos = i + 1;
        if !valid_topic_name(&topic.name) {
            bail!(
                "topic {pos}: invalid topic name (allowed: letters, digits, '-' and '_', 1-64 characters)"
            );
        }
        if topics[..i].iter().any(|o| o.name == topic.name) {
            bail!("topic {pos}: duplicate topic");
        }
    }
    Ok(())
}

/// Why an instance URL is rejected; carries no part of the entered value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlError {
    Invalid,
    Scheme,
    Credentials,
    QueryOrFragment,
}

/// The parsed URL's serialization (lowercase host, no default port) without
/// trailing `/`, so equivalent spellings map to the same instance.
pub fn check_url(raw: &str) -> std::result::Result<String, UrlError> {
    let parsed = url::Url::parse(raw.trim()).map_err(|_| UrlError::Invalid)?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(UrlError::Scheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(UrlError::Credentials);
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(UrlError::QueryOrFragment);
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// [`check_url`] with English errors naming at most the host.
pub fn normalize_url(raw: &str) -> Result<String> {
    // Never echo the raw value: it may carry credentials.
    let host = || {
        url::Url::parse(raw.trim())
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default()
    };
    check_url(raw).map_err(|e| match e {
        UrlError::Invalid => match url::Url::parse(raw.trim()) {
            Err(e) => anyhow!("invalid instance url: {e}"),
            Ok(_) => anyhow!("invalid instance url"),
        },
        UrlError::Scheme => anyhow!("instance url must start with http:// or https://"),
        UrlError::Credentials => anyhow!(
            "instance url for host {} must not contain credentials; use token or username/password",
            host()
        ),
        UrlError::QueryOrFragment => anyhow!(
            "instance url for host {} must not contain a query or fragment",
            host()
        ),
    })
}

/// Whether two instance URLs share scheme, host and port, so a credential
/// saved for one may be sent to the other. Paths may differ.
pub fn same_origin(a: &str, b: &str) -> bool {
    let origin = |raw: &str| {
        check_url(raw)
            .ok()
            .and_then(|n| url::Url::parse(&n).ok())
            .map(|u| u.origin())
    };
    match (origin(a), origin(b)) {
        (Some(a), Some(b)) => a.is_tuple() && a == b,
        _ => false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_compares_scheme_host_and_port() {
        assert!(same_origin("https://h", "https://H:443/sub/"));
        assert!(same_origin("https://h/a", "https://h/b"));
        assert!(same_origin("http://h:8080", "http://h:8080/x"));
        assert!(!same_origin("https://h", "https://g"));
        assert!(!same_origin("https://h", "http://h"));
        assert!(!same_origin("https://h", "https://h:8443"));
        assert!(!same_origin("https://h", "not a url"));
        assert!(!same_origin("", ""));
    }

    #[test]
    fn check_url_classifies_errors() {
        assert_eq!(check_url("https://H:443/x/").as_deref(), Ok("https://h/x"));
        assert_eq!(check_url("not a url"), Err(UrlError::Invalid));
        assert_eq!(check_url("ftp://h"), Err(UrlError::Scheme));
        assert_eq!(check_url("https://u:p@h"), Err(UrlError::Credentials));
        assert_eq!(check_url("https://h/?a=1"), Err(UrlError::QueryOrFragment));
        assert_eq!(check_url("https://h/#f"), Err(UrlError::QueryOrFragment));
        let e = normalize_url("https://u:s3cret@h").unwrap_err().to_string();
        assert!(e.contains("host h") && !e.contains("s3cret"), "{e}");
    }
}
