# OSC 9278 编号冲突：设备 smartshell 与 Warp shell hook 撞号导致日志噪音

关联：[[ohos-debug-lessons]]

- 日期：2026-09-24
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：OHOS 移植版全部交互式 zsh 会话（只要设备 `/etc/zshrc` 会 source smartshell 就必现）
- 状态：已修复，设备实测通过（含 A/B 探针判别）

## 问题描述

在 OHOS 设备上，Warp 启动的 zsh 会主动 source 设备自带的 `/etc/zshrc`（`app/assets/bundled/bootstrap/zsh_body.sh:1289-1290`），而该文件里又 `source /etc/smartshell.zsh`。smartshell 是设备提供的 shell 增强框架，它在每次命令的 `preexec` / `precmd` 时会向 pty 写入一条 OSC 序列，编号用的是 **9278**。

9278 恰好是 Warp 私有的 shell hook OSC 编号（`crates/warp_terminal/src/model/ansi/mod.rs:52` 的 `WARP_OSC_MARKER = b"9278"`）。于是 Warp 把 smartshell 的载荷当成自己的 hook 去反序列化，失败后打一条 `log::warn!`。

问题的实际危害是**日志噪音**：每条命令稳定产生 2 条警告，会污染 hilog，并作为 breadcrumb 进 Sentry。终端显示、命令执行、Warp 的各项功能都不受影响。

## 问题表现

- 设备 hilog 中反复出现：
  `Received malformed SourcedRcFileForWarp hook missing field \`hook\``（源码 `crates/warp_terminal/src/model/ansi/mod.rs:654`，级别 `warn`）。
- **每条命令 2 条**：smartshell 的 `smartshell_preexec` 对每条命令无条件发一条；`smartshell_precmd` 仅在上一条命令确实执行过时再发一条。因此"命令确实跑过"的交互模式下每条命令必然 2 条告警。
- 与命令内容、终端宽度、提示符模式无关，100% 必现。
- 终端里看不到任何异常：命令照常执行、输出正常、Warp 的块划分与提示符都正常。
- 只要设备 `/etc/zshrc` 存在且 source 了 smartshell，打开任意终端就会有。

## 问题原因

根因是 **Warp 与设备 smartshell 各自自选了同一个 4 位 OSC 私有编号，且 Warp 在解析时只凭"第二段是 `f`"就认定载荷属于自己，没有做归属校验**。

### 撞号的两端

Warp 的私有 OSC 号段定义在 `crates/warp_terminal/src/model/ansi/mod.rs:44-81`：

- `9277` — `WARP_IN_BAND_GENERATOR_OSC_MARKER`（in-band command output，注释写着 "9277 spells out \"WARP\" on a dialpad :)"）
- **`9278` — `WARP_OSC_MARKER`（shell hooks，就是撞车的这个）**
- `9279` — `WARP_RESET_GRID_OSC_MARKER`（reset ConPTY grid）
- `9280` — `WARP_COMPLETIONS_OSC_MARKER`（completions）

关键点：**这四个号不是 ANSI/ECMA-48 的保留号段**，是 Warp 自己挑的 4 位私有号。所以撞号不是"谁占了保留位"，而是"两边都自选、恰好撞上"。

9278 序列的第二段是子类型标记字符（`crates/warp_terminal/src/model/ansi/dcs_hooks.rs:17,22,27`）：

- `d` — `HEX_ENCODED_JSON_MARKER`（hex 编码的 JSON）
- `f` — `UNENCODED_JSON_MARKER`（未编码 JSON）
- `k` — `UNENCODED_KV_MARKER`（键值对形式）

smartshell 发的是：

```text
\e]9278;f;{"event": "PreCmd" | "PreExec", "data": { ... }}\a
```

第二段同样是 `f`，于是完整落进 Warp 的"未编码 JSON hook"分支。

### 解析链

1. OSC 分派进入 `WARP_OSC_MARKER` 分支（`mod.rs:1117`），取第二段字符得到 `f`。
2. 命中 `UNENCODED_JSON_MARKER` 分支（`mod.rs:1144`），取第三段作为 `data_str`。
3. 调 `serde_json::from_str::<DProtoHook>(&data_str)`（`mod.rs:1172`）。
4. `DProtoHook` 用**手写反序列化**，先解 envelope `RawDProtoHook`（`dcs_hooks.rs:116-121`）：
   ```rust
   #[derive(Deserialize)]
   struct RawDProtoHook {
       hook: String,
       value: serde_json::Value,
   }
   ```
   以及 `#[serde(tag = "hook")]` 的枚举定义（`dcs_hooks.rs:44-46`）。也就是说 **Warp 的 hook 载荷顶层必须带 `hook` 键**。
5. smartshell 的载荷顶层是 `event` / `data`，没有 `hook` → serde 报 `missing field \`hook\``。
6. 错误对象被传进 `handle_unencoded_hook(Err)`（`mod.rs:631`），在 `Err(err)` 分支（`mod.rs:653-655`）只打一条 `log::warn!`。

这条错误路径**不记录任何状态、不改变终端状态、不影响后续解析**——`Err` 分支里只有那一行日志。所以功能无损，纯粹是噪音。

另外注意：`validate_hook_session_id`（`mod.rs:557`）只在 `Ok` 路径被调用（`mod.rs:637-641`），撞号载荷在反序列化阶段就已经失败了，根本走不到 session 校验。

### 一个容易搞错的点：OHOS 上 Warp 的主 hook 通道走 DCS，不是 OSC

排查时容易误以为"OHOS 的 Warp hook 都走 OSC 9278"。实际上：

- `crates/warp_terminal/src/bootstrap.rs:31,66,140` 把脚本里的占位符 `@@USING_CON_PTY_BOOLEAN@@` 替换为 `cfg!(windows).to_string()`。
- 于是 `zsh_init_shell.sh:12` 在非 Windows 上走 `else` 分支：`printf '\x1b\x50\x24\x64%s\x1b\x5c'`，这是 **DCS**（`\x1bP$d...`），不是 OSC。

所以准确表述是"**非 Windows 一律走 DCS**"，**不是 OHOS 专属分支**，其它非 Windows 平台（macOS/Linux）同样如此。

在 OHOS 上真正使用 OSC 9278 的只有两处例外：

- `unknown_init_subshell.sh:8` —— 纯 OSC `\e]9278;f;{"hook": ...}`（注意它带 `hook` 键，是 Warp 自己的格式）。
- `zsh_body.sh:1082`、`:1178` —— 远程 SSH 场景用 `\e]9278;d;`（hex 编码）。

两条通道共用同一个处理函数 `handle_unencoded_hook`（`mod.rs:631`）：DCS 在 `unhook()` 的 `mod.rs:783` 调用，OSC 在 `mod.rs:1144` 调用。**这也是"改哪一端"的关键约束**——动这个函数会同时影响 OHOS 的主路径（DCS）。

### 排查中走过的死路（供后人省时间）

- **想改 smartshell 或 `/etc/zshrc`**：设备系统文件不可改，也不该由移植版去改；而且换了设备/固件还会变。放弃。
- **按"9278 是保留号"来推理**：一度以为可以要求对方避让。核实后是 Warp 自己挑的 4 位私号码，两边都是自选，没有"谁让谁"的法理依据。定调为"Warp 侧做归属判断"。
- **想用"首次告警后静音"做降噪**：需要引入状态（记"已经报过"），会改变行为语义，且掩盖后续真实错误。否决。
- **想改 Warp 的编号（整套 9277-9280 换号）**：见下节统计，改动面大，且换一个 4 位号仍可能与设备侧或其它工具的私有号再撞，属治标不治本。否决。

## 解决方案

在 **OSC 通道的入口**、`serde_json::from_str::<DProtoHook>` **之前**，加一层"载荷归属判断"：把载荷先解成 `serde_json::Value`，若顶层没有 `hook` 键，就判定它不是发给 Warp 的，按 `debug` 记录并 `return`。整块用 `#[cfg(target_env = "ohos")]` 包裹。

修改后的分支（`crates/warp_terminal/src/model/ansi/mod.rs:1144-1174`，新增部分为 `:1153-1167`）：

```rust
UNENCODED_JSON_MARKER => {
    // The payload for the OSC is contained in the third parameter.
    let Some(data_str) = params
        .get(2)
        .map(|osc_data| String::from_utf8_lossy(osc_data))
    else {
        log::warn!("Warp OSC marker did not contain payload");
        return;
    };
    // 9278 is Warp's private marker, not a reserved number: on OHOS the
    // device's own `/etc/zshrc` sources a shell integration that emits the
    // same marker with its own JSON shape. Warp's unencoded hook payloads
    // always carry a top-level `hook` key, so a payload without one was not
    // addressed to Warp and must not be reported as malformed.
    #[cfg(target_env = "ohos")]
    {
        let is_warp_hook_payload =
            serde_json::from_str::<serde_json::Value>(&data_str)
                .is_ok_and(|value| value.get("hook").is_some());
        if !is_warp_hook_payload {
            log::debug!("Ignoring OSC 9278 payload that carries no `hook` key");
            return;
        }
    }
    safe_debug!(
        safe: ("Received Warp OSC string for shell hook"),
        full: ("Received Warp OSC string for shell hook with JSON payload: {:?}", data_str)
    );
    let hook = serde_json::from_str::<DProtoHook>(&data_str);
    self.handle_unencoded_hook(hook)
}
```

### 判据为什么用 `hook` 键

这是 **Warp 协议自身的约定**，不是 smartshell 的特征串：

- `DProtoHook` 的 envelope（`dcs_hooks.rs:116-121`）把 `hook: String` 声明为必填字段；
- 枚举本身也标了 `#[serde(tag = "hook")]`（`dcs_hooks.rs:44-46`）；
- Warp 自己唯一的 OSC 未编码 hook 生成点 `unknown_init_subshell.sh:8` 输出的就是 `{"hook": "HOOK_NAME", "value": {...}}`。

因此判据的语义是准确的：**带 `hook` 键 = 发给 Warp 的载荷；不带 = 别人的载荷**。用"不含 smartshell 特征"这类否定式判据会把"真坏掉的 Warp 载荷"也一起放过，用协议字段则是正向定义。

### 为什么只改 OSC 入口、不动 DCS

- OHOS 的主 hook 通道走 DCS（见上文），而 DCS 上的载荷本来就是 Warp 自己的脚本生成的，不存在撞号，无需改动。**DCS 零改动 = 主路径零风险**。
- 若改共用函数 `handle_unencoded_hook` 的 `Err` 分支，会同时改变 DCS 主路径的报错行为，且无法区分"别人的载荷"与"真的格式坏了的 Warp 载荷"。

### 行为影响面（已逐条核实）

- **带 `hook` 键的载荷**：走原路径，逐字节相同，不受影响。
- **不带 `hook` 键的载荷**：不再进 `from_str`，改记 `debug` 并返回。原先的错误路径副作用只有一个 `log::warn!`，不记录状态、不改变终端状态，因此行为变化仅限于日志。
- **报错能力保留**：带 `hook` 键但结构错（例如未知 variant）仍然会走到原 `from_str` 失败并打 warn。
- **对 Sentry 也降噪**：`crates/warp_logging/src/native.rs:555-560` 把 Error/Warn/Info 记为 breadcrumb、Debug/Trace 忽略，所以换成 `debug` 后连 breadcrumb 都不再产生。
- **平台隔离**：`#[cfg(target_env = "ohos")]` 包裹，其它平台编译时整块剔除，行为逐字节不变（注释留在 `#[cfg]` 之外，不参与编译）。

### 被否决的替代方案

- **方案 A：把 `handle_unencoded_hook` 的 `Err` 分支 warn 降为 debug。**
  问题：该函数被 DCS 与 OSC 两条通道共用，会波及 OHOS 主路径（DCS）；且判据粗，等于把"真实格式错误的告警"一并关掉。
- **方案 C：改 Warp 的私有编号（避开 9278）。**
  改动量实测（本次统计）：作为编号字面量出现的 `9278` 在仓库中共 **36 处** —— 12 个 bootstrap 脚本文件里 35 处，`mod.rs:52` 1 处。若连整套 9277-9280 一起换，脚本侧共 **71 处**，Rust 侧 4 个常量（加 1 处 dialpad 注释）；测试侧 `mod_tests.rs` 另有 6 处 9280 用例，且 `mod_tests.rs` 中**没有** 9277/9278/9279 的用例。
  更关键的是**换了也治标不治本**：4 位自选号本就不唯一，撞上设备/CDN/其它工具的自选号只是概率问题。真正的缺陷是"收到同号载荷时不校验归属"，修这一点才根除。

### 验证

`./script/ohos/bundle` 编译通过、签名校验全绿，`hdc install -r` 装包成功，设备复验闭环。

复验方法：`hdc shell aa start -a EntryAbility -b com.hiwarp.terminal` 把 Warp 拉到前台，再用 `uitest uiInput text '<命令>'` + `uitest uiInput keyEvent 2054`（2054 = `KEYCODE_ENTER`）把命令真实注入 Warp 的输入框，然后 `hdc shell hilog -r` 清 buffer、抓 `diag` tag。这是本平台上唯一能对 GUI 终端做真实命令注入的手段（前提：Warp 必须是前台窗口，否则注入会落到别的应用里）。

| 探针 | 注入的载荷 | 期望 | 实测 |
| --- | --- | --- | --- |
| 真实命令 `pwd` | 由 smartshell 自己发出（顶层无 `hook`） | 无告警 | **0 条告警**（修复前该命令必产 2 条） |
| Test B | `{"hook":"NotAVariant","value":{}}` | 原样报错 | 报 `Received malformed SourcedRcFileForWarp hook unknown variant \`NotAVariant\`` ✓ |
| Test A | `{"probe":1}`（与 smartshell 同形） | 静默 | **静默** ✓ |

三条同时成立的意义：

- Test B 证明 **OSC 9278 的 `f;` 通道在修复后仍完全可用**，带 `hook` 键的载荷逐字节走原路径，真实格式错误的报错能力没有被削弱——这不是"把通道关掉"换来的安静。
- Test A 证明 **无 `hook` 键的载荷确实被守卫拦下**，这正是 smartshell 的载荷形态。
- 真实命令 `pwd` 的 0 告警，是问题现象的直接消除（对照：修复前每条命令 2 条）。

另需注意：本次全程 `W` 级别的其它日志（例如 `[fontdb] Fallback to loading from known font dir paths.`、`[warp::terminal::view] Expected to have session ...`）都照常出现在 hilog，说明**不是 hilog 把 `W` 过滤掉了**才显得安静。而新增的 `log::debug!("Ignoring OSC 9278 payload ...")` 一行**看不到**是正常的——`crates/warp_logging/src/native.rs:606` 把基线级别钉在 `LevelFilter::Info`（仅 `parse_default_env()` 允许 `RUST_LOG` 覆盖），debug 不进 hilog。

会话本身无回归：`InitShell` / `Bootstrapped` / `Preexec` / `CommandFinished` / `Precmd` / `InputBuffer` 各 hook 收发正常，`TERMINFO` 修复仍生效（`prepare_process_environment: TERMINFO=/data/app/zsh.org/zsh_5.9/share/terminfo`），终端块渲染与命令输出正常。

## 修改文件

- `crates/warp_terminal/src/model/ansi/mod.rs` — 在 OSC `UNENCODED_JSON_MARKER` 分支（`:1153-1167`）新增 OHOS 条件下（`#[cfg(target_env = "ohos")]`）的载荷归属判断：顶层无 `hook` 键的 OSC 9278 载荷按 `debug` 忽略并提前返回；消除设备 smartshell 与 Warp 的 9278 撞号日志噪音。DCS 通道（`:783`）与共用处理函数 `handle_unencoded_hook` 未改动。

未改动其它任何文件：本修复不涉及 bootstrap 脚本、`zsh.hnp` 内容、`/etc/zshrc` / smartshell，也不改变其它平台的行为。
