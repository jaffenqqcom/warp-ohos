//! HiLog logging implementation for the HarmonyOS (OpenHarmony) platform.
//!
//! Provides [`OhosLogger`], a [`log::Log`] implementation that redirects all
//! `log` crate output to the HiLog NDK API (`OH_LOG_Print`).  This is the
//! ohos counterpart of `native.rs` and `wasm.rs` inside `warp_logging/src/`.
//!
//! # Usage
//!
//! Call [`log_redirect_2_hilog`] at the very start of `hap_start_warp`
//! (before `main()` runs) so every log line after platform initialisation
//! is captured by HiLog:
//!
//! ```ignore
//! let _ = log_redirect_2_hilog();
//! ```
//!
//! # HiLog constants
//!
//! * `LOG_APP` — log type for application-level logs, value `0`
//! * `LOG_LEVEL_DEBUG` — debug severity, value `3`
//! * `LOG_LEVEL_INFO` — info severity, value `4`
//! * `LOG_LEVEL_WARN` — warn severity, value `5`
//! * `LOG_LEVEL_ERROR` — error severity, value `6`
//! * `LOG_LEVEL_FATAL` — fatal severity, value `7` (not emitted by this logger)

use std::ffi::CString;
use std::os::raw::c_char;

use anyhow::Result;
use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::LogConfig;

// ---------------------------------------------------------------------------
// HiLog NDK FFI
// ---------------------------------------------------------------------------

/// Log type for application-level logs.
const LOG_APP: i32 = 0;

/// Log level constants matching `<hilog/log.h>`.
const LOG_LEVEL_DEBUG: i32 = 3;
const LOG_LEVEL_INFO: i32 = 4;
const LOG_LEVEL_WARN: i32 = 5;
const LOG_LEVEL_ERROR: i32 = 6;

/// Fixed HiLog domain identifier for the Warp application (see CLAUDE.md
/// section 1.1.15 — domain: `0x0001`).
const WARP_DOMAIN: u32 = 0x0001;

/// Tag passed to every HiLog call.  Must be 31 bytes or fewer (HiLog
/// truncates silently).
const WARP_TAG: &str = "Warp";

/// Format string: `%{public}s` tells HiLog the argument is public data
/// (not redacted in release builds).
const HILOG_FORMAT: &str = "%{public}s";

extern "C" {
    /// `OH_LOG_Print` — the single-entry HiLog NDK API.
    ///
    /// ```c
    /// int OH_LOG_Print(LogType type, LogLevel level,
    ///                  unsigned int domain, const char *tag,
    ///                  const char *fmt, ...);
    /// ```
    fn OH_LOG_Print(
        log_type: i32,
        log_level: i32,
        domain: u32,
        tag: *const c_char,
        fmt: *const c_char,
        ...
    ) -> i32;
}

// ---------------------------------------------------------------------------
// OhosLogger
// ---------------------------------------------------------------------------

/// A [`Log`] implementation that writes every record to HiLog through the
/// `OH_LOG_Print` NDK call.
///
/// Filtering: [`enabled`](Self::enabled) accepts everything up to `Debug`
/// (inclusive); `Trace`-level messages are suppressed by default.
struct OhosLogger;

impl Log for OhosLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let hi_log_level = ohos_log_level(record.level());
        // Safety: the formatted message is a valid, null-terminated C
        // string, and OH_LOG_Print does not retain the pointer after
        // returning.
        let message = CString::new(format!("{}", record.args()))
            .expect("log message contained an interior null byte");

        let tag = CString::new(WARP_TAG)
            .expect("WARP_TAG contains interior null byte");
        let fmt = CString::new(HILOG_FORMAT)
            .expect("HILOG_FORMAT contains interior null byte");
        let msg_ptr = message.as_ptr();

        unsafe {
            OH_LOG_Print(LOG_APP, hi_log_level, WARP_DOMAIN, tag.as_ptr(), fmt.as_ptr(), msg_ptr);
        }
    }

    fn flush(&self) {
        // HiLog does not maintain an internal buffer; nothing to flush.
    }
}

/// Map a [`log::Level`] to the corresponding HiLog `LogLevel` integer.
///
/// `Trace` is mapped to `LOG_LEVEL_DEBUG` because HiLog does not have a
/// trace-level severity.
const fn ohos_log_level(level: Level) -> i32 {
    match level {
        Level::Error => LOG_LEVEL_ERROR,
        Level::Warn => LOG_LEVEL_WARN,
        Level::Info => LOG_LEVEL_INFO,
        Level::Debug => LOG_LEVEL_DEBUG,
        Level::Trace => LOG_LEVEL_DEBUG,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Install [`OhosLogger`] as the global `log` crate logger.
///
/// Once called, every `log::info!()`, `log::error!()`, etc. call is
/// forwarded to HiLog under the `"Warp"` tag.
///
/// This function should be invoked **once** at the very beginning of
/// `hap_start_warp`, before any Warp code produces log output.
///
/// # Errors
///
/// Returns an error if a global logger has already been installed.
pub fn log_redirect_2_hilog() -> Result<()> {
    log::set_boxed_logger(Box::new(OhosLogger))?;
    log::set_max_level(LevelFilter::Debug);
    Ok(())
}

/// Compatibility wrapper so this module can be used as `imp` via the
/// `pub use imp::init` re-export in `lib.rs`.
///
/// Internally calls [`log_redirect_2_hilog`]; the `_config` parameter is
/// accepted (to match the `fn init(_: LogConfig) -> Result<()>` signature
/// expected by the crate) but ignored because HiLog delegates filtering to
/// the NDK layer and does not use file-based log rotation.
pub fn init(_config: LogConfig) -> Result<()> {
    log_redirect_2_hilog()
}
