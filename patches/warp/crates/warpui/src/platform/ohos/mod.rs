// §2.4.1 — 平台模块导出 + init()
#![cfg(target_env = "ohos")]

mod app;
pub use app::App;

pub(crate) mod fonts;

/// 初始化 Ohos 平台后端。
///
/// 在 `app/src/lib.rs` 的 `run()` 函数执行过程中由 `platform::init()` 调用。
/// 注册 panic hook，使 panic 信息输出到 HiLog。
///
/// 幂等性：通过 AtomicBool 开关确保只执行一次，且不依赖 panic hook 链式叠加
/// 语义（每次调用 set_hook 会替换而非叠加前一个 hook）。如需在测试中重新初始化，
/// 需在进程重新启动的场景下进行。
pub fn init() {
    use std::panic;
    static INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let prev_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        log::error!("Warp panicked: {panic_info}");
        prev_hook(panic_info);
    }));
    log::info!("Ohos platform initialized");
}
