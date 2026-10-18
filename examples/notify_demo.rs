//! Manual check of the native notifier: shows one notification per level
//! (with buttons and an image) and prints activations. Exits after 30 s.
//!
//! macOS: `scripts/macos-dev-bundle.sh --example notify_demo`, then run
//! `target/ntfy-native.app/Contents/MacOS/ntfy-native`.

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use ntfy_native::notification::{
        ActivationPayload, ActivationSink, Button, Level, MediaRef, Notification, Notifier,
    };
    use ntfy_native::notify::MacNotifier;

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let sink: ActivationSink = Arc::new(|p| println!("activated: {p:?}"));
    let notifier = MacNotifier::new(sink)?;
    // Give the authorization request time to complete.
    std::thread::sleep(Duration::from_secs(3));

    // The system may move attachments, so hand it a copy of the fixture.
    let image = std::env::temp_dir().join(format!("ntfy-native-demo-{}.png", std::process::id()));
    std::fs::write(&image, include_bytes!("../tests/fixtures/sample.png"))?;

    let run = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let open = |url: &str| ActivationPayload::Open { url: url.into() };
    let base = Notification {
        id: String::new(),
        thread: "demo".into(),
        title: String::new(),
        body: String::new(),
        level: Level::Active,
        default_action: None,
        buttons: vec![],
        image: None,
        icon: None,
    };
    let notifications = [
        Notification {
            id: format!("demo-{run}-passive"),
            title: "Passive".into(),
            body: "No sound; hidden while a Focus is active.".into(),
            level: Level::Passive,
            default_action: Some(open("https://ntfy.sh/passive")),
            ..base.clone()
        },
        Notification {
            id: format!("demo-{run}-active"),
            title: "Active with buttons".into(),
            body: "Click the banner or a button.".into(),
            level: Level::Active,
            default_action: Some(open("https://ntfy.sh/active")),
            buttons: vec![
                Button {
                    label: "Open".into(),
                    payload: open("https://ntfy.sh/button"),
                },
                Button {
                    label: "Ack".into(),
                    payload: ActivationPayload::Http {
                        label: "Ack".into(),
                        method: "POST".into(),
                        url: "https://example.invalid/ack".into(),
                        headers: BTreeMap::new(),
                        body: Some("ack".into()),
                    },
                },
            ],
            image: Some(MediaRef::Local(image)),
            ..base.clone()
        },
        Notification {
            id: format!("demo-{run}-urgent"),
            title: "Urgent".into(),
            body: "Time-sensitive: breaks through a Focus if allowed.".into(),
            level: Level::Urgent,
            default_action: Some(open("https://ntfy.sh/urgent")),
            ..base
        },
    ];
    for n in &notifications {
        notifier.show(n)?;
        println!("shown: {}", n.id);
    }

    // Activations arrive on a framework queue; the main thread only waits.
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("notify_demo currently supports macOS only");
}
