//! Toast XML for Windows notifications (platform-neutral so it can be tested
//! everywhere).

use crate::notification::{Level, MediaRef, Notification};

/// The toast XML for `n`. `urgent_scenario` allows `scenario="urgent"` for
/// urgent notifications (unsupported on older Windows 10 builds).
pub fn build(n: &Notification, urgent_scenario: bool) -> String {
    let launch = n
        .default_action
        .as_ref()
        .map(|p| escape(&p.encode()))
        .unwrap_or_default();
    let mut xml = format!(r#"<toast launch="{launch}""#);
    if n.level == Level::Urgent && urgent_scenario {
        xml.push_str(r#" scenario="urgent""#);
    }
    xml.push_str(r#"><visual><binding template="ToastGeneric">"#);
    xml.push_str(&format!(
        "<text>{}</text><text>{}</text>",
        escape(&n.title),
        escape(&n.body)
    ));
    if let Some(src) = local_src(n.icon.as_ref()) {
        xml.push_str(&format!(
            r#"<image placement="appLogoOverride" src="{src}"/>"#
        ));
    }
    if let Some(src) = local_src(n.image.as_ref()) {
        xml.push_str(&format!(r#"<image src="{src}"/>"#));
    }
    xml.push_str("</binding></visual>");
    if n.level == Level::Passive {
        xml.push_str(r#"<audio silent="true"/>"#);
    }
    if !n.buttons.is_empty() {
        xml.push_str("<actions>");
        for b in &n.buttons {
            xml.push_str(&format!(
                r#"<action content="{}" arguments="{}" activationType="foreground"/>"#,
                escape(&b.label),
                escape(&b.payload.encode())
            ));
        }
        xml.push_str("</actions>");
    }
    xml.push_str("</toast>");
    xml
}

/// Escaped `file:///` URI of a local media file; remote media is not embedded.
fn local_src(media: Option<&MediaRef>) -> Option<String> {
    match media? {
        MediaRef::Local(path) => Some(escape(&format!("file:///{}", path.display()))),
        MediaRef::Remote(_) => None,
    }
}

/// Escapes the five XML special characters.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::notification::{ActivationPayload, Button};

    fn notification(level: Level) -> Notification {
        Notification {
            id: "id".into(),
            thread: "thread".into(),
            title: "Title".into(),
            body: "Body".into(),
            level,
            default_action: None,
            buttons: vec![],
            image: None,
            icon: None,
        }
    }

    fn open(url: &str) -> ActivationPayload {
        ActivationPayload::Open { url: url.into() }
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(escape(r#"<a&"b'>"#), "&lt;a&amp;&quot;b&apos;&gt;");
    }

    #[test]
    fn passive_is_silent() {
        let xml = build(&notification(Level::Passive), true);
        assert!(xml.contains(r#"<audio silent="true"/>"#), "{xml}");
        let xml = build(&notification(Level::Active), true);
        assert!(xml.contains("<toast"), "{xml}");
        assert!(!xml.contains("<audio"), "{xml}");
    }

    #[test]
    fn urgent_scenario_only_when_allowed() {
        let xml = build(&notification(Level::Urgent), true);
        assert!(xml.contains(r#"scenario="urgent""#), "{xml}");
        let xml = build(&notification(Level::Urgent), false);
        assert!(xml.contains("<toast") && !xml.contains("scenario"), "{xml}");
        let xml = build(&notification(Level::Active), true);
        assert!(xml.contains("<toast") && !xml.contains("scenario"), "{xml}");
    }

    #[test]
    fn launch_carries_default_action() {
        let mut n = notification(Level::Active);
        let xml = build(&n, true);
        assert!(xml.starts_with(r#"<toast launch="""#), "{xml}");
        let p = open("https://x/?a=1&b=2");
        n.default_action = Some(p.clone());
        let xml = build(&n, true);
        let launch = format!(r#"<toast launch="{}""#, escape(&p.encode()));
        assert!(xml.starts_with(&launch), "{xml}");
    }

    #[test]
    fn buttons_carry_encoded_payload() {
        let mut n = notification(Level::Active);
        let ack = ActivationPayload::Http {
            label: "Ack".into(),
            method: "POST".into(),
            url: "https://x/ack".into(),
            headers: Default::default(),
            body: Some(r#"{"a":"<b>"}"#.into()),
        };
        n.buttons = vec![
            Button {
                label: "Open <it>".into(),
                payload: open("https://x/open"),
            },
            Button {
                label: "Ack".into(),
                payload: ack.clone(),
            },
        ];
        let xml = build(&n, true);
        assert_eq!(xml.matches("<action ").count(), 2, "{xml}");
        assert!(
            xml.contains("<actions>") && xml.contains("</actions>"),
            "{xml}"
        );
        let first = format!(
            r#"<action content="Open &lt;it&gt;" arguments="{}" activationType="foreground"/>"#,
            escape(&open("https://x/open").encode())
        );
        let second = format!(
            r#"<action content="Ack" arguments="{}" activationType="foreground"/>"#,
            escape(&ack.encode())
        );
        assert!(xml.contains(&first), "{xml}");
        assert!(xml.contains(&second), "{xml}");
        assert!(xml.find(&first) < xml.find(&second), "{xml}");

        let xml = build(&notification(Level::Active), true);
        assert!(xml.contains("<toast") && !xml.contains("<actions"), "{xml}");
    }

    #[test]
    fn only_local_media_embedded() {
        let mut n = notification(Level::Active);
        n.image = Some(MediaRef::Remote("https://x/a.png".into()));
        n.icon = Some(MediaRef::Remote("https://x/i.png".into()));
        let xml = build(&n, true);
        assert!(xml.contains("<toast") && !xml.contains("<image"), "{xml}");

        n.image = Some(MediaRef::Local(PathBuf::from(r"C:\cache\a&b.png")));
        n.icon = Some(MediaRef::Local(PathBuf::from(r"C:\cache\icon.png")));
        let xml = build(&n, true);
        assert!(
            xml.contains(r#"<image placement="appLogoOverride" src="file:///C:\cache\icon.png"/>"#),
            "{xml}"
        );
        assert!(
            xml.contains(r#"<image src="file:///C:\cache\a&amp;b.png"/>"#),
            "{xml}"
        );
    }

    #[test]
    fn output_is_well_formed() {
        let mut n = notification(Level::Passive);
        n.title = "<&>".into();
        n.body = r#"a "quoted" 'body' </toast>"#.into();
        n.default_action = Some(open("https://x/?q=<&>"));
        n.buttons = vec![Button {
            label: "\"&'".into(),
            payload: open("https://x/'\""),
        }];
        n.image = Some(MediaRef::Local(PathBuf::from("/tmp/<a>.png")));
        let xml = build(&n, true);
        assert!(xml.contains("<text>&lt;&amp;&gt;</text>"), "{xml}");

        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut elements = 0;
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(quick_xml::events::Event::Start(_) | quick_xml::events::Event::Empty(_)) => {
                    elements += 1
                }
                Ok(_) => {}
                Err(e) => panic!("malformed XML at {}: {e}\n{xml}", reader.buffer_position()),
            }
        }
        // toast, visual, binding, 2 text, image, audio, actions, action
        assert_eq!(elements, 9, "{xml}");
    }
}
