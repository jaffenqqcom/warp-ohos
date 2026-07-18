// NAPI 工具函数：参数解析 / 值创建 / AbilityContext 缓存 / 全局环境
//
// 设计文档：§2.2.1 — NAPI .so 入口
//
// 全局 NAPI 环境（g_napi_env / g_exports_ref）在 napi_init.cpp 的 Init 中
// 初始化，由各 service_*.cpp 的 ohos_* C FFI 函数使用。
// 所有外部线程（Rust FFI 调用）通过 g_napi_mutex 串行化对 napi_env 的访问。

#pragma once

#include <cinttypes>
#include <hilog/log.h>
#include <mutex>
#include <string>
#include <napi/native_api.h>

/// 日志域和标签（各 service 文件通过 #undef LOG_TAG 覆盖为自己的标签）
#undef LOG_DOMAIN
#define LOG_DOMAIN 0x0001

/// 全局 NAPI 环境，在 napi_init.cpp 的 Init() 中初始化。
/// 各 service_*.cpp 的 C FFI 函数通过此 env 调用注册的 NAPI 方法。
extern napi_env g_napi_env;

/// 模块导出对象的全局引用，通过此引用获取注册的 NAPI 方法。
extern napi_ref g_exports_ref;

/// 串行化外部线程对 napi_env 的访问（Rust FFI 调用线程与 ArkTS 主线程不共享）。
extern std::mutex g_napi_mutex;

/// 从 NAPI 参数中提取 UTF-8 字符串。
inline std::string GetStringFromArg(napi_env env, napi_value value) {
    size_t len = 0;
    napi_get_value_string_utf8(env, value, nullptr, 0, &len);
    std::string result(len, '\0');
    if (len > 0) {
        napi_get_value_string_utf8(env, value, result.data(), len + 1, &len);
    }
    return result;
}

/// 创建 NAPI UTF-8 字符串。
inline napi_value CreateString(napi_env env, const std::string& str) {
    napi_value result;
    napi_create_string_utf8(env, str.c_str(), str.length(), &result);
    return result;
}

/// 创建 NAPI 布尔值。
inline napi_value CreateBool(napi_env env, bool value) {
    napi_value result;
    napi_get_boolean(env, value, &result);
    return result;
}
