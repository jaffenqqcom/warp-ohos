# Warp 模式（非 PS1）下输入命令并回车后终端卡死、无输出

关联：[[ohos-debug-lessons]]、[[2026-09-24-zsh-command-echo-duplicated]]

- 日期：2026-09-24（结论于当日晚修订，见顶部说明）
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：OHOS 移植版 zsh 集成，仅 Warp 模式（`WARP_HONOR_PS1 != 1`，默认）
- 状态：已修复，设备实测通过

> **本文档结论修订过一次。** 初版把根因判为"本平台 zsh 的一个渲染特性"，并据此改了 `zsh_body.sh` 的提示符写法。当日稍晚查明**真根因是 shell 缺 terminfo 终端描述**：那次 `zsh_body.sh` 改动只是规避，已还原为上游；真正的修复是给 shell 设 `$TERMINFO`。下文以修订后的结论为准，初版的误判过程保留在"排查过程中的误判"一节。

## 问题描述

在 OHOS 设备上以 Warp 模式（默认模式，即 `WARP_HONOR_PS1` 未设为 `1`）打开终端后，输入任意命令并按回车，命令不会被执行：终端没有任何输出，界面停在原地，看起来像卡死。若改用 PS1 模式（`WARP_HONOR_PS1=1`）打开同一会话，同样操作完全正常。问题在 Warp 模式下 100% 必现，与所输入的命令内容无关。

键盘输入本身是通的（字符能在编辑器里正常回显），被吞掉的只是"回车提交"这一步。

## 问题表现

- Warp 模式：输入命令 → 按 Enter → 无任何输出，命令不被执行，终端无响应。
- PS1 模式（`WARP_HONOR_PS1=1`）：完全正常。
- 字符回显正常，说明 IME / 键盘事件通道本身没问题，问题出在"提交执行"环节。
- 设备日志（加探针后）显示：Warp 模式启动序列里有
  `Received prompt marker: StartPrompt { kind: Initial }`，
  但**始终没有** `Received prompt marker: EndPrompt`。
- 同一批探针显示 `LineEditorStatus` 的 `EndPrompt` 分支从未命中，`is_line_editor_active` 恒为 `false`；`PtyController::can_write_to_pty()` 因此恒为 `false`，命令写入被一直压在 `pending_writes` 队列里不 flush。

## 问题原因

根因是 **zsh 拿不到 terminfo 终端描述**，致使其行编辑器（ZLE）丢弃了带可见文本的 `%{...%}` 零宽段，连同段内的 OSC 133 提示符标记一起丢掉。

### 因果链

1. zsh 集成靠 OSC 133 序列向 Warp 通告行编辑器状态：
   - `ESC ] 133 ; A BEL`（`prompt_prefix`）标记提示符开始；
   - `ESC ] 133 ; B BEL`（`suffix`）标记提示符结束。
2. Warp 侧 ansi 解析器收到 `133;B` 会派发 `EndPrompt` 事件：
   `crates/warp_terminal/src/model/ansi/mod.rs` 的 OSC 133 分派 → `PromptMarker::EndPrompt`。
3. `LineEditorStatus::handle_model_event` 只在收到 `EndPrompt`（且本会话是 zsh 且已收到过 precmd）时才把行编辑器置为 active：`app/src/terminal/line_editor_status.rs` 的 `EndPrompt` 分支（约 132–143 行）。
4. `PtyController` 只有在行编辑器 active（`can_write_to_pty()` 为真）时才把排队中的命令 flush 到 pty；否则写请求一直留在 `pending_writes`。
5. 因此：**`133;B` 没到 → `EndPrompt` 永不匹配 → 行编辑器永不 active → 命令永不 flush → 卡死。**

这段因果链自始至终成立，**错的只是它上面那一步——`133;B` 为什么没到**。

### 为什么 `133;B` 没到：缺 terminfo，而非"平台渲染特性"

**规则本身**：zsh 在拿不到可用终端描述（terminfo）时，ZLE 会**整段丢弃任何"内容含可打印文本或命令替换输出"的 `%{...%}` 段**——连段内的 OSC 一起丢；`%{<纯转义序列>%}`（段内只有转义序列、没有可见文本）仍正常输出。丢弃发生在 **ZLE 路径**，`print -P` 路径不受影响。

**设备上为何拿不到描述**：hnp 提供的 zsh（`zsh.hnp`）把 terminfo 搜索路径**编译期写死**为构建机目录 `/tmp/zshbuild/hnp/zsh/share/terminfo`（设备上不存在），而 Warp spawn pty 时只设 `TERM=xterm-256color`、从未设 `TERMINFO`；系统库 `/usr/share/terminfo` 又在沙箱读不到（详见 `2026-09-24-zsh-command-echo-duplicated.md`）。

**为什么正好命中 Warp 分支**：上游 Warp 分支的提示符是**整块**写法：

```zsh
# PROMPT_SUBST 开
PROMPT="%{$prompt_prefix\$(_warp_stripped_prompt)$suffix%}"
# PROMPT_SUBST 关
PROMPT="%{$prompt_prefix$REPLY$suffix%}"
```

展开后该段的形状是：`%{` + `OSC 133;A` + **可见文本（提示符正文）** + `OSC 133;B` + `%}`。段里含可见文本，于是缺 terminfo 时整段被丢，`133;A` 与 `133;B` 一起消失。

与之对照，PS1 分支一直用**分块**写法：

```zsh
PROMPT="%{$prompt_prefix%}$ORIGINAL_PROMPT%{$suffix%}"
```

OSC 各自独占 `%{...%}` 段、可见文本裸露在外。**注意：PS1 模式能跑，不是因为 PS1 分支有何特殊，而是因为它的 `%{...%}` 段里没有可见文本**，不触发丢弃规则。这正好解释了"PS1 能跑、Warp 不能跑"的现象差。

### 与"命令回显重复"是同一根因

同日修复的 `2026-09-24-zsh-command-echo-duplicated.md`（`pwd` → `pwdpwd`）是**同一个 terminfo 缺失**的另一症状：缺描述时 ZLE 的重画退化为"追加"而非"覆盖"。两者由**同一次修复**（设 `$TERMINFO`）一并治好。

### 判定过程中的关键事实

- **变量层是干净的**：设备实测 `WARP_P=[%{<E>]133;A%m %~ %# <E>]133;B%}]`，`$prompt_prefix` / `$suffix` 里确实带着 133 序列，命令替换也正常返回。问题不在变量拼装。
- **与本平台 zsh 版本、fork 权限无关**：本地 `/usr/bin/zsh` 5.9 上，只要 terminfo 可用，整块 / 分块两种写法 133 均正常注入；唯一能让整块写法丢 133 的变量是**terminfo 是否可用**。
- **`honor_ps1` 不参与输出处理**：它在 warp 侧只用于 spawn 时决定是否注入 `WARP_HONOR_PS1` 环境变量（`crates/warp_terminal/src/local_tty/unix.rs`），输出解析路径根本不读它。所以"warp 侧对 PS1 有特判"被排除。

### 修订时的决定性实验（本地 `/usr/bin/zsh` 5.9 + pty 真 ZLE）

- 上游**整块**写法 + **缺** terminfo → `133;A = 0`、`133;B = 0`（丢）
- **分块**写法 + **缺** terminfo → `133;A = 2`、`133;B = 2`（存活）
- 上游**整块**写法 + **有** terminfo → `2 / 2`
- **分块**写法 + **有** terminfo → `2 / 2`

"缺 terminfo"用"未设 `TERM`"或"`TERM=<不存在的条目>`"构造。**用 `TERMINFO=<空目录>` 无法剥夺**——zsh 会继续回落到 `/etc/terminfo`、`/usr/share/terminfo`；这是初版踩过的实验设计错误。

## 排查过程中的误判（诚实记录）

初版把"整块 `%{...%}` 在本平台 zsh 上丢 OSC"当成结论，之所以成形，有一个**时间线陷阱**：

- `zsh_body.sh` 的分块改动 ≈ 当日 02:00 落地；
- `TERMINFO` 修复 ≈ 当日 08:50 落地。

分块改动在前，且改完当场"设备实测通过"，于是被当成真修复。其实分块写法在缺 terminfo 时**碰巧**能让 `133;A/B`（块内没有可见文本）存活，症状被暂时掩盖。待查明真因、设好 `$TERMINFO` 后，上游整块写法恢复完全正常——分块改动不再有任何必要，遂还原。

### 调试中走过的死路（供后人省时间）

- 怀疑 fork / 权限：设备实测命令替换正常返回，排除。
- 怀疑 zsh 版本差异：本地同版本 zsh 两形态均正常，排除"脚本写法本身有错"。
- 怀疑 precmd hook 覆盖 `PROMPT`：经 warp 写入 pty 的探针命令同样卡死，无法作为干净对照，放弃。
- 怀疑 `LineEditorStatus` 状态机有 bug：探针证明状态机逻辑没问题，只是从没等到 `EndPrompt`。
- 本地 pty 复现：`zle .reset-prompt` 在命令上下文报 `widgets can only be called when ZLE is active`，不是有效实验手段，改用直接对比提示符形态 × terminfo 状态。
- **把"缺 terminfo"的实验做错**：先用 `TERMINFO=<空目录>`，因 zsh 有回落路径而无效；必须用"未设 `TERM`"或"`TERM=<不存在条目>`"才能真正剥夺。

### 过程教训

定位期间在已获授权的文件上反复请求授权、反复要求用户去设备手工做实验，消耗了用户耐心。此后确立做法：授权范围内直接改、设备验证一次到位、方向性认可即动手。

另有一条同源教训：**当"改写脚本"和"改运行环境"都能让症状消失时，先问哪个是根因**。本问题里两者都"有效"，但只有后者是根因；前者掩盖了真因而偏离了上游。

## 解决方案

**根因修复**：在 OHOS 专属启动路径里，把 shell 的 `$TERMINFO` 指向它自己旁边的那份数据库。

- `crates/entry_ohos/src/launch_app.rs` 新增 `point_shell_at_bundled_terminfo()`：用 `canonicalize` 解出 hnp 真实安装根（实测 `/data/app/zsh.org/zsh_5.9/`），取其 `share/terminfo`，`is_dir()` 通过才设 `$TERMINFO`；由 `prepare_process_environment` 调用。完整实现与理由见 `2026-09-24-zsh-command-echo-duplicated.md`。

**`zsh_body.sh` 已还原为上游**：初版的分块改动只是规避，根因修复后不再需要，`app/assets/bundled/bootstrap/zsh_body.sh` 已回退到上游 HEAD（`git diff` 为空）。

被否决的替代方案：

- 保留 `zsh_body.sh` 的分块改动：能在缺 terminfo 时"碰巧"出标记，但掩盖真因、偏离上游，且治不了命令回显重复。
- 改 Warp 侧 ansi 解析器用 `133;A` 兜底推断编辑器 active：掩盖提示符集成不完整的事实，影响面远超本问题。
- 让脚本同时输出独立的 `133;B`：等于同一个提示符发两份标记，语义重复、易被去重或乱序。

## 验证（设备实测，2026-09-24）

装包 + 前台拉起后：

- hilog 出现 `prepare_process_environment: TERMINFO=/data/app/zsh.org/zsh_5.9/share/terminfo`。
- Warp 模式启动序列有 `StartPrompt`，**且有 `EndPrompt`**（初版缺的正是 EndPrompt）。
- uitest 注入真实命令（`pwd`）：`Received Preexec hook` → `Received CommandFinished hook` → `Received Precmd hook` 链完整，输出 `/data/storage/el2/base/haps/entry/files/.config`，新 prompt 正常，**无卡死**。
- **无命令回显重复**（每条命令只显示一次）。
- 无 OSC 9278 malformed 告警（与本问题无关的同期修复，见 `2026-09-24-osc-9278-marker-conflict.md`）。

## 修改文件

- `crates/entry_ohos/src/launch_app.rs` — 新增 `TERMINFO_ENV` / `TERMINFO_SUBDIR` 常量与 `point_shell_at_bundled_terminfo()`，并在 `prepare_process_environment` 中调用。**这是本问题的真正修复。**
- `app/assets/bundled/bootstrap/zsh_body.sh` — 由初版的分块改动**还原为上游 HEAD**，净改动为零。

调试探针（初版加、均已回滚，仅记录不属修复）：

- `app/src/terminal/` 下 7 个文件的 28 处 `[OHOS-PROBE]` 探针 — 已回滚，`git status` 对该目录为空。
- `crates/entry_ohos/src/launch_app.rs` 的临时 `RUST_LOG` 诊断块 — 已回滚。

## 附：初版对照脚本

`zsh_body.sh.original`（1777 行，md5 `c1e9e21957d9a22d7dce39d2801907d2`）。

修订后它**与 `app/assets/bundled/bootstrap/zsh_body.sh` 当前内容完全相同**（脚本已还原为上游，两者 md5 一致）。它最初保存的是初版"修改前"脚本用于对照，现已无对照价值，可删。
