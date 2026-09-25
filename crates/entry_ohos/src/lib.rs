// The crate only has content for the OHOS target: it is the NAPI entry that the
// HAP loads. Compiling it on other targets would drag in the OpenHarmony ability
// machinery, so the whole crate is compiled out there.
#![cfg(target_env = "ohos")]

mod launch_app;

// Keeps `ohos_libc_shim` linked into the final cdylib: the `__wrap_*` symbols in
// it are where the build script redirects the path-taking libc calls, and no
// Rust code calls into the module directly. Without the declaration here the
// module would not be compiled and every `--wrap` redirection would fail to
// resolve.
mod ohos_libc_shim;

// OHOS libc lacks robust mutexes (pthread_mutexattr_setrobust /
// pthread_mutex_consistent) that std references, so export no-op shims for the
// final cdylib (libcore.so) to resolve those undefined symbols against.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_mutexattr_setrobust(
    _attr: *mut std::ffi::c_void,
    _robustness: std::ffi::c_int,
) -> std::ffi::c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_mutex_consistent(
    _mutex: *mut std::ffi::c_void,
) -> std::ffi::c_int {
    0
}
