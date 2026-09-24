//! hilog sink for the OHOS build: mirrors every record the `env_logger`
//! pipeline produces into the platform log service.
//!
//! An OHOS application has no terminal attached, so neither the stderr sink nor
//! the `warp.log` file is reachable while developing on device. `native.rs`
//! builds an `env_logger::Logger` with the level filters and the file sink warp
//! normally installs; [`init_hilog_logger`] takes that logger over and forwards
//! each record it accepts to hilog through `OH_LOG_Print`, so both sinks show
//! exactly the same lines.
//!
//! `log`'s facade does not hand `log::Log::log` a target string, only a
//! reference to the record, so the module path is folded into the message
//! instead and every line carries the same tag.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use log::{Log, Metadata, Record};

/// hilog `LogType`: third-party applications always log as `LOG_APP`.
const LOG_TYPE_APP: i32 = 0;

/// hilog `LogLevel` discriminants, as declared in `hilog/log.h`.
const HILOG_LEVEL_DEBUG: i32 = 3;
const HILOG_LEVEL_INFO: i32 = 4;
const HILOG_LEVEL_WARN: i32 = 5;
const HILOG_LEVEL_ERROR: i32 = 6;

/// Application domain id. hilog requires one to be passed and `0x0001` is the
/// value reserved for third-party use.
const HILOG_DOMAIN: u32 = 0x0001;

/// Tag that groups this application's records in `hilog`. The platform caps a
/// tag at 31 bytes.
const HILOG_TAG: &CStr = c"diag";

/// Format string that passes the whole message through as a public argument.
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

/// Wraps the `env_logger` instance so that every record it accepts also reaches
/// hilog. Level filtering stays with `env_logger` so the file sink and hilog
/// cannot drift apart.
struct HilogLogger {
    inner: env_logger::Logger,
}

impl Log for HilogLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.inner.enabled(record.metadata()) {
            return;
        }
        submit_to_hilog(record);
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Installs the hilog-mirroring logger in place of `env_logger`'s own
/// `Logger::init`.
///
/// Called by `native::init_internal` on OHOS. Builds that enable
/// `crash_reporting` are excluded there, because `sentry_log::SentryLogger`
/// already claims the single global logger slot.
pub(super) fn init_hilog_logger(base_logger: env_logger::Logger) {
    let max_level = base_logger.filter();
    log::set_boxed_logger(Box::new(HilogLogger { inner: base_logger }))
        .expect("Should not have already initialized a logger");
    log::set_max_level(max_level);
    log::info!("warp_logging::ohos: hilog sink installed, tag={HILOG_TAG:?}");
}

/// Writes `message` straight to hilog, bypassing the `log` facade.
///
/// The launch path in `warp_ohos` logs before `warp::run()` installs the
/// logger, and those lines would otherwise be dropped — which makes "the
/// redirect is broken" and "the process never got that far" indistinguishable
/// on device. Pairing a direct write with a nearby `log::` macro tells the two
/// apart: the direct line alone means the redirect is not live yet, and neither
/// line means the hilog link or the FFI itself is broken.
pub fn direct_hilog(message: &str) {
    let message = to_c_string(message.to_owned());
    unsafe {
        OH_LOG_Print(
            LOG_TYPE_APP,
            HILOG_LEVEL_INFO,
            HILOG_DOMAIN,
            HILOG_TAG.as_ptr(),
            HILOG_FORMAT.as_ptr(),
            message.as_ptr(),
        );
    }
}

/// Mirrors one record into hilog.
///
/// The return value of `OH_LOG_Print` is dropped on purpose: hilog is the last
/// channel available, so there is nowhere left to report a failure to.
fn submit_to_hilog(record: &Record) {
    let message = to_c_string(format!("[{}] {}", record.target(), record.args()));
    unsafe {
        OH_LOG_Print(
            LOG_TYPE_APP,
            hilog_level(record.level()),
            HILOG_DOMAIN,
            HILOG_TAG.as_ptr(),
            HILOG_FORMAT.as_ptr(),
            message.as_ptr(),
        );
    }
}

/// Converts a message for `%{public}s`. hilog takes a C string, so interior NUL
/// bytes have to go: they would either truncate the line or make `CString`
/// reject the message outright.
fn to_c_string(message: String) -> CString {
    let bytes = message
        .into_bytes()
        .into_iter()
        .map(|byte| if byte == 0 { b' ' } else { byte })
        .collect::<Vec<u8>>();
    CString::new(bytes).expect("interior NUL bytes were replaced above")
}

/// Maps `log::Level` onto the hilog discriminants, collapsing `Trace` into
/// `DEBUG` because hilog has no finer level.
fn hilog_level(level: log::Level) -> i32 {
    match level {
        log::Level::Error => HILOG_LEVEL_ERROR,
        log::Level::Warn => HILOG_LEVEL_WARN,
        log::Level::Info => HILOG_LEVEL_INFO,
        log::Level::Debug | log::Level::Trace => HILOG_LEVEL_DEBUG,
    }
}
