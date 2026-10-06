# OHOS 运行全屏 TUI 时输入法选词框不跟随光标

关联：[[ohos-debug-lessons]]

- 日期：2026-10-06
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：终端光标位置上报链路。普通 shell 提示符下光标常驻可见，问题不明显；全屏 TUI（`codebuddy`、`vim` 等会发 `ESC[?25l` 隐藏光标的程序）必现。
- 状态：已修复（源码改动尚未提交）。修复点为 OHOS 门控块，非 OHOS 平台编译期整块剔除。

## 问题描述

在 Warp 里运行全屏 TUI 程序（本次场景是 `codebuddy`）时，鸿蒙输入法的选词框停在旧位置、不跟随光标移动。普通 shell 提示符下基本正常——因为那时光标一直可见，位置缓存每帧都被刷新。

## 问题表现

- 进入全屏 TUI 后输入拼音，候选词框固定不动，不跟随 TUI 内的光标。
- 普通 shell 提示符（光标块可见）下选词框跟随正常，佐证问题与"光标是否可见"强相关。
- 触发条件：程序用 `ESC[?25l` 关闭光标（`TermMode::SHOW_CURSOR` 被清掉）。几乎所有全屏 TUI 都会这么做。

## 问题原因

根因是**IME 光标位置的唯一来源是"绘制期写入的位置缓存"，而位置缓存只在真正画光标时写**。一旦光标被隐藏、渲染路径跳过画光标，缓存就冻结在最后一帧可见位置。

### 因果链

1. 位置缓存写入点只有一个：`grid_renderer.rs` 的 `render_cursor`，它调用
   `ctx.position_cache.cache_position_indefinitely("terminal_view:cursor_{view_id}", RectF::new(...))`。
   除此之外没有任何地方写这个键。

2. 上层读取点：`TerminalView::active_cursor_position`（`app/src/terminal/view.rs`）用
   `ctx.element_position_by_id(self.cursor_position_id())` 读同一个键。`cursor_position_id` 也是
   `terminal_view:cursor_{view_id}`。

3. 每帧末差分上报：`warpui_core` 的 `App::report_active_cursor_position_update_if_changed`（`crates/warpui_core/src/core/app.rs`）
   把当前缓存值与上一次记录的 `last_observed_active_cursor_positions` 比较，只有**变化**才
   `report_active_cursor_position_update()`。后者经平台 delegate 的
   `active_cursor_position_updated()`（`crates/warpui/src/platform/ohos/windowing.rs`）发
   `AppEvent::ImeCursorPositionUpdated`，最终由 `event_loop.rs` 的 `update_ime_cursor_position`
   调 `delegate::update_ime_cursor`。

4. TUI 发 `ESC[?25l` 后，`TermMode::SHOW_CURSOR` 被清。渲染时有一段
   `if cursor_visible { ... draw_cursor / render_cursor ... }`，条件不成立 → **整段跳过** → 缓存不更新。

5. 缓存既不变，差分就恒判"没变" → 一次都不上报 → ArkTS 插件永远收不到新 rect → 选词框停在原地。

### 关键点：`ESC[?25l` 只清模式位，不改光标坐标

VT 解析器一直在更新 grid 里的实时光标点（`ansi_handler` 的 `goto()` 等），所以**实时光标位置始终是准的**，
只是没人把它抄进位置缓存。问题不在"坐标丢了"，而在"可见性"与"位置缓存"被错误地绑定了。

```
实时光标点（一直更新，正确）
        │  只有 render_cursor 会抄
        ▼
位置缓存 terminal_view:cursor_{view_id}
        │  只有 render_cursor 会写
        ▼
每帧差分 → 恒判"没变" → 不上报 IME        ← 光标隐藏后卡在这里
```

### 为什么 HiCodeer 没这个问题

参照 HiCodeer（Zed 的 OHOS 移植，`crates/terminal/src/alacritty.rs`）：它的光标位置直接读网格光标字段
——alacritty 的 `RenderableCursor` 里 `point` 与可见性是**解耦**的，可见性不影响 `point` 的读取，
也不经过"画了才缓存"这层。OHOS 后端应保持同样的解耦。

## 解决方案

关键洞察：**位置缓存只在"画光标"的时候写，但"画不画"和"光标在哪"是两件事。**
修复方式是在光标隐藏时**仍以实时 grid 光标位置调一次 `render_cursor`，但用 `CursorShape::Hidden`**，
让它只刷新缓存、不产生任何绘制。

- `render_cursor` 内 `CursorShape::Hidden` 分支为空（不调任何 `ctx.scene.draw_*`），所以这次调用零绘制开销，只完成第 1 步的 `cache_position_indefinitely`。
- 两处渲染路径各加一个 `#[cfg(target_env = "ohos")]` 门控块：

  1. `app/src/terminal/block_list_element.rs` 的 blocklist 渲染路径——**主屏**，`codebuddy` 走这条，是本次真正的修复点。
     取位置用 `output_grid.cursor_display_point()`，返回
     `CursorDisplayPoint::{Visible(point), HiddenCache(point)}` 两种，均持有实时点；
     门控条件为 `block.is_active_and_long_running() && !block.is_output_cursor_visible() && !hide_cursor_cell`。
  2. `app/src/terminal/alt_screen/alt_screen_element.rs` 的备用屏渲染路径——备用屏（vim 等全屏程序）。
     取位置用 `grid.cursor_render_point()`；门控条件为 `!cursor_visible && !hide_cursor_cell`。

`render_cursor` 本体（`grid_renderer.rs`，共享代码）一行未改，OHOS 只是在隐藏光标的分支上多调了它一次。

### 同链路上的配套修复（先前提交）

光标位置上报要真正抵达 IME，还依赖 `update_ime_cursor_position` 里不再有 `is_ime_open()` 门控。
该函数在之前提交里曾带一处"键盘未弹起就提前 return"的判断：键盘从隐藏转显示时，上报先于
`ImeStatusEvent` 到达，那次上报会被丢弃。该门控已移除（提交 `d0a7594`，改动在 OHOS 专属文件
`crates/warpui/src/platform/ohos/event_loop.rs` 内），使上报无条件执行。

### 备选方案与取舍

- **每帧无条件调 `render_cursor`**：会在光标可见时重复绘制/重复写缓存，无必要。以 `Hidden` shape 只在隐藏分支补充刷新，覆盖面最小。
- **让 `TerminalView::active_cursor_position` 直接读 grid 实时光标**：更贴近 HiCodeer，但要改共享的 `view.rs` 与位置缓存协议，波及面大。门控补刷是改动最小的等效解。
- **在 `ESC[?25l` 处理时主动写缓存**：需在 VT 解析器里插入平台分支，位置计算的 padding/origin 与渲染期上下文不一致，容易算错。

## 修改文件

- `app/src/terminal/block_list_element.rs` — 新增 `#[cfg(target_env = "ohos")]` 门控块：输出光标隐藏且 block 活跃时，用 `output_grid.cursor_display_point()` 取实时点、以 `CursorShape::Hidden` 调 `render_cursor` 刷新 IME 锚点缓存（主屏，codebuddy 修复点）。
- `app/src/terminal/alt_screen/alt_screen_element.rs` — 新增 `#[cfg(target_env = "ohos")]` 门控块：`!cursor_visible` 时用 `grid.cursor_render_point()` 以 `CursorShape::Hidden` 调 `render_cursor` 刷新缓存（备用屏）。
- `crates/warpui/src/platform/ohos/event_loop.rs` — （先前提交 `d0a7594`）删除 `update_ime_cursor_position` 中的 `is_ime_open()` 门控，使光标位置上上报无条件。

## 门控与跨平台约束

- `block_list_element.rs`、`alt_screen_element.rs` 的路径**不含 `ohos`**，属受保护文件。两处改动全部包在 `#[cfg(target_env = "ohos")]` 块内，已有代码一行未动（只在原逻辑之后插入独立块）；非 OHOS 平台编译期整块剔除，macOS / Linux / Windows 行为逐字节不变。
- `event_loop.rs` 位于 `crates/warpui/src/platform/ohos/`，是 OHOS 专属后端文件，不参与其它平台编译。
- 本次不触碰共享的 `render_cursor`、`App::report_active_cursor_position_update_if_changed`，其它平台的光标/IME 路径不受影响。

## 验证

**复现（修复前）**：在 OHOS 设备上打开 Warp，运行会隐藏光标的 TUI（如 `codebuddy`），
调出软键盘输入拼音，观察候选词框是否停留在进入 TUI 时的位置。

**确认修好**：同一场景下候选词框应随 TUI 内光标移动。可用普通 shell 提示符作对照
（两种场景都应跟随；提示符场景本来就正常）。

**代码侧自证**：
- 编译 OHOS 目标（debug，`script/ohos/bundle`）确认门控块参与编译且无警告。
- 在 `CursorShape::Hidden` 分支确认无任何 `ctx.scene.draw_*` 调用，保证该补刷不产生绘制。
- 非 OHOS 目标 `cargo check` 确认门控块被剔除、上游逻辑不变。
