# OHOS 后端 Enter/Tab/Escape 不产生字节：透传场景下回车无反应

关联：[[ohos-debug-lessons]]、[[2026-09-24-warp-mode-enter-hang]]、[[2026-09-24-zsh-command-echo-duplicated]]

- 日期：2026-09-25
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：OHOS 平台后端（`crates/warpui/src/platform/ohos/`）的按键字节编码；只影响"按键原样转发给 pty"的场景
- 状态：已修复，编译与覆盖装机通过；设备端自动化自证受阻（见"验证"一节）

## 问题描述

在 OHOS 版 Warp 上，**只有 Enter 这一个键失效**：按下后终端毫无反应，命令不被提交，界面停在原地；同一次会话里字母、数字、符号、方向键、Backspace、Ctrl+字母全部正常。失效范围限于"按键被原样转发给 pty"的场景——嵌套的交互式 shell 里、`hitshell` 桥接的远端 zsh 里、以及任何长驻程序里。而在 Warp 自己的提示符处回车是好的（正因为如此，用户才能先把 `zsh`、`hitshell` 这些命令执行起来）。Tab 与 Escape 有同一缺陷（见"问题原因"末段），只是没有被单独报告。

## 问题表现

三个来自使用者的最小复现，逐步收窄了范围：

- "在 hitshell 里，输入 enter，好像没有用，输入不了。" —— hitshell 桥接的远端 zsh 里回车无效。
- "没有跑 shell integration，启动 hishell 后，回车也不执行。" —— 排除了"Warp 注入的 zsh 集成劫持了回车"这一假设。
- "我在命令行下执行 zsh，然后 zsh 里回车也不响应，其他按键都可以。" —— 决定性复现：本地提示符执行 `zsh` 起一层嵌套 zsh 后回车失效，而同一会话其它按键全部正常。

可观察特征：

- 无任何报错、无异常日志，终端静默；不是崩溃、不是卡死（其它键仍有响应）。
- 与命令内容无关，与是否跑过 shell integration 无关，与 hitshell 无关（普通嵌套 `zsh` 即可复现）。
- 提示符处回车可用（`zsh`、`hitshell` 能被提交执行），所以"编辑器提交"通道与"原样转发"通道表现不一致。

## 问题原因

根因是 **OHOS 后端给 Enter 的 `chars` 是空字符串**：键名（keystroke）是对的，字节是缺的；而终端在"原样转发"通道上不看键名、只看 `chars`。

### 因果链

1. OHOS 后端把一次按键翻译成 `KeyDown { keystroke, chars }`，有两条输入路径，**两条都没给 Enter 字节**：
   - 物理键：`handle_key` 调 `keycodes::key_event_to_chars(...).unwrap_or_default()`（`crates/warpui/src/platform/ohos/event_loop.rs:939`）。而 `keycodes.rs` 的 `text_base`（`:180`）只实现了可打印字符与符号，**没有 Enter 分支**，返回 `None`，被兜底成 `""`。
   - IME：`send_key_press`（`event_loop.rs:669`）把 `chars` 硬编码为 `String::new()`，对所有命名键一律留空。
2. **键名映射本身没问题**：`key_name` 把 `KeyCode::Enter | KeyCode::NumpadEnter` 映射成 `"enter"`（`keycodes.rs:260`）。所以问题不在"认不出这个键"。
3. 终端侧的消费顺序（`app/src/terminal/block_list_element.rs:4604` 起）：
   - 先试 `KeystrokeWithDetails { keystroke, key_without_modifiers, chars }.to_escape_sequence(...)` —— 这一层只吃**键名**（功能键、Ctrl+C0 集、方向键、带 Alt 的 meta 前缀、Backspace）。plain Enter 在未启用 kitty 协议时返回 `None`。
   - 再落到 `key_down(chars)`（`:1284`），它的门槛是 `!chars.is_empty() && chars.chars().all(|c| c.is_control())`。`""` 两个条件都不满足 → 返回 `false`。
   - 于是事件被上抛，**一个字节都没往 pty 写**。
4. **为什么提示符处却正常**：那里焦点在输入编辑器，`is_terminal_focused == false`，`block_list_element` 在事件入口就直接 `return false`；回车改由编辑器的 `enter` 键名处理：`EditorEvent::Enter` → `input_enter`（`app/src/terminal/input.rs:11513`、`:13813`），**全程不看 `chars`**。这解释了两条通道的表现差，也解释了"为什么 `zsh` 本身还能被敲进去"。
5. 对照桌面端：winit 的软键盘路径显式给字节 —— `"enter" => "\r"`（`crates/warpui/src/windowing/winit/event_loop/mod.rs:2066-2069`）；macOS 则由 NSEvent 的 text 提供同一字节。OHOS 后端缺的正是这一步。

### 为什么字节是 `\r` 而不是 `\n`

终端上 Enter 送 CR（0x0D），由 pty 行规程的 ICRNL 翻成 NL。仓库里三处独立证据一致：`escape_sequences.rs:617`（`"enter" | "numpadenter" => "\r"`）、`app/src/terminal/view/init.rs:130`（numpadenter 绑定给 `KeyDown("\r")`）、winit 软键盘 `"enter" => "\r"`。

### 同类缺陷：Tab 与 Escape 也是空的（本次一并修）

`escape_sequences.rs:615` 的 `map_special_key_to_bytes` 正是"这些命名键自己不携带文字、必须单独映射字节"的官方清单：`enter / tab / escape / backspace / insert / delete / pageup / pagedown`。逐个对照它们的兜底情况：

- `backspace`：`to_escape_sequence` 内有 `backspace_keystroke_to_escape_sequence`（`escape_sequences.rs:665` → DEL）兜底，不需要 `chars`。
- `insert` / `delete` / `pageup` / `pagedown`：Terminal Context 有绑定（`view/init.rs:140,145,645,657`）。
- 方向键 / `home` / `end`：`cursor_movement_keystroke_to_escape_sequence` 或绑定覆盖。
- **`enter` / `tab` / `escape`：既无绑定、又不进 escape 编码 —— 三个都只能靠 `chars`，因此在 OHOS 上三个都哑。** 具体症状是嵌套 shell 里 Tab 补全失效、vim 里 Esc 出不来。

### 判定过程中的关键事实

- **Ctrl+字母不受此缺陷影响**：`key_event_to_chars` 里的 `control_character`（`keycodes.rs:170`）会把 `Ctrl+A..Z` 折叠成 `0x01..0x1A`，所以 `Ctrl+C` 等一直都好 —— 这也是"其它按键都可以"的一部分原因。
- **可打印字符不受影响**：它们由 IME 的 `TextInputEvent` → `TypedCharacters`（`event_loop.rs:639`）进入编辑器，不走 `key_down` 的控制字符过滤。
- **改动面收敛**：全仓 `key_event_to_chars` 只有一个消费点（`event_loop.rs:939`），`text_base` 只服务它，补分支不会外溢。

## 排查过程中的误判（诚实记录）

- **怀疑 hitshell 的 stdout 行缓冲未 flush**：这是更早一次排查里真实存在的一个 bug（relay 循环未 flush，表现为"无回显、像卡死"），但它与 Enter 无关；该修复已保留。此轮开始时先把这条排除掉了，没有重复在 hitshell 里绕圈。
- **怀疑 Warp 注入的 zsh 集成劫持了 `accept-line`**：被使用者一句"没有跑 shell integration"直接否掉；查代码也证实 `zsh_body.sh` 只绑 `\ei`/`\ep`/`\ew`/`^P`/`zle-line-init`，从未绑 `accept-line`。
- **怀疑 `window_visibility()` 门控吞掉输入**：hilog 当时确实在持续刷 `dropping frame because no window is active`（`event_loop.rs` 中 `active_window_id()` 为 `None` 时丢弃 Frame），且 `hidumper` 显示当时焦点在别的窗口（Warp 确实在后台）。但**同一时段没有任何 "dropping input" 记录**，属"后台丢帧"这一预期现象，与 Enter 无关，排除。
- **怀疑 `has_received_precmd()` 门控压住了提交**（`input.rs:8016` 附近）：被"普通嵌套 zsh 里也复现"排除 —— 嵌套 shell 根本没有 precmd，这条门控不参与那条通道。
- **怀疑 `hitshell.hnp` 未随覆盖安装刷新**：实测 hitdaemon 软链与数据目录 mtime 与安装时刻一致，且新二进制的行为当场生效，排除。

### 调试中走过的死路（供后人省时间）

- 不要从"shell 集成 / precmd / block 状态机"方向查这个症状：这些只影响编辑器提交通道，而故障恰恰在**不经编辑器**的那条通道上。
- 不要因为"回车在提示符处是好的"就认为 Enter 事件整体正常 —— 两条通道对 `chars` 的依赖完全不同，必须分别确认。
- 判定某个键在 OHOS 上是否可用，先问"它在 `to_escape_sequence` 或 Terminal Context 绑定里有没有兜底"；没有兜底的命名键就只能靠 `chars`。

## 解决方案

**根因修复：把这三个命名控制键的字节补齐，使 OHOS 与桌面后端等价。**

`crates/warpui/src/platform/ohos/keycodes.rs` 的 `text_base` 中新增：

```rust
// The named keys whose byte is a control character rather than printable
// text. The escape-sequence encoder covers most control keys through the
// keystroke name, but leaves these three unmapped without the kitty
// protocol, so on the desktop back-ends their byte arrives as OS text and
// has to be filled in here.
KeyCode::Enter | KeyCode::NumpadEnter => Some('\r'),
KeyCode::Tab => Some('\t'),
KeyCode::Escape => Some('\x1b'),
```

`crates/warpui/src/platform/ohos/event_loop.rs` 的 `send_key_press` 中，把 `chars: String::new()` 改为按键名取字节（照抄 winit 写法）：

```rust
// An IME reports Enter by name only, so the CR byte the pty needs has to
// be filled in here, matching the winit back-end.
let chars = match key.to_lowercase().as_str() {
    "enter" => "\r".to_string(),
    _ => String::new(),
};
```

注意 `text_base` 里 `ctrl` 的折叠仍会生效：`Ctrl+Enter` 经 `control_character('\r')` 返回 `None`，`chars` 仍为空，不会被误当成裸 Enter 送出。

被否决的替代方案：

- **给 `enter` 加一条 Terminal Context 绑定**（如 `FixedBinding::new("enter", TerminalAction::KeyDown("\r"), ...)`）：会与编辑器的 `enter` 处理争抢事件（提示符处回车提交依赖编辑器），且要逐个键补，仍漏 Tab/Escape。
- **在终端视图里对 `keystroke.key == "enter"` 特判塞 `\r`**：把"平台字节编码"的知识挪进了通用视图层，污染非 ohos 路径的上游代码，违反移植改动集中在 ohos 文件里的原则。
- **只修 Enter 不管 Tab/Escape**：三个键是同一根因、同一处代码，分开修只会留下两个已知哑键。

## 验证

已完成：

- `./script/ohos/bundle --check-only` 通过（dev profile，无 error）。
- `./script/ohos/bundle` 产出 HAP 并验签通过；`./install-local.sh` 覆盖安装成功。本次只有 `libs/arm64-v8a/libcore.so` 变化，HNP 五个 payload 均未改动（未使用 `--reinstall`，沙箱数据保留）。

设备端自动化自证（未完成，判据已备好）：在 Warp 提示符用 `uitest uiInput text 'zsh'` + `keyEvent 2054` 起第一层嵌套 zsh（此层靠编辑器提交，修前也能起，作对照），再注入一次 `zsh` 起第二层；用 `/proc/<pid>/{stat,cmdline}` 数嵌套层数，**深度 ≥ 2 即证明 Enter 的 `\r` 真进了 pty，停在 1 即未生效**。选用 `zsh` 单字命令是为了避开 `uitest` 丢空格的已知问题。阻塞原因：装包后 Warp 的窗口未在 WMS 中注册（`hidumper -s WindowManagerService` 查不到该应用的窗口，同时 hilog 持续 `dropping frame because no window is active`），`uitest` 找不到注入目标；此时硬注入会落到别的前台应用上，故停下未试。

## 修改文件

- `crates/warpui/src/platform/ohos/keycodes.rs` — `text_base` 新增 `Enter|NumpadEnter => '\r'`、`Tab => '\t'`、`Escape => '\x1b'` 三个分支（**本问题的修复主体**）。
- `crates/warpui/src/platform/ohos/event_loop.rs` — `send_key_press` 由硬编码空 `chars` 改为按键名给 `enter` 填 `"\r"`（覆盖软键盘/IME 路径）。

无诊断探针需要回滚：本轮定位靠静态读代码 + 设备侧取证（`/proc`、`hilog`、WMS dump）完成，未在源码中新增任何调试日志，也未落任何临时文件或截图。

## 遗留（独立问题，未夹在本次改动内）

- `keycodes.rs:260` 把 `NumpadEnter` 也映射成键名 `"enter"`，而桌面端约定是 `"numpadenter"`（`view/init.rs:129` 存在该绑定）。字节输出现已正确（`\r`），影响面仅限"用户自定义重绑小键盘回车不生效"。
- Warp 处于后台时 `event_loop.rs` 持续刷 `dropping frame because no window is active`，且 WMS 中查不到其窗口。属既存现象，与按键无关。
