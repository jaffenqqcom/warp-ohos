// §2.2.5 — URL 服务
//
// NAPI 方法 openUrl(url)，
// 通过 UIAbilityContext.startAbility() 打开外部 URL。
//
// ArkTS 侧在 aboutToAppear 中调用 setAbilityContext 传入 UIAbilityContext，
// C++ 侧存为 napi_ref，openUrl 时使用该 context 调用 startAbility。

#include <string>
#include <napi/native_api.h>
#include "napi_utils.h"

#undef LOG_TAG
#define LOG_TAG "Url"

/// 全局引用：ArkTS 传入的 UIAbilityContext，供 openUrl 调用 startAbility。
static napi_ref g_ability_context_ref = nullptr;

/// NAPI 方法：接收 ArkTS 传入的 UIAbilityContext，存为持久引用。
static napi_value SetAbilityContext(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);
    if (argc < 1) {
        OH_LOG_ERROR(LOG_APP, "SetAbilityContext: no argument");
        return nullptr;
    }
    if (g_ability_context_ref) {
        napi_delete_reference(env, g_ability_context_ref);
        g_ability_context_ref = nullptr;
    }
    napi_create_reference(env, args[0], 1, &g_ability_context_ref);
    OH_LOG_INFO(LOG_APP, "SetAbilityContext: stored context ref");
    return nullptr;
}

static napi_value OpenUrl(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);
    if (argc < 1) {
        return nullptr;
    }

    std::string url = GetStringFromArg(env, args[0]);

    if (!g_ability_context_ref) {
        OH_LOG_ERROR(LOG_APP, "OpenUrl: ability context not set (setAbilityContext not called)");
        return nullptr;
    }

    napi_value abilityContext;
    napi_get_reference_value(env, g_ability_context_ref, &abilityContext);

    // 构造 Want：{ uri: url, action: 'ohos.want.action.viewData' }
    napi_value want;
    napi_create_object(env, &want);
    napi_value uri = CreateString(env, url);
    napi_set_named_property(env, want, "uri", uri);
    napi_value action = CreateString(env, "ohos.want.action.viewData");
    napi_set_named_property(env, want, "action", action);

    // 调用 abilityContext.startAbility(want)
    napi_value startAbilityFn;
    if (napi_get_named_property(env, abilityContext, "startAbility", &startAbilityFn) != napi_ok) {
        OH_LOG_ERROR(LOG_APP, "OpenUrl: startAbility not found on context");
        return nullptr;
    }

    napi_value result;
    napi_call_function(env, abilityContext, startAbilityFn, 1, &want, &result);
    OH_LOG_INFO(LOG_APP, "OpenUrl: started ability for '%{public}s'", url.c_str());
    return result;
}

void RegisterUrl(napi_env env, napi_value exports) {
    OH_LOG_INFO(LOG_APP, "Url::Register called");
    napi_property_descriptor desc[] = {
        {"openUrl", nullptr, OpenUrl, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"setAbilityContext", nullptr, SetAbilityContext, nullptr, nullptr, nullptr, napi_default, nullptr},
    };
    napi_define_properties(env, exports, sizeof(desc) / sizeof(desc[0]), desc);
}

// ── C FFI 桥：通过 TSFN 安全调用 NAPI ───────────────────────────────────────

extern "C" {
extern void ohos_call_napi_ex(const char* method, const char* arg, char* result, size_t max_len);

void ohos_open_url(const char* url) {
    OH_LOG_INFO(LOG_APP, "Url::ohos_open_url called");
    ohos_call_napi_ex("openUrl", url, nullptr, 0);
}

} // extern "C"

// ── C FFI 桥：用于 Rust 侧通过 dlsym 直接设置 ability context ──────────────

extern "C" {

void ohos_set_ability_context(napi_env env, napi_value context) {
    OH_LOG_INFO(LOG_APP, "Url::ohos_set_ability_context called");
    if (g_ability_context_ref) {
        napi_delete_reference(env, g_ability_context_ref);
        g_ability_context_ref = nullptr;
    }
    napi_create_reference(env, context, 1, &g_ability_context_ref);
}

} // extern "C"
