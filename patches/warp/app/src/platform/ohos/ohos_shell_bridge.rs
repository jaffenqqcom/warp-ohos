// §2.5.3 — OHOS Shell 桥接 Rust 封装层
//
// 通过 dlsym 加载 libentry.so 导出的 C FFI 函数，提供两个高层结构体：
//   OhosPtyBridge  — Shell 长连接接口（替代 fork+exec bash）
//   OhosCommand    — 一次性命令接口（替代 std::process::Command）
//
// 条件编译：仅在 target_env = "ohos" 时编译。

#![cfg(target_env = "ohos")]

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::fd::FromRawFd;
use std::os::unix::io::RawFd;
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use warp_core::command::ExitCode;
use warpui::AssetProvider;

use crate::terminal::local_tty::spawner::PtyHandle;

// ── dlsym 支持 ──────────────────────────────────────────────────────────────

extern "C" {
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::os::raw::c_char)
        -> *mut std::ffi::c_void;
}

const RTLD_DEFAULT: *mut std::ffi::c_void = std::ptr::null_mut();

// C FFI 函数类型定义
type StartShellBridgeFn =
    unsafe extern "C" fn(i32, *const std::os::raw::c_char, *const std::os::raw::c_char, *mut i32)
        -> i32;
type ExecCommandFn =
    unsafe extern "C" fn(*const std::os::raw::c_char, *mut i32, *mut i32) -> i32;

// 用宏生成函数指针的延迟加载器（dlsym 在首次调用时解析）
macro_rules! load_ffi_fn {
    ($name:ident, $sig:ty) => {
        fn $name() -> Option<&'static $sig> {
            static FN: OnceLock<$sig> = OnceLock::new();
            let f = FN.get_or_init(|| {
                let cname = CString::new(stringify!($name)).unwrap();
                let ptr = unsafe { dlsym(RTLD_DEFAULT, cname.as_ptr().cast()) };
                if ptr.is_null() {
                    log::error!("dlsym({}) failed", stringify!($name));
                    return unsafe { std::mem::zeroed() };
                }
                unsafe { std::mem::transmute::<*mut std::ffi::c_void, $sig>(ptr) }
            });
            // 透过裸指针比较函数指针是否为 zeroed（dlsym 失败时设置）
            let p: *const $sig = f;
            if p.is_null() || p as usize == 0 {
                None
            } else {
                Some(f)
            }
        }
    };
}

load_ffi_fn!(ohos_start_shell_bridge, StartShellBridgeFn);
load_ffi_fn!(ohos_exec_command, ExecCommandFn);

// ── OhosPtyBridge ──────────────────────────────────────────────────────────

pub struct OhosPtyBridge;

impl OhosPtyBridge {
    /// 创建 PTY pair，启动桥接子进程，返回（master_fd, handle）。
    ///
    /// master_fd 是 PTY master 端 fd，直接传给 EventLoop 使用。
    /// OhosPtyHandle 用于管理子进程生命周期。
    pub fn spawn(
        env_vars: &[(OsString, OsString)],
    ) -> Result<(RawFd, OhosPtyHandle)> {

        // 1. 创建 PTY pair
        let (master_fd, slave_fd) = create_pty_pair()
            .context("OhosPtyBridge: failed to create PTY pair")?;
        log::info!("OhosPtyBridge::spawn: PTY created, master={}, slave={}", master_fd, slave_fd);

        // 2. 序列化环境变量为 "KEY=VALUE\n" 格式
        let env_str = serialize_env_vars(env_vars)
            .context("OhosPtyBridge: failed to serialize env vars")?;

        // 3. 加载 bash_init_shell.sh（与正常流程同一来源），写入 VM 后由 bash source
        let init_script_content = load_bash_init_script();
        log::info!("OhosPtyBridge::spawn: init_script loaded ({} bytes)", init_script_content.len());
        let init_script = CString::new(init_script_content.as_bytes())
            .context("OhosPtyBridge: init_script CString")?;

        // 4. 编码入口参数
        let c_env = CString::new(env_str.as_bytes())
            .context("OhosPtyBridge: env_vars CString")?;

        // 5. 调用 C FFI
        let func = ohos_start_shell_bridge()
            .ok_or_else(|| anyhow!("OhosPtyBridge: ohos_start_shell_bridge not available"))?;

        log::info!("OhosPtyBridge::spawn: calling ohos_start_shell_bridge, slave_fd={}", slave_fd);

        let mut pid: i32 = 0;
        let ret = unsafe { func(slave_fd, c_env.as_ptr(), init_script.as_ptr(), &mut pid) };

        log::info!("OhosPtyBridge::spawn: ohos_start_shell_bridge returned ret={}, pid={}", ret, pid);

        // 关闭 slave 端（子进程持有副本）
        unsafe { libc::close(slave_fd) };

        if ret != 0 {
            log::error!("OhosPtyBridge::spawn: bridge failed with ret={}, closing master_fd={}", ret, master_fd);
            unsafe { libc::close(master_fd) };
            return Err(anyhow!("OhosPtyBridge: ohos_start_shell_bridge failed: {}", ret));
        }

        log::info!("OhosPtyBridge::spawn: success, pid={}, master_fd={}", pid, master_fd);

        Ok((master_fd, OhosPtyHandle { pid: pid as u32 }))
    }
}

// ── OhosPtyHandle ──────────────────────────────────────────────────────────

/// 桥接子进程的生命周期管理句柄。
/// 不包含 Child 结构体（非 fork 创建），用 PID 直接管理。
pub struct OhosPtyHandle {
    pid: u32,
}

impl PtyHandle for OhosPtyHandle {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn has_process_terminated(&mut self) -> Result<bool> {
        log::info!("OhosPtyHandle::has_process_terminated: pid={}", self.pid);
        // 用 waitid(WNOHANG | WNOWAIT) 检查子进程终止状态
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let ret = unsafe {
            libc::waitid(
                libc::P_PID,
                self.pid as libc::id_t,
                &mut info as *mut libc::siginfo_t,
                libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if ret == 0 {
            // 子进程已退出
            log::info!("OhosPtyHandle: process {} terminated", self.pid);
            Ok(true)
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ECHILD) {
                // ECHILD 表示子进程已收割或不存在
                Ok(true)
            } else {
                // 仍在运行或其它状态
                Ok(false)
            }
        }
    }

    fn kill(&mut self) -> Result<()> {
        log::info!("OhosPtyHandle::kill: killing pid={}", self.pid);
        let ret = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
        if ret != 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(err.into());
        }
        // 等待子进程退出
        let mut status: i32 = 0;
        unsafe { libc::waitpid(self.pid as libc::pid_t, &mut status, 0) };
        log::info!("OhosPtyHandle: killed pid={}", self.pid);
        Ok(())
    }
}

// ── OhosCommand ────────────────────────────────────────────────────────────

/// 一次性命令执行接口，替代 std::process::Command。
///
/// 用法示例：
/// ```ignore
/// let output = OhosCommand::new("bash -c \"ls -la\"")
///     .current_dir("/tmp")
///     .spawn()?
///     .output_sync()?;
/// ```
pub struct OhosCommand {
    command: String,
    current_dir: Option<OsString>,
    _env_vars: Vec<(OsString, OsString)>,
}

impl OhosCommand {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            current_dir: None,
            _env_vars: Vec::new(),
        }
    }

    pub fn current_dir(mut self, dir: impl Into<OsString>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    #[allow(dead_code)]
    pub fn env(mut self, key: impl Into<OsString>, val: impl Into<OsString>) -> Self {
        self._env_vars.push((key.into(), val.into()));
        self
    }

    /// 启动子进程，返回 OhosChild。
    ///
    /// 如果设置了 work_dir，在命令前加上 "cd <work_dir> && " 前缀。
    pub fn spawn(&self) -> Result<OhosChild> {
        log::info!("OhosCommand::spawn: command={}, current_dir={:?}", self.command, self.current_dir);
        // 构造最终命令字符串
        let final_cmd = if let Some(ref dir) = self.current_dir {
            format!(
                "bash -c \"cd {} && {}\"",
                escape_path(dir),
                escape_for_shell(&self.command)
            )
        } else {
            format!("bash -c {}", escape_for_shell(&self.command))
        };

        let c_cmd = CString::new(final_cmd.as_bytes())
            .context("OhosCommand: command CString")?;

        let func = ohos_exec_command()
            .ok_or_else(|| anyhow!("OhosCommand: ohos_exec_command not available"))?;

        let mut pipe_fd: i32 = 0;
        let mut pid: i32 = 0;
        let ret = unsafe { func(c_cmd.as_ptr(), &mut pipe_fd, &mut pid) };

        if ret != 0 {
            return Err(anyhow!("OhosCommand: ohos_exec_command failed: {}", ret));
        }

        log::info!(
            "OhosCommand: spawned, cmd={}, pid={}, pipe_fd={}",
            self.command, pid, pipe_fd
        );

        Ok(OhosChild {
            pid: pid as u32,
            pipe_fd,
            kill_on_drop: true,
        })
    }
}

// ── OhosChild ──────────────────────────────────────────────────────────────

/// OhosCommand::spawn() 的返回结果。
///
/// 读取管道获取命令输出和退出码。
/// 数据格式：前 N 字节为 stdout 内容，最后 4 字节为退出码（i32, little-endian）。
pub struct OhosChild {
    pid: u32,
    pipe_fd: RawFd,
    kill_on_drop: bool,
}

impl OhosChild {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 同步读取：从管道读取数据，提取退出码，然后关闭管道。
    pub fn output_sync(mut self) -> Result<(Vec<u8>, i32)> {
        log::info!("OhosChild::output_sync: reading pipe_fd={}", self.pipe_fd);
        use std::io::Read;
        let mut file = unsafe { std::fs::File::from_raw_fd(self.pipe_fd) };
        let mut all_data = Vec::<u8>::new();
        let bytes_read = file.read_to_end(&mut all_data)
            .context("OhosChild: sync read output")?;
        log::info!("OhosChild::output_sync: read {} bytes from pipe", bytes_read);

        // 最后 4 字节是退出码
        if all_data.len() < 4 {
            self.pipe_fd = -1;
            return Ok((all_data, -1));
        }
        let exit_bytes: [u8; 4] = all_data[all_data.len() - 4..]
            .try_into()
            .unwrap_or([0, 0, 0, 255]);
        let exit_code = i32::from_le_bytes(exit_bytes);
        let stdout = all_data[..all_data.len() - 4].to_vec();

        log::info!("OhosChild::output_sync: exit_code={}, stdout_len={}", exit_code, stdout.len());

        self.pipe_fd = -1;
        Ok((stdout, exit_code))
    }

    /// 杀掉桥接子进程。
    pub fn kill(&mut self) -> Result<()> {
        if self.kill_on_drop {
            let ret = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
            if ret != 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(err.into());
                }
            }
            let mut status: i32 = 0;
            unsafe { libc::waitpid(self.pid as libc::pid_t, &mut status, 0) };
            log::info!("OhosChild: killed pid={}", self.pid);
        }
        Ok(())
    }
}

impl Drop for OhosChild {
    fn drop(&mut self) {
        if self.pipe_fd >= 0 {
            unsafe { libc::close(self.pipe_fd) };
        }
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────────────────

/// 创建 PTY pair，返回 (master_fd, slave_fd)。
/// 用 cfmakeraw 关闭 ECHO/ICANON/ISIG/IXON/ICRNL 等一切字符处理，
/// 确保字节原样通过 PTY 不被篡改。然后恢复 OPOST+ONLCR，使输出方向的
/// LF→CRLF 转换正常工作，避免终端仿真器状态异常。
fn create_pty_pair() -> Result<(RawFd, RawFd)> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;

    let ret = unsafe {
        libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null())
    };
    if ret != 0 {
        return Err(io::Error::last_os_error()).context("ohos: openpty failed");
    }

    // slave 端不是 shell，而是 SSH 桥接转发器。cfmakeraw 确保：
    // - ECHO 关闭：写入 master 的字节不被回显（解决 ^P^[[200~ 乱码）
    // - ICANON 关闭：C++ 桥接可立即读取字节，不受行缓冲限制
    // - ICRNL/IXON/ISIG 关闭：字节原样传输，不被 PTY 驱动篡改
    // 然后恢复 OPOST+ONLCR，确保输出方向 LF→CRLF 正常转换。
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    unsafe {
        libc::tcgetattr(slave, &mut termios);
        libc::cfmakeraw(&mut termios);
        termios.c_oflag |= libc::OPOST | libc::ONLCR;
        libc::tcsetattr(slave, libc::TCSANOW, &termios);
    }

    Ok((master, slave))
}

/// 将 (key, value) 环境变量列表序列化为 "KEY=VALUE\n" 字符串。
fn serialize_env_vars(vars: &[(OsString, OsString)]) -> Result<String> {
    let mut result = String::new();
    for (key, val) in vars {
        let k = key.to_str()
            .ok_or_else(|| anyhow!("env key not valid utf-8: {:?}", key))?;
        let v = val.to_str()
            .ok_or_else(|| anyhow!("env value not valid utf-8 for key {}: {:?}", k, val))?;
        result.push_str(k);
        result.push('=');
        result.push_str(v);
        result.push('\n');
    }
    Ok(result)
}

/// 转义命令字符串用于 bash -c。
/// 在字符串外包裹单引号，字符串内的单引号用 '\'' 转义。
fn escape_for_shell(cmd: &str) -> String {
    let mut result = String::with_capacity(cmd.len() + 4);
    result.push('\'');
    for ch in cmd.chars() {
        if ch == '\'' {
            result.push_str("'\\''");
        } else {
            result.push(ch);
        }
    }
    result.push('\'');
    result
}

/// 转义路径用于 bash -c "cd <path> && ..."
fn escape_path(path: &OsStr) -> String {
    escape_for_shell(path.to_str().unwrap_or(""))
}

/// 加载 bash_init_shell.sh，与正常流程同一来源。
/// 原始内容一字不动，仅替换模板变量 @@USING_CON_PTY_BOOLEAN@@ → false。
/// 追加 `touch "$HOME/.hushlogin"` 来抑制 bash_body.sh 二次打印 MOTD。
fn load_bash_init_script() -> String {
    let mut script = match crate::ASSETS.get("bundled/bootstrap/bash_init_shell.sh") {
        Ok(data) => {
            String::from_utf8_lossy(data.as_ref()).replace("@@USING_CON_PTY_BOOLEAN@@", "false")
        }
        Err(e) => {
            log::error!("OhosPtyBridge: load bash_init_shell.sh failed: {:?}", e);
            return String::new();
        }
    };
    // 让 bash_body.sh 跳过 MOTD 输出（PAM 已在 SSH 连接时打印过一次）。
    // bash_body.sh 会检查 $HOME/.hushlogin，存在则跳过 MOTD。
    script.push_str("\ntouch \"$HOME/.hushlogin\"\n");
    // 确保 UTF-8 locale，避免 bash 在首个多字节输入时做额外编码处理导致延迟
    script.push_str("export LANG=\"en_US.UTF-8\"\nexport LC_ALL=\"en_US.UTF-8\"\n");
    script
}

/// OHOS shell 检测：询问 VM 上是否存在指定 shell。
///
/// 由 shell.rs 的 supported_shell_path_and_type 在 OHOS 分支调用。
/// 用 OhosCommand 在 VM 上执行 which <shell>，判断该 shell 是否可用。
/// OHOS shell 检测：询问 VM 上是否存在指定 shell。
///
/// 由 shell.rs 的 supported_shell_path_and_type 在 OHOS 分支调用。
pub fn ohos_supported_shell(path_or_command: &str) -> Option<(std::path::PathBuf, warp_terminal::shell::ShellType)> {
    log::info!("ohos_supported_shell: checking path={}", path_or_command);
    let name = std::path::Path::new(path_or_command)
        .file_name()
        .and_then(|n| n.to_str());

    match name {
        Some("bash") | Some("sh") => {
            log::info!("ohos_supported_shell: matched bash for path={}", path_or_command);
            Some((std::path::PathBuf::from("/bin/bash"), warp_terminal::shell::ShellType::Bash))
        }
        Some("zsh") => {
            log::info!("ohos_supported_shell: matched zsh for path={}", path_or_command);
            Some((std::path::PathBuf::from("/bin/zsh"), warp_terminal::shell::ShellType::Zsh))
        }
        Some("fish") => {
            log::info!("ohos_supported_shell: matched fish for path={}", path_or_command);
            Some((std::path::PathBuf::from("/usr/bin/fish"), warp_terminal::shell::ShellType::Fish))
        }
        _ => {
            log::warn!("ohos_supported_shell: no match for path={}, name={:?}", path_or_command, name);
            None
        }
    }
}

///
/// 由 unix.rs 的 get_pw_entry 在 OHOS 分支调用。
/// 用 OhosCommand 执行 getent passwd warp，解析返回用户信息。
pub fn ohos_get_passwd() -> Option<(String, String, String)> {
    log::info!("ohos_get_passwd: calling OhosCommand for 'getent passwd warp'");
    let child = match OhosCommand::new("getent passwd warp".to_string()).spawn() {
        Ok(child) => child,
        Err(e) => {
            log::error!("ohos_get_passwd: OhosCommand spawn failed: {}", e);
            return None;
        }
    };
    let (stdout, exit_code) = match child.output_sync() {
        Ok(result) => result,
        Err(e) => {
            log::error!("ohos_get_passwd: output_sync failed: {}", e);
            return None;
        }
    };
    let line = match std::str::from_utf8(&stdout) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            log::error!("ohos_get_passwd: utf8 decode failed: {}", e);
            return None;
        }
    };

    log::info!("ohos_get_passwd: raw output ({} bytes), exit_code={}, line='{}'",
               stdout.len(), exit_code, line);

    // 格式：warp:x:1000:1000:gecos:/home/warp:/bin/bash
    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() < 7 {
        log::warn!("ohos_get_passwd: unexpected getent output: '{}', parts={}", line, parts.len());
        return None;
    }

    let result = Some((
        parts[0].to_string(), // 用户名: warp
        parts[5].to_string(), // 家目录: /home/warp
        parts[6].to_string(), // shell: /bin/bash
    ));
    log::info!("ohos_get_passwd: parsed user={}, dir={}, shell={}",
               parts[0], parts[5], parts[6]);
    result
}
