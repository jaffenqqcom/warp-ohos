// §2.2.5 — SSH 桥接子进程
//
// libohos_shell.so 的桥接子进程入口，由 OH_Ability_StartNativeChildProcess 创建。
// 两个入口点：ShellBridgeMain（Shell 长连接，PTY ↔ SSH channel 双向桥接）
// 和 ExecCmdMain（一次性命令，SSH exec + 输出回传）。
//
// 公共函数 ssh_connect_to_vm 封装 TCP 连接 + libssh2 握手 + 密码认证。
//
// 编译依赖：mbedTLS（静态），libssh2（静态，mbedTLS 后端）。
// 许可证：mbedTLS Apache-2.0，libssh2 BSD-3-Clause。

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/select.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <arpa/inet.h>
#include <termios.h>
#include <errno.h>

#include <libssh2.h>
#include <AbilityKit/native_child_process.h>
#include <hilog/log.h>

#undef LOG_DOMAIN
#define LOG_DOMAIN 0x0001
#undef LOG_TAG
#define LOG_TAG "ohos_bridge"

// === 连接参数 ================================================================

static constexpr const char* VM_HOST = "172.16.100.2";
static constexpr int VM_PORT = 22;
static constexpr const char* WARP_USER = "warp";
static constexpr const char* WARP_PASS = "12345678";
static constexpr const char* PTY_FD_NAME = "pty";
static constexpr const char* OUTPUT_FD_NAME = "output";
static constexpr const char* ENV_INIT_DELIM = "---WARP_INIT---";
static constexpr int BUF_SIZE = 4096;

// === 辅助函数 ================================================================

// 把 fd 设为非阻塞模式
static void set_nonblocking(int fd) {
    int flags = fcntl(fd, F_GETFL, 0);
    fcntl(fd, F_SETFL, flags | O_NONBLOCK);
}

// 从 fdList 中查找指定名称的 fd，没找到返回 -1
static int32_t find_named_fd(const NativeChildProcess_FdList* list, const char* name) {
    if (!list || !list->head) return -1;
    NativeChildProcess_Fd* cur = list->head;
    while (cur) {
        if (cur->fdName && strcmp(cur->fdName, name) == 0) return cur->fd;
        cur = cur->next;
    }
    return -1;
}

// 解析 entryParams：按 ENV_INIT_DELIM 分隔为环境变量段和 init 脚本段
// 返回两个段的起始指针和长度，零长度表示该段为空
static void parse_entry_params(const char* entry_params,
                               const char** env_vars, int* env_vars_len,
                               const char** init_script, int* init_script_len) {
    *env_vars = entry_params;
    const char* delim = entry_params ? strstr(entry_params, ENV_INIT_DELIM) : nullptr;
    if (delim) {
        *env_vars_len = static_cast<int>(delim - entry_params);
        *init_script = delim + strlen(ENV_INIT_DELIM);
        if (**init_script == '\n') (*init_script)++;
        *init_script_len = static_cast<int>(strlen(*init_script));
    } else {
        *env_vars_len = entry_params ? static_cast<int>(strlen(entry_params)) : 0;
        *init_script = "";
        *init_script_len = 0;
    }
}

// PTY 缓冲区满时重试间隔（微秒）
static constexpr useconds_t WRITE_RETRY_USLEEP = 1000; // 1ms

// 确保全部数据写入 fd（处理部分写入、EINTR 和 PTY 缓冲区满）
// PTY 是非阻塞模式，当 Rust 侧 EventLoop 未及时读取时 write 返回 EAGAIN，
// 此函数会重试直到写入完成，避免因 EAGAIN 直接返回导致数据丢失。
static ssize_t write_all(int fd, const void* buf, size_t count) {
    size_t written = 0;
    while (written < count) {
        ssize_t n = write(fd, static_cast<const char*>(buf) + written, count - written);
        if (n > 0) {
            written += n;
        } else if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            // PTY 缓冲区满，返回部分写入的字节数，由主循环 pending_buf 处理
            return static_cast<ssize_t>(written);
        } else if (n < 0 && errno == EINTR) {
            continue;
        } else {
            return n;
        }
    }
    return static_cast<ssize_t>(written);
}

// === ssh_connect_to_vm：公共 SSH 连接函数 =====================================

static LIBSSH2_SESSION* ssh_connect_to_vm(int* out_sock) {
    // 创建 TCP socket
    int sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock < 0) {
        OH_LOG_ERROR(LOG_APP, "ssh_connect: socket failed, errno=%{public}d", errno);
        return nullptr;
    }
    int nodelay = 1;
    setsockopt(sock, IPPROTO_TCP, TCP_NODELAY, &nodelay, sizeof(nodelay));
    OH_LOG_INFO(LOG_APP, "ssh_connect: socket created, fd=%{public}d", sock);

    // 连接到 VM
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(VM_PORT);
    if (inet_pton(AF_INET, VM_HOST, &addr.sin_addr) <= 0) {
        OH_LOG_ERROR(LOG_APP, "ssh_connect: invalid VM address %{public}s", VM_HOST);
        close(sock);
        return nullptr;
    }
    OH_LOG_INFO(LOG_APP, "ssh_connect: connecting to %{public}s:%{public}d...", VM_HOST, VM_PORT);
    if (connect(sock, reinterpret_cast<struct sockaddr*>(&addr), sizeof(addr)) < 0) {
        OH_LOG_ERROR(LOG_APP, "ssh_connect: connect to %{public}s:%{public}d failed, "
                     "errno=%{public}d", VM_HOST, VM_PORT, errno);
        close(sock);
        return nullptr;
    }
    OH_LOG_INFO(LOG_APP, "ssh_connect: TCP connected to %{public}s:%{public}d", VM_HOST, VM_PORT);

    // 初始化 libssh2 会话
    LIBSSH2_SESSION* session = libssh2_session_init();
    if (!session) {
        OH_LOG_ERROR(LOG_APP, "ssh_connect: libssh2_session_init failed");
        close(sock);
        return nullptr;
    }

    // 阻塞模式用于初始化阶段
    libssh2_session_set_blocking(session, 1);

    // 握手
    OH_LOG_INFO(LOG_APP, "ssh_connect: starting libssh2 handshake...");
    int rc = libssh2_session_handshake(session, sock);
    if (rc != 0) {
        OH_LOG_ERROR(LOG_APP, "ssh_connect: handshake failed, rc=%{public}d", rc);
        libssh2_session_free(session);
        close(sock);
        return nullptr;
    }
    OH_LOG_INFO(LOG_APP, "ssh_connect: handshake OK");

    // 密码认证
    OH_LOG_INFO(LOG_APP, "ssh_connect: authenticating as %{public}s...", WARP_USER);
    rc = libssh2_userauth_password(session, WARP_USER, WARP_PASS);
    if (rc != 0) {
        OH_LOG_ERROR(LOG_APP, "ssh_connect: password auth failed, rc=%{public}d", rc);
        libssh2_session_free(session);
        close(sock);
        return nullptr;
    }

    OH_LOG_INFO(LOG_APP, "ssh_connect: connected and authenticated to %{public}s as %{public}s",
                VM_HOST, WARP_USER);
    *out_sock = sock;
    return session;
}

// === 构建 init rcfile 内容 ===================================================
//
// 把环境变量段转为 export 语句，追加 init 脚本段，组成完整的 /tmp/warp_init.sh 内容
// 返回写入的字节数，不超过 buf_size

static int build_init_script(char* buf, int buf_size,
                              const char* env_vars, int env_vars_len,
                              const char* init_script, int init_script_len) {
    int pos = 0;

    // 追加环境变量 export
    const char* p = env_vars;
    const char* end = env_vars + env_vars_len;
    while (p < end && pos < buf_size - 1) {
        const char* nl = static_cast<const char*>(memchr(p, '\n', end - p));
        int line_len = nl ? static_cast<int>(nl - p) : static_cast<int>(end - p);
        if (line_len > 0 && pos + 8 + line_len < buf_size) {
            memcpy(buf + pos, "export ", 7);
            pos += 7;
            memcpy(buf + pos, p, line_len);
            pos += line_len;
            buf[pos++] = '\n';
        }
        // 空行（如 env_vars_len==0 的情况）跳过
        p = nl ? nl + 1 : end;
    }

    // 追加 init 脚本段
    if (init_script_len > 0 && pos + 1 + init_script_len < buf_size) {
        if (pos > 0 && buf[pos - 1] != '\n') buf[pos++] = '\n';
        memcpy(buf + pos, init_script, init_script_len);
        pos += init_script_len;
    }

    // 确保最后有换行
    if (pos > 0 && buf[pos - 1] != '\n' && pos < buf_size - 1) {
        buf[pos++] = '\n';
    }
    buf[pos] = '\0';
    return pos;
}

// === ShellBridgeMain：Shell 长连接入口 ========================================
//
// 由 OH_Ability_StartNativeChildProcess("libohos_shell.so:ShellBridgeMain") 创建。
// 接收 PTY slave fd（命名 fd "pty"）和 env_vars + init_script（entryParams）。
// 连接 VM，在 SSH channel 上启动 bash，写入 init rcfile 并 source，然后在
// PTY 和 SSH channel 之间双向转发数据。SSH 断开后自动退出。

extern "C" {

void ShellBridgeMain(NativeChildProcess_Args args) {
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: started");

    // 提取 PTY fd
    int32_t pty_fd = find_named_fd(&args.fdList, PTY_FD_NAME);
    if (pty_fd < 0) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain: no pty fd provided");
        return;
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: pty_fd=%{public}d", pty_fd);

    // 解析 entryParams
    const char* env_vars = nullptr;
    int env_vars_len = 0;
    const char* init_script = nullptr;
    int init_script_len = 0;
    parse_entry_params(args.entryParams, &env_vars, &env_vars_len,
                       &init_script, &init_script_len);
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: env_vars_len=%{public}d, init_script_len=%{public}d",
                env_vars_len, init_script_len);

    // SSH 连接 VM
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: calling ssh_connect_to_vm...");
    int sock = -1;
    LIBSSH2_SESSION* session = ssh_connect_to_vm(&sock);
    if (!session) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain: ssh_connect_to_vm failed");
        return;
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: ssh_connect_to_vm OK, sock=%{public}d", sock);

    // 打开 SSH channel
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: opening SSH channel...");
    LIBSSH2_CHANNEL* channel = libssh2_channel_open_session(session);
    if (!channel) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain: channel_open_session failed");
        libssh2_session_free(session);
        close(sock);
        return;
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: SSH channel opened");

    // 请求远程 PTY
    const char* term_type = getenv("TERM");
    if (!term_type) term_type = "xterm-256color";
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: requesting PTY, term=%{public}s", term_type);
    if (libssh2_channel_request_pty_ex(channel,
                                       term_type,
                                       static_cast<unsigned int>(strlen(term_type)),
                                       nullptr, 0,
                                       LIBSSH2_TERM_WIDTH, LIBSSH2_TERM_HEIGHT,
                                       LIBSSH2_TERM_WIDTH_PX, LIBSSH2_TERM_HEIGHT_PX) != 0) {
        OH_LOG_WARN(LOG_APP, "ShellBridgeMain: request_pty failed, continuing");
    } else {
        OH_LOG_INFO(LOG_APP, "ShellBridgeMain: PTY allocated on VM");
    }

    // 启动干净 bash（不加载 profile/bashrc，避免 MOTD 等干扰输出）
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: starting bash via channel_exec...");
    if (libssh2_channel_exec(channel, "bash --norc --noprofile -i") != 0) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain: channel_exec failed, falling back to channel_shell");
        if (libssh2_channel_shell(channel) != 0) {
            OH_LOG_ERROR(LOG_APP, "ShellBridgeMain: channel_shell also failed");
            libssh2_channel_close(channel);
            libssh2_session_free(session);
            close(sock);
            return;
        }
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: shell started on VM");

    // 写 init 脚本前先设置非阻塞模式（socket blocking 下 channel_write 会卡死）
    set_nonblocking(pty_fd);
    set_nonblocking(sock);
    libssh2_session_set_blocking(session, 0);

    // 通过 SSH channel 把 init rcfile 写入 VM 的 /tmp/warp_init.sh 并 source
    char init_buf[65536];
    int init_len = build_init_script(init_buf, sizeof(init_buf),
                                      env_vars, env_vars_len,
                                      init_script, init_script_len);
    if (init_len > 0) {
        char cmd_buf[65536 + 128];
        int cmd_len = snprintf(cmd_buf, sizeof(cmd_buf),
            "cat > /tmp/warp_init.sh << 'WARPEOF'\n%.*s\nWARPEOF\nsource /tmp/warp_init.sh\n",
            init_len, init_buf);
        if (cmd_len > 0 && cmd_len < static_cast<int>(sizeof(cmd_buf))) {
            // 非阻塞模式下循环写直到完成
            const char* p = cmd_buf;
            int remaining = cmd_len;
            while (remaining > 0) {
                ssize_t n = libssh2_channel_write(channel, p, remaining);
                if (n > 0) {
                    p += n;
                    remaining -= n;
                } else if (n == LIBSSH2_ERROR_EAGAIN) {
                    fd_set wfds; FD_ZERO(&wfds); FD_SET(sock, &wfds);
                    struct timeval tv = {.tv_sec = 5, .tv_usec = 0};
                    select(sock + 1, nullptr, &wfds, nullptr, &tv);
                } else {
                    OH_LOG_WARN(LOG_APP, "ShellBridgeMain: channel_write error, rc=%{public}zd", n);
                    break;
                }
            }
            OH_LOG_INFO(LOG_APP, "ShellBridgeMain: init script written to /tmp/warp_init.sh");
        }
    }

    // 给 bash 一点时间处理初始命令
    usleep(100000);

    // 主循环：PTY ↔ SSH channel 双向转发
    unsigned short current_rows = 0, current_cols = 0;
    char buf[BUF_SIZE];

    // 待写缓冲：处理 PTY 写满 EAGAIN 时暂存数据，由 select 在可写时写入
    char pending_buf[BUF_SIZE];
    size_t pending_len = 0;
    size_t pending_offset = 0;

    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: entering forwarding loop (select-based)");

    // 诊断：记录前 20 次转发事件（用 hilog 自带时间戳）
    int diag_chunk_count = 0;

    while (true) {
        // 用 select 同时等待 PTY 和 SSH socket，1 秒超时用于 resize 检查和 keepalive
        fd_set read_fds;
        fd_set write_fds;
        FD_ZERO(&read_fds);
        FD_ZERO(&write_fds);
        FD_SET(pty_fd, &read_fds);
        FD_SET(sock, &read_fds);
        int max_fd = (pty_fd > sock) ? pty_fd : sock;
        // 有待写数据时监听 PTY 可写事件
        if (pending_len > 0) {
            FD_SET(pty_fd, &write_fds);
        }
        struct timeval tv = {.tv_sec = 1, .tv_usec = 0};

        int sel_ret = select(max_fd + 1, &read_fds, pending_len > 0 ? &write_fds : nullptr, nullptr, &tv);

        if (sel_ret < 0) {
            if (errno == EINTR) continue;
            OH_LOG_INFO(LOG_APP, "ShellBridgeMain: select error, errno=%{public}d, exiting", errno);
            break;
        }

        // 优先排空待写缓冲（PTY 可写时）
        if (pending_len > 0 && FD_ISSET(pty_fd, &write_fds)) {
            ssize_t n = write(pty_fd, pending_buf + pending_offset, pending_len);
            if (n > 0) {
                pending_len -= n;
                pending_offset += n;
                if (pending_len == 0) {
                    pending_offset = 0;
                }
                // 写了一些，继续新的数据转发
            } else if (n < 0 && errno != EAGAIN && errno != EINTR) {
                OH_LOG_INFO(LOG_APP, "ShellBridgeMain: pending write error, errno=%{public}d, exiting", errno);
                break;
            }
            // EAGAIN: 下次 select 再试
        }

        // PTY → SSH：读取用户输入，转发到远程 shell
        if (FD_ISSET(pty_fd, &read_fds)) {
            ssize_t n = read(pty_fd, buf, sizeof(buf));
            if (n > 0) {
                if (diag_chunk_count < 20) {
                    OH_LOG_INFO(LOG_APP, "BRIDGE_DIAG: PTY→SSH bytes=%{public}zd", n);
                    diag_chunk_count++;
                }
                libssh2_channel_write(channel, buf, n);
            } else if (n < 0 && errno != EAGAIN) {
                OH_LOG_INFO(LOG_APP, "ShellBridgeMain: pty read error, errno=%{public}d, exiting", errno);
                break;
            }
        }

        // SSH → PTY：读取远程 shell 输出，写入本地 PTY
        // 仅在待写缓冲为空时才读 SSH，避免数据堆积
        if (pending_len == 0 && FD_ISSET(sock, &read_fds)) {
            while (true) {
                ssize_t n = libssh2_channel_read(channel, buf, sizeof(buf));
                if (n > 0) {
                    if (diag_chunk_count < 20) {
                        OH_LOG_INFO(LOG_APP, "BRIDGE_DIAG: SSH→PTY #%{public}d bytes=%{public}zd",
                                    diag_chunk_count, n);
                        diag_chunk_count++;
                    }
                    ssize_t written = write_all(pty_fd, buf, n);
                    if (written < n) {
                        // 部分写入：剩余数据存入待写缓冲
                        if (written < 0) written = 0;
                        size_t remaining = static_cast<size_t>(n) - static_cast<size_t>(written);
                        if (remaining > sizeof(pending_buf)) {
                            OH_LOG_WARN(LOG_APP, "ShellBridgeMain: pending buf overflow, dropping %{public}zu bytes",
                                        remaining - sizeof(pending_buf));
                            remaining = sizeof(pending_buf);
                        }
                        memcpy(pending_buf, buf + written, remaining);
                        pending_len = remaining;
                        pending_offset = 0;
                        break; // 等待下次 select 写 PTY
                    }
                } else if (n == LIBSSH2_ERROR_EAGAIN) {
                    int dir = libssh2_session_block_directions(session);
                    if (dir & LIBSSH2_SESSION_BLOCK_OUTBOUND) {
                        fd_set wfds; FD_ZERO(&wfds); FD_SET(sock, &wfds);
                        struct timeval wtv = {.tv_sec = 1, .tv_usec = 0};
                        select(sock + 1, nullptr, &wfds, nullptr, &wtv);
                        continue;
                    }
                    break;
                } else if (n < 0) {
                    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: channel read error, rc=%{public}zd, exiting", n);
                    break;
                } else {
                    break;
                }
            }
        }

        // 检查 SSH channel 是否已关闭（远程 bash 退出）
        if (libssh2_channel_eof(channel)) {
            OH_LOG_INFO(LOG_APP, "ShellBridgeMain: channel EOF, exiting");
            break;
        }

        // 检查 PTY 窗口尺寸变化，通知远程 bash
        struct winsize ws;
        if (ioctl(pty_fd, TIOCGWINSZ, &ws) == 0) {
            if (ws.ws_row != current_rows || ws.ws_col != current_cols) {
                OH_LOG_INFO(LOG_APP, "ShellBridgeMain: resize %{public}d×%{public}d → %{public}d×%{public}d",
                           current_rows, current_cols, ws.ws_row, ws.ws_col);
                libssh2_channel_request_pty_size_ex(channel,
                    ws.ws_col, ws.ws_row, 0, 0);
                current_rows = ws.ws_row;
                current_cols = ws.ws_col;
            }
        }

        // 发送 keepalive 维持连接
        int keepalive_rc = 0;
        libssh2_keepalive_send(session, &keepalive_rc);
    }

    // 清理
    libssh2_channel_close(channel);
    libssh2_channel_free(channel);
    libssh2_session_free(session);
    close(sock);
    close(pty_fd);

    OH_LOG_INFO(LOG_APP, "ShellBridgeMain: exiting");
}

// === ExecCmdMain：一次性命令入口 ==============================================
//
// 由 OH_Ability_StartNativeChildProcess("libohos_shell.so:ExecCmdMain") 创建。
// 接收命令字符串（entryParams）和输出管道写端 fd（命名 fd "output"）。
// 通过 SSH 在 VM 上执行命令，把 stdout 和退出码写入管道，然后退出。

void ExecCmdMain(NativeChildProcess_Args args) {
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: started");

    // 提取输出管道 fd
    int32_t pipe_fd = find_named_fd(&args.fdList, OUTPUT_FD_NAME);
    if (pipe_fd < 0) {
        OH_LOG_ERROR(LOG_APP, "ExecCmdMain: no output pipe fd provided");
        return;
    }
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: pipe_fd=%{public}d", pipe_fd);

    // 命令字符串从 entryParams 获取
    const char* command = args.entryParams ? args.entryParams : "";
    if (command[0] == '\0') {
        OH_LOG_ERROR(LOG_APP, "ExecCmdMain: empty command");
        close(pipe_fd);
        return;
    }
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: command=%{public}s", command);

    // SSH 连接 VM
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: calling ssh_connect_to_vm...");
    int sock = -1;
    LIBSSH2_SESSION* session = ssh_connect_to_vm(&sock);
    if (!session) {
        OH_LOG_ERROR(LOG_APP, "ExecCmdMain: ssh_connect_to_vm failed");
        close(pipe_fd);
        return;
    }
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: ssh_connect_to_vm OK, sock=%{public}d", sock);

    // 阻塞模式用于一次性命令
    libssh2_session_set_blocking(session, 1);

    // 打开 channel
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: opening SSH channel...");
    LIBSSH2_CHANNEL* channel = libssh2_channel_open_session(session);
    if (!channel) {
        OH_LOG_ERROR(LOG_APP, "ExecCmdMain: channel_open_session failed");
        libssh2_session_free(session);
        close(sock);
        close(pipe_fd);
        return;
    }
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: channel opened, executing command...");

    // 使用 exec 模式（shell 模式在 OHOS fork 下也会卡）
    if (libssh2_channel_exec(channel, command) != 0) {
        OH_LOG_ERROR(LOG_APP, "ExecCmdMain: channel_exec failed");
        libssh2_channel_close(channel);
        libssh2_session_free(session);
        close(sock);
        close(pipe_fd);
        return;
    }
    OH_LOG_INFO(LOG_APP, "ExecCmdMain: exec sent OK, reading output...");

    // 读取输出——用 select 等待数据，不轮询
    char buf[BUF_SIZE];
    ssize_t total_read = 0;
    ssize_t n;

    // 关键：socket 和 session 都要非阻塞
    int sock_flags = fcntl(sock, F_GETFL, 0);
    fcntl(sock, F_SETFL, sock_flags | O_NONBLOCK);
    libssh2_session_set_blocking(session, 0);

    // 最多等待 20 秒，用 select 挂起
    const int MAX_SELECT_WAIT_MS = 20000;
    int waited_ms = 0;

    while (waited_ms < MAX_SELECT_WAIT_MS) {
        n = libssh2_channel_read(channel, buf, sizeof(buf));
        if (n > 0) {
            total_read += n;
            write_all(pipe_fd, buf, n);
            waited_ms = 0; // 有数据则重置超时
        } else if (n == LIBSSH2_ERROR_EAGAIN) {
            int dir = libssh2_session_block_directions(session);
            fd_set rfds, wfds;
            FD_ZERO(&rfds); FD_ZERO(&wfds);
            FD_SET(sock, &rfds);
            if (dir & LIBSSH2_SESSION_BLOCK_OUTBOUND)
                FD_SET(sock, &wfds);

            struct timeval tv = {.tv_sec = 0, .tv_usec = 100000}; // 100ms
            int sel = select(sock + 1, &rfds, &wfds, nullptr, &tv);
            if (sel == 0) waited_ms += 100; // 超时
            // sel > 0: 有数据，下次循环读
        } else if (n < 0) {
            char* errmsg = nullptr;
            libssh2_session_last_error(session, &errmsg, nullptr, 0);
            OH_LOG_WARN(LOG_APP, "ExecCmdMain: channel_read error, rc=%{public}zd, errmsg=%{public}s",
                        n, errmsg ? errmsg : "unknown");
            break;
        } else {
            break;
        }

        if (libssh2_channel_eof(channel)) break;
    }

    OH_LOG_INFO(LOG_APP, "ExecCmdMain: done reading, total_read=%{public}zd, waited=%{public}dms",
                total_read, waited_ms);

    // 再试 stderr
    char stderr_buf[512];
    while (libssh2_channel_read_stderr(channel, stderr_buf, sizeof(stderr_buf)) > 0) {
        total_read += write_all(pipe_fd, stderr_buf, sizeof(stderr_buf));
    }

    // 获取退出码并写入管道
    int exit_code = libssh2_channel_get_exit_status(channel);
    write_all(pipe_fd, &exit_code, sizeof(exit_code));

    OH_LOG_INFO(LOG_APP, "ExecCmdMain: command finished, total_read=%{public}zd, exit_code=%{public}d",
                total_read, exit_code);

    // 清理
    libssh2_channel_close(channel);
    libssh2_channel_free(channel);
    libssh2_session_free(session);
    close(sock);
    close(pipe_fd);

    OH_LOG_INFO(LOG_APP, "ExecCmdMain: exiting");
}

// === ShellBridgeMain2：Socket 桥接入口（替代 PTY 方案）=============================
//
// 由 OH_Ability_StartNativeChildProcess("libohos_shell.so:ShellBridgeMain2") 创建。
// 接收 data_fd（数据通道）和 control_fd（控制通道）两个命名 fd。
// 连接 VM 后发 ready 信号，然后转发 data_fd ↔ SSH 数据，监控 control_fd 的 resize 指令。
// 不创建本地 PTY，VM 上的 bash PTY 就够了。
//
// control_fd 协议：
//   4 字节: [cols:u16][rows:u16] → libssh2_channel_request_pty_size_ex()

void ShellBridgeMain2(NativeChildProcess_Args args) {
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: started");

    // 提取命名 fd
    int32_t data_fd = find_named_fd(&args.fdList, "data");
    int32_t control_fd = find_named_fd(&args.fdList, "control");
    if (data_fd < 0 || control_fd < 0) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain2: missing fd (data=%d, control=%d)", data_fd, control_fd);
        return;
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: data_fd=%{public}d, control_fd=%{public}d", data_fd, control_fd);

    // 解析 entryParams
    const char* env_vars = nullptr;
    int env_vars_len = 0;
    const char* init_script = nullptr;
    int init_script_len = 0;
    parse_entry_params(args.entryParams, &env_vars, &env_vars_len,
                       &init_script, &init_script_len);
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: env_vars_len=%{public}d, init_script_len=%{public}d",
                env_vars_len, init_script_len);

    // SSH 连接 VM
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: calling ssh_connect_to_vm...");
    int sock = -1;
    LIBSSH2_SESSION* session = ssh_connect_to_vm(&sock);
    if (!session) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain2: ssh_connect_to_vm failed");
        return;
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: ssh_connect_to_vm OK, sock=%{public}d", sock);

    // 打开 SSH channel
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: opening SSH channel...");
    LIBSSH2_CHANNEL* channel = libssh2_channel_open_session(session);
    if (!channel) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain2: channel_open_session failed");
        libssh2_session_free(session);
        close(sock);
        return;
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: SSH channel opened");

    // 请求远程 PTY
    const char* term_type = getenv("TERM");
    if (!term_type) term_type = "xterm-256color";
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: requesting PTY, term=%{public}s", term_type);
    if (libssh2_channel_request_pty_ex(channel,
                                       term_type,
                                       static_cast<unsigned int>(strlen(term_type)),
                                       nullptr, 0,
                                       80, 24, 0, 0) != 0) {
        OH_LOG_WARN(LOG_APP, "ShellBridgeMain2: request_pty failed, continuing");
    } else {
        OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: PTY allocated on VM");
    }

    // 启动干净 bash
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: starting bash via channel_exec...");
    if (libssh2_channel_exec(channel, "bash --norc --noprofile -i") != 0) {
        OH_LOG_ERROR(LOG_APP, "ShellBridgeMain2: channel_exec failed, falling back to channel_shell");
        if (libssh2_channel_shell(channel) != 0) {
            OH_LOG_ERROR(LOG_APP, "ShellBridgeMain2: channel_shell also failed");
            libssh2_channel_close(channel);
            libssh2_session_free(session);
            close(sock);
            return;
        }
    }
    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: shell started on VM");

    // 配置 SSH keepalive
    libssh2_keepalive_config(session, 1, 30);

    // 设非阻塞模式（与 ShellBridgeMain 一致：在写 init 脚本前设置）
    set_nonblocking(data_fd);
    set_nonblocking(sock);
    libssh2_session_set_blocking(session, 0);

    // 通过 SSH channel 把 init rcfile 写入 VM 的 /tmp/warp_init.sh 并 source
    char init_buf[65536];
    int init_len = build_init_script(init_buf, sizeof(init_buf),
                                      env_vars, env_vars_len,
                                      init_script, init_script_len);
    if (init_len > 0) {
        char cmd_buf[65536 + 128];
        int cmd_len = snprintf(cmd_buf, sizeof(cmd_buf),
            "cat > /tmp/warp_init.sh << 'WARPEOF'\n%.*s\nWARPEOF\nsource /tmp/warp_init.sh\n",
            init_len, init_buf);
        if (cmd_len > 0 && cmd_len < static_cast<int>(sizeof(cmd_buf))) {
            // 非阻塞模式下循环写直到完成
            const char* p = cmd_buf;
            int remaining = cmd_len;
            while (remaining > 0) {
                ssize_t n = libssh2_channel_write(channel, p, remaining);
                if (n > 0) {
                    p += n;
                    remaining -= n;
                } else if (n == LIBSSH2_ERROR_EAGAIN) {
                    fd_set wfds; FD_ZERO(&wfds); FD_SET(sock, &wfds);
                    struct timeval tv = {.tv_sec = 5, .tv_usec = 0};
                    select(sock + 1, nullptr, &wfds, nullptr, &tv);
                } else {
                    OH_LOG_WARN(LOG_APP, "ShellBridgeMain2: init channel_write error, rc=%{public}zd", n);
                    break;
                }
            }
            OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: init script written to /tmp/warp_init.sh");
        }
    }

    // 给 bash 一点时间处理初始命令
    usleep(100000);

    // 发 ready 信号给 Rust 侧
    char ready = 'R';
    write(control_fd, &ready, 1);

    // 主循环：data_fd ↔ SSH channel 双向转发 + control_fd 控制
    char buf[BUF_SIZE];

    // 待写缓冲：处理 data_fd 写满 EAGAIN 时暂存数据
    char pending_buf[BUF_SIZE];
    size_t pending_len = 0;
    size_t pending_offset = 0;

    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: entering forwarding loop (select-based)");

    // 诊断：记录前 20 次转发事件
    int diag_chunk_count = 0;

    while (true) {
        fd_set read_fds;
        fd_set write_fds;
        FD_ZERO(&read_fds);
        FD_ZERO(&write_fds);
        FD_SET(data_fd, &read_fds);
        FD_SET(sock, &read_fds);
        FD_SET(control_fd, &read_fds);
        int max_fd = (data_fd > sock) ? data_fd : sock;
        if (control_fd > max_fd) max_fd = control_fd;
        // 有待写数据时监控 data_fd 可写
        if (pending_len > 0) {
            FD_SET(data_fd, &write_fds);
        }
        struct timeval tv = {.tv_sec = 1, .tv_usec = 0};

        int sel_ret = select(max_fd + 1, &read_fds, pending_len > 0 ? &write_fds : nullptr, nullptr, &tv);

        if (sel_ret < 0) {
            if (errno == EINTR) continue;
            OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: select error, errno=%{public}d, exiting", errno);
            break;
        }

        // 优先排空待写缓冲（data_fd 可写时）
        if (pending_len > 0 && FD_ISSET(data_fd, &write_fds)) {
            ssize_t n = write(data_fd, pending_buf + pending_offset, pending_len);
            if (n > 0) {
                pending_len -= n;
                pending_offset += n;
                if (pending_len == 0) {
                    pending_offset = 0;
                }
            } else if (n < 0 && errno != EAGAIN && errno != EINTR) {
                OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: pending write error, errno=%{public}d, exiting", errno);
                break;
            }
        }

        // control_fd → resize 指令
        if (FD_ISSET(control_fd, &read_fds)) {
            uint16_t size[2];
            int n = read(control_fd, size, sizeof(size));
            if (n == 4) {
                OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: resize %dx%d", size[0], size[1]);
                libssh2_channel_request_pty_size_ex(channel, size[0], size[1], 0, 0);
            } else if (n <= 0) {
                OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: control_fd closed, exiting");
                break;
            }
        }

        // data_fd → SSH：读取用户输入，转发到远程 shell
        if (FD_ISSET(data_fd, &read_fds)) {
            ssize_t n = read(data_fd, buf, sizeof(buf));
            if (n > 0) {
                if (diag_chunk_count < 20) {
                    OH_LOG_INFO(LOG_APP, "BRIDGE_DIAG: data_fd→SSH bytes=%{public}zd", n);
                    diag_chunk_count++;
                }
                libssh2_channel_write(channel, buf, n);
            } else if (n < 0 && errno != EAGAIN) {
                OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: data_fd read error, errno=%{public}d, exiting", errno);
                break;
            }
        }

        // SSH → data_fd：读取远程 shell 输出，写入本地
        // 仅在待写缓冲为空时才读 SSH，避免数据堆积
        if (pending_len == 0 && FD_ISSET(sock, &read_fds)) {
            while (true) {
                ssize_t n = libssh2_channel_read(channel, buf, sizeof(buf));
                if (n > 0) {
                    if (diag_chunk_count < 20) {
                        OH_LOG_INFO(LOG_APP, "BRIDGE_DIAG: SSH→data_fd #%{public}d bytes=%{public}zd",
                                    diag_chunk_count, n);
                        diag_chunk_count++;
                    }
                    ssize_t written = write_all(data_fd, buf, n);
                    if (written < n) {
                        if (written < 0) written = 0;
                        size_t remaining = static_cast<size_t>(n) - static_cast<size_t>(written);
                        if (remaining > sizeof(pending_buf)) {
                            OH_LOG_WARN(LOG_APP, "ShellBridgeMain2: pending buf overflow, dropping %{public}zu bytes",
                                        remaining - sizeof(pending_buf));
                            remaining = sizeof(pending_buf);
                        }
                        memcpy(pending_buf, buf + written, remaining);
                        pending_len = remaining;
                        pending_offset = 0;
                        break;
                    }
                } else if (n == LIBSSH2_ERROR_EAGAIN) {
                    int dir = libssh2_session_block_directions(session);
                    if (dir & LIBSSH2_SESSION_BLOCK_OUTBOUND) {
                        fd_set wfds; FD_ZERO(&wfds); FD_SET(sock, &wfds);
                        struct timeval wtv = {.tv_sec = 1, .tv_usec = 0};
                        select(sock + 1, nullptr, &wfds, nullptr, &wtv);
                        continue;
                    }
                    break;
                } else if (n < 0) {
                    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: channel read error, rc=%{public}zd, exiting", n);
                    break;
                } else {
                    break;
                }
            }
        }

        // 发送 SSH keepalive
        {
            int keepalive_rc = 0;
            libssh2_keepalive_send(session, &keepalive_rc);
        }

        // 检查 SSH 通道关闭
        if (libssh2_channel_eof(channel)) {
            OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: channel EOF, exiting");
            break;
        }
    }

    // 清理
    libssh2_channel_close(channel);
    libssh2_channel_free(channel);
    libssh2_session_free(session);
    close(sock);
    close(data_fd);
    close(control_fd);

    OH_LOG_INFO(LOG_APP, "ShellBridgeMain2: exiting");
}

} // extern "C"
