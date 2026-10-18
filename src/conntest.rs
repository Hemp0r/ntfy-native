//! Connection test for the setup wizard: server reachable, really ntfy,
//! credentials accepted, read access per topic. Results never contain the
//! entered URL or secrets.

use futures_util::future::join_all;

use crate::instance_edit::{AuthDraft, InstanceDraft};
use crate::ipc::{TestOutcome, TopicAccess};
use crate::ntfy::client::{NtfyError, health, topic_readable};
use crate::secrets::Auth;
use crate::validate::{normalize_url, validate_topics};

/// Probed when the draft has no topics, only to check the credentials.
const AUTH_CHECK_TOPIC: &str = "ntfy-native-auth-check";

/// `stored` is the instance's saved credential, used where the draft keeps it
/// (secret `None`).
pub async fn test_connection(
    http: &reqwest::Client,
    draft: &InstanceDraft,
    stored: Option<Auth>,
) -> TestOutcome {
    let invalid = |message: String| TestOutcome::InvalidInput { message };
    let base = match normalize_url(&draft.url) {
        Ok(url) => url,
        Err(e) => return invalid(e.to_string()),
    };
    if let Err(e) = validate_topics(&draft.topics) {
        return invalid(e.to_string());
    }
    let auth = match auth_from(&draft.auth, stored) {
        Ok(auth) => auth,
        Err(message) => return invalid(message.into()),
    };

    match health(http, &base, auth.as_ref()).await {
        Ok(true) => {}
        Ok(false) => return TestOutcome::NotNtfy,
        Err(e) => return failure(e),
    }

    if draft.topics.is_empty() {
        return match topic_readable(http, &base, AUTH_CHECK_TOPIC, auth.as_ref()).await {
            Ok(_) => TestOutcome::Ok { topics: Vec::new() },
            Err(e) => failure(e),
        };
    }
    let checks = draft.topics.iter().map(|t| async {
        topic_readable(http, &base, &t.name, auth.as_ref())
            .await
            .map(|readable| TopicAccess {
                name: t.name.clone(),
                readable,
            })
    });
    let mut topics = Vec::new();
    for result in join_all(checks).await {
        match result {
            Ok(access) => topics.push(access),
            Err(e) => return failure(e),
        }
    }
    TestOutcome::Ok { topics }
}

fn auth_from(draft: &AuthDraft, stored: Option<Auth>) -> Result<Option<Auth>, &'static str> {
    match draft {
        AuthDraft::None => Ok(None),
        AuthDraft::Token { token: Some(t) } if !t.trim().is_empty() => {
            Ok(Some(Auth::Token(t.clone())))
        }
        AuthDraft::Token { token: Some(_) } => Err("token required"),
        AuthDraft::Token { token: None } => match stored {
            Some(auth @ Auth::Token(_)) => Ok(Some(auth)),
            _ => Err("token required"),
        },
        AuthDraft::Basic { username, .. } if username.trim().is_empty() => Err("username required"),
        AuthDraft::Basic {
            username,
            password: Some(p),
        } if !p.is_empty() => Ok(Some(Auth::Basic {
            username: username.clone(),
            password: p.clone(),
        })),
        AuthDraft::Basic {
            password: Some(_), ..
        } => Err("password required"),
        AuthDraft::Basic {
            username,
            password: None,
        } => match stored {
            Some(Auth::Basic {
                username: u,
                password,
            }) if &u == username => Ok(Some(Auth::Basic {
                username: u,
                password,
            })),
            _ => Err("password required"),
        },
    }
}

fn failure(e: NtfyError) -> TestOutcome {
    let message = match e {
        NtfyError::Auth(_) => return TestOutcome::AuthFailed,
        NtfyError::Network { kind, .. } => kind.label().to_owned(),
        NtfyError::RateLimited { .. } => "rate limited".to_owned(),
        NtfyError::Status(code) => format!("HTTP {code}"),
    };
    TestOutcome::Unreachable { message }
}
