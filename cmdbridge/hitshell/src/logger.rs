//! Logging sink for the hitshell bridge.
//!
//! The bridge's standard output carries the remote shell's bytes, so nothing
//! here may ever write to stdout: a record mixed into that stream would appear
//! inside the user's terminal session. On OHOS records go to hilog; on a plain
//! host they go to stderr, where only the local bring-up path looks for them.
//!
//! On OHOS a logger is always installed: the key nodes of a run are recorded
//! even without `--log`, because the device has no console a failure could be
//! read from. `--log` widens it to the debug-level diagnostics. On a plain host
//! `--log` remains what installs a logger at all: there the run has a terminal,
//! and `main` prints the failure that ends it to stderr either way.

/// Installs the process-wide logger. `verbose` is the `--log` flag.
pub fn init(verbose: bool) {
    #[cfg(target_env = "ohos")]
    ohos::init(verbose);
    #[cfg(not(target_env = "ohos"))]
    if verbose {
        stderr_logger::init();
    }
}

#[cfg(target_env = "ohos")]
mod ohos {
    use std::ffi::{CStr, CString, c_char};

    use log::{Level, LevelFilter, Log, Metadata, Record};

    /// LOG_APP: third-party applications always use this log type.
    const LOG_APP: i32 = 0;
    /// hilog LogLevel enum: DEBUG=3 / WARN=5 / ERROR=6. INFO=4 is unused: the
    /// device filters app records below WARN, so nothing is ever recorded at it
    /// (see `hilog_level`).
    const LOG_LEVEL_DEBUG: i32 = 3;
    const LOG_LEVEL_WARN: i32 = 5;
    const LOG_LEVEL_ERROR: i32 = 6;
    /// App service domain, 0x0001 and up are user-defined.
    const HILOG_DOMAIN: u32 = 0x0001;
    /// hilog tag, kept short enough (<31 bytes) to avoid truncation.
    const HILOG_TAG: &CStr = c"Hitshell";
    /// Single `%{public}s` format: the whole message is one public plain-text arg.
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

    /// Maps a `log` level to its hilog LogLevel value.
    ///
    /// Info is recorded as WARN. The device keeps app records below WARN out of
    /// the hilog buffer entirely -- they are not merely hidden on display -- so
    /// a key node of the bridge's normal progress, which is what info carries,
    /// would otherwise leave no trace of a run that hung instead of failing.
    /// Warn therefore does double duty; error keeps its own level, so a real
    /// failure still stands out from the progress around it.
    fn hilog_level(level: Level) -> i32 {
        match level {
            Level::Error => LOG_LEVEL_ERROR,
            Level::Warn | Level::Info => LOG_LEVEL_WARN,
            Level::Debug | Level::Trace => LOG_LEVEL_DEBUG,
        }
    }

    struct HilogLogger;

    impl Log for HilogLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            let message = format!("[hitshell:{}] {}", record.target(), record.args());
            let Ok(message_c) = CString::new(message) else {
                return;
            };
            // SAFETY: message_c is a NUL-terminated C string alive for the call;
            // the variadic argument is consumed by the `%{public}s` format.
            unsafe {
                OH_LOG_Print(
                    LOG_APP,
                    hilog_level(record.level()),
                    HILOG_DOMAIN,
                    HILOG_TAG.as_ptr(),
                    HILOG_FORMAT.as_ptr(),
                    message_c.as_ptr(),
                );
            }
        }

        fn flush(&self) {}
    }

    static LOGGER: HilogLogger = HilogLogger;

    /// Installs the hilog logger as the process-wide `log` logger. `verbose`
    /// (the `--log` flag) widens it from the key nodes of a run to its
    /// debug-level diagnostics.
    pub fn init(verbose: bool) {
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(if verbose {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        });
    }
}

#[cfg(not(target_env = "ohos"))]
mod stderr_logger {
    use log::{LevelFilter, Log, Metadata, Record};

    struct StderrLogger;

    impl Log for StderrLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            eprintln!("hitshell {}: {}", record.level(), record.args());
        }

        fn flush(&self) {}
    }

    /// Installs the stderr logger as the process-wide `log` logger.
    pub fn init() {
        let _ = log::set_logger(&StderrLogger);
        log::set_max_level(LevelFilter::Info);
    }
}
