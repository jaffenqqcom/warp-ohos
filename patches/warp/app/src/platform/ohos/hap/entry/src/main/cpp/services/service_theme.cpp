// §2.2.5 — 主题服务
//
// NAPI 方法 getColorMode()，
// 通过 ArkTS configuration.colorMode 获取系统深色/浅色主题。
//
// C FFI 桥（ohos_get_color_mode）由 Rust 侧通过 extern "C" 调用。
// 由于 ArkJS 禁止跨线程 NAPI 调用，使用 TSFN 桥接（service_tsfn.cpp）
// 将 NAPI 调用转发到 UI 线程执行。

#include <string>
#include <napi/native_api.h>
#include "napi_utils.h"

#undef LOG_TAG
#define LOG_TAG "Theme"

static napi_value GetColorMode(napi_env env, napi_callback_info info) {
    napi_value global;
    napi_get_global(env, &global);

    napi_value config;
    if (napi_get_named_property(env, global, "configuration", &config) != napi_ok) {
        return CreateString(env, "light");
    }

    napi_value colorMode;
    if (napi_get_named_property(env, config, "colorMode", &colorMode) != napi_ok) {
        return CreateString(env, "light");
    }
    return colorMode;
}

void RegisterTheme(napi_env env, napi_value exports) {
    OH_LOG_INFO(LOG_APP, "Theme::Register called");
    napi_property_descriptor desc[] = {
        {"getColorMode", nullptr, GetColorMode, nullptr, nullptr, nullptr, napi_default, nullptr},
    };
    napi_define_properties(env, exports, sizeof(desc) / sizeof(desc[0]), desc);
}

// ── C FFI 桥：通过 TSFN 安全调用 ArkTS ───────────────────────────────────────

/// 在 service_tsfn.cpp 中实现的 TSFN 通用调用函数。
extern "C" void ohos_call_napi(const char* method, char* result, size_t max_len);

extern "C" {

const char* ohos_get_color_mode() {
    static thread_local std::string buf;
    char result[128] = {0};
    ohos_call_napi("getColorMode", result, sizeof(result));
    buf = result;
    return buf.c_str();
}

} // extern "C"
