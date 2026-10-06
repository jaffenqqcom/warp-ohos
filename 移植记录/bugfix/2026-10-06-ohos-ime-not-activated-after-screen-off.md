# OHOS 黑屏/前后台切换恢复后输入法有概率激活不了

关联：[[ohos-debug-lessons]]

- 日期：2026-10-06
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：OHOS 专属事件循环的 IME 绑定时机。仅影响 OHOS 后端；macOS / Linux / Windows 的 IME 路径不经过此文件。
- 状态：已修复。修复文件路径含 `ohos`，属 OHOS 专属后端，非 OHOS 平台不参与编译。

## 问题描述

电脑黑屏恢复、或应用切后台再回前台等场景后，鸿蒙输入法有**很大概率激活不了**：输入框已聚焦，但软键盘不出来、也无法输入。往往要再切一次焦点才能恢复。

## 问题表现

- 屏幕熄灭后唤醒、或应用被切到后台再回前台，光标已聚焦于终端输入区，但软键盘不弹出。
- 概率性出现，不是每次都触发；再次切换焦点（例如再切出去再切回来）有机会恢复。
- 普通 shell 与全屏 TUI 都可能遇到。

## 问题原因

根因是 **IME 绑定采用了"边沿触发"**：只在焦点从"未聚焦"变到"已聚焦"的那一次请求绑定键盘。
但系统在应用离开前台期间会主动丢掉已绑定的 IME 会话，回到前台时需要**重新绑定**；
而黑屏恢复时系统并不保证先补发一次"失去焦点"，于是"边沿"不成立，重新绑定被跳过。

### 因果链

1. `Translator::handle` 里，焦点类事件（`Start | GainedFocus | Resume`）用
   `WINDOW_FOCUSED.swap(true, ...)` 检测边沿：只有当此前是 `false` 时才发
   `AppEvent::OpenImeRequested`（真正去 bind IME），并把 `FocusChanged(true)` 交给 app。

2. `LostFocus | Pause | Stop` 把 `WINDOW_FOCUSED` 置 `false`。**只有经过这一支，"边沿"才会被重新武装。**

3. 黑屏恢复 / 前后台切换时，系统可能**只重发 `GainedFocus` / `Resume`**，而没有先发
   `LostFocus` / `Pause` 清掉 latch。此时 `WINDOW_FOCUSED` 仍为 `true` →
   `swap(true)` 返回旧值 `true` → 边沿不成立 → **不再发 `OpenImeRequested`**。

4. 与此同时，系统在该应用离开前台期间已经**丢弃了绑定的 IME 会话**。回到前台后本应重新 bind，
   这一次 bind 恰好被上面的边沿逻辑跳过，于是软键盘再也起不来。

### 为什么这是"概率性"的

系统是否补发 `LostFocus` / `Pause` 取决于具体的熄屏/后台路径与厂商实现。补发了就一切正常
（边沿被重新武装），没补发就卡住——所以表现为"有很大概率激活不了"而非必现。

### 参照

HiCodeer（Zed 的 OHOS 移植）在每次获得焦点时都**强制重挂** IME，不做边沿判断，因此不受此影响。

## 解决方案

关键洞察：**重新绑定一个已经绑定的 IME 会话是幂等的，所以没有理由只在边沿上做一次。**
正确判据不是"焦点有没有变化"，而是"当前是否处于焦点状态"——每次收到焦点类事件都请求一次绑定即可。

### 改动

- 去掉边沿限制：`Start | GainedFocus | Resume` 分支**每次都**发 `AppEvent::OpenImeRequested`。
  `WINDOW_FOCUSED` 不再用于边沿判断，只保留为"最新焦点状态"的记录（见下）。
- 保留两个**兜底时机**，它们覆盖焦点事件到不了的场景：

  1. `SurfaceCreate`：窗口的 surface 可能晚于焦点出现，或首次绑定请求发生在 bridge 会话就绪之前；
     此处若 `WINDOW_FOCUSED` 已为 `true`，再请求一次（幂等）。
  2. `VisibilityChanged(true)`：2in1 标题栏最小化/还原**不会**发任何 `windowStageEvent`
     ——离开时不发 `Stop` / `LostFocus`，回来时也不发 `Resume` / `GainedFocus`，
     可见性回调是这条转换上**唯一的信号**，因此在它里面也请求一次。

修改前后对比：

```rust
// 修前：只有 false -> true 的边沿才请求
if !WINDOW_FOCUSED.swap(true, Ordering::AcqRel) {
    self.send(AppEvent::OpenImeRequested);
}

// 修后：焦点类事件每次都请求，绑定幂等
WINDOW_FOCUSED.store(true, Ordering::Release);
self.send(AppEvent::OpenImeRequested);
```

`OpenImeRequested` 最终由 `event_loop.rs` 的 `AppEvent::OpenImeRequested` 分支调
`delegate::open_ime_async()` 执行；重复请求只是一次幂等的 bridge 调用。

## 修改文件

- `crates/warpui/src/platform/ohos/event_loop.rs` — `Translator::handle` 的
  `Start | GainedFocus | Resume` 分支由「边沿触发」改为「每次请求」：
  `WINDOW_FOCUSED` 只记录状态，`OpenImeRequested` 无条件发送；同步更新 `WINDOW_FOCUSED`、
  `SurfaceCreate`、`VisibilityChanged` 三处注释，使其与新的语义一致。

## 门控与跨平台约束

- 该文件位于 `crates/warpui/src/platform/ohos/`，是 OHOS 专属后端，只对 `target_env = "ohos"` 参与编译，**无需再包 `#[cfg]`**。
- 本次未触碰任何共享代码，`crates/warpui/src/windowing/winit/` 与 macOS / Linux / Windows 的 IME 绑定逻辑完全不受影响。
- 幂等绑定不改变"从未聚焦"路径的行为：非焦点事件不会触发绑定，重复焦点事件自然收敛到同一绑定的会话。

## 验证

**复现（修复前）**：在 OHOS 设备上运行 Warp 并聚焦输入区，熄灭屏幕（或切后台）再唤醒/切回，
观察软键盘是否弹出。概率性失败，可多次重复以命中。

**确认修好**：同样反复熄屏恢复 / 前后台切换，软键盘每次都应正常弹出；
最小化后再还原（2in1 标题栏，覆盖 `VisibilityChanged` 兜底路径）也应正常。

**代码侧自证**：编译 OHOS 目标（debug）确认无警告；确认 `Start | GainedFocus | Resume`
分支不再依赖 `WINDOW_FOCUSED` 的旧值，且 `WINDOW_FOCUSED` 仍被 `SurfaceCreate` 分支读取。
