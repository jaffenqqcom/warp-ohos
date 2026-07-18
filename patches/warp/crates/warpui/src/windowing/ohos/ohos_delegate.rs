use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

fn ignore_poison<T>(e: PoisonError<T>) -> T {
    e.into_inner()
}

use warpui_core::accessibility::AccessibilityContent;
use warpui_core::keymap::Keystroke;
use warpui_core::modals::{AlertDialog, ModalId};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::notification::{RequestPermissionsOutcome, UserNotification};
use warpui_core::platform::SendNotificationErrorCallback;
use warpui_core::platform::{
    Cursor, Delegate, DispatchDelegate, FilePickerCallback, FilePickerConfiguration,
    MicrophoneAccessState, RequestNotificationPermissionsCallback,
    SaveFilePickerCallback, SaveFilePickerConfiguration, SystemTheme, TerminationMode,
};
use warpui_core::platform::file_picker::FilePickerError;
use warpui_core::AppContext;
use warpui_core::{ApplicationBundleInfo, Clipboard, WindowId};

static PENDING_APP_CALLBACKS: Mutex<Vec<Box<dyn FnOnce(&mut AppContext) + Send>>> =
    Mutex::new(Vec::new());

// ── C FFI 动态符号查找 ─────────────────────────────────────────────────────
//
// libentry.so 导出的 C FFI 桥函数（service_*.cpp）通过 dlsym(RTLD_DEFAULT)
// 运行时查找。libentry.so 是 entry NAPI 模块，Ability 启动时自动加载。

extern "C" {
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::os::raw::c_char) -> *mut std::ffi::c_void;
}

const RTLD_DEFAULT: *mut std::ffi::c_void = std::ptr::null_mut();

macro_rules! ffi_fn {
    (fn $name:ident($($arg:ident: $aty:ty),*) -> $ret:ty) => {
        unsafe fn $name($($arg: $aty),*) -> $ret {
            static FN: OnceLock<unsafe extern "C" fn($($aty),*) -> $ret> = OnceLock::new();
            let f = FN.get_or_init(|| {
                let cname = CString::new(stringify!($name)).unwrap();
                let ptr = dlsym(RTLD_DEFAULT, cname.as_ptr().cast());
                if ptr.is_null() {
                    log::error!("dlsym({}) failed", stringify!($name));
                    return std::mem::zeroed();
                }
                std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn($($aty),*) -> $ret>(ptr)
            });
            f($($arg),*)
        }
    };
    (fn $name:ident($($arg:ident: $aty:ty),*)) => {
        ffi_fn!(fn $name($($arg: $aty),*) -> ());
    };
    (fn $name:ident() -> $ret:ty) => {
        unsafe fn $name() -> $ret {
            static FN: OnceLock<unsafe extern "C" fn() -> $ret> = OnceLock::new();
            let f = FN.get_or_init(|| {
                let cname = CString::new(stringify!($name)).unwrap();
                let ptr = dlsym(RTLD_DEFAULT, cname.as_ptr().cast());
                if ptr.is_null() {
                    log::error!("dlsym({}) failed", stringify!($name));
                    return std::mem::zeroed();
                }
                std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn() -> $ret>(ptr)
            });
            f()
        }
    };
    (fn $name:ident()) => {
        unsafe fn $name() {
            static FN: OnceLock<unsafe extern "C" fn()> = OnceLock::new();
            let f = FN.get_or_init(|| {
                let cname = CString::new(stringify!($name)).unwrap();
                let ptr = dlsym(RTLD_DEFAULT, cname.as_ptr().cast());
                if ptr.is_null() {
                    log::error!("dlsym({}) failed", stringify!($name));
                    return;
                }
                std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn()>(ptr)
            });
            f()
        }
    };
}

ffi_fn!(fn ohos_clipboard_read(callback: extern "C" fn(*const c_char)));
ffi_fn!(fn ohos_clipboard_write(text: *const c_char));
ffi_fn!(fn ohos_pick_files(mime_types: *const c_char, callback: extern "C" fn(*const c_char)));
ffi_fn!(fn ohos_save_file(name: *const c_char, callback: extern "C" fn(*const c_char)));
ffi_fn!(fn ohos_send_notification(title: *const c_char, text: *const c_char));
ffi_fn!(fn ohos_request_notif_perm(callback: extern "C" fn(bool)));
ffi_fn!(fn ohos_get_color_mode() -> *const c_char);
ffi_fn!(fn ohos_open_url(url: *const c_char));
ffi_fn!(fn ohos_get_display_info() -> *const c_char);
ffi_fn!(fn ohos_terminate_self());
ffi_fn!(fn ohos_close_ime());

pub fn enqueue_app_callback(cb: Box<dyn FnOnce(&mut AppContext) + Send>) {
    PENDING_APP_CALLBACKS
        .lock()
        .unwrap_or_else(ignore_poison)
        .push(cb);
}

pub fn dispatch_pending_callbacks(ctx: &mut AppContext) {
    let pending = PENDING_APP_CALLBACKS
        .lock()
        .unwrap_or_else(ignore_poison)
        .drain(..)
        .collect::<Vec<_>>();
    for cb in pending {
        cb(ctx);
    }
}

// ── 回调捕获辅助 ──────────────────────────────────────────────────────────────

static CLIPBOARD_READ_BUF: Mutex<Option<String>> = Mutex::new(None);
static PICK_FILES_BUF: Mutex<Option<String>> = Mutex::new(None);
static SAVE_FILE_BUF: Mutex<Option<String>> = Mutex::new(None);
static NOTIF_PERM_BUF: AtomicBool = AtomicBool::new(false);
static NOTIF_PERM_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_clipboard_read(text: *const c_char) {
    let s = if text.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
    };
    *CLIPBOARD_READ_BUF.lock().unwrap_or_else(ignore_poison) = Some(s);
}

extern "C" fn on_pick_files(result: *const c_char) {
    let s = if result.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(result) }.to_string_lossy().into_owned()
    };
    *PICK_FILES_BUF.lock().unwrap_or_else(ignore_poison) = Some(s);
}

extern "C" fn on_save_file(result: *const c_char) {
    let s = if result.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(result) }.to_string_lossy().into_owned()
    };
    *SAVE_FILE_BUF.lock().unwrap_or_else(ignore_poison) = Some(s);
}

extern "C" fn on_notif_perm(granted: bool) {
    NOTIF_PERM_BUF.store(granted, Ordering::SeqCst);
    NOTIF_PERM_RECEIVED.store(true, Ordering::SeqCst);
}

fn parse_json_str_array(json: &str) -> Vec<String> {
    let json = json.trim();
    if json.is_empty() || json == "[]" {
        return Vec::new();
    }
    let inner = match json
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .map(|s| s.trim())
    {
        Some(s) if s.is_empty() => return Vec::new(),
        Some(s) => s,
        None => return vec![json.to_string()],
    };
    let mut results = Vec::new();
    let mut remaining = inner.trim();
    while !remaining.is_empty() {
        if !remaining.starts_with('"') {
            results.push(remaining.to_string());
            break;
        }
        remaining = &remaining[1..];
        if let Some(end) = remaining.find('"') {
            results.push(remaining[..end].to_string());
            remaining = remaining[end + 1..].trim();
            if remaining.starts_with(',') {
                remaining = remaining[1..].trim();
            }
        } else {
            results.push(remaining.to_string());
            break;
        }
    }
    results
}

// ── OhosClipboard ─────────────────────────────────────────────────────────

pub struct OhosClipboard;

impl Clipboard for OhosClipboard {
    fn read(&mut self) -> ClipboardContent {
        *CLIPBOARD_READ_BUF.lock().unwrap_or_else(ignore_poison) = None;
        unsafe { ohos_clipboard_read(on_clipboard_read) };
        ClipboardContent {
            plain_text: CLIPBOARD_READ_BUF.lock().unwrap_or_else(ignore_poison).take().unwrap_or_default(),
            ..ClipboardContent::default()
        }
    }

    fn write(&mut self, contents: ClipboardContent) {
        if let Ok(cstr) = CString::new(contents.plain_text) {
            unsafe { ohos_clipboard_write(cstr.as_ptr()) };
        }
    }
}

// ── PendingDispatchDelegate ─────────────────────────────────────────────────

struct PendingDispatchDelegate {
    pending: Mutex<Vec<async_task::Runnable>>,
    main_thread_id: std::thread::ThreadId,
}

impl PendingDispatchDelegate {
    fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            main_thread_id: std::thread::current().id(),
        }
    }

    fn drain_pending(&self) -> Vec<async_task::Runnable> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(ignore_poison))
    }
}

impl DispatchDelegate for PendingDispatchDelegate {
    fn is_main_thread(&self) -> bool {
        std::thread::current().id() == self.main_thread_id
    }

    fn run_on_main_thread(&self, task: async_task::Runnable) {
        log::trace!("PendingDispatchDelegate: run_on_main_thread called");
        self.pending
            .lock()
            .unwrap_or_else(ignore_poison)
            .push(task);
    }
}

// ── OhosDelegate ───────────────────────────────────────────────────────────

pub struct OhosDelegate {
    clipboard: OhosClipboard,
    dispatch: Arc<PendingDispatchDelegate>,
    /// 追踪软键盘打开/关闭状态，供 `is_ime_open()` 查询。
    /// 由 `set_ime_open()` 更新（在收到 Ime::Enabled/Disabled 事件时）。
    ime_open: AtomicBool,
}

impl OhosDelegate {
    pub fn new() -> Self {
        Self {
            clipboard: OhosClipboard,
            dispatch: Arc::new(PendingDispatchDelegate::new()),
            ime_open: AtomicBool::new(false),
        }
    }

    /// 更新软键盘打开/关闭状态。
    /// 在收到 winit Ime::Enabled / Ime::Disabled 事件时由事件循环调用。
    pub fn set_ime_open(&self, open: bool) {
        log::info!("OhosDelegate::set_ime_open: {}", open);
        self.ime_open.store(open, Ordering::SeqCst);
    }


    pub fn drain_pending_dispatch_tasks(&self) -> Vec<async_task::Runnable> {
        self.dispatch.drain_pending()
    }
}

impl Delegate for OhosDelegate {
    fn dispatch_delegate(&self) -> Arc<dyn DispatchDelegate> {
        self.dispatch.clone()
    }

    fn request_user_attention(&self, _window_id: WindowId) {
        log::info!("OhosDelegate::request_user_attention: {_window_id:?}");
        if let (Ok(title_cstr), Ok(body_cstr)) = (
            CString::new("Warp"),
            CString::new("Application needs your attention"),
        ) {
            unsafe { ohos_send_notification(title_cstr.as_ptr(), body_cstr.as_ptr()) };
        }
    }

    fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }

    fn system_theme(&self) -> SystemTheme {
        // TODO: 恢复为动态检测。当前 OHOS 设备默认返回 light 导致文字黑色不可见。
        SystemTheme::Dark
    }

    fn open_url(&self, url: &str) {
        if let Ok(cstr) = CString::new(url) {
            unsafe { ohos_open_url(cstr.as_ptr()) };
        }
    }

    fn open_file_path(&self, path: &Path) {
        log::info!("OhosDelegate::open_file_path: {path:?}");
        if let Some(path_str) = path.to_str() {
            if let Ok(uri_cstr) = CString::new(format!("file://{path_str}")) {
                unsafe { ohos_open_url(uri_cstr.as_ptr()) };
            }
        }
    }

    fn open_file_path_in_explorer(&self, path: &Path) {
        log::info!("OhosDelegate::open_file_path_in_explorer: {path:?}");
        if let Some(path_str) = path.to_str() {
            if let Ok(uri_cstr) = CString::new(format!("file://{path_str}")) {
                unsafe { ohos_open_url(uri_cstr.as_ptr()) };
            }
        }
    }

    fn open_file_picker(
        &self,
        callback: FilePickerCallback,
        file_picker_config: FilePickerConfiguration,
    ) {
        let mime_types = file_picker_config
            .file_types()
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if let Ok(cstr) = CString::new(mime_types) {
            *PICK_FILES_BUF.lock().unwrap_or_else(ignore_poison) = None;
            unsafe { ohos_pick_files(cstr.as_ptr(), on_pick_files) };
            let json_result = PICK_FILES_BUF.lock().unwrap_or_else(ignore_poison).take();
            let result = match json_result {
                Some(json) => Ok(parse_json_str_array(&json)),
                None => Err(FilePickerError::DialogFailed("no result".into())),
            };
            enqueue_app_callback(Box::new(move |ctx| callback(result, ctx)));
        }
    }

    fn open_save_file_picker(
        &self,
        callback: SaveFilePickerCallback,
        config: SaveFilePickerConfiguration,
    ) {
        let name = config.default_filename.as_deref().unwrap_or("file");
        if let Ok(cstr) = CString::new(name) {
            *SAVE_FILE_BUF.lock().unwrap_or_else(ignore_poison) = None;
            unsafe { ohos_save_file(cstr.as_ptr(), on_save_file) };
            let result = SAVE_FILE_BUF.lock().unwrap_or_else(ignore_poison).take();
            enqueue_app_callback(Box::new(move |ctx| callback(result, ctx)));
        }
    }

    fn application_bundle_info(&self, _bundle_identifier: &str) -> Option<ApplicationBundleInfo<'_>> {
        None
    }

    fn show_native_platform_modal(&self, _id: ModalId, _modal: AlertDialog) {
        log::info!("OhosDelegate::show_native_platform_modal: {_id:?}");
    }

    fn request_desktop_notification_permissions(
        &self,
        on_completion: RequestNotificationPermissionsCallback,
    ) {
        NOTIF_PERM_RECEIVED.store(false, Ordering::SeqCst);
        unsafe { ohos_request_notif_perm(on_notif_perm) };
        let outcome = if NOTIF_PERM_RECEIVED.load(Ordering::SeqCst)
            && NOTIF_PERM_BUF.load(Ordering::SeqCst)
        {
            RequestPermissionsOutcome::Accepted
        } else {
            RequestPermissionsOutcome::PermissionsDenied
        };
        enqueue_app_callback(Box::new(move |ctx| on_completion(outcome, ctx)));
    }

    fn send_desktop_notification(
        &self,
        notification_content: UserNotification,
        _window_id: WindowId,
        _on_error: SendNotificationErrorCallback,
    ) {
        let title = notification_content.title().to_owned();
        let body = notification_content.body().to_owned();
        if let (Ok(title_cstr), Ok(body_cstr)) = (CString::new(title), CString::new(body)) {
            unsafe { ohos_send_notification(title_cstr.as_ptr(), body_cstr.as_ptr()) };
        }
    }

    fn set_cursor_shape(&self, _cursor: Cursor) {
        log::info!("OhosDelegate::set_cursor_shape: {_cursor:?}");
    }

    fn close_ime_async(&self, _window_id: WindowId) {
        // IME 与窗口绑定，不随焦点变化 detach（类似 Linux/macOS）。
        // 只更新状态标记，不影响 IME 连接。
        log::info!("OhosDelegate::close_ime_async: ime state unchanged (window-bound IME)");
        self.ime_open.store(false, Ordering::Relaxed);
    }

    fn is_ime_open(&self) -> bool {
        self.ime_open.load(Ordering::Relaxed)
    }

    fn open_character_palette(&self) {
        log::info!("OhosDelegate::open_character_palette");
    }

    fn set_accessibility_contents(&self, _content: AccessibilityContent) {
        log::info!("OhosDelegate::set_accessibility_contents");
    }

    fn register_global_shortcut(&self, _shortcut: Keystroke) {
        log::info!("OhosDelegate::register_global_shortcut: {_shortcut:?}");
    }

    fn unregister_global_shortcut(&self, _shortcut: &Keystroke) {
        log::info!("OhosDelegate::unregister_global_shortcut: {_shortcut:?}");
    }

    fn terminate_app(&self, termination_mode: TerminationMode) {
        match termination_mode {
            TerminationMode::Cancellable => {
                log::info!("OhosDelegate::terminate_app: graceful termination requested, deferring to app");
            }
            TerminationMode::ForceTerminate | TerminationMode::ContentTransferred => {
                unsafe { ohos_terminate_self() };
            }
        }
    }

    fn is_screen_reader_enabled(&self) -> Option<bool> {
        None
    }

    fn microphone_access_state(&self) -> MicrophoneAccessState {
        MicrophoneAccessState::NotDetermined
    }
}

// ── OHOS 特有 IME 方法（不在 Delegate trait 中） ─────────────────────────

/// 唤起软键盘（IME 一直与窗口绑定，不随焦点变化 detach）。
/// 在 EditorView 获焦时由 EditorView::on_focus() 调用。
#[cfg(target_env = "ohos")]
pub fn activate_ime() {
    log::info!("ohos::activate_ime: showing keyboard via WARP_APP");
    if let Some(app) = super::get_warp_app() {
        app.show_keyboard();
    } else {
        log::warn!("ohos::activate_ime: WARP_APP not available");
    }
}

/// 隐藏软键盘（不 detach IME，回调仍然注册）。
#[cfg(target_env = "ohos")]
pub fn deactivate_ime() {
    log::info!("ohos::deactivate_ime: hiding keyboard via WARP_APP");
    if let Some(app) = super::get_warp_app() {
        app.hide_keyboard();
    }
}
