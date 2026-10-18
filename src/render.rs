use crate::emoji;
use crate::i18n::Strings;
use crate::notification::{ActivationPayload, Button, Level, MediaRef, Notification};
use crate::ntfy::model::{Action, Attachment, Message};

const MAX_BUTTONS: usize = 3;

pub fn render(instance_id: &str, msg: &Message, urgent_allowed: bool, s: &Strings) -> Notification {
    let mut emojis = Vec::new();
    let mut unknown = Vec::new();
    for tag in &msg.tags {
        match emoji::lookup(tag) {
            Some(e) => emojis.push(e),
            None => unknown.push(tag.as_str()),
        }
    }
    let name = msg.title.as_deref().unwrap_or(&msg.topic);
    let title = if emojis.is_empty() {
        name.to_string()
    } else {
        format!("{} {name}", emojis.join(" "))
    };

    let file = msg.attachment.as_ref().filter(|a| !a.is_image());
    let image = msg.attachment.as_ref().filter(|a| a.is_image());

    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = msg.message.as_deref().filter(|t| !t.is_empty()) {
        parts.push(text.to_string());
    }
    if !unknown.is_empty() {
        parts.push(format!("{}: {}", s.tags_label, unknown.join(", ")));
    }
    if let Some(a) = file {
        parts.push(format!("📎 {}", attachment_name(a)));
    }

    let level = match msg.priority {
        0..=2 => Level::Passive,
        5.. if urgent_allowed => Level::Urgent,
        _ => Level::Active,
    };

    let default_action = msg
        .click
        .as_ref()
        .map(|url| ActivationPayload::Open { url: url.clone() })
        .or_else(|| file.map(|a| ActivationPayload::Open { url: a.url.clone() }));

    let buttons = msg
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::View { label, url } => Some(Button {
                label: label.clone(),
                payload: ActivationPayload::Open { url: url.clone() },
            }),
            Action::Http {
                label,
                url,
                method,
                headers,
                body,
            } => Some(Button {
                label: label.clone(),
                payload: ActivationPayload::Http {
                    label: label.clone(),
                    method: method.clone(),
                    url: url.clone(),
                    headers: headers.clone(),
                    body: body.clone(),
                },
            }),
            Action::Unsupported => None,
        })
        .take(MAX_BUTTONS)
        .collect();

    Notification {
        id: format!("{instance_id}:{}", msg.id),
        thread: format!("{instance_id}/{}", msg.topic),
        title,
        body: parts.join("\n"),
        level,
        default_action,
        buttons,
        image: image.map(|a| MediaRef::Remote(a.url.clone())),
        icon: msg.icon.as_ref().map(|u| MediaRef::Remote(u.clone())),
    }
}

fn attachment_name(a: &Attachment) -> String {
    if !a.name.is_empty() {
        return a.name.clone();
    }
    url::Url::parse(&a.url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.next_back().map(str::to_string))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "attachment".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;
    use crate::notification::{ActivationPayload as AP, Level, MediaRef};
    use crate::ntfy::model::{Action, Attachment};

    fn msg() -> Message {
        Message {
            id: "abc".into(),
            time: 0,
            topic: "backups".into(),
            title: None,
            message: None,
            tags: vec![],
            priority: 3,
            click: None,
            icon: None,
            actions: vec![],
            attachment: None,
        }
    }

    fn att(name: &str, mime: Option<&str>) -> Attachment {
        Attachment {
            name: name.into(),
            mime: mime.map(Into::into),
            size: None,
            url: "https://h/f".into(),
        }
    }

    #[test]
    fn title_with_emoji_and_fallback_to_topic() {
        let mut m = msg();
        m.tags = vec!["warning".into(), "skull".into(), "nas".into()];
        m.message = Some("disk full".into());
        let n = render("inst", &m, false, &EN);
        assert_eq!(n.title, "⚠️ 💀 backups");
        assert_eq!(n.body, "disk full\nTags: nas");
        m.title = Some("Alert".into());
        m.tags = vec!["nas".into()];
        m.message = None;
        let n = render("inst", &m, false, &EN);
        assert_eq!(n.title, "Alert");
        assert_eq!(n.body, "Tags: nas");
    }

    #[test]
    fn priority_levels() {
        let lv = |p, u| {
            let mut m = msg();
            m.priority = p;
            render("i", &m, u, &EN).level
        };
        assert_eq!(lv(1, false), Level::Passive);
        assert_eq!(lv(2, false), Level::Passive);
        assert_eq!(lv(3, false), Level::Active);
        assert_eq!(lv(4, false), Level::Active);
        assert_eq!(lv(5, false), Level::Active);
        assert_eq!(lv(5, true), Level::Urgent);
        assert_eq!(lv(4, true), Level::Active);
    }

    #[test]
    fn click_and_attachments() {
        let mut m = msg();
        m.click = Some("https://click".into());
        m.attachment = Some(att("a.pdf", Some("application/pdf")));
        let n = render("i", &m, false, &EN);
        assert_eq!(
            n.default_action,
            Some(AP::Open {
                url: "https://click".into()
            })
        );
        assert_eq!(n.body, "📎 a.pdf");

        m.click = None;
        let n = render("i", &m, false, &EN);
        assert_eq!(
            n.default_action,
            Some(AP::Open {
                url: "https://h/f".into()
            })
        );
        assert!(n.body.contains("📎 a.pdf"));

        m.attachment = Some(att("p.png", Some("image/png")));
        let n = render("i", &m, false, &EN);
        assert_eq!(n.image, Some(MediaRef::Remote("https://h/f".into())));
        assert_eq!(n.body, "");
        assert_eq!(n.default_action, None);

        m.attachment = Some(att("", None));
        assert_eq!(render("i", &m, false, &EN).body, "📎 f");
        m.attachment = Some(Attachment {
            url: "https://h/".into(),
            ..att("", None)
        });
        assert_eq!(render("i", &m, false, &EN).body, "📎 attachment");
    }

    #[test]
    fn icon_is_remote() {
        let mut m = msg();
        m.icon = Some("https://i/x.png".into());
        assert_eq!(
            render("i", &m, false, &EN).icon,
            Some(MediaRef::Remote("https://i/x.png".into()))
        );
    }

    #[test]
    fn buttons_mapped_and_capped() {
        let view = |l: &str| Action::View {
            label: l.into(),
            url: format!("https://{l}"),
        };
        let mut m = msg();
        m.actions = vec![
            view("a"),
            Action::Http {
                label: "b".into(),
                url: "https://b".into(),
                method: "POST".into(),
                headers: Default::default(),
                body: None,
            },
            Action::Unsupported,
            view("c"),
            view("d"),
        ];
        let n = render("i", &m, false, &EN);
        let labels: Vec<_> = n.buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["a", "b", "c"]);
        assert!(matches!(n.buttons[1].payload, AP::Http { .. }));
        assert!(matches!(n.buttons[0].payload, AP::Open { .. }));
    }

    #[test]
    fn ids_and_thread() {
        let n = render("inst", &msg(), false, &EN);
        assert_eq!(n.id, "inst:abc");
        assert_eq!(n.thread, "inst/backups");
    }
}
