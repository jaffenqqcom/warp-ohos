# 窗口大小变化后 shell 内运行的程序不刷新

## 问题描述

在 Warp（OHOS 移植版）中拖动或改变窗口大小后，终端里运行的交互式程序（vim、htop、less、tmux 等）不会按新尺寸重绘，画面保持旧布局直到用户手动按键触发重绘。shell 自身提示符能正确重排，出问题的只有被 shell 放到前台作业里的那些程序。

这不是 Warp 侧的布局问题：终端仿真器的行列数算对了，真 shell 的 pty `winsize` 也确实被更新了，`stty size` / `TIOCGWINSZ` 能读到新值。丢的是「尺寸变化了，请你重绘」这个通知，也就是 `SIGWINCH`。

## 问题表现

- 改变窗口大小后，`vim` / `htop` 的界面框线仍按旧行列绘制，被裁切或留白。
- 在 shell 里执行 `stty size`，输出的是**新**尺寸，说明 winsize 已到位。
- shell 自己的提示符（zsh）会正确重排 —— 因为 zsh 不是靠 `SIGWINCH` 重排那么简单的路径，且它在 resize 后本来就会重绘。
- 用探针脚本（`trap 'echo WINCH $(stty size)' WINCH`）前台运行，改变窗口大小后**一条 `WINCH` 都不打印**。
- 同一探针脚本在**没有 job control 介入**的场景（进程与 session leader 同进程组）下，改变窗口大小会正常打印 `WINCH`。
- 复现条件稳定：只要 shell 开了 job control（zsh 交互式默认如此），把程序放进独立前台进程组，100% 复现。后台作业（`cmd &`）不在前台组、本来就该收不到信号，属于正确行为，不在本问题范围内。

## 问题原因

### 完整链路与断点位置

窗口 resize 的数据通路本身是通的，四段全部确认过：

- **平台事件**：ArkTS `BridgeHost.ets` 注册 `componentWindow.on("windowSizeChange", ...)`，基座 `openharmony-ability` 的 `lifecycle.rs:205` 投递 `Event::WindowResize(Size)`；XComponent 的 surfaceChanged 也会投同一事件（`render/xcomponent.rs:87`）。
- **Warp 重排**：`AppEvent::Resized` → `set_surface_size` → `notify_window_resized` → warpui_core `resize_callback` → 下一帧 `after_layout` → `TerminalView::after_terminal_view_layout` → emit `Event::Resize`。
- **到本地 pty**：`Event::Resize` → `PtyIntent::Resize` → `PtyController::resize_pty` → `Message::Resize` → `local_tty/event_loop.rs:176` → `unix.rs:717` 对本地 pty master 做 `TIOCSWINSZ`。
- **到真 shell**：hitshell 每 200ms 轮询本地 pty 的 `local_size()`（`hitshell/src/main.rs:487`），变化时 `channel.window_change(cols, rows, 0, 0)`（`hitshell/src/pty.rs:228`）→ hitdaemon `window_change_request`（`hitdaemon/src/sshd.rs:168`）→ `pty::resize` → `set_size` 对真 pty master 做 `TIOCSWINSZ`（`hitdaemon/src/pty.rs:94`）。

断点就在最后一步之后：`TIOCSWINSZ` 已经把 winsize 写进内核，但内核**没有**向该 tty 的当前前台进程组投递 `SIGWINCH`。

### 内核行为的实测证据

用与本机 hitdaemon 逐字等价的方式建 pty（`posix_openpt(O_RDWR|O_NOCTTY)` + `grantpt`/`unlockpt`、fork 前在 master 上设初始 winsize、`O_NOCTTY` 打开 slave、先 dup2 到 0/1/2 再 `setsid` 再 `TIOCSCTTY`、resize 打 master），做四组对照：

```
场景                                      TIOCSWINSZ 后      再 kill(-pgrp, SIGWINCH)
应用与 session leader 同进程组            收到                收到
应用独立前台组（setpgid + tcsetpgrp）     收不到              收到
```

两端都用 `pthread_sigmask(SIG_BLOCK)` + `sigtimedwait(..., 5s)` 同步等待内核投递，绕开语言层 handler 时序问题，结论一致。第三组（显式 kill）是关键对照：它证明应用本身、信号处理、进程组寻址全都正常，唯一没发生的就是内核那次投递。

进一步观测到：

- `tcgetpgrp(slave)` 读到的**已经是**作业的进程组，说明 shell 的 `tcsetpgrp` 生效了、`ctrl.pgrp` 已更新。
- session leader（zsh 自己）同样 `TIMEOUT`，说明内核连 leader 都没通知。
- 应用主动 `TIOCGWINSZ` 在任何情况下都能读到新尺寸。

所以这是本机内核（`HongMeng Kernel 1.13.0 #1 SMP Mon Sep 21 03:01:52 UTC 2026 aarch64`）与 Linux 上游的**平台行为差异**：Linux 上 `TIOCSWINSZ` 之后内核对当前前台组投递 `SIGWINCH`，这台设备上只要前台组被 `tcsetpgrp` 换过就不再投递。

因果链要说清，避免归错责任：

```
daemon: ioctl(master, TIOCSWINSZ)  ──> 内核记录 winsize（确实生效）
                                        │
                                        └── 本应：发给 ctrl.pgrp（当前前台组）
                                            实际：不发（前台组被 tcsetpgrp 换过时）
shell: tcsetpgrp(slave, job_pgrp)  ──> 只改 ctrl.pgrp，不负责发信号
```

shell 既没有「通知内核发信号」的职责，也没有收到信号再补发给作业的机会（它自己都没收到）。唯一知道「resize 发生了」的进程是调用 `TIOCSWINSZ` 的 hitdaemon。

### 排查过程中走过的死路

这些方向都被逐个证伪，记录下来避免以后重走：

- **以为窗口事件没进 Warp**：基座确实投递 `WindowResize`，且 XComponent surfaceChanged 也投；warp 侧 `after_layout` 每帧都跑，`Event::Resize` 有发出。
- **以为 hitshell 收尺寸参数**：它不收，靠每 200ms 轮询本地 pty winsize，这个设计是对的。
- **以为 master 与 slave 设置效果不同**：两种都试过，结果完全一致，差异不在 fd。
- **以为 `openpty(3)` 与手动 `posix_openpt` 有隐藏差异**：逐字复刻 daemon 建法，结果不变。
- **以为初始 winsize 设置时机 / dup2 与 setsid 顺序 / 是否保留 slave fd 有影响**：逐项排除，均无影响。
- **以为 daemon 漏做了 `tcsetpgrp`**：核对源码，`cmdbridge/hitdaemon/src` 全目录没有 `tcsetpgrp`、`TIOCSPGRP`、`setpgid`；`pty.rs` 的 `pre_exec` 只有 `setsid` + `TIOCSCTTY`。`tcsetpgrp` 是 zsh 的 job control 做的（hitshell 传的是 `exec /usr/bin/zsh`，zsh 顶替 sh、pid 不变，所以它是 session leader）。daemon 若自己去 `tcsetpgrp` 反而会抢走 shell 的前台组，是错的。
- **以为应用没被正确放进前台组**：应用内 `tcgetpgrp(0)` 等于自身 pgrp，前台身份正常。
- **以为是 Python handler 时序或信号被屏蔽**：改用 `sigtimedwait` 同步等待、检查信号掩码，排除。

## 解决方案

### 核心思路

保留 `TIOCSWINSZ` 作为主路径（它负责真正更新尺寸，也是 POSIX 的既定契约），在它成功之后由 hitdaemon 显式补发一次 `kill(-fg_pgrp, SIGWINCH)`。这是**补偿**而非替代：内核该发却不发时补上，内核正常发时多收一次（`SIGWINCH` 默认忽略、重复只多一次重绘，无害）。

### 关键实现细节

- **前台组从哪里取**：用 **master** fd 的 `TIOCGPGRP`。实测可行（sh 的 pgrp 是 52889 时它返回 52900，正是前台 `sleep` 的组）。**不能**用 slave 的 `tcgetpgrp`/`TIOCGPGRP` —— slave 不是 daemon 的控制终端，返回 `ENOTTY`；因此也不需要为了这个功能保留 slave fd（当前 `run_pty_shell` 在 spawn 后就 `drop(slave)`，保持原样）。
- **发给谁**：`kill(-pgrp, SIGWINCH)`，负号表示整个进程组。不做应用白名单、不按单个 pid 发（只发 pid 会漏掉作业 fork 出的子进程）、也不跳过 shell（终端空闲时前台组就是 shell 自己，它靠这个重绘提示符）。
- **失败顺序**：`set_size` 失败就直接返回，不在尺寸未更新时发信号让应用读到旧值。

### 代码

`cmdbridge/hitdaemon/src/pty.rs:136`：

```rust
pub async fn resize(conn_id: u64, channel: ChannelId, cols: u32, rows: u32) {
    log::info!("pty: resize request channel={channel} to {cols}x{rows}");
    let Some(master) = master_for(conn_id, channel) else {
        log::debug!("pty: resize request for unknown channel={channel}");
        return;
    };
    let master_guard = master.lock().await;
    let fd = master_guard.get_ref().as_raw_fd();
    if let Err(err) = set_size(fd, clamp_dimension(cols), clamp_dimension(rows)) {
        log::warn!("pty: resize channel={channel} to {cols}x{rows}: {err}");
        return;
    }
    log::debug!("pty: resized channel={channel} to {cols}x{rows}");
    forward_window_signal(fd, channel);
}
```

`cmdbridge/hitdaemon/src/pty.rs:160`：

```rust
fn forward_window_signal(fd: RawFd, channel: ChannelId) {
    let mut pgrp: libc::pid_t = 0;
    // SAFETY: TIOCGPGRP writes a pid_t through the pointer it is given.
    if unsafe { libc::ioctl(fd, libc::TIOCGPGRP, &mut pgrp) } != 0 {
        log::warn!(
            "pty: channel={channel} cannot read the foreground group: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    if pgrp <= 0 {
        log::debug!("pty: channel={channel} has no foreground process group");
        return;
    }
    // SAFETY: a negative pid sends the signal to the whole process group.
    if unsafe { libc::kill(-pgrp, libc::SIGWINCH) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            log::debug!("pty: channel={channel} foreground group {pgrp} is gone");
        } else {
            log::warn!("pty: channel={channel} cannot signal group {pgrp}: {err}");
        }
        return;
    }
    log::debug!("pty: channel={channel} forwarded SIGWINCH to group {pgrp}");
}
```

### 为什么不用别的方案

- **只做 `TIOCSWINSZ` 靠内核**：这是常规做法（GNU screen 的 `ChangeScreenSize` 就是一句 `ioctl(p->w_ptyfd, TIOCSWINSZ, &glwz)`，不显式 kill），但在这台设备的内核上不成立，已实测。
- **让 daemon 做 `tcsetpgrp`**：错位。job control 归 shell，daemon 抢前台组会破坏 shell 的作业管理。
- **在每个应用里自己轮询尺寸**：要改应用，且第三方程序改不了。
- **只在 OHOS 打补丁、用 `#[cfg(target_env = "ohos")]` 门控**：曾按此写，后经确认 `cmdbridge` 目录整体只服务 OHOS，无跨平台构建要求，门控已去掉，避免留下无意义的条件分支。

### 残余风险与边界

- **同组场景会重复一次**：无前台作业时内核本来会发，再补一次。`SIGWINCH` 默认忽略，重复只可能导致多一次重绘。
- **极小竞态**：读 `pgrp` 与 `kill` 之间 shell 可能切换前台作业，结果是旧组白收一次、新组漏一次，窗口在微秒级；resize 本身也是异步的。
- **`pgrp <= 0` 跳过、`ESRCH` 降为 `debug`**：不会刷 `warn`。
- **锁内只有两次同步 syscall**：微秒级，不长时间占用 `AsyncMutex`，不阻塞 runtime。
- **日志可见性**：daemon 的 logger 是 `LevelFilter::Info`，本次新增的成功/跳过日志是 `debug`，设备上默认不输出；只有两次 `warn` 会可见。所以运行时判据建议用应用侧探针，而不是日志。
- **未覆盖 fallback 路径**：hitdaemon 不可达时 hitshell 会 `exec` bundled zsh（自身变成 zsh），此时 warp 的本地 pty 就是真 shell 的 pty，尺寸靠 warp 的 `TIOCSWINSZ` 直接生效，同样会中这个内核缺陷。那是异常路径，修点会落在 `crates/warp_terminal/src/local_tty/unix.rs`（共享文件，需单独授权），本次未改。
- **结论的适用范围**：以上只是本机这版内核（HongMeng Kernel 1.13.0）的行为，不能替所有 OHOS 设备下结论。换设备或换内核版本，应把「同组 / 独立前台组 / 显式 kill」三组对照重跑一遍，再决定补丁是否还需要。

### 验证方式

机制级验证已完成（可在任意 OHOS shell 里复跑，探针脚本置于临时目录、用完即删）：

- 同进程组：改 winsize → 应用 `WINCH <rows> <cols>`。
- 独立前台组：改 winsize → 无输出；`kill(-pgrp, WINCH)` → `WINCH <rows> <cols>`。

端到端验证需要把新 `hitdaemon` 装到设备并让运行中的 daemon 换成新二进制（daemon 常驻，覆盖安装只换磁盘产物、不换内存进程）。换新后应用侧判据：

```sh
trap 'echo "WINCH $(stty size 2>/dev/null)"' WINCH
echo "READY $(stty size 2>/dev/null)"
while :; do sleep 1; done
```

在 Warp 里**前台直接运行**（不要放后台、不要进管道，否则落不到独立前台组，测不出问题），改变窗口大小：

- 修复生效：每次 resize 打一条 `WINCH <rows> <cols>`，且尺寸是新的。
- 未生效：只有 `READY`，没有 `WINCH`。

## 修改文件

- `cmdbridge/hitdaemon/src/pty.rs` — `resize()` 在 `TIOCSWINSZ` 成功后调用新增的 `forward_window_signal()`；新增函数用 master fd 的 `TIOCGPGRP` 取前台进程组并向整组 `kill(-pgrp, SIGWINCH)`，各失败分支带 `warn`/`debug` 日志；`set_size` 失败改为直接返回，避免尺寸未更新就发信号。

本次提交（`8fbaea3`）另带两处与问题本身无关的同期改动，一并记录以免误解：

- `hap/build-profile.json5` — 更新调试签名证书与口令路径（本机签名配置变更）。
- `hap/entry/src/main/module.json5` — 移除 `ohos.permission.ALLOW_EXTERNAL_NATIVE_CODE` 权限声明。

相关背景：[[ohos-debug-lessons]]
