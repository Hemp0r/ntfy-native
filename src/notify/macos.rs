//! Native notifications via `UNUserNotificationCenter`.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use block2::{DynBlock, RcBlock};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSInteger, NSSet, NSString, NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNErrorCode, UNErrorDomain, UNMutableNotificationContent,
    UNNotification, UNNotificationAction, UNNotificationActionOptions, UNNotificationAttachment,
    UNNotificationCategory, UNNotificationCategoryOptions, UNNotificationDefaultActionIdentifier,
    UNNotificationInterruptionLevel, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use super::category_id;
use crate::notification::{
    ActivationPayload, ActivationSink, Level, MediaRef, Notification, Notifier,
};

/// The system keeps at most this many button categories registered.
const MAX_CATEGORIES: usize = 32;

const DEFAULT_KEY: &str = "default";

/// Set once "not authorized" has been logged; cleared by a successful add.
static NOT_ALLOWED_LOGGED: AtomicBool = AtomicBool::new(false);

pub struct MacNotifier {
    // The center references its delegate weakly.
    _delegate: Retained<Delegate>,
    categories: Mutex<CategoryLru>,
}

impl MacNotifier {
    /// Installs the notification delegate and requests authorization. Must be
    /// called before the event loop starts so that a launch caused by a click
    /// on a notification is delivered.
    pub fn new(on_activate: ActivationSink) -> Result<Self> {
        // Callers may run on threads without an autorelease pool.
        autoreleasepool(|_| Self::new_in_pool(on_activate))
    }

    fn new_in_pool(on_activate: ActivationSink) -> Result<Self> {
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            bail!(
                "the app must run from an app bundle (see scripts/macos-dev-bundle.sh) \
                 to use native notifications"
            );
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::new(on_activate);
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let on_auth = RcBlock::new(|granted: Bool, err: *mut NSError| {
            // SAFETY: the framework passes a valid NSError or null.
            let err = unsafe { err.as_ref() };
            if granted.as_bool() {
                tracing::info!("notifications authorized");
                return;
            }
            match err.filter(|e| classify(e) != AddError::NotAllowed) {
                Some(e) => tracing::warn!(
                    error = %e.localizedDescription(),
                    "notification authorization failed"
                ),
                None => tracing::warn!("notifications not authorized by the user"),
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &on_auth,
        );

        Ok(Self {
            _delegate: delegate,
            categories: Mutex::new(CategoryLru::default()),
        })
    }

    /// Registers the category if needed. Returns whether the registered set
    /// changed.
    fn register_category(
        &self,
        center: &UNUserNotificationCenter,
        id: &str,
        labels: &[&str],
    ) -> bool {
        let mut lru = self.categories.lock().unwrap_or_else(|e| e.into_inner());
        if !lru.touch(id, labels) {
            return false;
        }
        let categories: Vec<_> = lru.entries.iter().map(|(id, l)| category(id, l)).collect();
        center.setNotificationCategories(&NSSet::from_retained_slice(&categories));
        true
    }

    fn show_in_pool(&self, n: &Notification) {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&n.title));
        content.setBody(&NSString::from_str(&n.body));
        content.setThreadIdentifier(&NSString::from_str(&n.thread));
        content.setInterruptionLevel(interruption_level(n.level));
        if n.level != Level::Passive {
            content.setSound(Some(&UNNotificationSound::defaultSound()));
        }

        let mut info: Vec<(String, String)> = Vec::new();
        if let Some(p) = &n.default_action {
            info.push((DEFAULT_KEY.into(), p.encode()));
        }
        for (i, b) in n.buttons.iter().enumerate() {
            info.push((format!("b{i}"), b.payload.encode()));
        }
        set_user_info(&content, &info);

        let mut categories_changed = false;
        if !n.buttons.is_empty() {
            let labels: Vec<&str> = n.buttons.iter().map(|b| b.label.as_str()).collect();
            let id = category_id(&labels);
            categories_changed = self.register_category(&center, &id, &labels);
            content.setCategoryIdentifier(&NSString::from_str(&id));
        }

        let identifier = NSString::from_str(&n.id);
        let image = match &n.image {
            Some(MediaRef::Local(path)) => attachment(path)
                .inspect_err(|e| {
                    tracing::warn!(id = %n.id, error = %format!("{e:#}"), "notification image skipped")
                })
                .ok(),
            _ => None,
        };
        let plain = request(&identifier, &content);
        let (request, fallback) = match image {
            Some(image) => {
                content.setAttachments(&NSArray::from_retained_slice(&[image]));
                (request(&identifier, &content), Some(plain))
            }
            None => (plain, None),
        };

        if categories_changed {
            // Category registration is asynchronous; reading the categories
            // back completes after it, so the request then finds its buttons.
            let id = n.id.clone();
            let then_add = RcBlock::new(move |_: NonNull<NSSet<UNNotificationCategory>>| {
                let center = UNUserNotificationCenter::currentNotificationCenter();
                add(&center, &id, &request, fallback.clone());
            });
            center.getNotificationCategoriesWithCompletionHandler(&then_add);
        } else {
            add(&center, &n.id, &request, fallback);
        }
    }
}

impl Notifier for MacNotifier {
    fn show(&self, n: &Notification) -> Result<()> {
        // Called from the engine's thread, which has no autorelease pool.
        autoreleasepool(|_| self.show_in_pool(n));
        Ok(())
    }
}

fn request(
    id: &NSString,
    content: &UNMutableNotificationContent,
) -> Retained<UNNotificationRequest> {
    // The request copies the content.
    UNNotificationRequest::requestWithIdentifier_content_trigger(id, content, None)
}

#[derive(Debug, PartialEq, Eq)]
enum AddError {
    NotAllowed,
    Attachment,
    Other,
}

fn classify(err: &NSError) -> AddError {
    // SAFETY: an immutable string constant of the framework.
    let un_domain = unsafe { UNErrorDomain }.is_some_and(|d| *d == *err.domain());
    classify_code(un_domain, err.code())
}

fn classify_code(un_domain: bool, code: NSInteger) -> AddError {
    match UNErrorCode(code) {
        _ if !un_domain => AddError::Other,
        UNErrorCode::NotificationsNotAllowed => AddError::NotAllowed,
        UNErrorCode(100..=199) => AddError::Attachment,
        _ => AddError::Other,
    }
}

/// Adds the request without waiting. If it fails because of the attachment
/// and `fallback` (the same notification without attachment) is given, that
/// is added instead.
fn add(
    center: &UNUserNotificationCenter,
    id: &str,
    request: &UNNotificationRequest,
    fallback: Option<Retained<UNNotificationRequest>>,
) {
    let id = id.to_owned();
    let done = RcBlock::new(move |err: *mut NSError| {
        // SAFETY: the framework passes a valid NSError or null.
        let Some(err) = (unsafe { err.as_ref() }) else {
            NOT_ALLOWED_LOGGED.store(false, Ordering::Relaxed);
            return;
        };
        let error = err.localizedDescription();
        match (classify(err), &fallback) {
            (AddError::NotAllowed, _) => {
                if !NOT_ALLOWED_LOGGED.swap(true, Ordering::Relaxed) {
                    tracing::warn!("notifications not authorized");
                }
            }
            (AddError::Attachment, Some(plain)) => {
                tracing::warn!(%id, %error, "notification image rejected, retrying without");
                let center = UNUserNotificationCenter::currentNotificationCenter();
                add(&center, &id, plain, None);
            }
            _ => tracing::warn!(%id, %error, "adding notification failed"),
        }
    });
    center.addNotificationRequest_withCompletionHandler(request, Some(&done));
}

fn interruption_level(level: Level) -> UNNotificationInterruptionLevel {
    match level {
        Level::Passive => UNNotificationInterruptionLevel::Passive,
        Level::Active => UNNotificationInterruptionLevel::Active,
        Level::Urgent => UNNotificationInterruptionLevel::TimeSensitive,
    }
}

fn set_user_info(content: &UNMutableNotificationContent, info: &[(String, String)]) {
    let keys: Vec<Retained<NSString>> = info.iter().map(|(k, _)| NSString::from_str(k)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<Retained<NSString>> = info.iter().map(|(_, v)| NSString::from_str(v)).collect();
    let dict = NSDictionary::<NSString, NSString>::from_retained_objects(&keys, &values);
    // SAFETY: a dictionary of strings is a valid untyped dictionary and a
    // valid property list, as userInfo requires.
    let dict: Retained<NSDictionary> = unsafe { Retained::cast_unchecked(dict) };
    // SAFETY: see above.
    unsafe { content.setUserInfo(&dict) };
}

/// Note: the system may move the file into its own attachment store.
fn attachment(path: &Path) -> Result<Retained<UNNotificationAttachment>> {
    let url = NSURL::from_file_path(path).context("invalid image path")?;
    // SAFETY: no options dictionary is passed.
    unsafe {
        UNNotificationAttachment::attachmentWithIdentifier_URL_options_error(
            &NSString::from_str("image"),
            &url,
            None,
        )
    }
    .map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))
}

fn category(id: &str, labels: &[String]) -> Retained<UNNotificationCategory> {
    let actions: Vec<_> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(&format!("a{i}")),
                &NSString::from_str(label),
                UNNotificationActionOptions::empty(),
            )
        })
        .collect();
    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(id),
        &NSArray::from_retained_slice(&actions),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    )
}

/// Registered button categories, least recently used first.
#[derive(Default)]
struct CategoryLru {
    entries: VecDeque<(String, Vec<String>)>,
}

impl CategoryLru {
    /// Marks `id` as most recently used. Returns whether the set changed.
    fn touch(&mut self, id: &str, labels: &[&str]) -> bool {
        if let Some(pos) = self.entries.iter().position(|(e, _)| e == id) {
            let entry = self.entries.remove(pos).expect("position is valid");
            self.entries.push_back(entry);
            return false;
        }
        if self.entries.len() == MAX_CATEGORIES {
            self.entries.pop_front();
        }
        let labels = labels.iter().map(|l| (*l).to_owned()).collect();
        self.entries.push_back((id.to_owned(), labels));
        true
    }
}

/// The `userInfo` key holding the payload for a response's action.
fn payload_key(action_id: &str, default_action_id: &str) -> Option<String> {
    if action_id == default_action_id {
        return Some(DEFAULT_KEY.into());
    }
    let index = action_id.strip_prefix('a')?;
    (!index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())).then(|| format!("b{index}"))
}

fn response_payload(response: &UNNotificationResponse) -> Option<ActivationPayload> {
    // SAFETY: an immutable string constant of the framework.
    let default_id = unsafe { UNNotificationDefaultActionIdentifier };
    let key = payload_key(
        &response.actionIdentifier().to_string(),
        &default_id.to_string(),
    )?;
    let info = response.notification().request().content().userInfo();
    let value = info.objectForKey(&NSString::from_str(&key))?;
    let value = value.downcast::<NSString>().ok()?;
    ActivationPayload::decode(&value.to_string())
}

struct DelegateIvars {
    on_activate: ActivationSink,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and Delegate does not
    // implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "NtfyNativeNotificationDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &DynBlock<dyn Fn()>,
        ) {
            let on_activate = &self.ivars().on_activate;
            let handled = catch_unwind(AssertUnwindSafe(|| match response_payload(response) {
                Some(p) => on_activate(p),
                None => tracing::debug!(
                    action = %response.actionIdentifier(),
                    "notification response without payload"
                ),
            }));
            if handled.is_err() {
                tracing::error!("notification activation handler panicked");
            }
            completion.call(());
        }
    }
);

impl Delegate {
    fn new(on_activate: ActivationSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { on_activate });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    const DEFAULT_ID: &str = "com.apple.UNNotificationDefaultActionIdentifier";

    #[test]
    fn payload_key_maps_actions() {
        assert_eq!(
            payload_key(DEFAULT_ID, DEFAULT_ID).as_deref(),
            Some("default")
        );
        assert_eq!(payload_key("a0", DEFAULT_ID).as_deref(), Some("b0"));
        assert_eq!(payload_key("a2", DEFAULT_ID).as_deref(), Some("b2"));
        assert_eq!(payload_key("a", DEFAULT_ID), None);
        assert_eq!(payload_key("ax", DEFAULT_ID), None);
        assert_eq!(
            payload_key(
                "com.apple.UNNotificationDismissActionIdentifier",
                DEFAULT_ID
            ),
            None
        );
    }

    #[test]
    fn classify_code_retries_only_attachment_errors() {
        assert_eq!(classify_code(true, 1), AddError::NotAllowed);
        assert_eq!(classify_code(true, 100), AddError::Attachment);
        assert_eq!(classify_code(true, 105), AddError::Attachment);
        assert_eq!(classify_code(true, 1401), AddError::Other);
        assert_eq!(classify_code(false, 1), AddError::Other);
        assert_eq!(classify_code(false, 100), AddError::Other);
    }

    #[test]
    fn category_lru_keeps_recent_and_evicts_oldest() {
        let mut lru = CategoryLru::default();
        for i in 0..MAX_CATEGORIES {
            assert!(lru.touch(&format!("c{i}"), &["x"]));
        }
        assert!(!lru.touch("c0", &["x"]), "known category changes nothing");
        assert!(lru.touch("new", &["y"]));
        let ids: Vec<&str> = lru.entries.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids.len(), MAX_CATEGORIES);
        assert!(!ids.contains(&"c1"), "least recently used evicted");
        assert!(ids.contains(&"c0") && ids.contains(&"new"));
        assert_eq!(lru.entries.back().unwrap().1, vec!["y".to_owned()]);
    }

    #[test]
    fn new_fails_outside_bundle() {
        let sink: ActivationSink = Arc::new(|_| {});
        let err = MacNotifier::new(sink)
            .err()
            .expect("test binary has no bundle");
        assert!(err.to_string().contains("app bundle"), "{err}");
    }

    #[test]
    fn platform_notifier_falls_back_outside_bundle() {
        let n = super::super::platform_notifier(Arc::new(|_| {}), Path::new("/nonexistent"));
        let shown = n.show(&Notification {
            id: "i".into(),
            thread: "t".into(),
            title: "T".into(),
            body: "B".into(),
            level: Level::Active,
            default_action: None,
            buttons: vec![],
            image: None,
            icon: None,
        });
        assert!(shown.is_ok());
    }

    #[test]
    fn notifier_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MacNotifier>();
    }
}
