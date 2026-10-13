use openharmony_ability::{RunningLockGuard, acquire_running_lock};

/// A guard object that keeps the screen awake while it remains in scope.
///
/// The lock is a shared count on the ArkTS side, so several guards (e.g. one per
/// in-flight agent request) hold the screen on until the last one is dropped.
pub struct Guard {
    inner: Option<RunningLockGuard>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.inner.is_some() {
            log::info!("No longer preventing sleep");
        }
    }
}

/// Returns a guard that prevents system sleep while it remains in scope.
///
/// The screen-on request is driven through the ability host, which is the only
/// supported path on OHOS (see `openharmony-ability`'s running-lock module). When
/// ArkTS has not registered its running-lock actions yet, the guard degrades to
/// doing nothing and logs why.
pub fn prevent_sleep(reason: &'static str) -> Guard {
    match acquire_running_lock(reason) {
        Ok(inner) => {
            log::info!("Preventing sleep with reason: {reason}");
            Guard { inner: Some(inner) }
        }
        Err(error) => {
            log::warn!("Not preventing sleep: the OHOS running lock is unavailable: {error}");
            Guard { inner: None }
        }
    }
}
