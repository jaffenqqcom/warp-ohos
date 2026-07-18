// §7.1 — 日志重定向：OhosLogger 实现
// §3.10.6 — ohos 分支的条件编译路径

use std::ffi::CStr;
use std::ffi::CString;
use std::os::raw::c_char;

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::{LogConfig, LogDestination};

// ── HiLog NDK FFI ──────────────────────────────────────────────────────────

const LOG_APP: i32 = 0;

const LOG_LEVEL_DEBUG: i32 = 3;
const LOG_LEVEL_INFO: i32 = 4;
const LOG_LEVEL_WARN: i32 = 5;
const LOG_LEVEL_ERROR: i32 = 6;

const WARP_DOMAIN: u32 = 0x0001;
const WARP_TAG: &CStr = c"Warp";
const HILOG_FORMAT: &CStr = c"%{public}s";

#[link(name = "hilog_ndk.z")]
unsafe extern "C" {
    fn OH_LOG_Print(
        log_type: i32,
        log_level: i32,
        domain: u32,
        tag: *const c_char,
        fmt: *const c_char,
        ...
    ) -> i32;
}

// ── OhosLogger ─────────────────────────────────────────────────────────────

struct OhosLogger;

impl Log for OhosLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let hi_log_level = ohos_log_level(record.level());
        let message = record.args().to_string();
        let msg_cstr = match std::ffi::CString::new(message) {
            Ok(cstr) => cstr,
            Err(_) => {
                // CString::new 在字符串含内部空字节时失败，
                // 概率极低，且此时无更合适的日志输出渠道，
                // 直接跳过该条日志。
                return;
            }
        };

        unsafe {
            OH_LOG_Print(
                LOG_APP,
                hi_log_level,
                WARP_DOMAIN,
                WARP_TAG.as_ptr(),
                HILOG_FORMAT.as_ptr(),
                msg_cstr.as_ptr(),
            );
        }
    }

    fn flush(&self) {
        // HiLog 由系统守护进程管理缓冲区，无需 Rust 侧 flush。
    }
}

const fn ohos_log_level(level: Level) -> i32 {
    match level {
        Level::Error => LOG_LEVEL_ERROR,
        Level::Warn => LOG_LEVEL_WARN,
        Level::Info => LOG_LEVEL_INFO,
        Level::Debug | Level::Trace => LOG_LEVEL_DEBUG,
    }
}

// ── 公开 API ───────────────────────────────────────────────────────────────

/// 直接调用 HiLog（不走 log 宏重定向），用于验证 hilog FFI 链接是否正常。
///
/// 与 `log::info!()` 配合使用可区分两类问题：
/// - 本条日志出现、`log::info!()` 不出现 → `log_redirect_2_hilog()` 重定向失败
/// - 本条日志不出现 → hilog NDK 链接或 `OH_LOG_Print` FFI 调用本身有问题
pub fn direct_hilog_info(tag: &str, message: &str) {
    let tag_cstr = CString::new(tag).unwrap_or_else(|_| CString::new("Warp").unwrap());
    let msg_cstr = CString::new(message).unwrap_or_else(|_| CString::new("").unwrap());
    unsafe {
        OH_LOG_Print(
            LOG_APP,
            LOG_LEVEL_INFO,
            WARP_DOMAIN,
            tag_cstr.as_ptr(),
            HILOG_FORMAT.as_ptr(),
            msg_cstr.as_ptr(),
        );
    }
}

pub fn log_redirect_2_hilog() -> Result<(), log::SetLoggerError> {
    // 静默忽略 set_boxed_logger 的错误（如 logger 已注册），使多次调用不 panic。
    let _ = log::set_boxed_logger(Box::new(OhosLogger));
    // 日志级别固定为 Debug，与 native 平台一致。LogConfig 当前无 log_level 字段，
    // 设置更高级别（如 Info）可能丢失 Debug/Trace 级别的诊断日志。
    log::set_max_level(LevelFilter::Debug);
    Ok(())
}

pub fn init(config: LogConfig) -> Result<(), log::SetLoggerError> {
    log_redirect_2_hilog()?;
    // 记录配置信息以便调试时确认日志初始化参数
    log::info!("OhosLogger initialized: is_cli={}, log_destination={:?}", config.is_cli, config.log_destination);
    if matches!(config.log_destination, Some(LogDestination::File)) {
        log::info!("OhosLogger: File destination requested but OHOS uses HiLog, ignoring");
    }
    Ok(())
}
