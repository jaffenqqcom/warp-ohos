# OHOS 后端物理键盘数字行无输入：缺失「KeyDown 未处理则补发文本」的兜底

关联：[[ohos-debug-lessons]]、[[2026-09-25-ohos-enter-tab-escape-no-bytes]]

- 日期：2026-09-29
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：OHOS 平台后端（`crates/warpui/src/platform/ohos/`）的可打印文本投递路径；只影响「物理键盘按下后没有任何视图处理该 KeyDown」的那一类键
- 状态：已修复，编译与覆盖装机通过；设备端最终确认（数字上屏、字母不重复）待补

## 问题描述

在 OHOS 版 Warp 上，**物理键盘主键盘区的数字行 `0-9` 按下去毫无反应**：终端不出现任何字符，其它键（字母、符号、方向键、Backspace、Ctrl+字母）全部正常。失效范围限于「主键盘数字行」——小键盘数字不受影响（它走的是另一套 IME 文本通道）。这是使用者最初报告的症状（"数字键无法输入，按数字键没有任何反应，请解决"），且**一直如此**，不是偶发。

## 问题表现

- 物理键盘按下 `0-9`，终端无任何反应；同一时刻字母、符号、控制键正常。
- 无报错、无异常日志、无卡顿；不是崩溃，也不是焦点丢失（同一次会话里字母能正常上屏）。
- 与键盘布局无关、与是否开启软键盘无关、与命令内容无关；只要焦点在终端，数字就哑。
- 与「Enter/Tab/Escape 无字节」（[[2026-09-25-ohos-enter-tab-escape-no-bytes]]）是**不同根因**：那个是按键到达了终端但 `chars` 为空，这个是文本**根本没有被投递**。

## 问题原因

根因是 **OHOS 后端缺了 winit 那条「KeyDown 未被任何视图处理 → 用按键自带 `chars` 补发一次 `TypedCharacters`」的兜底**，而 OHOS 的 IME 又恰好不把主键盘数字行转成文本，于是数字按下的文本两边都没有来源。

### 因果链

1. **GUI 前端只靠 `TypedCharacters` 插入可打印文本。** `Event::KeyDown` 路径对可打印字符是明确不处理的：`app/src/terminal/block_list_element.rs:1284` 的 `key_down(chars)` 门槛是 `!chars.is_empty() && chars.chars().all(|c| c.is_control())`，普通可打印字符返回 `false`。这条设计的意图就是「可打印文本应由 `TypedCharacters` 送来」。
2. **winit 后端与集成测试驱动都实现了那条兜底。** winit 在派发 `KeyDown` 后，若未被处理且不处于组合态，就用按键自带的 `chars` 补发 `TypedCharacters`（`crates/warpui/src/windowing/winit/event_loop/mod.rs:1080` 附近）；集成测试驱动同理（`crates/warpui_core/src/integration/step.rs:660` 附近）。
3. **OHOS 路径没有这条兜底。** 整个 ohos 路径里唯一的 `TypedCharacters` 来源是 ArkTS IME 插件的 `insertText` → `ImeEvent::TextInputEvent` → `TypedCharacters`（`crates/warpui/src/platform/ohos/event_loop.rs` 的 `handle_ime`）。
4. **OHOS 的 IME 不把主键盘数字行转成文本。** 它只把一部分物理键（字母、符号）转成 `insertText`；数字行的 key-down 不被它消费，因而一路走到了 NDK 的 `handle_key`，构造出正确的 keystroke 与 `chars`，但派发后没有任何视图处理，事件到此为止 —— 文本再没有第二个来源。

### 设备日志证据（`diag` tag，pid 23482）

数字键（`Key3`/`Key5`/`Key8` 等共 8 次）——NDK 全收到、映射正确、派发后 `handled=false`，**之后没有任何 `TypedCharacters` 跟进**：

```
raw code=Key3 action=Down ctrl=false alt=false shift=false
code=Key3 keystroke={ ... key: "3" } chars="3"
dispatch_to_active_window: KeyDown key="3" chars="3" handled=false     ← 到此为止
raw code=Key3 action=Up
```

字母键（`S`/`D`/`F` 等共 8 次）——**`handle_key` 只见 `Up`，`Down` 被 IME 吃掉；文本由 IME 送来，走的是另一条通道**：

```
handle_ime: TextInputEvent text="d"
dispatch_to_active_window: TypedCharacters chars="d" handled=true      ← 字母走这条，能输入
raw code=D action=Up
```

计数佐证：该次捕获中 `handle_ime TextInputEvent` 共 9 条，**全部是字母/符号，没有一条是数字**。

这张对照表把故障锁死在「数字的文本没有任何生产者」这一段：不是映射错（`keycodes.rs` 的 `text_base` 对数字行自洽）、不是被快捷键吃掉（全仓无裸数字绑定）、不是事件没到（`raw code=KeyN action=Down` 已证）。

### 为什么字母不受影响、且补发不会造成重复

字母的 key-down 在 IME 层就被消费，**根本没有走到 `handle_key` 之后的派发**（日志里字母只有 `Up`）。所以「KeyDown 未被处理才补发」这条兜底对字母永远不触发，字母的文本仍只由 IME 送一次 —— 修复不会让字母出现两次。

## 解决方案

**根因修复：在 OHOS 事件循环的 `AppEvent::Input` 分支补上那条兜底 —— 先派发 KeyDown，未被处理且按键自带非空文本时，补发一次 `TypedCharacters`。**

`crates/warpui/src/platform/ohos/event_loop.rs` 的 `process_event`：

```rust
AppEvent::Input(input) => {
    // The GUI front-ends insert printable text only through
    // `TypedCharacters`: the terminal's `KeyDown` handler deliberately
    // declines printable characters so that one can follow. The OHOS IME
    // converts only some physical keys to text — the main-keyboard digit
    // row arrives as a bare key event with no IME text — so a press that
    // no view handled falls back to the text the key event carries, as
    // the winit back-end and the integration-test driver do. Keys the IME
    // does convert never reach here: it consumes their key-down while
    // producing the text, so this cannot double-insert.
    let fallback_chars = match &input {
        WindowEvent::KeyDown {
            keystroke,
            chars,
            is_composing,
            ..
        } if !*is_composing && !keystroke.cmd && !chars.is_empty() => Some(chars.clone()),
        _ => None,
    };
    let handled = dispatch_to_active_window(ui_app, callbacks, "input", input);
    if handled == Some(false)
        && let Some(chars) = fallback_chars
    {
        dispatch_to_active_window(
            ui_app,
            callbacks,
            "input text",
            WindowEvent::TypedCharacters { chars },
        );
    }
}
```

`dispatch_to_active_window` 的返回类型由 `()` 改为 `Option<bool>`：`None` 表示事件被丢弃（无焦点窗口或窗口已消失），`Some(handled)` 表示已派发并回报是否被处理。这一改动使调用方能判断「要不要补发」，其余调用点（文件拖拽、文件投放）用 `let _ =` 接住返回值。

补发的门控与 winit 保持一致：`!is_composing`（组合态文本由 IME 统一提交）、`!keystroke.cmd`（带 Cmd 的按键不是文本输入）、`!chars.is_empty()`（没有文本可补）。

被否决的替代方案：

- **在 `keycodes.rs` / 终端视图里对数字行特判塞文本**：把通用的「文本投递」知识挪进了平台键映射层或通用视图层，污染非 ohos 路径的上游代码，且只补数字、符号/未来其它「IME 不转的键」仍会漏。兜底放在事件循环入口，一次覆盖所有同类键。
- **给数字行加 Terminal Context 绑定**：要为每个键逐个补，且会与终端的文本插入路径争抢事件，方向错误。
- **依赖 IME 插件改造（让 ArkTS 侧把数字行也 `insertText`）**：需改动 `openharmony-ability` 框架仓的 ArkTS 插件，影响面外溢到框架，且与「能用 core 直接实现就不新增插件」的既定原则相悖。事件循环里的兜底是纯 Rust、就地闭环。

## 验证

已完成：

- `./script/ohos/bundle` 通过（dev profile，无 error、无 unused warning）；`./install-local.sh` 覆盖安装成功（未使用 `--reinstall`，沙箱数据保留），应用自动重启。

设备端确认（待补）：把 Warp 窗口切到前台并点击取焦，依次输入 `123` 与 `abc`。判据：

- 数字 `123` 应逐字上屏（修复前无任何反应）；
- 字母 `abc` 应各出现**一次**（修复后仍由 IME 单一来源投递，不得重复）。

## 修改文件

- `crates/warpui/src/platform/ohos/event_loop.rs` — **本问题的修复主体**：`AppEvent::Input` 分支新增「KeyDown 未处理则用自带 `chars` 补发 `TypedCharacters`」的兜底；`dispatch_to_active_window` 返回值由 `()` 改为 `Option<bool>` 并调整各调用点。同轮一并回滚了排查期间加入的输入路径诊断日志（`handle_key`/`handle_ime` 的临时 warn 已删，两条诊断降回 `debug`），以及 OHOS 路径上的正常流程日志清理。
- `crates/warp_logging/src/ohos.rs` — 配套改动（同轮落地的日志级别收紧）：新增 `HILOG_MAX_LEVEL = Level::Warn`，`HilogLogger::enabled` 与 `log::set_max_level` 均夹到该上限，使 hilog 只出 `warn`/`error`。与数字键根因无关，属排查降噪。

## 排查过程中的误判与死路（诚实记录）

- **误以为 `keycodes.rs` 的数字映射有问题**：静态核对 `text_base` / `digit_index` / `letter_index` 后确认数字行自洽；设备日志进一步证明 `Key1` 能产出 `key:"1"` + `chars="1"`。映射层从未出错，排除。
- **误以为数字被某条快捷键绑定吃掉**：全仓检索无「无修饰符裸数字」的按键绑定，排除。
- **`uitest uiInput keyEvent/text` 无法复现本故障**：该注入走的是 ArkTS 焦点层，不触发 XComponent 的 NDK key 回调，因此打不到 `handle_key` 这条物理键盘路径。本问题只能靠「真机按键 + hilog 捕获」取证。
- **首轮诊断日志（`info` 级）在设备上看不到**：OHOS 上 info 级应用日志不可见（即便系统全局级别调到 Debug），改为 `warn` 后才捕获到。**判定 OHOS 应用日志是否可见，先用 `warn` 落一条探针**，别在 `info` 上空等。
- **一次捕获「按了没反应」其实是焦点不在 Warp**：设备上同时挂着 `hishell`、`hipreview` 等前台任务，Warp 虽在栈顶却未拿到键盘焦点，那批按键一条都没到应用。**抓按键日志前先确认 Warp 取焦**（点击窗口），并在日志里核对是否出现 `raw code=... action=Down`。

### 调试中走过的死路（供后人省时间）

- 排查「某类键无输入」时，先分清三条通道：**NDK 物理键回调（`handle_key`）**、**IME 文本提交（`insertText` → `TypedCharacters`）**、**终端消费端（`key_down` / `to_escape_sequence`）**。本故障的文本缺口发生在「物理键走到了、但没有任何人把它转成文本」这一环，只在消费端或映射端找是找不到的。
- 判据落在**派发返回值**上：`dispatch_event(...).handled` 是 `true`/`false`，直接告诉你「有没有视图认领」。补齐兜底前，这个返回值就是唯一的判断依据 —— 平时它被丢弃，不可见。
- OHOS 与桌面后端的**文本投递契约不同**：桌面是「按键自带 text + OS 输入源」，OHOS 把一部分键的文本职责交给了 IME。凡是「某类键在 OHOS 上哑、桌面正常」的现象，优先怀疑这条契约差异，而不是键映射表。
