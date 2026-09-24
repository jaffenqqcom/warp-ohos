//! The OHOS platform delegate.
//!
//! Bridges warp's platform requests to the ability. Capabilities that the
//! OpenHarmony bridge does not expose yet are reported through `warn` logs when
//! they are first requested, so the gaps are visible at runtime instead of
//! silently succeeding.

use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;

use openharmony_ability::{ColorMode, OpenHarmonyApp};
use openharmony_ability_plugin_cursor::CursorExt;
use openharmony_ability_plugin_filepicker::{
    dialog_type as filepicker_dialog_type, FileDialogFilter, FileDialogOptions, FilePickerExt,
};
use openharmony_ability_plugin_ime::ImeExt;

use super::clipboard::Clipboard;
use super::event_loop::{AppEvent, EventSender};
use crate::clipboard::InMemoryClipboard;
use crate::keymap::Keystroke;
use crate::notification::{NotificationSendError, RequestPermissionsOutcome};
use crate::platform::{self, Cursor};

/// Stores the ID of the application's main thread.
static MAIN_THREAD_ID: OnceLock<thread::ThreadId> = OnceLock::new();

/// Whether the system soft keyboard is currently shown.
///
/// The authoritative source is the ArkTS side: the IME plugin reports every
/// show/hide transition through the `keyboard-status` bridge event, and the
/// event loop writes the result here. It is process-global rather than a field
/// of [`AppDelegate`] because the event loop, not the delegate, receives that
/// event.
static IME_OPEN: AtomicBool = AtomicBool::new(false);

/// Marks the current thread as the application's main thread.
///
/// Panics if called more than once.
pub(super) fn mark_current_thread_as_main() {
    MAIN_THREAD_ID
        .set(thread::current().id())
        .expect("should only call mark_current_thread_as_main once!");
}

/// Values of the OHOS `Input_PointerStyle` enumeration
/// (`multimodalinput/oh_pointer_style.h`, API 22+) that warp's cursor set maps
/// onto. The system call takes the enumeration as a plain `i32`.
mod pointer_style {
    /// Arrow pointer, the system default.
    pub(super) const DEFAULT: i32 = 0;
    /// Cross with fine positioning aids, used for precision selection.
    pub(super) const CROSS: i32 = 13;
    /// Pointer carrying a copy badge.
    pub(super) const CURSOR_COPY: i32 = 14;
    /// Prohibited action.
    pub(super) const CURSOR_FORBID: i32 = 15;
    /// Hand closed as if holding a dragged object.
    pub(super) const HAND_GRABBING: i32 = 17;
    /// Hand open, ready to drag.
    pub(super) const HAND_OPEN: i32 = 18;
    /// Hand with a pointing index finger.
    pub(super) const HAND_POINTING: i32 = 19;
    /// Horizontal resize arrows.
    pub(super) const RESIZE_LEFT_RIGHT: i32 = 22;
    /// Vertical resize arrows.
    pub(super) const RESIZE_UP_DOWN: i32 = 23;
    /// Text-selection beam.
    pub(super) const TEXT_CURSOR: i32 = 26;
}

/// Maps a warp cursor onto the OHOS pointer style that renders it.
///
/// The match names every [`Cursor`] variant rather than ending in a wildcard, so
/// a variant added upstream fails to compile here instead of silently falling
/// back to the arrow.
fn cursor_to_pointer_style(cursor: Cursor) -> i32 {
    match cursor {
        Cursor::Arrow => pointer_style::DEFAULT,
        Cursor::IBeam => pointer_style::TEXT_CURSOR,
        Cursor::Crosshair => pointer_style::CROSS,
        Cursor::OpenHand => pointer_style::HAND_OPEN,
        Cursor::ClosedHand => pointer_style::HAND_GRABBING,
        Cursor::NotAllowed => pointer_style::CURSOR_FORBID,
        Cursor::PointingHand => pointer_style::HAND_POINTING,
        Cursor::ResizeLeftRight => pointer_style::RESIZE_LEFT_RIGHT,
        Cursor::ResizeUpDown => pointer_style::RESIZE_UP_DOWN,
        Cursor::DragCopy => pointer_style::CURSOR_COPY,
    }
}

/// Explains the global-shortcut error codes that have a cause the user can act
/// on.
///
/// Returns an empty string for the codes whose raw value already says enough,
/// so callers can append the result to a log message unconditionally.
fn hotkey_rejection_hint(code: i32) -> &'static str {
    match code {
        openharmony_ability::INPUT_OCCUPIED_BY_SYSTEM => " (reserved by the system)",
        openharmony_ability::INPUT_OCCUPIED_BY_OTHER => " (already held by another application)",
        openharmony_ability::INPUT_DEVICE_NOT_SUPPORTED => " (the keyboard cannot produce it)",
        _ => "",
    }
}

pub(super) struct AppDelegate {
    /// The ability app, needed for the system calls the platform layer issues
    /// directly (the cursor style is the only one today).
    app: OpenHarmonyApp,
    clipboard: Option<Clipboard>,
    /// Used when the system pasteboard cannot be created, so that copy and paste
    /// keep working within the session.
    memory_clipboard: InMemoryClipboard,
    cursor_shape: Cell<Cursor>,
    /// The pointer style the system last accepted, cached so that repeated
    /// requests for the shape already in effect skip the system call.
    pointer_style: Cell<Option<i32>>,
    event_sender: EventSender,
    query_microphone_access: bool,
    /// Set once the accessibility-tree gap has been reported, so that the
    /// warning is emitted once per session rather than on every update.
    accessibility_gap_reported: Cell<bool>,
}

impl AppDelegate {
    pub(super) fn new(
        app: OpenHarmonyApp,
        event_sender: EventSender,
        query_microphone_access: bool,
    ) -> Self {
        let clipboard = match Clipboard::new() {
            Ok(clipboard) => Some(clipboard),
            Err(err) => {
                log::error!(
                    "ohos::delegate::AppDelegate::new: the system pasteboard is unavailable, \
                     falling back to an in-memory clipboard: {err:#}"
                );
                None
            }
        };

        // Global shortcuts are held by the framework's hotkey registry. The
        // handler turns a fired hotkey back into a keystroke and queues it, so
        // the action it is bound to runs on warp's own thread rather than on
        // whatever thread the input service reports from.
        {
            let event_sender = event_sender.clone();
            openharmony_ability::set_hotkey_triggered_handler(Box::new(move |pre_keys, final_key| {
                let Some(keystroke) = super::hotkey::hotkey_to_keystroke(pre_keys, final_key) else {
                    return;
                };
                if event_sender
                    .send(AppEvent::GlobalShortcutTriggered(keystroke))
                    .is_err()
                {
                    log::debug!(
                        "ohos::delegate: a global shortcut fired after the event loop was gone"
                    );
                }
            }));
        }

        Self {
            app,
            clipboard,
            memory_clipboard: InMemoryClipboard::default(),
            cursor_shape: Cell::new(Cursor::Arrow),
            pointer_style: Cell::new(None),
            event_sender,
            query_microphone_access,
            accessibility_gap_reported: Cell::new(false),
        }
    }

    fn send_event(&self, event: AppEvent) {
        if self.event_sender.send(event).is_err() {
            log::debug!("ohos::delegate: tried to send an event, but the event loop is gone");
        }
    }

    /// Reports a capability the port does not provide yet.
    fn report_gap(&self, capability: &str) {
        log::warn!("ohos::delegate: {capability} is not available on the OHOS back-end yet");
    }
}

impl platform::Delegate for AppDelegate {
    fn dispatch_delegate(&self) -> Arc<dyn platform::DispatchDelegate> {
        Arc::new(DispatchDelegate {
            event_sender: self.event_sender.clone(),
        })
    }

    fn request_user_attention(&self, _window_id: crate::WindowId) {
        self.report_gap("requesting user attention");
    }

    fn clipboard(&mut self) -> &mut dyn crate::Clipboard {
        match &mut self.clipboard {
            Some(clipboard) => clipboard,
            None => &mut self.memory_clipboard,
        }
    }

    fn system_theme(&self) -> platform::SystemTheme {
        // The configuration is only populated once the ability reports a change,
        // so an unset mode is treated as light.
        match super::global_app().map(|app| app.config().color_mode) {
            Some(ColorMode::Dark) => platform::SystemTheme::Dark,
            Some(ColorMode::Light) => platform::SystemTheme::Light,
            Some(ColorMode::NoSet) | None => platform::SystemTheme::Light,
        }
    }

    fn open_url(&self, url: &str) -> bool {
        open_url_in_system(url)
    }

    fn open_file_path(&self, path: &std::path::Path) {
        // warp's `open_file_path` is "double-click the path" and leaves the choice of
        // handler to the platform. Here a directory is only reachable through the file
        // manager, while a file belongs to the application registered for its type.
        let mode = if path.is_dir() {
            SystemOpenMode::FileManager
        } else {
            SystemOpenMode::RegisteredApp
        };
        open_path_with_system(path, mode);
    }

    fn open_file_path_in_explorer(&self, path: &std::path::Path) {
        open_path_with_system(path, SystemOpenMode::FileManager);
    }

    fn open_file_picker(
        &self,
        callback: platform::FilePickerCallback,
        file_picker_config: platform::FilePickerConfiguration,
    ) {
        // The OHOS picker lists either files or folders, never both at once, so a
        // configuration that allows folders shows a folder picker -- the same
        // reduction the Linux back-end makes.
        let dialog_kind = if file_picker_config.allows_folder() {
            filepicker_dialog_type::OPEN_FOLDER
        } else {
            filepicker_dialog_type::OPEN_FILE
        };
        let filters = file_picker_config
            .file_types()
            .iter()
            .map(|file_type| {
                FileDialogFilter::new()
                    .name(file_type.display_name())
                    .pattern(file_type.extensions().join(";"))
            })
            .collect();
        let allow_many = file_picker_config.allows_multi_select();
        let options = FileDialogOptions::new(dialog_kind)
            .allow_many(allow_many)
            .filters(filters);
        log::info!(
            "ohos::delegate::open_file_picker: dialog_kind={dialog_kind}, allow_many={allow_many}"
        );
        show_file_dialog_off_thread(
            self.app.clone(),
            options,
            self.event_sender.clone(),
            move |paths, ctx| callback(Ok(paths), ctx),
        );
    }

    fn open_save_file_picker(
        &self,
        callback: platform::SaveFilePickerCallback,
        config: platform::SaveFilePickerConfiguration,
    ) {
        // The configuration carries paths, while the picker wants a URI for its starting
        // location; `default_location` accepts either and the plugin does the mapping.
        let mut options = FileDialogOptions::new(filepicker_dialog_type::SAVE_FILE);
        if let Some(directory) = config.default_directory.as_ref() {
            options = options.default_location(directory.to_string_lossy().into_owned());
        }
        if let Some(filename) = config.default_filename.as_ref() {
            options = options.default_file_name(filename.clone());
        }
        log::info!(
            "ohos::delegate::open_save_file_picker: dialog_kind={}, \
             has_default_directory={}, has_default_filename={}",
            filepicker_dialog_type::SAVE_FILE,
            config.default_directory.is_some(),
            config.default_filename.is_some()
        );
        show_file_dialog_off_thread(
            self.app.clone(),
            options,
            self.event_sender.clone(),
            move |paths, ctx| callback(paths.into_iter().next(), ctx),
        );
    }

    fn application_bundle_info(
        &self,
        _bundle_identifier: &str,
    ) -> Option<crate::ApplicationBundleInfo<'_>> {
        None
    }

    fn show_native_platform_modal(
        &self,
        _id: crate::modals::ModalId,
        _modal: crate::modals::AlertDialog,
    ) {
        self.report_gap("native platform modals");
    }

    fn request_desktop_notification_permissions(
        &self,
        on_completion: platform::RequestNotificationPermissionsCallback,
    ) {
        // The ability bridge exposes no notification API, so permission cannot
        // be requested at all.
        log::warn!(
            "ohos::delegate::request_desktop_notification_permissions: OHOS notifications are not \
             wired up; reporting the permission as denied"
        );
        self.send_event(AppEvent::RunCallback(Box::new(move |ctx| {
            on_completion(RequestPermissionsOutcome::PermissionsDenied, ctx);
        })));
    }

    fn send_desktop_notification(
        &self,
        _notification_content: crate::notification::UserNotification,
        _window_id: crate::WindowId,
        on_error: platform::SendNotificationErrorCallback,
    ) {
        log::warn!(
            "ohos::delegate::send_desktop_notification: OHOS notifications are not wired up; the \
             notification was dropped"
        );
        self.send_event(AppEvent::RunCallback(Box::new(move |ctx| {
            on_error(NotificationSendError::PermissionsDenied, ctx);
        })));
    }

    fn set_cursor_shape(&self, cursor: Cursor) {
        // The pointer is drawn by the system; the requested shape is kept so that
        // later queries stay consistent, and the matching OHOS pointer style is
        // applied to the app's window.
        self.cursor_shape.set(cursor);
        let style = cursor_to_pointer_style(cursor);
        // Hoverable elements request a shape on every mouse move, so only the
        // transition reaches the system. The cache records the style the system
        // accepted rather than the one requested: a call that lands before the
        // ArkTS side has pushed the window id fails and stays retryable.
        if self.pointer_style.get() == Some(style) {
            return;
        }
        log::debug!("ohos::delegate::set_cursor_shape: cursor={cursor:?}, pointer_style={style}");
        if self.app.set_cursor_style(style) {
            self.pointer_style.set(Some(style));
        }
    }

    #[cfg(feature = "test-util")]
    fn get_cursor_shape(&self) -> Cursor {
        self.cursor_shape.get()
    }

    fn close_ime_async(&self, _window_id: crate::WindowId) {
        // The soft keyboard is a system-owned session on OHOS: the system hides
        // it when the window resigns active or the user dismisses it, and the
        // ArkTS IME plugin reports every transition back through the
        // keyboard-status bridge event. Detaching here instead would tear the
        // keyboard down while warp still holds window focus, because warp
        // requests this on every focused-view change. The request is therefore
        // only recorded; the authoritative state lives in `IME_OPEN`.
        log::info!(
            "ohos::delegate::close_ime_async: leaving the soft keyboard to the system; the IME \
             status event reports the real state"
        );
    }

    fn is_ime_open(&self) -> bool {
        IME_OPEN.load(Ordering::Acquire)
    }

    fn open_character_palette(&self) {
        self.report_gap("the character palette");
    }

    fn set_accessibility_contents(&self, _content: crate::accessibility::AccessibilityContent) {
        if !self.accessibility_gap_reported.replace(true) {
            log::warn!(
                "ohos::delegate::set_accessibility_contents: the OHOS accessibility tree is not \
                 populated by the native back-end yet"
            );
        }
    }

    fn register_global_shortcut(&self, shortcut: Keystroke) {
        let Some((pre_keys, final_key)) = super::hotkey::keystroke_to_hotkey(&shortcut) else {
            return;
        };
        if let Err(error) = openharmony_ability::register_hotkey(&pre_keys, final_key) {
            log::warn!(
                "ohos::delegate::register_global_shortcut: the system rejected '{shortcut:?}': \
                 {error}{}",
                hotkey_rejection_hint(error)
            );
        }
    }

    fn unregister_global_shortcut(&self, shortcut: &Keystroke) {
        let Some((pre_keys, final_key)) = super::hotkey::keystroke_to_hotkey(shortcut) else {
            return;
        };
        if let Err(error) = openharmony_ability::unregister_hotkey(&pre_keys, final_key) {
            log::warn!(
                "ohos::delegate::unregister_global_shortcut: the system rejected removing \
                 '{shortcut:?}': {error}"
            );
        }
    }

    fn terminate_app(&self, termination_mode: platform::TerminationMode) {
        // Leaving the ability itself is an ArkTS decision; ending warp's loop is
        // all the native side can do on its own.
        log::info!(
            "ohos::delegate::terminate_app: stopping warp's event loop ({termination_mode:?}); the \
             ability keeps the process alive"
        );
        self.send_event(AppEvent::Terminate(termination_mode));
    }

    fn is_screen_reader_enabled(&self) -> Option<bool> {
        None
    }

    fn microphone_access_state(&self) -> platform::MicrophoneAccessState {
        if !self.query_microphone_access {
            return platform::MicrophoneAccessState::Denied;
        }
        log::warn!(
            "ohos::delegate::microphone_access_state: the microphone permission is not queried \
             through the native back-end yet; reporting it as not determined"
        );
        platform::MicrophoneAccessState::NotDetermined
    }

    fn is_gui(&self) -> bool {
        true
    }
}

struct DispatchDelegate {
    event_sender: EventSender,
}

impl platform::DispatchDelegate for DispatchDelegate {
    fn is_main_thread(&self) -> bool {
        thread::current().id()
            == *MAIN_THREAD_ID
                .get()
                .expect("should have marked a thread as the main thread")
    }

    fn run_on_main_thread(&self, task: async_task::Runnable) {
        // See crate::windowing::winit::delegate::DispatchDelegate for why we use ManuallyDrop.
        if self
            .event_sender
            .send(AppEvent::RunTask(ManuallyDrop::new(task)))
            .is_err()
        {
            log::debug!("ohos::delegate: tried to send a task, but the event loop is gone");
        }
    }
}

/// Binds the system IME to the focused editor, which brings up the soft
/// keyboard.
///
/// The ArkTS side retries while the edit box has not taken focus yet, so calling
/// this again after an early failure is safe. `IME_OPEN` is deliberately not set
/// here: the keyboard is only genuinely open once the system says so, and that
/// arrives through the keyboard-status event.
pub(super) fn open_ime_async() {
    let Some(app) = super::global_app() else {
        log::warn!("ohos::delegate::open_ime_async: no ability app is installed");
        return;
    };
    log::info!("ohos::delegate::open_ime_async: showing the soft keyboard");
    match app.ime() {
        Ok(client) => match crate::r#async::block_on(client.attach()) {
            Ok(ack) if ack.accepted => {
                log::info!(
                    "ohos::delegate::open_ime_async: the IME session is bound to the focused editor"
                );
            }
            Ok(_) => {
                log::warn!(
                    "ohos::delegate::open_ime_async: the editor has not taken focus yet, so the \
                     keyboard was not shown"
                );
            }
            Err(err) => {
                log::error!(
                    "ohos::delegate::open_ime_async: binding the IME session failed: {err}"
                );
            }
        },
        Err(err) => {
            log::error!("ohos::delegate::open_ime_async: the IME bridge is unavailable: {err}");
        }
    }
}

/// Records the soft keyboard state the ArkTS IME plugin reported.
pub(super) fn set_ime_open(open: bool) {
    log::debug!("ohos::delegate::set_ime_open: open={open}");
    IME_OPEN.store(open, Ordering::Release);
}

/// Opens `url` with the system link opener.
///
/// Also used by the headless back-end, which on OHOS has no URL opening path of
/// its own.
pub(crate) fn open_url_in_system(url: &str) -> bool {
    use openharmony_ability_plugin_url::UrlExt;

    let Some(app) = super::global_app() else {
        log::warn!(
            "ohos::delegate::open_url_in_system: no ability app is installed; cannot open {url}"
        );
        return false;
    };
    log::info!("ohos::delegate::open_url_in_system: opening {url}");
    match crate::r#async::block_on(app.open_url(url.to_owned())) {
        Ok(()) => true,
        Err(err) => {
            log::error!("ohos::delegate::open_url_in_system: opening {url} failed: {err}");
            false
        }
    }
}

/// What the system should do with a path handed to it.
#[derive(Clone, Copy)]
enum SystemOpenMode {
    /// Let the application registered for the file's type open it.
    RegisteredApp,
    /// Show the path inside the system file manager.
    FileManager,
}

impl SystemOpenMode {
    /// Name used in log records.
    fn name(self) -> &'static str {
        match self {
            SystemOpenMode::RegisteredApp => "the registered application",
            SystemOpenMode::FileManager => "the file manager",
        }
    }
}

/// Opens `path` with the system handler `mode` names.
///
/// Both modes live in the `ohos.openbysys` bridge plugin, whose ArkTS half converts the
/// local path into its `file://` URI and either dispatches the implicit
/// `ohos.want.action.viewData` want or opens the `filemanager://openDirectory` link.
fn open_path_with_system(path: &std::path::Path, mode: SystemOpenMode) {
    use openharmony_ability_plugin_openbysys::OpenBySysExt;

    let Some(app) = super::global_app() else {
        log::warn!(
            "ohos::delegate::open_path_with_system: no ability app is installed; cannot open {}",
            path.display()
        );
        return;
    };
    let path = path.to_string_lossy().into_owned();
    let mode_name = mode.name();
    log::info!("ohos::delegate::open_path_with_system: opening {path} with {mode_name}");
    let result = match mode {
        SystemOpenMode::RegisteredApp => crate::r#async::block_on(app.open_file(path.clone())),
        SystemOpenMode::FileManager => {
            crate::r#async::block_on(app.reveal_in_file_manager(path.clone()))
        }
    };
    if let Err(err) = result {
        log::error!(
            "ohos::delegate::open_path_with_system: opening {path} with {mode_name} failed: {err}"
        );
    }
}

/// Shows the system file dialog on a dedicated thread and delivers the selected
/// paths to `on_paths` on the main thread.
///
/// The dialog stays open until the user decides, so it cannot run on the main
/// thread: blocking here would freeze the UI for as long as the picker is up. A
/// dialog the user dismisses and one the platform rejects both arrive as an
/// empty selection, because the callers read "nothing selected" as a cancel and
/// treating a rejection as an error would raise a failure on every dismissal.
fn show_file_dialog_off_thread(
    app: OpenHarmonyApp,
    options: FileDialogOptions,
    event_sender: EventSender,
    on_paths: impl FnOnce(Vec<String>, &mut crate::AppContext) + Send + Sync + 'static,
) {
    let dialog_kind = options.dialog_type.clone();
    let spawn_result = std::thread::Builder::new()
        .name("File Picker".to_string())
        .spawn(move || {
            let paths = match crate::r#async::block_on(app.show_file_dialog(options)) {
                Ok(response) => {
                    log::debug!(
                        "ohos::delegate: the {dialog_kind} dialog picked {} entry(ies)",
                        response.files.len()
                    );
                    local_paths_from_uris(&response.files)
                }
                Err(err) => {
                    log::warn!(
                        "ohos::delegate: the {dialog_kind} dialog returned no selection: {err}"
                    );
                    Vec::new()
                }
            };
            let delivered = event_sender.send(AppEvent::RunCallback(Box::new(move |ctx| {
                on_paths(paths, ctx);
            })));
            if delivered.is_err() {
                log::debug!(
                    "ohos::delegate: tried to deliver a {dialog_kind} result, but the event loop \
                     is gone"
                );
            }
        });
    if let Err(err) = spawn_result {
        log::error!("ohos::delegate: spawning the file dialog thread failed: {err}");
    }
}

/// Maps file URIs the system handed over (a picker selection, a drop) to the
/// local sandbox paths `std::fs` can open, dropping any URI that does not
/// resolve.
///
/// `openharmony_ability::path_from_uri` also persists the read-write grant the
/// system attached to the URI, which is what keeps the file readable after a
/// restart.
pub(super) fn local_paths_from_uris(uris: &[String]) -> Vec<String> {
    uris.iter()
        .filter_map(|uri| match openharmony_ability::path_from_uri(uri) {
            Some(path) => Some(path.to_string_lossy().into_owned()),
            None => {
                log::error!("ohos::delegate: no local path maps to the picked URI {uri}");
                None
            }
        })
        .collect()
}
