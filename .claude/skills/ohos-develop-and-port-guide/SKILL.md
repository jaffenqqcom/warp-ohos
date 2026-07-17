---
name: ohos-develop-and-port-guide
description: Use when building, debugging, or porting code to OHOS (OpenHarmony/HarmonyOS NEXT). Covers rendering pipeline (wgpu/vsync/scene build), NAPI module registration (IME/TSFN), shared library loading, FFI symbol resolution, process/child-process creation, logging, font loading, and OHOS-specific sandbox restrictions.
---

# OHOS 代码开发及移植经验总结

## Overview

OHOS (OpenHarmony/HarmonyOS NEXT) 使用 MUSL-LDSO 动态链接器，应用沙箱有严格的安全策略（namespace 隔离 + SELinux）。这些限制导致标准 Linux 上不会出现的问题。本 skill 收录已知的故障模式及解决经验。

## Migration: OhosApplicationHandler → Standard EventLoop

### 架构对比

旧架构使用自建的 `OhosApplicationHandler` 和 `OhosWindowManager`，但标准层的 `downcast_window` 期望 `windowing::winit::Window` 类型，转型失败导致所有窗口事件被丢弃。

```
旧架构（已废弃）:
  init_ability → EventLoop::run_app(&mut OhosApplicationHandler)
    → OhosWindowManager → OhosWindow
    × downcast_window panic（类型不匹配）

新架构（当前）:
  init_ability → EventLoop::<CustomEvent>::with_user_event()
    → event_loop.run(闭包) → WinitEventLoop::handle_event()
    → OhosWindowManager → StandardWindow
    → redraw_window() → Window::render()
    ✓ downcast_window 成功（StandardWindow）
```

### 关键迁移步骤

**1. winit::Window 携带真实 native_window**
```rust
pub struct Window {
    id: WindowId,
    native_window: OpenHarmonyApp,
}
```
`raw_window_handle_rwh_06()` 返回真实 `OH_NativeWindow*` 而非 dangling。

**2. 缓存 surface 尺寸**
`CACHED_SURFACE_SIZE`（AtomicU64）避免每帧查询 XComponent 导致 surface 重配。

**3. CustomEvent 通过 UserPayload 传输**
`OhosEvent::UserPayload(Box<dyn Any + Send>)` — 替代被丢弃的 `let _ = event;`。

**4. StandardWindow 替换 OhosWindow**
`open_window()` 创建 `windowing::winit::Window`（`downcast_window` 转型目标）。

**5. request_redraw() 触发 PENDING_REDRAW**
原为空函数，改为设置 AtomicBool + 发送 UserEvent 唤醒 EventLoop。vsync 不再无条件设置 PENDING_REDRAW。

**6. 修饰键手动跟踪**
OHOS 不产生 `ModifiersChanged`，在 `convert_window_event` 的修饰键检查中手动更新 `window_state.modifiers`。

**7. Tab/Enter 设置 text 字段**
避免被标准层的修饰键检查误判为修饰键：`Tab → "\t"`, `Enter → "\r"`。

**8. 符号键 keycode 映射**
在 `keycodes.rs` 中添加 `[]\;',./` `` ` `` `-` `=` 的 `to_physical_key`/`to_logical` 映射。

**9. 鼠标右键按钮映射**
ArkTS `MouseButton.Right` 值是 2，不是 1。NAPI `on_mouse_event` 需用 `2→Right, 1→Middle`。

---

## Common Failure Patterns

### 1. `extern "C"` FFI Symbol Not Found

**Symptom:** MUSL-LDSO logs `relocating failed: symbol not found` for a symbol defined in another .so.

**Root cause:** Rust `extern "C"` declarations create undefined symbol references in the final ELF. The providing .so either lacks a `DT_NEEDED` entry or is isolated by namespace policy.

**Resolution (priority order):**

| Approach | When | How |
|----------|------|-----|
| `#[link]` + RPATH | Providing .so is at a known path and namespace allows | Add `#[link(name = "libname")]` on the extern block. In `build.rs`, add `cargo:rustc-link-arg=-Wl,-rpath,<path>` |
| `dlsym(RTLD_DEFAULT)` | Symbol exists in an already-loaded .so but namespace hides it | `dlopen("libname.so")` + `dlsym(handle, "fn_name")` at runtime. Cache with `OnceLock`. |
| Thread for fork | Sandbox blocks `fork()`/`exec()` | Replace `Command::new().spawn()` with `std::thread::spawn()` |

### 2. NDK Stub Libraries Have Alias Names

NDK sysroot 中的 stub .so 为所有函数生成同名别名，编译通过。但设备上的真实库只导出标准名。

**Known mismatches:**
- 使用 `OH_NativeWindow_GetBufferHandleFromNative`（不是 `_GetBufferHandle`）

**Verification:**
```bash
aarch64-unknown-linux-ohos-nm -D /path/to/ndk/sysroot/.../libnative_window.so
```

### 3. Namespace Isolation Blocking RPATH

**Symptom:** RPATH works intermittently. MUSL-LDSO logs `check ns accessible failed`.

**Root cause:** OHOS namespace policies restrict which paths the dynamic linker may search.

**Resolution:** Use `dlsym` runtime lookup instead of compile-time DT_NEEDED + RPATH.

### 4. `NAPI_MODULE(entry, Init)` Not Called

**Symptom:** `g_napi_env` is never set even though `NAPI_MODULE(entry, Init)` is defined.

**Root cause:** OHOS does not automatically trigger `NAPI_MODULE` init functions for the entry module.

**Resolution:**
- Add a C FFI function in `napi_init.cpp` (e.g., `ohos_init_service_bridge`)
- Call it from Rust via `dlopen("libentry.so")` + `dlsym`

### 5. Child Process Creation Blocked

**Symptom:** `Command::new().spawn()` returns `EPERM` (os error 1).

**Root cause:** OHOS sandbox blocks `fork()`/`exec()`/`posix_spawn()`.

**Resolution:** `TerminalServer`: use `std::thread` instead of child process. `socketpair()` and `dup2()` permitted.

### 6. File Logger Initialization Fails

**Symptom:** OHOS returns `EISDIR` or `EPERM` from `warp_logging::native::init()`.

**Root cause:** `native.rs` tries to create log files. OHOS logs flow through OhosLogger → hilog.

**Resolution:** Skip file-based logging init:
```rust
fn init_internal(...) -> Result<()> {
    #[cfg(target_env = "ohos")]
    { return Ok(()); }
}
```

### 7. `rust-embed` Assets Not Found at Runtime (Debug Build)

**Symptom:** `ASSETS.get(path)` returns `Err("no asset exists at path ...")` even though the file exists in the source tree. Occurs only in debug builds on device.

**Root cause:** `rust-embed` in debug mode reads files from the filesystem at runtime (via `std::fs::read`), not from embedded binary data. In cross-compilation scenarios (OHOS), the files exist on the build machine but NOT on the device.

**Other platforms:** macOS/Linux debug builds work because they run on the same machine where the source files exist. WASM needs `debug-embed` because browsers have no filesystem.

**Resolution in `crates/warp_assets/Cargo.toml`:**
```toml
# Before: only WASM gets debug-embed
[target.'cfg(target_family = "wasm")'.dependencies]
rust-embed = { workspace = true, features = ["debug-embed"] }

# After: OHOS also needs embedded assets (cross-compilation)
[target.'cfg(any(target_family = "wasm", target_env = "ohos"))'.dependencies]
rust-embed = { workspace = true, features = ["debug-embed"] }
```

Release builds always embed data, so `--release` also works around this issue.

### 8. `dlsym(RTLD_DEFAULT)` Can't Find Symbols from `dlopen`'d Library

**Symptom:** `dlsym(RTLD_DEFAULT, "ohos_xxx")` returns NULL for functions defined in a library loaded via `dlopen`. The library was loaded successfully (confirmed via `dlsym` for a different function from the same library).

**Root cause:** The library was loaded with `RTLD_LOCAL` (the default for `dlopen`), which makes its symbols invisible to `dlsym(RTLD_DEFAULT, ...)`.

**Resolution:** Use `RTLD_GLOBAL` when calling `dlopen`:
```rust
// Before: symbols hidden from RTLD_DEFAULT
let lib = libc::dlopen(b"libentry.so\0".as_ptr().cast(), libc::RTLD_LAZY | libc::RTLD_LOCAL);

// After: symbols visible to RTLD_DEFAULT
let lib = libc::dlopen(b"libentry.so\0".as_ptr().cast(), libc::RTLD_LAZY | libc::RTLD_GLOBAL);
```

`dlopen` for an already-loaded library just increments the reference count. If the library was first loaded with `RTLD_LOCAL`, calling `dlopen` again with `RTLD_GLOBAL` promotes the symbols to global visibility.

### 9. `napi_call_function` From Background Thread → `SIGABRT` (ecma_vm cannot run in multi-thread)

**Symptom:** Process crashes with `SIGABRT` when calling NAPI functions (`napi_call_function`, `napi_get_reference_value`, etc.) from a background thread. The crash message is:
```
[CheckThread] Fatal: ecma_vm cannot run in multi-thread! thread:<UI>TID currentThread:<warp-main>TID
```

Stack trace shows `EcmaVM::CheckThread()` → `abort`.

**Root cause:** OHOS's ArkJS runtime (based on Huawei Panda EcmaScript engine) performs thread-safety checks. The `napi_env` is associated with the thread that created it (the ArkTS UI thread). Using it from any other thread triggers a fatal abort.

**This is by design:** N-API/ArkJS on OHOS is NOT thread-safe. All NAPI calls MUST happen on the UI thread.

**Why other platforms don't have this issue:** On macOS/Linux, Warp runs as a native desktop app where the UI event loop is on the main thread and NAPI is not used (winit handles windowing directly on the main thread).

**Resolution:** Use `napi_create_threadsafe_function` (TSFN) to safely bridge between background threads and the UI thread. This is the standard N-API mechanism for cross-thread calls.

**Implementation pattern:**

```cpp
// 1. Create TSFN during module Init (UI thread, safe)
#include <napi/native_api.h>
#include <condition_variable>
#include <mutex>

struct NapiCallRequest {
    char method[256];
    char result[4096];
    std::mutex mtx;
    std::condition_variable cv;
    bool done = false;
};

static napi_threadsafe_function g_tsfn = nullptr;

static void TSFNCallJS(napi_env env, napi_value js_cb, void* context, void* data) {
    auto* req = static_cast<NapiCallRequest*>(data);
    // Runs on UI thread — NAPI calls are safe here
    napi_value exports;
    napi_get_reference_value(env, g_exports_ref, &exports);
    napi_value fn;
    napi_get_named_property(env, exports, req->method, &fn);
    napi_value result;
    napi_call_function(env, exports, fn, 0, nullptr, &result);
    // ... convert result to string, store in req->result ...
    std::lock_guard<std::mutex> lock(req->mtx);
    req->done = true;
    req->cv.notify_one();
}

void ohos_create_napi_bridge(napi_env env) {
    napi_value name;
    napi_create_string_utf8(env, "WarpNapiBridge", NAPI_AUTO_LENGTH, &name);
    napi_create_threadsafe_function(
        env, nullptr, nullptr, name,
        0, 1, nullptr, nullptr, nullptr,
        TSFNCallJS, &g_tsfn
    );
    napi_unref_threadsafe_function(env, g_tsfn); // Don't prevent process exit
}

// 2. Call from any thread (background thread safe)
void ohos_call_napi(const char* method, char* result, size_t max_len) {
    NapiCallRequest req;
    strncpy(req.method, method, sizeof(req.method) - 1);
    napi_call_threadsafe_function(g_tsfn, &req, napi_tsfn_blocking);
    std::unique_lock<std::mutex> lock(req.mtx);
    req.cv.wait(lock, [&req] { return req.done; });
    strncpy(result, req.result, max_len - 1);
    result[max_len - 1] = '\0';
}
```

**Key points:**
- TSFN must be created on the UI thread (in `Init()` or `NAPI_MODULE`)
- TSFN callback (`TSFNCallJS`) runs on the UI thread's event loop
- The calling thread blocks on a condition variable until the UI thread completes
- One TSFN can serve ALL `ohos_*` functions — pass the method name as a parameter
- `napi_unref_threadsafe_function` prevents the TSFN from keeping the process alive

**Thread model (unchanged — still 2 threads):**
```
UI Thread (ArkTS main) ← TSFN bridge → warp-main Thread (Rust)
     ↑ napi_call_function safe           ↓ ohos_call_napi("getColorMode", ...)
     ↓ (event loop processes request)    ↑ (blocks on cv.wait)
```

### 10. Remote Build: C++ Source Files Not Synced

**Symptom:** The `build-vm.sh` Rust compilation succeeds, but the remote HAP build fails with linker errors for newly added C++ files or undefined symbols from modified C++ code.

**Root cause:** The remote build machine at `172.16.100.1:8022` has its own local copy of the workspace at `/storage/Users/currentUser/workspace/warp-winit/`. `build-vm.sh` only copies `libwarp.so` to the shared filesystem; it does NOT sync C++ source files.

**Resolution:** Manually copy C++ source files to the remote machine before building:
```bash
scp -P 8022 <local_cpp_file> 172.16.100.1:/storage/Users/currentUser/workspace/warp-winit/<relative_path>
```

For new files, also update `CMakeLists.txt` on the remote machine.

### 11. 文字渲染失败：cosmic_text 找不到打包字体 + owned_ttf_parser 像素格式不匹配

**症状：** 屏幕有背景色和图片（如 Warp logo），但没有任何文字显示。或者渲染出色块但看不到实际字符形状。

**根因有三层，需逐一排查：**

**第 1 层：cosmic_text 和 OhosFontDB 使用独立的 fontdb 数据库**

OhosFontDB 有两个独立的 `fontdb::Database`：
- db_a（`OhosFontDB.db`）：包含打包字体（Hack、Roboto）和系统字体
- db_b（`cosmic_text::FontSystem.db`）：**只包含系统字体**，没有打包字体

当 `layout_text()` 指定 `Family::Name("Hack")` 时，cosmic_text 从 db_b 找不到 Hack，回退到系统字体。

**修复：** 在 `OhosFontDB` 新增 `extra_font_data` 字段，`load_from_bytes()` 时保存字体数据副本，`with_font_system()` 初始化时也加载这些数据。

```rust
// with_font_system() 初始化时加载打包字体
let extra_guard = self.extra_font_data.lock().unwrap_or_else(ignore_poison);
for (family_name, data_vec) in extra_guard.iter() {
    for data in data_vec {
        db.load_font_source(Source::Binary(Arc::new(data.clone())));
    }
}
```

如果 FontSystem 已初始化后才加载新字体，需「热注入」：

```rust
// load_from_bytes() 末尾
drop(db); // 先释放 db 锁
if let Ok(mut fs_guard) = self.font_system.lock() {
    if let Some(ref mut fs) = *fs_guard {
        let cosmic_db = fs.db_mut();
        if let Ok(extra) = self.extra_font_data.lock() {
            if let Some((_, data_vec)) = extra.last() {
                for data in data_vec {
                    cosmic_db.load_font_source(Source::Binary(Arc::new(data.clone())));
                }
            }
        }
    }
}
```

**第 2 层：`owned_ttf_parser` 无法提取某些字体的轮廓**

某些 OHOS 系统字体（如彩色字体/COLR）能被 `TtfFace::parse()` 成功解析 cmap（可以查字形索引），但 `glyph_bounding_box()` 和 `outline_glyph()` 返回 None。

**当前策略：** 不预过滤这些字体，让 `rasterize_glyph()` 在无法提取轮廓时优雅降级（返回空 glyph），WGSL 着色器跳过 `canvas.size() == 0` 的条目。这样可以保留 cmap 查询能力，不影响字体回退匹配。过早过时采取预过滤 (is_font_usable) 会丢失这些字体对其他字符的匹配能力。

**第 3 层（最隐蔽）：`rasterize_glyph()` 像素格式与着色器不匹配**

WGSL glyph 着色器用红色通道做对比增强：
```wgsl
let contrasted = enhance_contrast(tex_color.r, k);
```

但 OhosFontDB 的 `rasterize_glyph()` 输出 `[255, 255, 255, a]` —— R 通道始终为 255。着色器看到 R=1.0 认为 100% 覆盖，渲染为实心色块。

**修复：** 将覆盖率放入 R 通道：
```rust
// Before: 渲染为实心色块
let rgba: Vec<u8> = pixels.iter().flat_map(|&a| vec![255u8, 255u8, 255u8, a]).collect();

// After: 正常显示字符形状
let rgba: Vec<u8> = pixels.iter().flat_map(|&a| vec![a, a, a, a]).collect();
```

**验证方法：** 在 `render_current_window()` 中注入测试字符到新独立层：

```rust
use warpui_core::scene::ClipBounds;
let scene_mut = Rc::make_mut(&mut scene);
scene_mut.start_layer(ClipBounds::None);
scene_mut.draw_glyph(Vector2F::new(100.0, 200.0), glyph_id, font_id, 96.0, ColorU::new(255, 255, 255, 255));
```

**注意：** 测试字符必须放在新层（`start_layer`），否则场景中其他失败字形（如空格）会导致 glyph pipeline 在第一个 Err 时 `return None` 跳过整个层。

**涉及修改的文件：**
- `crates/warpui/src/platform/ohos/fonts.rs` — 三处修复
- `crates/warpui/src/windowing/ohos/ohos_window_manager.rs` — 测试代码（调试完后清理）

### 第 4 层（基线对齐）：`glyph_raster_bounds()` 缺少 TTF→Screen 坐标系翻转

**症状：** 文字能显示，但同一单词内不同字母顶部对齐而非底部对齐。例如单词 "Shell" 中 'e' 的顶部与 'h'、'l' 对齐，'e' 底部悬空。CFF 和 TTF 字体都受影响。

**根因：** `owned_ttf_parser` 返回 TTF 坐标（y-up），其中 y_min=glyph 底部（基线处）、y_max=glyph 顶部（基线上方）。`glyph_raster_bounds()` 计算 bitmap 偏移时直接用 `y_min * em_scale`（对大多数无下伸字母 y_min=0），结果 bitmap 左上角落在基线位置。

但 bitmap 第 0 行存储的是 glyph 顶部（y_max），因此 glyph 顶部位于基线处，底部向下延伸——所有字母顶部对齐。

```
TTF 坐标 (y-up)：                当前 (错误)                      修复后 (正确)
   y_max ─────   (顶部)       ┌─ baseline ─                   ┌─ baseline - y_max*em_scale
     │                         │  bitmap 第0行=顶部             │  bitmap 第0行=顶部
     │   glyph                 │  (显示在基线处)                 │  (显示在基线上方)
   y_min ─────   (底部=基线)   │  bitmap 最后行=底部             │
                               └─ baseline + height ─         ┌─ baseline (底部落在基线)
                                (顶部在基线，底部向下延伸)       │  bitmap 最后行=底部
                                                               └─ baseline + |y_min|*em_scale
```

**修复：** 用 `y_max`（origin + size）而非 `y_min` 计算 bitmap 的 y 偏移：

```rust
// Before (错误): 顶部对齐
let scaled_origin = vec2i(
    (typo.origin().x() * em_scale).floor() as i32,
    (typo.origin().y() * em_scale).floor() as i32,  // y_min * em_scale → 顶部在基线
);

// After (正确): 基线对齐
// y_max = typo.origin().y() + typo.size().y()
// screen_y = baseline_y - y_max * em_scale
let scaled_origin = vec2i(
    (typo.origin().x() * em_scale).floor() as i32,
    (-(typo.origin().y() + typo.size().y()) * em_scale).floor() as i32,
);
```

**涉及文件：** `crates/warpui/src/platform/ohos/fonts.rs` — `glyph_raster_bounds()` 函数

**验证方法：** 部署后检查 "Shell" 等包含不同高度字母的单词：'e' 底部应和其他字符平齐，不再悬空。

---

## 12. 输入事件适配（键盘/鼠标/触控板/触摸屏）

### 架构概览

输入事件统一走标准 WinitEventLoop::handle_event() -> convert_window_event()。

旧架构的 OhosApplicationHandler 自建转换全部移除。

### 关键适配点

| 适配点 | 原因 | 处理 |
|--------|------|------|
| 修饰键状态 | OHOS 无 ModifiersChanged | window_state.modifiers.set() |
| Tab/Enter 被拦截 | text=None 被当修饰键 | text: \t/\r |
| 符号键不识别 | keycodes.rs 缺映射 | 添加 [];',./ - = 映射 |
| 鼠标右键值错 | ArkTS Right=2 映射成 Middle | 2->Right, 1->Middle |
| downcast_window panic | OhosWindow 类型不匹配 | 改用 StandardWindow |

### 事件处理对照

| 事件 | 旧方式 | 新方式（标准层） |
|------|-------|----------------|
| KeyboardInput | handle_key_event() 自建 | convert_keyboard_input_event() |
| 修饰键 | update_modifier_state | ModifierKeyChanged + modifiers.set() |
| Touch | 自建转鼠标 | convert_touch_started/moved/ended |
| IME Enter | -> KeyDown(enter) | -> TypedCharacters(\n) |
| IME Backspace | -> KeyDown(backspace) | -> SetMarkedText("") |


## 13. Shell 执行与输出渲染 — OHOS toybox sh 适配

### 问题

OHOS 设备只有 toybox `sh`，不支持：
- `PROMPT_COMMAND` / `DEBUG trap`（bash 特有）
- `precmd_functions` / `preexec_functions`（zsh 特有）
- `stty` 命令（设备上未安装）

Warp 的正常 shell 集成依赖三个 DCS 序列：**Preexec**（命令开始）、**CommandFinished**（命令完成、创建新 block）、**Precmd**（设置新 block 元数据）。这些都由 bash/zsh 的钩子机制触发。toybox sh 无法提供这些钩子。

### 方案

**核心思路**：用 Rust 侧逻辑替代 shell 钩子，不依赖 shell 发送 Preexec/CommandFinished。

| 替代来源 | 作用 |
|---------|------|
| `write_command()` 调用时 | 替代 Preexec——Rust 内部知道命令已写入 |
| PS1 中的 `printf` | 替代 Precmd——每次提示符前输出 Precmd DCS |
| 下次 `write_command()` 触发 | 替代 CommandFinished——auto-finalize 上一个 block |

### 需要处理的要点

**1. Bootstrap**：toybox sh 无法执行 bash 专用 bootstrap 脚本（bash_body.sh，1353 行）。改为在 `sh_init.sh` 中用 `printf` 发送合成 DCS 序列（InitShell + CommandFinished + Precmd + Bootstrapped），完成 session 创建和标记。

**2. 每次提示符发 Precmd DCS**：通过 `export PS1='$(printf "\033P$d%s\033\\" "$PRECMD_HEX")$ '`，每次 shell 显示提示符时自动输出 Precmd DCS。这是通知 Rust 侧"上一个命令已结束"的关键信号。

**3. Kill_buffer 前缀**：bash 使用 `\x10`（Ctrl-P）清除行编辑器缓冲区，toybox sh 不支持，需跳过。

**4. Auto-finalize block**：将 auto-finalize 逻辑放在 `start_active_block()` 中（每次 `write_command()` 调用时触发）。检测 block 状态是否为 `BeforeExecution`——Precmd 会重置 state 为 `BeforeExecution`，所以每个命令都能触发。不要放在 `precmd()` 中，否则每次 PS1 都会创建空 block。

**5. Generator 命令**：`warp_run_generator_command` 是 bash_body.sh 中定义的函数，toybox sh 没有。跳过所有 in-band generator 命令的发送。在 `sh_init.sh` 中保留 `warp_run_generator_command() { :; }` 桩函数作为兜底。

**6. LineEditorStatus 激活**：`can_write_to_pty()` 依赖 `LineEditorStatus::is_active()`。在非 zsh 模式下，Precmd ModelEvent 到达后 50ms 自动激活 line editor，使后续命令能被写入 PTY。

**7. `stty` 不存在**：设备上没有 `stty` 命令，不要调用任何 `stty` 操作。

### 开发者注意事项

- 所有非 ohos 文件的修改必须用 `#[cfg(target_env = "ohos")]` 包裹，不影响其他平台
- 不修改已有非 OHOS 适配代码，只能新增 cfg 分支
- 设备只有 toybox sh，不要使用 bash/zsh 语法
- generator 命令的 echo 是启动时一次性现象，可接受



## 14. OHOS 系统库链接与命名空间限制

### 问题

OHOS 使用 MUSL-LDSO，应用沙箱有 namespace 隔离策略。直接链接某些系统库导致启动崩溃。

### 已知可安全链接的库

```cmake
target_link_libraries(entry PUBLIC
    ace_napi.z           # NAPI 运行时
    ace_ndk.z            # NDK 基础库
    libhilog_ndk.z.so    # 日志
    libchild_process.so  # 子进程创建
    libpasteboard.so     # 剪贴板
    libudmf.so           # UDMF 数据管理
)
```

### 需用 dlopen 加载的库

libnative_drawing.so、libnative_window.so 等图形库受命名空间限制，必须用 dlopen：

```cpp
void* h = dlopen("libnative_drawing.so", RTLD_NOW | RTLD_GLOBAL);
if (!h) return -1;
auto* fn = dlsym(h, "OH_Drawing_FunctionName");
```

### Rust 侧调用

```rust
type NativeFn = unsafe extern "C" fn(...) -> i32;
static FN: OnceLock<Option<NativeFn>> = OnceLock::new();
let f = FN.get_or_init(|| unsafe {
    let ptr = dlsym(RTLD_DEFAULT, cname.as_ptr().cast());
    if ptr.is_null() { None } else { Some(std::mem::transmute(ptr)) }
});
```

---

## 15. OHOS Drawing API 字体渲染

### API 总览（libnative_drawing.so，基于 Skia）

| API | 用途 | API Level |
|-----|------|-----------|
| OH_Drawing_MemoryStreamCreate | 内存数据流 | 12 |
| OH_Drawing_TypefaceCreateFromStream | 从数据流创建字体 | 12 |
| OH_Drawing_FontCreate / FontSetHinting / FontSetSubpixel | 创建字体、hinting、子像素 AA | 11/12 |
| OH_Drawing_BitmapCreate / BitmapBuild / BitmapGetPixels | 离屏位图 + 像素读回 | 8 |
| OH_Drawing_CanvasCreate / CanvasBind / CanvasDrawTextBlob | Canvas 绘制 | 11 |
| OH_Drawing_TextBlobBuilderCreate / AllocRunPos / Make | 单字形 TextBlob | 11 |

### 单字形光栅化流程

```cpp
auto* stream = OH_Drawing_MemoryStreamCreate(font_data, len, true);
auto* typeface = OH_Drawing_TypefaceCreateFromStream(stream, 0);
auto* font = OH_Drawing_FontCreate();
OH_Drawing_FontSetTypeface(font, typeface);
OH_Drawing_FontSetTextSize(font, size_pt);
OH_Drawing_FontSetHinting(font, FONT_HINTING_NORMAL);
OH_Drawing_FontSetSubpixel(font, true);

auto* bmp = OH_Drawing_BitmapCreate();
OH_Drawing_BitmapBuild(bmp, w, h, nullptr);
auto* cv = OH_Drawing_CanvasCreate();
OH_Drawing_CanvasBind(cv, bmp);
OH_Drawing_CanvasClear(cv, 0);

auto* bld = OH_Drawing_TextBlobBuilderCreate();
auto* rb = OH_Drawing_TextBlobBuilderAllocRunPos(bld, font, 1, nullptr);
const_cast<uint16_t*>(rb->glyphs)[0] = gid;
const_cast<float*>(rb->pos)[0] = 0;
const_cast<float*>(rb->pos)[1] = fs * 0.9f;
auto* tb = OH_Drawing_TextBlobBuilderMake(bld);
OH_Drawing_CanvasDrawTextBlob(cv, tb, 0, 0);

void* pixels = OH_Drawing_BitmapGetPixels(bmp);
```

### RunBuffer 结构（必须精确，4 指针 = 32 字节）

```cpp
typedef struct {
    uint16_t* glyphs;   // 字形索引
    float* pos;         // 位置 (x0,y0,...)
    char* utf8text;     // UTF-8 文本
    uint32_t* clusters; // 字形簇索引
} OH_Drawing_RunBuffer;
```

自定义 struct 字段数必须完全匹配，内存越界导致 SIGABRT。

---

## 16. 字体回退（Font Fallback）运行时学习 + MRU 缓存

### 挑战

OHOS 有 258 个系统字体（791 face，229 个去重族），但无 fontconfig。原始方案每字符遍历全部字体（`get_font_data` 克隆 ~500KB/font × 229）耗时 6-18 秒。

### 当前方案：运行时学习 MRU 缓存

**核心思路：** 不在初始化时全量扫描 cmap（省 660ms），遇到新 Unicode 区块时沿优先级列表找到第一个匹配字体就停，缓存该字体到区块的 MRU（Most Recently Used）列表中。同区块后续字符优先检查最近匹配的字体，连续字符极高概率在同一字体中命中。

#### 数据结构

```rust
struct OhosFontDB {
    /// 已发现的字体列表（MRU 顺序）：block_number → [最近匹配...最早发现]
    block_cache: Mutex<HashMap<u8, Vec<FontId>>>,
    /// 按优先级排序的去重字体列表：mono > hack > code > sans > hei > deng > cjk > alpha
    sorted_fonts: OnceLock<Vec<FontId>>,
    /// 字体数据缓存：原始数据 &[u8] 引用（不克隆）
    font_data_cache: Mutex<HashMap<PathBuf, Vec<u8>>>,
}
```

#### Fallback 匹配流程

```
fallback_fonts(ch, primary_font_id):
  1. block = (ch.码点 >> 8) & 0xFF          // Unicode 区块号
  2. 查 block_cache:
     a. 有缓存列表 → 按 MRU 顺序检查每个字体
        → 某字体有该字符 → 提升到队首，返回 [该字体]
        → 全部没有 → 跳到步骤 3
     b. 无缓存 → 跳过步骤 2，到步骤 3

  3. 从 sorted_fonts 遍历（跳过已缓存的）：
     → 找到第一个支持该字符的字体
     → 插入 block_cache 队首（MRU 更新）
     → 返回 [该字体]

  4. 都找不到 → 返回 []（缺字）
```

**关键优化：`font_has_glyph` 不克隆字体数据**

```rust
fn font_has_glyph(&self, font_id: FontId, ch: char) -> Option<bool> {
    // 直接从 font_data_cache 取 &[u8] 引用传给 TtfFace::parse
    // 不克隆 Vec<u8>，避免 500KB × 229 = 114MB 拷贝
    let db = self.db.lock()...;
    match &face.source {
        Source::File(path) => {
            let mut cache = self.font_data_cache.lock()...;
            let data: &[u8] = cache.get(&path)?;    // 取引用，不克隆
            let ttf = TtfFace::parse(data, 0).ok()?; // parse from reference
            Some(ttf.glyph_index(ch).is_some())
        }
        ...
    }
}
```

#### 运行时行为

```
区块 0x4E (CJK) 第一个字符 "一":
  1. block_cache 无缓存 → 沿 sorted_fonts 开始扫
  2. Hack(no) → HarmonyOS Sans(yes) → 发现！一般在 ~50 个字体处匹配
  3. block_cache[0x4E] = [HarmonyOS Sans]
  4. 返回 [HarmonyOS Sans]  → warpui_core 检查 → 有该字形 → 缓存

区块 0x4E 第二个字符 "二":
  1. block_cache → [HarmonyOS Sans]
  2. 检查 HarmonyOS Sans → 有 "二" → 返回 [HarmonyOS Sans]（MRU 不变）

区块 0x27 (Dingbats) 字符 "✽":
  1. block_cache 无缓存 → 扫 sorted_fonts
  2. 发现 HarmonyOS Sans 不支持 → 继续 → Noto Sans CJK 支持
  3. block_cache[0x27] = [Noto Sans CJK]
  4. 返回 [Noto Sans CJK]

同区块再次需要不同的字体:
  1. block_cache[0x27] = [Noto Sans CJK, HarmonyOS Sans]（被之前的 0x4E 添加的）
  2. 检查 Noto Sans CJK(no) → 检查 HarmonyOS Sans(no) → 全不命中
  3. 从 sorted_fonts 继续扫 → 找到新字体 → 插入队首
  4. block_cache[0x27] = [新字体, Noto Sans CJK, HarmonyOS Sans]
```

#### 性能

| 场景 | 耗时 |
|------|------|
| 初始化（FontSystem + 排序） | ~37ms（无 cmap 预扫描） |
| 每区块首次字符 fallback | 平均 7ms（~50 次 font_has_glyph，无克隆） |
| 同区块后续字符 fallback | <0.2ms（MRU 命中，1 次查询） |
| 全区块首次 Hermes 启动 | ~300ms（~40 个区块 × 7ms） |

#### 初始化优先级排序算法

```rust
const SENTINELS: [&str; 7] = ["mono", "hack", "code", "sans", "hei", "deng", "cjk"];
all.sort_by(|(_, _, a), (_, _, b)| {
    let a_sc = SENTINELS.iter().position(|k| a.contains(k)).unwrap_or(7);
    let b_sc = SENTINELS.iter().position(|k| b.contains(k)).unwrap_or(7);
    a_sc.cmp(&b_sc).then(a.cmp(&b))
});
// Hack → HarmonyOS Sans → ... → CJK fonts → remainder (alpha)
```

### 淘汰的旧方案

| 旧方案 | 问题 | 替代 |
|--------|------|------|
| `is_font_usable` 预过滤 | 丢字体、耗时 | `rasterize_glyph` 优雅降级 |
| 初始化全量 cmap 扫描 | 660ms | 运行时学习 + MRU 缓存 |
| `hot_fallback` 单值覆盖 | 后发现的覆盖优先级高的 | `block_cache` MRU 多值列表 |
| CJK sentinel 哨兵共享 | 仅 CJK | block_cache 覆盖所有区块 |
| `get_font_data` 克隆数据 | 114MB/扫描 | `font_has_glyph` 引用不克隆 |


## Build Environment

### NDK clang 不能直接在 openEuler VM 上运行

OHOS NDK 的 clang 是 Mach-O 格式（链接到 macOS harmonybrew），无法在 Linux VM 上执行。在 `/tmp/ohos-clang-wrapper` 创建包装器：
```bash
LOCAL_CLANG="/tmp/ohos-clang-wrapper"; mkdir -p "$LOCAL_CLANG"
for tool in clang clang++; do
    cat > "$LOCAL_CLANG/$tool" << 'EOF'
#!/usr/bin/env bash
exec /usr/local/bin/ohos-$tool "$@"
EOF
done
export CC_aarch64_unknown_linux_ohos="$LOCAL_CLANG/clang"
export CXX_aarch64_unknown_linux_ohos="$LOCAL_CLANG/clang++"
```

### libc++ ABI 命名空间

OHOS NDK 的 libc++ 用 `__n1` 命名空间，但默认头文件定义为 `__1`。需从 `libcxx-ohos/include/c++/v1/__config_site` 覆盖到 NDK 标准路径。

### 共享文件系统不支持硬链接

Cargo 的硬链接操作在 NFS/共享文件系统上报 `Operation not permitted`。`CARGO_TARGET_DIR` 必须指向本地文件系统（如 `/tmp/warp-target/`）。

### cargo 锁文件残留

构建中断后 `.cargo-lock` 可能残留。`build-vm.sh` 自动清理超过 5 分钟的锁。

## Remote Build Architecture

本地 openEuler VM 做 Rust 交叉编译，SSH 远程 172.16.100.1:8022 做 HAP 打包（CMake + hvigor）：
1. 本地 cargo → `libwarp.so`
2. 复制到共享文件系统 `entry/libs/arm64-v8a/`
3. SSH → `build-hap.sh` → `entry-default-signed.hap`

### 内核线程数限制

本地 VM（openEuler）内核配置的**单进程可创建最大线程数只有 128**。Rust workspace 编译时 cargo + rustc + LLVM 创建的线程超过此限制，导致 `resource temporarily unavailable`。`-j` 参数不生效，因为单个 rustc 进程内部就创建了大量 LLVM 线程。

**目前无解**，需在本机（PC）的虚拟 Linux 环境中编译，那里没有线程数限制。

## Debugging

hilog 必须按 `com.wap.ohos` 过滤：
```bash
timeout 5 hdc hilog 2>&1 | grep "com.wap.ohos" | grep "MUSL-LDSO\|MMG.*warp"
timeout 5 hdc hilog -l I 2>&1 | grep "com.wap.ohos/Warp\|com.wap.ohos/napi_init"
```

清空缓存后启动：
```bash
hdc shell "hilog -r" && sleep 1 && hdc shell "aa start -a EntryAbility -b com.wap.ohos"
```

查 app PID：
```bash
hdc shell ps -ef | grep com.wap.ohos | grep -v grep
```

## Rendering Pipeline（渲染管线）

### 模块架构与职责

```
openharmony-ability (event.rs / xcomponent.rs)
    ↓ vsync frame callback / 触摸事件 / 生命周期事件
    ↓ 通过 mpsc channel 发送 OhosEvent
winit (platform_impl/ohos/mod.rs)
    ↓ EventLoop::run() 接收 OhosEvent，转换为 winit Event
    ↓ 通过 event_handler 分发到 ApplicationHandler
warpui (platform/ohos/app.rs)
    ↓ OhosApplicationHandler 处理 RedrawRequested / Resumed / Resized
    ↓ 调用 OhosWindowManager 相应方法
warpui (windowing/ohos/ohos_window_manager.rs)
    ↓ OhosWindowManager 管理窗口状态、wgpu 资源、渲染调度
    ↓ OhosWindow 持有 scene、needs_redraw、SharedWindowState
wgpu (rendering/wgpu/)
    ↓ Resources (surface + device + queue + adapter)
    ↓ Renderer::render() 执行实际 GPU 渲染
```

### 初始化流程（启动顺序）

```
1. ArkTS EntryAbility.onCreate()
   → 加载 NAPI 模块，调用 init_ability()
   → 创建 OpenHarmonyApp，存入全局 WARP_APP

2. ArkTS Index.aboutToAppear()
   → 调用 startWarpMain() → 启动 warp-main 线程

3. ohos_entry::start_warp_main()
   → 设置环境变量 (XDG dirs)
   → 暴露事件通道 expose_event_channel()
   → 创建 winit EventLoopBuilder
   → 调用 App::run()

4. OhosApp::run() (app.rs:60)
   → callbacks.initialize_app() → 初始化所有 App 模块
   → 创建 winit EventLoop（with_openharmony_app）
   → event_loop.run_app(&mut OhosApplicationHandler)

5. OhosApplicationHandler::resumed()
   → 桥接 OpenHarmonyApp.native_window() 到 SharedWindowState
   → 设置 surface 参数 (set_surface_params)

6. OpenWindow 动作（从 root_view:open_from_restored 触发）
   → OhosWindowManager::open_window()
   → 创建 OhosWindow（scene=None, needs_redraw=false）
   → init_wgpu_resources()
     → OhosTrustedWindow::from_usize(nw_ptr)
     → init_wgpu_instance()（创建全局 wgpu::Instance）
     → Resources::new(trusted_window) → surface + device + queue + adapter
     → 输出: "wgpu resources initialized successfully"

7. OH_NativeVSync 开始驱动帧循环
   → winit EventLoop::run() 开头调用 init_vsync()
   → OH_NativeVSync_RequestFrame → ohos_vsync_callback
   → 回调发送 OhosEvent::WindowRedraw 到事件通道
   → 请求下一帧 vsync（单次模式，每帧重新请求）
```

### 每帧渲染流程（RedrawRequested）

```
1. OH_NativeVSync 回调 → 发送 OhosEvent::WindowRedraw
2. winit EventLoop → ohos_event_to_winit_event() → WindowEvent::RedrawRequested
3. OhosApplicationHandler::window_event()
   → 调用 wm.render_current_window(&mut app.callbacks)

4. OhosWindowManager::render_current_window()
   ├─ 获取 OhosWindow (Rc<OhosWindow>)
   ├─ scene 为 None？→ app_callbacks.for_window().build_scene() 创建新 scene
   ├─ needs_render()？→ 跳转到 wgpu 渲染
   │    ├─ 获取 wgpu Resources (surface + device + queue)
   │    ├─ 按需创建 Renderer
   │    ├─ renderer.render(scene, resources, ...)
   │    │    ├─ 获取 surface texture
   │    │    ├─ 渲染 scene 到 texture
   │    │    └─ present() 提交到交换链
   │    └─ 成功 → needs_redraw = false
   └─ wgpu 失败且无 wgpu → render_native_window()（纯色回退）
```

### 窗口尺寸变化检测机制

OHOS 上窗口 resize 检测存在特殊问题：
ArkTS 的 `window.on("windowSizeChange")` 在启动时触发一次后，**后续分屏 resize 可能不触发**（该行为依赖 OHOS 版本）。
XComponent 的 NDK `on_surface_changed` 回调也**不触发**于分屏 resize。

**解决方案**：每帧通过 `OH_NativeXComponent_GetXComponentSize` 查询 XComponent 表面尺寸。

```
render_current_window() 每帧:
  1. 调用 OpenHarmonyApp::surface_size()
     → native_xcomponent().size(native_window)
     → OH_NativeXComponent_GetXComponentSize（NDK 调用，仅读取缓存 int，< 1μs）
  2. 比较返回尺寸与本地存储的 surface_size
  3. 不同则:
     → set_surface_params(新尺寸)
     → update_surface_size() → SET_BUFFER_GEOMETRY + wgpu surface 重配
  4. 下一帧用新尺寸渲染
```

**调用路径及时延**：
```
XComponent 帧回调（UI 线程，每帧触发）
  → OH_NativeXComponent_GetXComponentSize     [NDK 层，读缓存 int，< 1μs]
  → 比较尺寸变化                              [< 1μs]
  → OH_NativeWindow_NativeWindowHandleOpt
    (SET_BUFFER_GEOMETRY) + wgpu configure    [~1-5ms]
  → 下一帧 vsync → 新尺寸渲染                  [等待 0~16ms]
总计: 1~5ms 处理 + 最多 1 vsync = 17~21ms 最坏情况
```

**关键经验**：
- **不要**用 `OH_NativeWindow_NativeWindowRequestBuffer` 轮询缓冲区尺寸——与 wgpu EGL 缓冲区池冲突，导致闪屏
- **不要**在 ArkTS 侧加 `setInterval` 定时器轮询 `window.getWindowProperties()`——浪费 CPU
- **应该**每帧用 `OH_NativeXComponent_GetXComponentSize`——NDK 层缓存 int，零开销
- 配合原有的 `Resized` 事件（`window.on("windowSizeChange")`）路径，构成双重保障：事件路径优先，帧回调路径兜底

**涉及修改的文件**：
- `../openharmony-ability/crates/ability/src/app.rs` — 新增 `OpenHarmonyApp::surface_size()` 方法
- `crates/warpui/src/windowing/ohos/ohos_window_manager.rs` — 每帧在 `render_current_window()` 中检查尺寸变化

### 核心模块接口关系

| 接口/特质 | 定义位置 | OHOS 实现 | 用途 |
|-----------|----------|-----------|------|
| `WindowManager` | `warpui_core::platform` | `OhosWindowManager` | 窗口创建、销毁、渲染调度 |
| `Window` | `warpui_core::platform` | `OhosWindow` | 窗口属性、回调访问 |
| `WindowContext` | `warpui_core::platform` | `OhosWindow` | `render_scene()`、`request_redraw()`、尺寸/缩放 |
| `ApplicationHandler` | `winit::application` | `OhosApplicationHandler` | 事件循环回调（resumed/window_event/about_to_wait） |
| `HasWindowHandle` | `wgpu::rwh` | `OhosTrustedWindow` | wgpu surface 创建所需的原生窗口句柄 |
| `HasDisplayHandle` | `wgpu::rwh` | `OhosTrustedWindow` | wgpu surface 创建所需的 display 句柄 |

### 渲染管线相关结构

| 结构/类型 | 所在文件 | 用途 |
|-----------|---------|------|
| `Surface`（wgpu） | wgpu Surface | 与 OHOS XComponent 关联的 GPU 渲染表面 |
| `Adapter`（wgpu） | wgpu Adapter | GPU 适配器，与 `Surface` 兼容 |
| `Device`（wgpu） | wgpu Device | 逻辑 GPU 设备，创建资源和命令 |
| `Queue`（wgpu） | wgpu Queue | GPU 命令提交队列 |
| `OhosTrustedWindow` | `crates/warpui/src/windowing/ohos/trusted_window.rs` | 包装 OH_NativeWindow 指针，提供 `HasWindowHandle` + `HasDisplayHandle` |
| `WGPUContext` | `renderer.rs` | 渲染上下文，持有 resources 和字型回调 |
| `Renderer` | `renderer.rs` | 渲染器，持有 rect/glyph/image 管线 |

### wgpu 初始化及资源创建

```
OhosTrustedWindow(包装 OH_NativeWindow 指针)
  → 实现 HasWindowHandle + HasDisplayHandle
  → wgpu::Instance::create_surface(trusted_window)
  → 创建 wgpu Surface

Resources::new(trusted_window, ...)
  → 枚举适配器 → 选择兼容 surface 的适配器
  → 创建 Device + Queue
  → 创建 SurfaceConfiguration（优先级: Bgra8Unorm > Rgba8Unorm）
  → configure_surface()
  → 创建 uniforms + quad resources
  → 注册 device_lost 回调
```

### 设置 surface 格式

```rust
// resources.rs create_surface_config():
let caps = surface.get_capabilities(adapter);
let preferred_formats = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm];
if cfg!(target_env = "ohos") {
    if let Some(fmt) = preferred_formats.iter().find(|f| caps.formats.contains(f)) {
        config.format = *fmt;
    }
}
```

### 设置 present_mode

```rust
// OHOS: 多数设备仅支持 Fifo
if cfg!(target_env = "ohos") {
    config.present_mode = PresentMode::Fifo;
} else {
    config.present_mode = PresentMode::AutoNoVsync;
}
```

### 设置后端选择

```rust
fn wgpu_backend_options() -> wgpu::Backends {
    if cfg!(target_env = "ohos") {
        wgpu::Backends::GL  // OpenGL ES 3.2
    } else {
        wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all())
    }
}
```

### 渲染诊断开关

`crates/warpui/src/windowing/ohos/ohos_window_manager.rs` 中有一个静态开关控制两种渲染模式：

```rust
static DIAGNOSTIC_MODE: AtomicBool = AtomicBool::new(false);
```

- `true`：启用 shader 测试图案（全屏棋盘格 + 绿色方框 + 色带 + SDF W 字），**绕过整个场景渲染管线**，直接通过 WGSL 片段着色器绘制。用于确认 wgpu surface/device/queue 基础渲染正常
- `false`（默认）：正常场景渲染，走完整的 Rect/Glyph/Image 管线

切换方式：编译前修改 `AtomicBool::new(true/false)`，或调用 `set_diagnostic_mode(true/false)` 动态切换。

**诊断层级**（DIAGNOSTIC_MODE=false 时仍始终激活）：
- **帧级诊断**（`renderer/frame.rs`）：左上角绿色方块，在帧渲染通道结束后叠加
- **渲染器级诊断**（`renderer.rs`）：右下角蓝色方块，在 submit 前叠加

另外 `render_current_window` 中构建场景后会向场景注入测试元素（黄色半透明矩形 + 红色 72pt 'W' glyph + 彩色棋盘格图像），用于验证三种管线在正常场景渲染路径下是否工作。

**使用场景**：
1. 黑屏时先设 `true` → 若看到测试图案说明 wgpu 基础渲染正常，问题在场景管线
2. 设回 `false` → 观察帧级/渲染器级诊断方块是否出现，定位具体哪段管线断裂

### 原生缓冲渲染（纯色回退）

当 wgpu 不可用时，直接操作 OH_NativeWindow 缓冲区队列：

```
OH_NativeWindow_NativeWindowHandleOpt(SET_BUFFER_GEOMETRY, w, h)
OH_NativeWindow_NativeWindowRequestBuffer → BufferHandle
  → vir_addr 写入 BGRA8888 像素
OH_NativeWindow_NativeWindowFlushBuffer(region)
```

## NAPI Bridge（JS ↔ Rust 桥接）

### 调用路径（TSFN 异步调用）

```
ArkTS 方法调用
  → napi 模块入口（libwarp.so）
  → #[napi] 宏展开 → 同步调用（仅 UI 线程安全）
  → 后台线程 → ohos_call_napi("methodName", ...)
    → napi_call_threadsafe_function(tsfn, &request)
    → UI 线程 TSFN 处理 → 调用 NAPI 方法
    → cv.notify_one() 唤醒后台线程
```

### IME 输入法处理（OHOS vs 其他平台关键差异）

OHOS 的 IME 行为与其他平台有显著差异，是输入问题的主要根源。

#### IME 回调接口

OHOS IME 只注册 **4 个回调**，均不携带修饰键信息：

| 回调 | 签名 | 用途 |
|------|------|------|
| `insert_text` | `Fn(String)` | 文本输入 — 意外包含控制字符 |
| `on_backspace` | `Fn(i32)` | 退格键 — 通过独立回调 |
| `on_enter` | `Fn(EnterKey)` | 回车键 — 通过独立回调 |
| `on_status_change` | `Fn(KeyboardStatus)` | 键盘显示/隐藏（None/Hide/Show，仅软键盘） |

**重要差异：**
- **没有 `preedit` 回调**：（`ImePayload::Preedit` 变体是死代码，从未被创建）
- **没有修饰键信息**：Ctrl/Alt/Shift 信息无法从 IME 获取，必须走 ArkTS `onKeyEvent`
- **控制字符**：IME 可能将 Enter/Backspace/Escape 等以 `TextInputEvent("\n")` 形式发送

#### ImePayload 信封设计

`ImePayload` 是 IME 事件在跨线程通道（`EVENT_CHANNEL`）中传递的信封。建议保持精简：

```rust
enum ImePayload {
    Commit(String),                                     // 文字
    Preedit(String),                                    // IME 拼字中（OHOS 从未使用，保留兼容）
    ImeStatus(bool),                                    // true=启用, false=禁用
    KeyboardEvent { key: String, chars: Option<String> },  // 所有按键统一走这个信封
}
```

#### 控制字符 → KeyEvent 转换

OHOS IME 通过 `insert_text` 发送的控制字符**不是**文字，应转换为 KeyEvent：

```rust
ImeEvent::TextInputEvent(txt) => {
    let text = &txt.text;
    if text == "\n" || text == "\r" {
        ImePayload::KeyboardEvent { key: "enter".into(), chars: None }
    } else if text == "\x7f" || text == "\x08" {
        ImePayload::KeyboardEvent { key: "backspace".into(), chars: None }
    } else if text == "\t" {
        ImePayload::KeyboardEvent { key: "tab".into(), chars: None }
    } else if text == "\x1b" {
        ImePayload::KeyboardEvent { key: "escape".into(), chars: None }
    } else if text.len() == 1 {
        let c = text.chars().next().unwrap();
        if c.is_ascii() && !c.is_ascii_control() {
            ImePayload::KeyboardEvent { key: text.clone(), chars: Some(text.clone()) }
        } else {
            ImePayload::Commit(text.clone())
        }
    } else {
        ImePayload::Commit(text.clone())
    }
}
```

**为什么这样转换：** 这保证了 Enter 进入快捷键系统（匹配 `"enter"`→提交命令），而不是作为 `TypedCharacters("\n")` 插入换行。

#### 修饰键状态同步

OHOS 不产生 `WindowEvent::ModifiersChanged`，需要在 `ohos_event_to_winit_event` 中手动生成。

**核心机制：** `OhosEvent::KeyEvent` 信封自带 `modifier_state: u8` 字段（当前总是传 0）。出队列时，KeyEvent 和 ImeEvent 路径都先发 `ModifiersChanged` 再发 `KeyboardInput`，两个 winit 事件在同一个 `vec!` 中连续发出：

```rust
// OhosEvent::KeyEvent → vec![ModifiersChanged, KeyboardInput]（同一事件中）
let mut winit_mods = ModifiersState::default();
let ms = MODIFIER_STATE.load(Ordering::Acquire);
if ms & MOD_ALT != 0 { winit_mods |= ModifiersState::ALT; }
if ms & MOD_CTRL != 0 { winit_mods |= ModifiersState::CONTROL; }
if ms & MOD_SHIFT != 0 { winit_mods |= ModifiersState::SHIFT; }
if ms & MOD_SUPER != 0 { winit_mods |= ModifiersState::SUPER; }
let mods: Modifiers = winit_mods.into();
vec![
    Event::WindowEvent { event: WindowEvent::ModifiersChanged(mods), .. },
    Event::WindowEvent { event: WindowEvent::KeyboardInput { .. }, .. },
]
```

`MODIFIER_STATE` 由 `update_modifier_state()` 在 `OhosEvent::KeyEvent` 处理中维护（修饰键按下/弹起时更新）。

**IME 事件路径**：IME 不携带修饰键信息，所以 `ModifiersChanged` 用 `default()` 清除所有修饰键：

```rust
let mods: Modifiers = ModifiersState::default().into();
```

**ArkTS 侧**：ArkTS `KeyEvent.getModifierKeyState(keyCode)` 是逐个键检查的 API（返回 boolean），无法直接获得完整的修饰键 bitmask。所以第三个参数始终传 0，修饰键状态完全由 Rust 侧的 `update_modifier_state()` 跟踪。

**注意：** ArkTS 的 `onKeyEvent` 也会为字符键触发（不是在 IME 激活后就不触发了）。所以每次字符按键也会经过 `update_modifier_state` 路径，确保 MODIFIER_STATE 及时更新。

#### IME 恢复（最小化/恢复）

应用最小化后回到前台时，OHOS 可能 detach 了 IME。需要在 `Event::Resume` 中重新 attach：

```rust
// ohos_entry.rs run_loop
Event::Resume(_loader) => {
    if let Some(app) = get_warp_app() {
        app.show_keyboard();
    }
}
```

`show_keyboard()` 内部会调用 `OH_InputMethodController_Attach` 重新 attach IME。

#### 典型故障模式

| 症状 | 根因 | 解决 |
|------|------|------|
| Enter 插入换行而非提交命令 | IME 发 `TextInputEvent("\n")` → Commit | 转成 `KeyboardEvent{key:"enter"}` |
| Backspace 插入不可见字符 | IME 发 `TextInputEvent("\x7f")` → Commit | 转成 `KeyboardEvent{key:"backspace"}` |
| 方向键输出文本 | IME 转义序列 (`\x1b[A`) 被 Commit | 通过 `\x1b` 拦截转 Escape |
| 所有快捷键不匹配 | `window_state.modifiers.alt=true` 卡住 | 每次事件发 `ModifiersChanged` |
| 刚打开没光标，最小化后有 | 首次焦点链没到 EditorView | 暂未解决（框架行为） |
| 切后台再回来 IME 失效 | IME 被 detach 未重新 attach | `Event::Resume` 中调 `show_keyboard()` |
| 中文输入时用不了 | 只有输入，没有预编辑显示 | 缺 preedit 回调（OHOS 限制） |

### 剪贴板 ArkTS 调用（C FFI + TSFN）

```
warp-main 线程调用 ohos_get_clipboard()
  → TSFN（UI 线程安全桥接）
  → UI 线程: pasteboard.createData() + systemPasteboard.setData()
  → 返回值通过 condition_variable 传回
```

## init_ability 生命周期

```
onCreate()
  → init_ability（#[ability] 宏生成，自动注册 NativeAbility.onCreate）
  → 设置全局 WARP_APP
  → 暴露 mpsc 事件通道 (expose_event_channel)
  → app.run_loop(closure) → 注册事件处理器
```

```rust
#[ability]
fn init_ability(app: OpenHarmonyApp) {
    warpui::windowing::ohos::set_warp_app(app.clone());
    let tx = expose_event_channel();
    app.run_loop(move |event| match event {
        Event::Resume(_loader) => log::info!("ohos_entry: onResume"),
        Event::SaveState(_saver) => log::info!("ohos_entry: onSaveState"),
        other => {
            let ohos_event = ability_event_to_ohos_event(&other);
            let _ = tx.send(ohos_event);
        }
    });
}
```

### 启动顺序约束

```
ArkTS: Index.aboutToAppear()
  → 调用 startWarpMain()
  → 创建 warp-main 线程（非阻塞, 立即返回）

warp-main 线程:
  → log_redirect_2_hilog()（日志重定向）
  → set_current_dir(...)
  → set_ohos_environment(...)
  → crate::main()
     → OhosApp::new() → App::new()
     → OhosApp::run()
       → 创建 winit EventLoop
       → event_loop.run_app(handler)
         → handler.resumed() → init_wgpu_resources()

ArkTS: XComponent.onLoad()
  → 创建 OpenHarmonyApp 并设置 WARP_APP
  → 初始化 NAPI 桥
  → 注册 XComponent 回调
```

**重要**：`startWarpMain` 和 `init_ability` 执行在不同的线程。`init_ability` 在 ArkTS 能力创建时调用（Ability 线程），`startWarpMain` 在 Index.aboutToAppear 时调用（UI 线程 -> warp-main 线程）。**两个线程都可能先运行**。`WARP_APP` 和事件通道的设计支持这种不确定性。

### XComponent 回调注册

```
XComponent 创建 → on_surface_created:
  1. 获取 XComponent 尺寸 → 存入 OpenHarmonyApp.rect
  2. 获取 native_window → 存入 OpenHarmonyApp.raw_window
  3. 初始化 IME
  4. 注册 on_frame_callback（每帧触发 WindowRedraw）
  5. 注册 on_surface_destroyed
  6. 注册 on_surface_changed（表面尺寸变化时触发）
  7. 注册 on_touch_event
  8. 注册 on_key_event
  9. xcomponent.register_callback()

on_frame_callback（每帧）:
  1. 发送 Event::WindowRedraw(IntervalInfo) → mpsc → winit → RedrawRequested
```

### OpenHarmonyApp 线程安全性

| 字段 | 类型 | 线程安全 | 说明 |
|------|------|----------|------|
| `inner` | `Arc<RwLock<OpenHarmonyAppInner>>` | ✅ `Send + Sync` | 读写锁，UI 线程读/写，warp 线程读 |
| `event_loop` | `Arc<RefCell<Option<Box<dyn FnMut(Event)>>>>` | ⚠️ `!Send` + `!Sync` | 仅 UI 线程访问，禁用 Send/Sync lint |
| `ime` | `Arc<RefCell<Option<IME>>>` | ⚠️ 同 event_loop | 同上 |

```rust
// app.rs: 通过 #[allow] 绕过 clippy 警告
#[allow(clippy::arc_with_non_send_sync)]
event_loop: Arc::new(RefCell::new(None)),
```

## Display Info

显示信息通过 Display 服务获取。在 NAPI 中调用 `display.getDefaultDisplaySync()`：

```cpp
// service_display.cpp: getDisplayInfo()
napi_load_module(env, "@kit.ArkUI", &displayModule);
napi_get_named_property(env, displayModule, "display", &display);
napi_call_function(env, display, nullptr, 0, nullptr, &defaultDisplay);
napi_get_named_property(env, defaultDisplay, "width", &width);
napi_get_named_property(env, defaultDisplay, "height", &height);
napi_get_named_property(env, defaultDisplay, "density", &density);
napi_get_named_property(env, defaultDisplay, "orientation", &orientation);
```

通过 `ohos_get_display_info()` C FFI 从 Rust 获取：
```rust
extern "C" { fn ohos_get_display_info() -> *const c_char; }
let info = unsafe { CStr::from_ptr(ohos_get_display_info()) };
```

## 沙箱环境路径配置

```rust
fn set_ohos_environment(files_dir: &str, cache_dir: &str, temp_dir: &str, current_dir: &str) {
    // ArkTS context.filesDir → XDG_DATA_HOME
    // ArkTS context.cacheDir → XDG_CACHE_HOME
    // ArkTS context.tempDir → TMPDIR
    // ArkTS context.filesDir → current_dir + XDG_CONFIG_HOME
}
```

ArkTS 上下文传入路径：
```typescript
warpModule.startWarpMain(context.filesDir, context.cacheDir, context.tempDir, context.filesDir);
```

## NAPI 模块加载

两 .so 架构：

```
libentry.so（NAPI .so，C++）
  → napi_init.cpp: RegisterDisplay/RegisterClipboard/...
  → 通过 C FFI 桥 (ohos_*) 供 Rust 调用
  → libwarp.so 在 HAP 的同级目录（entry/libs/arm64-v8a/）

libwarp.so（Rust .so，cdylib）
  → #[napi] / #[ability] 宏自动注册 NAPI 方法
  → 导出 hap_start_warp C 符号
  → 通过 RPATH $ORIGIN 解析 libentry.so 符号
```

### 快速验证 NAPI 方法注册

```bash
# 查看 Rust .so 导出的 NAPI 符号
aarch64-unknown-linux-ohos-nm -D libwarp.so | grep napi_register
```

## Application Lifecycle（应用生命周期）

### Ability 状态转换

```
onCreate()
  → init_ability（NativeAbility.onCreate → #[ability] 宏）
  → 设置 WARP_APP

onWindowStageCreate()
  → 注册窗口事件（windowSizeChange、avoidAreaChange 等）
  → 加载 pages/Index（ArkTS UI）

onForeground() → WindowStageEventType.SHOWN → Event::Start
onWindowStageEvent(ACTIVE) → Event::GainedFocus
onWindowStageEvent(INACTIVE) → Event::LostFocus
onWindowStageEvent(HIDDEN) → Event::Stop
onWindowStageEvent(RESUMED) → Event::Resume(SaveLoader)
onWindowStageEvent(PAUSED) → Event::Pause
```

### 系统事件转发

| ArkTS 事件 | Rust Event 变体 | 说明 |
|-----------|-----------------|------|
| `window.on("windowSizeChange")` | `WindowResize(Size)` | 窗口尺寸变化（可能不触发） |
| `window.on("avoidAreaChange")` | `AvoidAreaChange` | 安全区域变化 |
| `windowStage.on("windowStageEvent")` | `Start/GainedFocus/LostFocus/Pause/Stop/Resume` | 生命周期事件 |
| `onConfigurationUpdated` | `ConfigChanged` | 配置变化（语言、方向等） |
| `onMemoryLevel` | `LowMemory` | 系统内存不足 |
