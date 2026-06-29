//! macOS-specific debug notification helpers: UNUserNotification delegate,
//! category registration, and the blocking send helper.
//!
//! All items are gated `#[cfg(all(debug_assertions, target_os = "macos"))]`.

use super::output::{
    debug_macos_notification_category_identifier, debug_macos_notification_open_action_identifier,
    debug_notification_cli_activation_output_path_from,
    debug_notification_cli_diagnostic_jsonl_path_from,
};

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) fn debug_macos_notification_error_message(
    error: *mut objc2::runtime::AnyObject,
) -> Option<String> {
    if error.is_null() {
        return None;
    }

    // SAFETY: `error` was confirmed non-null above and is the `NSError *` handed
    // back by a UserNotifications completion handler, so it points to a live
    // NSError. `&*error` reborrows that checked pointer only for these message
    // sends; localizedDescription/domain (-> NSString) and code (-> isize) are
    // valid NSError selectors whose return types match the bindings.
    unsafe {
        use objc2::msg_send;
        use objc2::rc::Retained;
        use objc2_foundation::NSString;

        let desc: Retained<NSString> = msg_send![&*error, localizedDescription];
        let domain: Retained<NSString> = msg_send![&*error, domain];
        let code: isize = msg_send![&*error, code];
        Some(format!("{} (domain: {}, code: {})", desc, domain, code))
    }
}

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) fn debug_notification_send_once<T>(
    sender: &std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::SyncSender<T>>>>,
    value: T,
) {
    if let Some(sender) = sender.lock().ok().and_then(|mut guard| guard.take()) {
        let _ = sender.send(value);
    }
}

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) fn register_debug_macos_notification_category(
    center: &objc2_user_notifications::UNUserNotificationCenter,
) {
    use objc2_foundation::{NSArray, NSSet, NSString};
    use objc2_user_notifications::{
        UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
        UNNotificationCategoryOptionNone,
    };

    let action_identifier = NSString::from_str(debug_macos_notification_open_action_identifier());
    let action_title = NSString::from_str("Open Maekon");
    let action = UNNotificationAction::actionWithIdentifier_title_options(
        &action_identifier,
        &action_title,
        UNNotificationActionOptions::Foreground,
    );
    let actions = NSArray::from_slice(&[&*action]);
    let intent_identifiers = NSArray::<NSString>::from_slice(&[]);
    let category_identifier = NSString::from_str(debug_macos_notification_category_identifier());
    let category = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &category_identifier,
        &actions,
        &intent_identifiers,
        UNNotificationCategoryOptionNone,
    );
    let categories = NSSet::from_slice(&[&*category]);
    center.setNotificationCategories(&categories);
}

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) mod debug_macos_notification_delegate {
    use block2::DynBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{define_class, extern_methods, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{NSObject, NSObjectProtocol};
    use objc2_user_notifications::{
        UNNotification, UNNotificationPresentationOptions, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tracing::debug;

    static ACTIVATION_ROUTE: Mutex<Option<String>> = Mutex::new(None);
    static ACTIVATION_OUTPUT_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
    static DIAGNOSTIC_JSONL_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        struct MaekonDebugNotificationDelegate;

        // SAFETY: MaekonDebugNotificationDelegate is an NSObject subclass, so it
        // soundly conforms to NSObjectProtocol (no extra methods required).
        unsafe impl NSObjectProtocol for MaekonDebugNotificationDelegate {}

        // SAFETY: this type implements every required UNUserNotificationCenter
        // Delegate selector below with the exact Objective-C signature declared
        // by the `#[unsafe(method(...))]` attributes, so the runtime can
        // dispatch the protocol's callbacks to it correctly.
        unsafe impl UNUserNotificationCenterDelegate for MaekonDebugNotificationDelegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present_notification(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                write_diagnostic_event("will_present");
                let options = UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound;
                completion_handler.call((options,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive_notification_response(
                &self,
                _center: &UNUserNotificationCenter,
                _response: &UNNotificationResponse,
                completion_handler: &DynBlock<dyn Fn()>,
            ) {
                write_diagnostic_event("did_receive_response");
                write_activation_marker("notification_response");
                activate_maekon_app();
                completion_handler.call(());
            }
        }
    );

    impl MaekonDebugNotificationDelegate {
        extern_methods!(
            #[unsafe(method(new))]
            fn new() -> Retained<Self>;
        );
    }

    pub(crate) fn install(
        center: &UNUserNotificationCenter,
        route: Option<&str>,
        output_path: Option<PathBuf>,
        diagnostic_jsonl_path: Option<PathBuf>,
    ) {
        if let Ok(mut guard) = ACTIVATION_ROUTE.lock() {
            *guard = route.map(ToOwned::to_owned);
        }
        if let Ok(mut guard) = ACTIVATION_OUTPUT_PATH.lock() {
            *guard = output_path;
        }
        if let Ok(mut guard) = DIAGNOSTIC_JSONL_PATH.lock() {
            *guard = diagnostic_jsonl_path;
        }

        let delegate = MaekonDebugNotificationDelegate::new();
        let protocol_delegate: &ProtocolObject<dyn UNUserNotificationCenterDelegate> =
            ProtocolObject::from_ref(&*delegate);
        center.setDelegate(Some(protocol_delegate));
        write_diagnostic_event("delegate_installed");

        // UNUserNotificationCenter stores its delegate weakly. This path is
        // debug-only, so leaking one tiny delegate keeps click handling alive
        // for the VM canary process lifetime without adding lifecycle state.
        std::mem::forget(delegate);
    }

    pub(crate) fn record_reopen_activation() {
        write_diagnostic_event("app_reopen");
        write_activation_marker("app_reopen");
        activate_maekon_app();
    }

    fn write_diagnostic_event(event: &'static str) {
        let route = ACTIVATION_ROUTE.lock().ok().and_then(|guard| guard.clone());
        let diagnostic_jsonl_path = DIAGNOSTIC_JSONL_PATH
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let Some(diagnostic_jsonl_path) = diagnostic_jsonl_path else {
            return;
        };

        if let Some(parent) = diagnostic_jsonl_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                debug!("debug notification diagnostic mkdir failed: {error}");
            }
        }

        let observed_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let payload = json!({
            "debug_notification_diagnostic": true,
            "backend": "macos-unuser",
            "event": event,
            "route": route,
            "category_identifier": super::debug_macos_notification_category_identifier(),
            "open_action_identifier": super::debug_macos_notification_open_action_identifier(),
            "observed_at_ms": observed_at_ms,
        });
        let line = payload.to_string();
        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&diagnostic_jsonl_path)
        {
            Ok(file) => file,
            Err(error) => {
                debug!("debug notification diagnostic open failed: {error}");
                return;
            }
        };
        if let Err(error) = std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes()) {
            debug!("debug notification diagnostic write failed: {error}");
        }
    }

    fn write_activation_marker(source: &'static str) {
        let route = ACTIVATION_ROUTE.lock().ok().and_then(|guard| guard.clone());
        let output_path = ACTIVATION_OUTPUT_PATH
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let Some(output_path) = output_path else {
            return;
        };

        if let Some(parent) = output_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                debug!("debug notification activation marker mkdir failed: {error}");
            }
        }

        let activated_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let payload = json!({
            "debug_notification_activation": true,
            "backend": "macos-unuser",
            "source": source,
            "route": route,
            "focus_main_window": true,
            "activated_at_ms": activated_at_ms,
        });
        if let Err(error) = std::fs::write(output_path, payload.to_string()) {
            debug!("debug notification activation marker write failed: {error}");
        }
    }

    fn activate_maekon_app() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        app.unhide(None);
        if let Some(window) = app.mainWindow().or_else(|| app.keyWindow()) {
            window.makeKeyAndOrderFront(None);
        }
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) fn install_debug_macos_notification_delegate_from_env() {
    let activation_route = super::parsers::debug_notification_activation_route_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_ACTIVATION_ROUTE")
            .ok()
            .as_deref(),
    );
    let activation_output_path = debug_notification_cli_activation_output_path_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_ACTIVATION_OUTPUT")
            .ok()
            .as_deref(),
    );
    let diagnostic_jsonl_path = debug_notification_cli_diagnostic_jsonl_path_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_DIAGNOSTIC_JSONL")
            .ok()
            .as_deref(),
    );
    if activation_route.is_none()
        && activation_output_path.is_none()
        && diagnostic_jsonl_path.is_none()
    {
        return;
    }

    let notification_center =
        objc2_user_notifications::UNUserNotificationCenter::currentNotificationCenter();
    debug_macos_notification_delegate::install(
        &notification_center,
        activation_route.as_deref(),
        activation_output_path,
        diagnostic_jsonl_path,
    );
}

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) fn show_debug_macos_unuser_notification<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    title: &str,
    body: &str,
    activation_route: Option<&str>,
    activation_output_path: Option<std::path::PathBuf>,
    diagnostic_jsonl_path: Option<std::path::PathBuf>,
) -> Result<(), String> {
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    let title = title.to_string();
    let body = body.to_string();
    let activation_route = activation_route.map(ToOwned::to_owned);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let tx = Arc::new(Mutex::new(Some(tx)));
    let add_sender = Arc::clone(&tx);

    app_handle
        // SAFETY: this closure runs on the main thread (guaranteed by
        // run_on_main_thread). It looks up the UNMutableNotificationContent /
        // UNNotificationRequest classes via AnyClass::get and bails out through
        // the `else` arms if either is unavailable, so every msg_send! below
        // targets a valid class/instance with a matching selector signature.
        .run_on_main_thread(move || unsafe {
            let Some(content_class) = AnyClass::get(c"UNMutableNotificationContent") else {
                debug_notification_send_once(
                    &add_sender,
                    Err("macos notification content class unavailable".to_string()),
                );
                return;
            };
            let Some(request_class) = AnyClass::get(c"UNNotificationRequest") else {
                debug_notification_send_once(
                    &add_sender,
                    Err("macos notification request class unavailable".to_string()),
                );
                return;
            };

            let notification_center =
                objc2_user_notifications::UNUserNotificationCenter::currentNotificationCenter();
            debug_macos_notification_delegate::install(
                &notification_center,
                activation_route.as_deref(),
                activation_output_path,
                diagnostic_jsonl_path,
            );
            register_debug_macos_notification_category(&notification_center);

            let content: Retained<AnyObject> = msg_send![content_class, new];
            let title = NSString::from_str(&title);
            let body = NSString::from_str(&body);
            let category_identifier =
                NSString::from_str(debug_macos_notification_category_identifier());
            let _: () = msg_send![&*content, setTitle: &*title];
            let _: () = msg_send![&*content, setBody: &*body];
            let _: () = msg_send![&*content, setCategoryIdentifier: &*category_identifier];

            if let Some(sound_class) = AnyClass::get(c"UNNotificationSound") {
                let sound: *mut AnyObject = msg_send![sound_class, defaultSound];
                if !sound.is_null() {
                    let _: () = msg_send![&*content, setSound: sound];
                }
            }

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis());
            let identifier = NSString::from_str(&format!("maekon-debug-notification-{now}"));
            let request: *mut AnyObject = msg_send![
                request_class,
                requestWithIdentifier: &*identifier,
                content: &*content,
                trigger: std::ptr::null_mut::<AnyObject>()
            ];
            if request.is_null() {
                debug_notification_send_once(
                    &add_sender,
                    Err("macos notification request creation failed".to_string()),
                );
                return;
            }

            let sender = Arc::clone(&add_sender);
            let handler = RcBlock::new(move |error: *mut AnyObject| {
                let result = match debug_macos_notification_error_message(error) {
                    Some(error) => Err(format!("macos notification delivery failed: {error}")),
                    None => Ok(()),
                };
                debug_notification_send_once(&sender, result);
            });

            let _: () = msg_send![
                &*notification_center,
                addNotificationRequest: request,
                withCompletionHandler: &*handler
            ];

            std::mem::forget(handler);
        })
        .map_err(|error| error.to_string())?;

    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "timed out waiting for macOS notification delivery".to_string())?
}
