//! Manual check of the native notifier: shows one notification per level
//! (with buttons and an image) and prints activations. Exits after 30 s.
//!
//! macOS: `scripts/macos-dev-bundle.sh --example notify_demo`, then run
//! `target/ntfy-native.app/Contents/MacOS/ntfy-native`.
//!
//! Windows: `cargo run --example notify_demo`. The demo registers itself as
//! the toast activator; a click after it exited relaunches it with
//! `-ToastActivated`, and it then only prints the activation.

#[cfg(any(target_os = "macos", windows))]
fn main() -> anyhow::Result<()> {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use ntfy_native::notification::{
        ActivationPayload, ActivationSink, Button, Level, MediaRef, Notification, Notifier,
    };

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let sink: ActivationSink = Arc::new(|p| println!("activated: {p:?}"));
    #[cfg(target_os = "macos")]
    let notifier = {
        let n = ntfy_native::notify::MacNotifier::new(sink)?;
        // Give the authorization request time to complete.
        std::thread::sleep(Duration::from_secs(3));
        n
    };
    #[cfg(windows)]
    let notifier = ntfy_native::notify::WinNotifier::new(sink)?;

    let com_activated = std::env::args()
        .skip(1)
        .any(|a| a.eq_ignore_ascii_case("-ToastActivated") || a.eq_ignore_ascii_case("-Embedding"));
    if com_activated {
        println!("started by a notification click, waiting for the activation");
        std::thread::sleep(Duration::from_secs(30));
        return Ok(());
    }

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
            body: "No sound; hidden by Focus / Do Not Disturb.".into(),
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
            image: Some(MediaRef::Local(image.clone())),
            icon: Some(MediaRef::Local(image)),
            ..base.clone()
        },
        Notification {
            id: format!("demo-{run}-urgent"),
            title: "Urgent".into(),
            body: "Breaks through Focus / Do Not Disturb if allowed.".into(),
            level: Level::Urgent,
            default_action: Some(open("https://ntfy.sh/urgent")),
            ..base
        },
    ];
    for n in &notifications {
        notifier.show(n)?;
        println!("shown: {}", n.id);
    }

    // Activations arrive on a framework queue or COM thread; the main
    // thread only waits.
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    eprintln!("notify_demo supports macOS and Windows only");
}
