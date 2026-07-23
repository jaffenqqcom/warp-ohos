// §2.2.4 — SSH 桥接 C FFI 导出层
//
// 两个 C 符号导出函数，编译进 libentry.so，Rust 侧通过 ffi_fn 宏用 dlsym 加载。
//
// ohos_start_shell_bridge：Shell 长连接接口，启动 ShellBridgeMain 子进程。
// ohos_exec_command：一次性命令接口，启动 ExecCmdMain 子进程。

#include <cstdlib>
#include <cstring>
#include <unistd.h>
#include <errno.h>
#include <hilog/log.h>
#include <AbilityKit/native_child_process.h>

#undef LOG_DOMAIN
#define LOG_DOMAIN 0x0001
#undef LOG_TAG
#define LOG_TAG "ohos_shell"

static constexpr const char* PTY_FD_NAME = "pty";
static constexpr const char* OUTPUT_FD_NAME = "output";
static constexpr const char* ENV_INIT_DELIM = "---WARP_INIT---";

extern "C" {

// Shell 长连接接口
//
// 接收 PTY slave fd、环境变量字符串、init 脚本字符串。
// 内部拼装 entryParams（env_vars + \n + delimiter + \n + init_script）
// 通过 OH_Ability_StartNativeChildProcess 启动 libohos_shell.so 的
// ShellBridgeMain 入口点。返回子进程 PID。
int32_t ohos_start_shell_bridge(int32_t pty_fd,
                                const char* env_vars,
                                const char* init_script,
                                int32_t* pid_out) {
    OH_LOG_INFO(LOG_APP, "ohos_start_shell_bridge: pty_fd=%{public}d, env_vars=%{public}s, init_script_len=%{public}zu",
                pty_fd, env_vars ? env_vars : "null", init_script ? strlen(init_script) : 0);
    if (pty_fd < 0 || env_vars == nullptr || init_script == nullptr || pid_out == nullptr) {
        OH_LOG_ERROR(LOG_APP, "ohos_start_shell_bridge: invalid params");
        return NCP_ERR_INVALID_PARAM;
    }

    // 拼装 entryParams = env_vars + \n + delimiter + \n + init_script
    size_t env_len = strlen(env_vars);
    size_t delim_len = strlen(ENV_INIT_DELIM);
    size_t init_len = strlen(init_script);
    char* entry_params = static_cast<char*>(malloc(
        env_len + 1 + delim_len + 1 + init_len + 1));
    if (!entry_params) {
        OH_LOG_ERROR(LOG_APP, "ohos_start_shell_bridge: malloc failed");
        return NCP_ERR_INVALID_PARAM;
    }
    char* p = entry_params;
    memcpy(p, env_vars, env_len); p += env_len;
    *p++ = '\n';
    memcpy(p, ENV_INIT_DELIM, delim_len); p += delim_len;
    *p++ = '\n';
    memcpy(p, init_script, init_len); p += init_len;
    *p = '\0';

    // 设置子进程参数：PTY fd 通过命名 fd 传递
    NativeChildProcess_Fd fd_node;
    fd_node.fdName = const_cast<char*>(PTY_FD_NAME);
    fd_node.fd = pty_fd;
    fd_node.next = nullptr;

    NativeChildProcess_Args args;
    args.entryParams = entry_params;
    args.fdList.head = &fd_node;

    NativeChildProcess_Options options;
    options.isolationMode = NCP_ISOLATION_MODE_NORMAL;
    options.reserved = 0;

    int32_t pid = -1;
    Ability_NativeChildProcess_ErrCode ret =
        OH_Ability_StartNativeChildProcess(
            "libohos_shell.so:ShellBridgeMain", args, options, &pid);

    free(entry_params);

    if (ret != NCP_NO_ERROR) {
        OH_LOG_ERROR(LOG_APP,
            "ohos_start_shell_bridge: StartNativeChildProcess failed: %{public}d", ret);
        return static_cast<int32_t>(ret);
    }

    *pid_out = pid;
    OH_LOG_INFO(LOG_APP, "ohos_start_shell_bridge: child started, pid=%{public}d", pid);
    return 0;
}

// 一次性命令接口
//
// 接收命令字符串，创建管道，通过 OH_Ability_StartNativeChildProcess
// 启动 libohos_shell.so 的 ExecCmdMain 入口点。输出管道写端传入子进程，
// 读端通过 pipe_fd_out 返回给 Rust 侧。返回子进程 PID。
int32_t ohos_exec_command(const char* command,
                          int32_t* pipe_fd_out,
                          int32_t* pid_out) {
    OH_LOG_INFO(LOG_APP, "ohos_exec_command: command=%{public}s", command ? command : "null");
    if (command == nullptr || pipe_fd_out == nullptr || pid_out == nullptr) {
        OH_LOG_ERROR(LOG_APP, "ohos_exec_command: invalid params");
        return NCP_ERR_INVALID_PARAM;
    }

    // 创建管道
    int pipe_fds[2];
    if (pipe(pipe_fds) != 0) {
        OH_LOG_ERROR(LOG_APP, "ohos_exec_command: pipe failed, errno=%{public}d", errno);
        return NCP_ERR_INVALID_PARAM;
    }
    OH_LOG_INFO(LOG_APP, "ohos_exec_command: pipe created, read_fd=%{public}d, write_fd=%{public}d",
                pipe_fds[0], pipe_fds[1]);

    // 设置子进程参数：输出管道写端通过命名 fd 传递
    NativeChildProcess_Fd fd_node;
    fd_node.fdName = const_cast<char*>(OUTPUT_FD_NAME);
    fd_node.fd = pipe_fds[1];
    fd_node.next = nullptr;

    NativeChildProcess_Args args;
    args.entryParams = const_cast<char*>(command);
    args.fdList.head = &fd_node;

    NativeChildProcess_Options options;
    options.isolationMode = NCP_ISOLATION_MODE_NORMAL;
    options.reserved = 0;

    int32_t pid = -1;
    Ability_NativeChildProcess_ErrCode ret =
        OH_Ability_StartNativeChildProcess(
            "libohos_shell.so:ExecCmdMain", args, options, &pid);

    // 父进程关闭写端，子进程拥有它
    close(pipe_fds[1]);

    if (ret != NCP_NO_ERROR) {
        OH_LOG_ERROR(LOG_APP,
            "ohos_exec_command: StartNativeChildProcess failed: %{public}d", ret);
        close(pipe_fds[0]);
        return static_cast<int32_t>(ret);
    }

    *pipe_fd_out = pipe_fds[0];
    *pid_out = pid;
    OH_LOG_INFO(LOG_APP, "ohos_exec_command: child started, pid=%{public}d, pipe_fd=%{public}d",
                pid, pipe_fds[0]);
    return 0;
}

// Socket 方案：不创建 PTY，用两个 socketpair（数据 + 控制）启动子进程
int32_t ohos_start_shell_bridge2(int32_t data_fd,
                                  int32_t control_fd,
                                  const char* env_vars,
                                  const char* init_script,
                                  int32_t* pid_out) {
    OH_LOG_INFO(LOG_APP, "ohos_start_shell_bridge2: data_fd=%{public}d, control_fd=%{public}d",
                data_fd, control_fd);
    if (data_fd < 0 || control_fd < 0 || env_vars == nullptr || init_script == nullptr || pid_out == nullptr) {
        OH_LOG_ERROR(LOG_APP, "ohos_start_shell_bridge2: invalid params");
        return NCP_ERR_INVALID_PARAM;
    }

    // 拼装 entryParams = env_vars + \n + delimiter + \n + init_script
    size_t env_len = strlen(env_vars);
    size_t delim_len = strlen(ENV_INIT_DELIM);
    size_t init_len = strlen(init_script);
    char* entry_params = static_cast<char*>(malloc(env_len + 1 + delim_len + 1 + init_len + 1));
    if (!entry_params) {
        OH_LOG_ERROR(LOG_APP, "ohos_start_shell_bridge2: malloc failed");
        return NCP_ERR_INVALID_PARAM;
    }
    char* p = entry_params;
    memcpy(p, env_vars, env_len); p += env_len;
    *p++ = '\n';
    memcpy(p, ENV_INIT_DELIM, delim_len); p += delim_len;
    *p++ = '\n';
    memcpy(p, init_script, init_len); p += init_len;
    *p = '\0';

    // 两个命名 fd 传给子进程
    NativeChildProcess_Fd fd_nodes[2];
    fd_nodes[0].fdName = const_cast<char*>("data");
    fd_nodes[0].fd = data_fd;
    fd_nodes[0].next = &fd_nodes[1];
    fd_nodes[1].fdName = const_cast<char*>("control");
    fd_nodes[1].fd = control_fd;
    fd_nodes[1].next = nullptr;

    NativeChildProcess_Args args;
    args.entryParams = entry_params;
    args.fdList.head = fd_nodes;

    NativeChildProcess_Options options;
    options.isolationMode = NCP_ISOLATION_MODE_NORMAL;
    options.reserved = 0;

    int32_t pid = -1;
    Ability_NativeChildProcess_ErrCode ret =
        OH_Ability_StartNativeChildProcess(
            "libohos_shell.so:ShellBridgeMain2", args, options, &pid);

    free(entry_params);

    if (ret != NCP_NO_ERROR) {
        OH_LOG_ERROR(LOG_APP, "ohos_start_shell_bridge2: StartNativeChildProcess failed: %{public}d", ret);
        return static_cast<int32_t>(ret);
    }

    *pid_out = pid;
    OH_LOG_INFO(LOG_APP, "ohos_start_shell_bridge2: child started, pid=%{public}d", pid);
    return 0;
}

} // extern "C"
