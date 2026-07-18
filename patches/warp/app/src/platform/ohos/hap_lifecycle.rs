// §3.9 — HAP 生命周期管理
//
// 管理 HAP 生命周期状态。提供 on_foreground() 和 on_background() 方法
// 记录前后台状态转换；on_destroy() 触发 Warp 主程序完整退出。
//
// 调用路径：
//   ArkTS onForeground → NAPI .so onAppForeground() → C FFI ohos_on_foreground()
//   → Rust HapLifecycle::on_foreground() → 更新状态
//
//   ArkTS onDestroy → NAPI .so onAppDestroy() → C FFI ohos_on_destroy()
//   → Rust HapLifecycle::on_destroy() → 设置终止标志 → 关闭事件队列

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

/// 全局终止标志位，由 on_destroy 设置，事件循环在每次迭代后检查。
static TERMINATE_FLAG: AtomicBool = AtomicBool::new(false);

/// 终止互斥锁和条件变量，用于 wait_for_termination 阻塞等待。
static TERMINATION_MUTEX: Mutex<()> = Mutex::new(());
static TERMINATION_CONDVAR: Condvar = Condvar::new();

/// HAP 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Created,
    Foreground,
    Background,
    Destroyed,
}

static LIFECYCLE_STATE: Mutex<LifecycleState> = Mutex::new(LifecycleState::Created);

/// HAP 生命周期状态管理。
pub struct HapLifecycle;

impl HapLifecycle {
    /// 处理 onForeground 事件。
    /// 注意：Surface 可见性由 NDK OnSurfaceShow 回调驱动渲染恢复，
    /// 本方法仅更新状态值，不直接驱动渲染。
    pub fn on_foreground() {
        log::info!("HapLifecycle: on_foreground");
        if let Ok(mut state) = LIFECYCLE_STATE.lock() {
            *state = LifecycleState::Foreground;
        }
    }

    /// 处理 onBackground 事件。
    /// 注意：Surface 暂停由 NDK OnSurfaceHide 回调驱动，
    /// 本方法仅更新状态值，不直接暂停渲染。
    pub fn on_background() {
        log::info!("HapLifecycle: on_background");
        if let Ok(mut state) = LIFECYCLE_STATE.lock() {
            *state = LifecycleState::Background;
        }
    }

    /// 处理 Ability 销毁事件，设置终止标志位并通知等待线程。
    pub fn on_destroy() {
        log::info!("HapLifecycle: on_destroy");
        TERMINATE_FLAG.store(true, Ordering::SeqCst);
        if let Ok(mut state) = LIFECYCLE_STATE.lock() {
            *state = LifecycleState::Destroyed;
        }
        // 通知 wait_for_termination 中等待的线程
        TERMINATION_CONDVAR.notify_all();
    }

    /// 检查 Warp 是否应终止。
    pub fn is_terminating() -> bool {
        TERMINATE_FLAG.load(Ordering::Relaxed)
    }

    /// 阻塞等待终止信号。
    /// 在 winit EventLoop::run 返回后调用，作为防止时序窗口的兜底机制。
    /// 当 on_destroy 设置 TERMINATE_FLAG 并通知 Condvar 时返回。
    pub fn wait_for_termination() {
        let mut guard = TERMINATION_MUTEX.lock().unwrap();
        while !TERMINATE_FLAG.load(Ordering::SeqCst) {
            guard = TERMINATION_CONDVAR.wait(guard).unwrap();
        }
    }

    /// 读取当前生命周期状态。
    pub fn current_state() -> LifecycleState {
        LIFECYCLE_STATE.lock().map(|s| *s).unwrap_or(LifecycleState::Created)
    }
}
