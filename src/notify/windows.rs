//! Native notifications via WinRT toasts, activated through a COM server.

use std::cell::Cell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Context, Result};
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, PROPERTYKEY, RPC_E_CHANGED_MODE,
};
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_AppUserModel_ID, PKEY_AppUserModel_ToastActivatorCLSID,
};
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromCLSID, PROPVARIANT, PropVariantClear, PropVariantToGUID,
    PropVariantToStringAlloc,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
    CoCreateInstance, CoInitializeEx, CoRegisterClassObject, CoTaskMemFree, CoUninitialize,
    IClassFactory, IClassFactory_Impl, IPersistFile, REGCLS_MULTIPLEUSE, STGM_READ,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegSetValueExW,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    SetCurrentProcessExplicitAppUserModelID, ShellLink,
};
use windows::core::{
    BOOL, Error, GUID, HSTRING, IUnknown, Interface, PCWSTR, PWSTR, Ref, implement,
};

use super::fnv1a;
use super::shortcut::{ShortcutProps, needs_update};
use super::toast_xml::attempts;
use crate::notification::{ActivationPayload, ActivationSink, Notification, Notifier};
use crate::paths::{APP_ID, APP_NAME};

/// COM class of the toast activator. Fixed: installers and the system
/// registration refer to it.
pub const TOAST_ACTIVATOR_CLSID: &str = "AE88E831-148B-400C-8273-6ABC4FDEA102";
const CLSID: GUID = GUID::from_u128(0xAE88E831_148B_400C_8273_6ABC4FDEA102);

/// Shown in toast headers and the Windows notification settings.
const ICON_PNG: &[u8] = include_bytes!("../../assets/icon-256.png");
const ICON_FILE_NAME: &str = "icon.png";
const SHORTCUT_FILE_NAME: &str = "ntfy-native.lnk";
/// Bounds the wait for the shortcut check at startup.
const SHORTCUT_TIMEOUT: Duration = Duration::from_secs(3);

fn aumid_key() -> String {
    format!(r"Software\Classes\AppUserModelId\{APP_ID}")
}

thread_local! {
    static WINRT_READY: Cell<bool> = const { Cell::new(false) };
}

pub struct WinNotifier {
    _private: (),
}

impl WinNotifier {
    /// Registers the app for toasts (AUMID and COM activator in HKCU, Start
    /// menu shortcut) and starts the COM server that receives clicks, also
    /// when a click relaunched the app. Must be called before the event loop
    /// starts.
    pub fn new(on_activate: ActivationSink) -> Result<Self> {
        let exe = std::env::current_exe().context("locating the executable")?;
        let clsid = format!("{{{TOAST_ACTIVATOR_CLSID}}}");
        let aumid_key = aumid_key();
        set_string(&aumid_key, Some("DisplayName"), APP_NAME)?;
        set_string(&aumid_key, Some("CustomActivator"), &clsid)?;
        set_string(
            &format!(r"Software\Classes\CLSID\{clsid}\LocalServer32"),
            None,
            &format!("\"{}\" -ToastActivated", exe.display()),
        )?;
        // SAFETY: plain FFI call with a valid string.
        unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) }
            .context("setting the AppUserModelID")?;
        // Without it toasts show, but are not kept in the Notification Center.
        ensure_start_menu_shortcut(exe);

        // Without the activator clicks are ignored, but toasts still show.
        if let Err(e) = start_activator(on_activate) {
            tracing::warn!(
                error = %format!("{e:#}"),
                "toast activation unavailable, clicks on notifications are ignored"
            );
        }
        Ok(Self { _private: () })
    }
}

impl Notifier for WinNotifier {
    fn show(&self, n: &Notification) -> Result<()> {
        ensure_winrt();
        let tag = HSTRING::from(short_hash(&n.id));
        let group = HSTRING::from(short_hash(&n.thread));
        let mut last = None;
        for (i, xml) in attempts(n).enumerate() {
            match show_toast(&xml, &tag, &group) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::debug!(id = %n.id, attempt = i + 1, error = %format!("{e:#}"), "toast rejected");
                    last = Some(e);
                }
            }
        }
        // The engine logs the final failure at warn level.
        Err(last
            .unwrap_or_else(|| anyhow::anyhow!("no toast to show"))
            .context(format!("showing toast {}", n.id)))
    }
}

/// Writes the app icon to `dir` (if missing or outdated) and registers it
/// as `IconUri` next to `DisplayName`. An outdated icon that cannot be
/// replaced (e.g. locked by the shell) is still registered.
pub fn register_icon(dir: &Path) -> Result<()> {
    let path = std::path::absolute(dir.join(ICON_FILE_NAME)).context("resolving the icon path")?;
    if let Err(e) = super::write_if_changed(&path, ICON_PNG) {
        if !path.is_file() {
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
        tracing::warn!(error = %e, path = %path.display(), "updating the toast icon failed, keeping the old one");
    }
    set_string(&aumid_key(), Some("IconUri"), &path.display().to_string())
}

/// Creates or repairs the Start-menu shortcut on a helper thread with its own
/// COM apartment, waiting at most [`SHORTCUT_TIMEOUT`] so the first toast
/// usually finds it in place. The thread logs its own outcome, so a failure
/// after the timeout is not lost.
fn ensure_start_menu_shortcut(exe: PathBuf) {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("start-menu-shortcut".into())
        .spawn(move || {
            // SAFETY: plain FFI call on a fresh thread.
            let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let result = if init.is_ok() {
                // Formatted (and the error dropped) before CoUninitialize, so
                // no COM error info outlives the apartment.
                let r = update_shortcut(&exe).map_err(|e| format!("{e:#}"));
                // SAFETY: balances the successful CoInitializeEx; all COM
                // objects were released inside `update_shortcut`.
                unsafe { CoUninitialize() };
                r
            } else {
                Err(format!("initializing COM: {}", Error::from(init)))
            };
            if let Err(e) = result {
                tracing::warn!(
                    error = %e,
                    "start menu shortcut unavailable, notifications may not stay in the Notification Center"
                );
            }
            let _ = tx.send(());
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "starting the shortcut thread failed");
        return;
    }
    match rx.recv_timeout(SHORTCUT_TIMEOUT) {
        Ok(()) => {}
        Err(RecvTimeoutError::Timeout) => {
            tracing::warn!("start menu shortcut check is taking long, continuing");
        }
        Err(RecvTimeoutError::Disconnected) => {
            tracing::warn!("start menu shortcut thread failed");
        }
    }
}

fn update_shortcut(exe: &Path) -> Result<()> {
    let lnk = programs_dir()?.join(SHORTCUT_FILE_NAME);
    let wanted = ShortcutProps {
        target: exe.display().to_string(),
        app_id: APP_ID.to_owned(),
        activator_clsid: TOAST_ACTIVATOR_CLSID.to_owned(),
    };
    let existing = if lnk.exists() {
        match read_shortcut(&lnk) {
            Ok(props) => Some(props),
            Err(e) => {
                tracing::info!(error = %format!("{e:#}"), "existing shortcut unreadable, rewriting it");
                None
            }
        }
    } else {
        None
    };
    if !needs_update(existing.as_ref(), &wanted) {
        return Ok(());
    }
    write_shortcut(&lnk, exe).with_context(|| format!("writing {}", lnk.display()))?;
    tracing::info!(path = %lnk.display(), "start menu shortcut written");
    Ok(())
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs` of the current user.
fn programs_dir() -> Result<PathBuf> {
    // SAFETY: valid folder id; the returned string is freed below.
    let raw = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) }
        .context("locating the Start menu folder")?;
    // SAFETY: `raw` is a valid NUL-terminated string from the shell.
    let path = unsafe { raw.to_hstring() }.to_os_string();
    // SAFETY: allocated by SHGetKnownFolderPath with the COM allocator.
    unsafe { CoTaskMemFree(Some(raw.0 as *const c_void)) };
    Ok(PathBuf::from(path))
}

fn new_shell_link() -> Result<IShellLinkW> {
    // SAFETY: plain COM instantiation on an initialized thread.
    unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .context("creating a shell link")
}

fn read_shortcut(lnk: &Path) -> Result<ShortcutProps> {
    let link = new_shell_link()?;
    let file: IPersistFile = link.cast()?;
    // SAFETY: valid string; `buf` outlives the call.
    let target = unsafe {
        file.Load(&HSTRING::from(lnk), STGM_READ)?;
        let mut buf = vec![0u16; 32 * 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    };
    let store: IPropertyStore = link.cast()?;
    Ok(ShortcutProps {
        target,
        app_id: read_property(&store, &PKEY_AppUserModel_ID, |pv| {
            // SAFETY: `pv` is a valid PROPVARIANT; the string is freed below.
            let raw = unsafe { PropVariantToStringAlloc(pv) }.ok()?;
            // SAFETY: NUL-terminated string from the property system.
            let text = unsafe { raw.to_string() }.ok();
            // SAFETY: allocated with the COM allocator.
            unsafe { CoTaskMemFree(Some(raw.0 as *const c_void)) };
            text
        }),
        activator_clsid: read_property(&store, &PKEY_AppUserModel_ToastActivatorCLSID, |pv| {
            // SAFETY: `pv` is a valid PROPVARIANT.
            unsafe { PropVariantToGUID(pv) }
                .ok()
                .map(|g| format!("{g:?}"))
        }),
    })
}

/// The property converted by `convert`, or `""` if unset or of another type.
fn read_property(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    convert: impl FnOnce(&PROPVARIANT) -> Option<String>,
) -> String {
    // SAFETY: valid key; the value is cleared below.
    let Ok(mut pv) = (unsafe { store.GetValue(key) }) else {
        return String::new();
    };
    let value = convert(&pv).unwrap_or_default();
    // SAFETY: `pv` came from GetValue and is not used afterwards.
    unsafe {
        let _ = PropVariantClear(&mut pv);
    }
    value
}

fn write_shortcut(lnk: &Path, exe: &Path) -> Result<()> {
    let link = new_shell_link()?;
    let exe_w = HSTRING::from(exe);
    let dir_w = HSTRING::from(exe.parent().unwrap_or(exe));
    // SAFETY: valid strings for the duration of each call.
    unsafe {
        link.SetPath(&exe_w)?;
        link.SetWorkingDirectory(&dir_w)?;
        link.SetDescription(&HSTRING::from(APP_NAME))?;
        link.SetIconLocation(&exe_w, 0)?;
    }
    let store: IPropertyStore = link.cast()?;
    // SetValue copies the value, so the string may live in our own buffer;
    // this PROPVARIANT must not be passed to PropVariantClear.
    let app_id: Vec<u16> = APP_ID.encode_utf16().chain([0]).collect();
    let mut id_value = PROPVARIANT::default();
    // SAFETY: sets the type tag and matching member of a zeroed PROPVARIANT.
    unsafe {
        let inner = &mut *id_value.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = PWSTR(app_id.as_ptr().cast_mut());
    }
    // SAFETY: valid key and value; `app_id` outlives the call.
    unsafe { store.SetValue(&PKEY_AppUserModel_ID, &id_value) }
        .context("setting the AppUserModelID")?;
    // SAFETY: valid GUID; the allocated value is cleared below.
    let mut clsid_value = unsafe { InitPropVariantFromCLSID(&CLSID) }?;
    // SAFETY: valid key and value.
    let set = unsafe { store.SetValue(&PKEY_AppUserModel_ToastActivatorCLSID, &clsid_value) };
    // SAFETY: initialized by InitPropVariantFromCLSID, not used afterwards.
    unsafe {
        let _ = PropVariantClear(&mut clsid_value);
    }
    set.context("setting the toast activator CLSID")?;
    // SAFETY: plain COM calls.
    unsafe {
        store.Commit().context("committing shortcut properties")?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(lnk), true)
            .context("saving the shortcut")?;
    }
    Ok(())
}

fn show_toast(xml: &str, tag: &HSTRING, group: &HSTRING) -> Result<()> {
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))
        .context("loading the toast XML")?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.SetTag(tag)?;
    toast.SetGroup(group)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)?;
    Ok(())
}

/// Tag and group must not exceed 64 characters; this is 16.
fn short_hash(s: &str) -> String {
    format!("{:016x}", fnv1a(&[s]))
}

/// Joins the calling thread to the multithreaded apartment once, unless it
/// already belongs to an apartment.
fn ensure_winrt() {
    WINRT_READY.with(|ready| {
        if ready.replace(true) {
            return;
        }
        // SAFETY: plain FFI call; the thread never leaves the apartment.
        if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            && e.code() != RPC_E_CHANGED_MODE
        {
            tracing::warn!(error = %e, "initializing WinRT failed");
        }
    });
}

/// Sets a string value below `HKEY_CURRENT_USER`, creating the key; `None`
/// sets the default value.
fn set_string(subkey: &str, name: Option<&str>, value: &str) -> Result<()> {
    let subkey_w = HSTRING::from(subkey);
    let mut key = HKEY::default();
    // SAFETY: valid strings and out pointer.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &subkey_w,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    }
    .ok()
    .with_context(|| format!(r"creating HKCU\{subkey}"))?;
    let data: Vec<u8> = value
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let name_w = name.map(HSTRING::from);
    let name_ptr = name_w
        .as_ref()
        .map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    // SAFETY: `key` is open; `data` is a NUL-terminated UTF-16 string.
    let set = unsafe { RegSetValueExW(key, name_ptr, None, REG_SZ, Some(&data)) };
    // SAFETY: `key` is open and not used afterwards.
    unsafe {
        let _ = RegCloseKey(key);
    }
    set.ok()
        .with_context(|| format!(r"setting HKCU\{subkey}\{}", name.unwrap_or("(default)")))
}

/// Registers the activator's class factory from a dedicated thread in the
/// multithreaded apartment, so activations arrive on COM worker threads
/// independent of the main thread's apartment and message loop. The thread
/// stays parked to keep the registration alive for the process lifetime.
fn start_activator(on_activate: ActivationSink) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("toast-activator".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let registered = register_factory(on_activate);
            let ok = registered.is_ok();
            let _ = tx.send(registered);
            if ok {
                loop {
                    std::thread::park();
                }
            }
        })
        .context("starting the toast activator thread")?;
    rx.recv().context("toast activator thread exited")?
}

fn register_factory(on_activate: ActivationSink) -> Result<()> {
    // SAFETY: plain FFI call on a fresh thread.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .context("initializing COM")?;
    let factory: IClassFactory = ActivatorFactory { on_activate }.into();
    // SAFETY: COM holds a reference to the factory until the process exits.
    unsafe { CoRegisterClassObject(&CLSID, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE) }
        .context("registering the toast activator")?;
    Ok(())
}

#[implement(IClassFactory)]
struct ActivatorFactory {
    on_activate: ActivationSink,
}

impl IClassFactory_Impl for ActivatorFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if object.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked for null above.
        unsafe { *object = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activator: INotificationActivationCallback = Activator {
            on_activate: self.on_activate.clone(),
        }
        .into();
        // SAFETY: valid interface id and out pointer from the caller.
        unsafe { activator.query(iid, object) }.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

#[implement(INotificationActivationCallback)]
struct Activator {
    on_activate: ActivationSink,
}

impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(
        &self,
        _app_user_model_id: &PCWSTR,
        invoked_args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        let handled = catch_unwind(AssertUnwindSafe(|| {
            let args = if invoked_args.is_null() {
                String::new()
            } else {
                // SAFETY: COM passes a valid NUL-terminated string.
                unsafe { invoked_args.to_string() }.unwrap_or_default()
            };
            match ActivationPayload::decode(&args) {
                Some(p) => (self.on_activate)(p),
                None => tracing::debug!("toast activation without payload"),
            }
        }));
        handled.map_err(|_| {
            tracing::error!("toast activation handler panicked");
            Error::from(E_FAIL)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clsid_matches_string() {
        assert_eq!(format!("{CLSID:?}"), TOAST_ACTIVATOR_CLSID);
    }

    #[test]
    fn short_hash_is_stable_and_short() {
        let a = short_hash("message-id");
        assert_eq!(a, short_hash("message-id"));
        assert_ne!(a, short_hash("message-id2"));
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn notifier_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WinNotifier>();
    }
}
