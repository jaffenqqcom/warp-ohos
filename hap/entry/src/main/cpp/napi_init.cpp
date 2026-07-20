// §2.2.1 — NAPI .so 入口（libentry.so）
//
// libentry.so 提供：
//   1. 系统服务 NAPI 注册（clipboard, file_picker, notification, theme 等）
//   2. 全局 NAPI 环境 g_napi_env / g_exports_ref（供 service_*.cpp 的 C FFI 桥使用）
//   3. Rust → C++ → NAPI → ArkTS 的 C FFI 桥函数（ohos_* 系列）
//
// libwarp.so 的 NAPI 模块注册由 Rust #[napi] / #[ability] 宏自动完成。
// libentry.so 与 libwarp.so 在设备上位于同一目录，通过 RPATH $ORIGIN
// 让 MUSL-LDSO 解析跨 so 符号引用。
//
// CMake 配置：不链接 libwarp.so（无 DT_NEEDED 依赖），避免循环依赖。

#include <cstdlib>
#include <hilog/log.h>
#include <napi/native_api.h>

#include "napi_utils.h"

#undef LOG_DOMAIN
#define LOG_DOMAIN 0x0001
#undef LOG_TAG
#define LOG_TAG "napi_init"

// 全局 NAPI 环境定义
napi_env g_napi_env = nullptr;
napi_ref g_exports_ref = nullptr;
std::mutex g_napi_mutex;

// 系统服务注册函数在 services/ 目录中各独立文件实现
extern "C" void ohos_create_napi_bridge(napi_env);
extern void RegisterClipboard(napi_env env, napi_value exports);
extern void RegisterFilePicker(napi_env env, napi_value exports);
extern void RegisterNotification(napi_env env, napi_value exports);
extern void RegisterTheme(napi_env env, napi_value exports);
extern void RegisterUrl(napi_env env, napi_value exports);
extern void RegisterDisplay(napi_env env, napi_value exports);
extern void RegisterIme(napi_env env, napi_value exports);

// ── 模块注册 ─────────────────────────────────────────────────────────────────

static napi_value Init(napi_env env, napi_value exports) {
    OH_LOG_INFO(LOG_APP, "napi_init: registering NAPI module (Init)");

    // 存储全局 NAPI 环境供 C FFI 桥使用
    g_napi_env = env;
    napi_create_reference(env, exports, 1, &g_exports_ref);

    // 注册系统服务方法
    RegisterClipboard(env, exports);
    RegisterFilePicker(env, exports);
    RegisterNotification(env, exports);
    RegisterTheme(env, exports);
    RegisterUrl(env, exports);
    RegisterDisplay(env, exports);
    RegisterIme(env, exports);
    return exports;
}

// OHOS: NAPI_MODULE(entry, Init) 不会被自动调用。
// 真实入口是 ohos_init_service_bridge（由 Rust register_service_bridge 调用）。

// ── C FFI 桥：Rust → C++（无需 NAPI 的通用函数） ─────────────────────────

extern "C" {

/// 由 Rust 侧调用的系统服务桥初始化函数。
/// 注册剪贴板/主题/通知/文件选择器等 NAPI 方法并设置全局 napi_env，
/// 供 service_*.cpp 的 C FFI 桥（ohos_get_color_mode 等）使用。
void ohos_init_service_bridge(napi_env env, napi_value exports) {
    if (g_napi_env != nullptr) {
        return; // 已经初始化过
    }
    g_napi_env = env;
    napi_create_reference(env, exports, 1, &g_exports_ref);
    RegisterClipboard(env, exports);
    RegisterFilePicker(env, exports);
    RegisterNotification(env, exports);
    RegisterTheme(env, exports);
    RegisterUrl(env, exports);
    RegisterDisplay(env, exports);
    RegisterIme(env, exports);

    // 创建 TSFN 桥接，供后台线程安全调用 NAPI
    ohos_create_napi_bridge(env);

    OH_LOG_INFO(LOG_APP, "napi_init: ohos_init_service_bridge done");
}

void ohos_terminate_self() {
    std::_Exit(0);
}

} // extern "C"
