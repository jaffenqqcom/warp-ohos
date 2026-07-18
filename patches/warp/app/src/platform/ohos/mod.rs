// §2.5.2 — HAP 生命周期胶水层：模块导出 + init()
#![cfg(target_env = "ohos")]

mod hap_lifecycle;
pub(crate) mod ohos_entry;
pub(crate) mod ohos_shell_bridge;

pub use hap_lifecycle::HapLifecycle;

/// 初始化鸿蒙应用层平台后端。
///
/// 在 `app/src/platform/mod.rs::init()` 中调用。
/// 注意：panic hook 注册在 crates/warpui/src/platform/ohos/mod.rs::init() 中，
/// 本函数仅输出初始化日志和注册生命周期桥接。
pub fn init() {
    log::info!("Ohos app platform initialized");
    // 注册终止等待桥接，供 warpui crate 调用
    warpui::windowing::ohos::set_termination_waiter(Box::new(|| {
        crate::platform::ohos::HapLifecycle::wait_for_termination();
    }));
}
