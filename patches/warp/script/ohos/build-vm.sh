#!/usr/bin/env bash
# §4.1 — OHOS 交叉编译脚本 (openEuler VM 版)
#
# 在 openEuler aarch64 VM 上交叉编译 Warp → HarmonyOS aarch64，包括 HAP 打包。
#
# 区别于 build.sh (Mac 版):
#   - 自动加载 ~/.cargo/env (Rust 不在默认 PATH 中)
#   - NDK / SDK 路径指向 /mnt/linux_share/openharmony/...
#   - 链接器 / 工具链在 /usr/local/bin
#   - 自动覆盖链接器名为 ohos-clang
#   - target 放本地 /tmp（共享目录不支持 cargo 硬链接）
#   - 自动配置 hvigor local.properties SDK 路径
#
# 使用方式:
#   ./script/ohos/build-vm.sh [--release] [cargo args...]
#
# 示例:
#   ./script/ohos/build-vm.sh                       # debug 构建
#   ./script/ohos/build-vm.sh --release              # release 构建
#   ./script/ohos/build-vm.sh --release -p warp      # 仅编译 warp crate

set -euo pipefail

# ── 计时辅助 ──────────────────────────────────────────────────────────────
TIMER_START=$(date +%s)
timer() {
    local now=$(date +%s)
    local elapsed=$((now - TIMER_START))
    local label="$1"
    printf "[build-vm]   [%02d:%02d] %s\n" $((elapsed / 60)) $((elapsed % 60)) "$label"
}

# ── 常量 ──────────────────────────────────────────────────────────────────
OHOS_TARGET="aarch64-unknown-linux-ohos"
OHOS_NDK_HOME_DEFAULT="/mnt/linux_share/openharmony/hvigor/SDK/openharmony/native"
OHOS_SDK_ROOT="/mnt/linux_share/openharmony/hvigor/SDK"
HVIGORW="/mnt/linux_share/openharmony/hvigor/bin/hvigorw"
NODE_HOME="/usr/local/lib/node22"

# ── target 目录 ─────────────────────────────────────────────────────────────
# 共享文件系统不支持 cargo 的硬链接操作，必须放本地 FS
DEFAULT_TARGET_DIR="/tmp/warp-target"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$DEFAULT_TARGET_DIR}"
export CARGO_TERM_PROGRESS_WHEN=never
mkdir -p "$CARGO_TARGET_DIR"

# ── cargo 锁检查 ──────────────────────────────────────────────────────────
# 如果上一轮构建被中断，cargo 锁可能残留，导致新构建无限阻塞。
CARGO_LOCK="$CARGO_TARGET_DIR/.cargo-lock"
if [ -f "$CARGO_LOCK" ]; then
    LOCK_AGE=$(($(date +%s) - $(stat -c%Y "$CARGO_LOCK" 2>/dev/null || echo 0)))
    if [ "$LOCK_AGE" -gt 300 ]; then  # 锁超过 5 分钟，认为是残留
        echo "[build-vm] WARNING: Stale cargo lock found ($LOCK_AGE seconds old), removing..."
        rm -f "$CARGO_LOCK"
    else
        echo "[build-vm] Waiting for cargo lock (age: ${LOCK_AGE}s)..."
        # 最多等 60 秒
        for i in $(seq 1 60); do
            sleep 1
            [ ! -f "$CARGO_LOCK" ] && break
        done
        if [ -f "$CARGO_LOCK" ]; then
            echo "[build-vm] ERROR: Cargo lock still held after 60s, removing forcefully..."
            rm -f "$CARGO_LOCK"
        fi
    fi
fi

# ═══════════════════════════════════════════════════════════════════════════
# 1. 加载 Rust
# ═══════════════════════════════════════════════════════════════════════════
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if [ -f "$CARGO_HOME/env" ]; then
    # shellcheck source=/dev/null
    source "$CARGO_HOME/env"
fi

if ! command -v cargo &>/dev/null; then
    echo "[build-vm] ERROR: cargo not found. Check CARGO_HOME=$CARGO_HOME"
    exit 1
fi

echo "[build-vm] Rust toolchain: $(rustc --version)"
echo "[build-vm] cargo: $(cargo --version | head -1)"

# ═══════════════════════════════════════════════════════════════════════════
# 2. OHOS 目标检查
# ═══════════════════════════════════════════════════════════════════════════
if ! rustup target list --installed 2>/dev/null | grep -q "$OHOS_TARGET"; then
    echo "[build-vm] Adding OHOS target '$OHOS_TARGET'..."
    rustup target add "$OHOS_TARGET"
fi

# ═══════════════════════════════════════════════════════════════════════════
# 3. OHOS NDK 环境
# ═══════════════════════════════════════════════════════════════════════════
export OHOS_NDK_HOME="${OHOS_NDK_HOME:-$OHOS_NDK_HOME_DEFAULT}"

if [ ! -d "$OHOS_NDK_HOME/sysroot" ]; then
    echo "[build-vm] ERROR: OHOS NDK sysroot not found at $OHOS_NDK_HOME"
    echo "  Set OHOS_NDK_HOME or check the NDK installation."
    exit 1
fi
echo "[build-vm] OHOS NDK: $OHOS_NDK_HOME"

# aws-lc-sys 等 crate 的 cmake 构建需要显式设置工具链文件路径
# （自动推导会拼接出 double native 的错误路径）
export CMAKE_TOOLCHAIN_FILE_aarch64_unknown_linux_ohos="$OHOS_NDK_HOME/build/cmake/ohos.toolchain.cmake"
export CMAKE_MAKE_PROGRAM="/usr/bin/make"

# 覆盖 cmake 工具链文件中的编译器路径（NDK 自带的 clang-15 无法在 openEuler 上运行）
# 把 NDK 的 llvm/bin/clang 脚本替换为调用 ohos-clang 的包装器
NDK_CLANG_BIN="$OHOS_NDK_HOME/llvm/bin"

# 创建本地 clang 包装器（放在 /tmp，不修改共享 NDK）
# aws-lc-sys 等依赖 CMake 的 crate 需要通过 CMAKE_TOOLCHAIN_FILE 找到编译器。
# 共享 NDK 的 clang 文件是断链（指向 harmonybrew），不能直接使用。
# 在 /tmp 下创建包装器并用环境变量覆盖路径。
LOCAL_CLANG="/tmp/ohos-clang-wrapper"
mkdir -p "$LOCAL_CLANG"

for tool in clang clang++; do
    wrapper="$LOCAL_CLANG/$tool"
    linker="ohos-$tool"
    cat > "$wrapper" << WRAPEOF
#!/usr/bin/env bash
exec /usr/local/bin/$linker "\$@"
WRAPEOF
    chmod +x "$wrapper" 2>/dev/null || true
done

# 用环境变量让 cargo/cmake 使用 /tmp 下的包装器而非 NDK 的断链
export CC_aarch64_unknown_linux_ohos="$LOCAL_CLANG/clang"
export CXX_aarch64_unknown_linux_ohos="$LOCAL_CLANG/clang++"
# cmake toolchain 需要通过 CMAKE_C_COMPILER 找到编译器，用 CMAKE_TOOLCHAIN_FILE
# 中的编译器路径会指向 NDK，因此直接 export 环境变量覆盖
export CC="$LOCAL_CLANG/clang"
export CXX="$LOCAL_CLANG/clang++"
# 部分 Rust sys crate（如 aws-lc-sys）通过 CMake 编译 C 代码，CMake 的
# toolchain file 指向 NDK 的 clang-15（与本机 libc 不兼容）。NDK toolchain
# 通过 set(CMAKE_C_COMPILER) 覆盖环境变量，因此直接替换 NDK 的 clang 为
# 本机可运行的 ohos-clang 包装器。
NDK_CLANG="$OHOS_NDK_HOME/llvm/bin/clang"
NDK_CLANGXX="$OHOS_NDK_HOME/llvm/bin/clang++"
# 检查 clang 是否已指向 ohos-clang，避免重复备份
if [ -f "$NDK_CLANG" ] && ! grep -q "ohos-clang" "$NDK_CLANG" 2>/dev/null; then
    echo "[build-vm] Patching NDK clang -> ohos-clang wrapper"
    [ ! -f "$NDK_CLANG.orig" ] && cp "$NDK_CLANG" "$NDK_CLANG.orig" 2>/dev/null || true
    [ ! -f "$NDK_CLANGXX.orig" ] && cp "$NDK_CLANGXX" "$NDK_CLANGXX.orig" 2>/dev/null || true
    cat > "$NDK_CLANG" << 'CLANGEOF'
#!/usr/bin/env bash
exec /usr/local/bin/ohos-clang "$@"
CLANGEOF
    cp "$NDK_CLANG" "$NDK_CLANGXX" 2>/dev/null || true
fi

echo "[build-vm] Local clang wrappers created at $LOCAL_CLANG"

# SDK 的 cmake/ninja 是 Mach-O 格式（macOS），在 Linux 上无法执行，替换为系统版本
for TOOL in cmake ninja; do
    SDK_TOOL="$OHOS_NDK_HOME/build-tools/cmake/bin/$TOOL"
    SDK_ORIG="$SDK_TOOL.orig"
    SYSTEM_TOOL="/usr/bin/$TOOL"
    if [ -f "$SDK_TOOL" ] && [ ! -f "$SDK_ORIG" ] && [ -x "$SYSTEM_TOOL" ]; then
        echo "[build-vm] Patching SDK $TOOL ($(file "$SDK_TOOL" | grep -o 'Mach-O')) -> $SYSTEM_TOOL"
        mv "$SDK_TOOL" "$SDK_ORIG"
        cat > "$SDK_TOOL" << TOOL_EOF
#!/usr/bin/env bash
exec $SYSTEM_TOOL "\$@"
TOOL_EOF
        chmod +x "$SDK_TOOL" 2>/dev/null || true
        echo "[build-vm]   Patched $SDK_TOOL -> $SYSTEM_TOOL"
    fi
done

# ── 修复 libc++ ABI 命名空间 ────────────────────────────────────────────
# OHOS NDK 预编译的 libc++ 库（libc++_static.a/libc++_shared.so）使用 __n1
# ABI 命名空间，但默认头文件 __config_site 定义为 __1，导致链接时符号
# 不匹配。libcxx-ohos 目录提供了 OHOS 正确的 __config_site。
LIBCXX_CONFIG="$OHOS_NDK_HOME/llvm/include/c++/v1/__config_site"
LIBCXX_OHOS_CONFIG="$OHOS_NDK_HOME/llvm/include/libcxx-ohos/include/c++/v1/__config_site"
if [ -f "$LIBCXX_OHOS_CONFIG" ] && [ -f "$LIBCXX_CONFIG" ]; then
    CURR_NS=$(grep '_LIBCPP_ABI_NAMESPACE' "$LIBCXX_CONFIG" | awk '{print $3}')
    TGT_NS=$(grep '_LIBCPP_ABI_NAMESPACE' "$LIBCXX_OHOS_CONFIG" | awk '{print $3}')
    if [ "$CURR_NS" != "$TGT_NS" ] && [ -n "$TGT_NS" ]; then
        echo "[build-vm] Patching libc++ ABI namespace: $CURR_NS → $TGT_NS"
        cp "$LIBCXX_OHOS_CONFIG" "$LIBCXX_CONFIG"
    fi
fi

# NDK 的 llvm/bin 加入 PATH（部分脚本依赖此路径）
export PATH="$OHOS_NDK_HOME/llvm/bin:$PATH"

# ═══════════════════════════════════════════════════════════════════════════
# 4. 链接器配置
# ═══════════════════════════════════════════════════════════════════════════
# .cargo/config.toml 中 linker = "aarch64-unknown-linux-ohos-clang"，但安装
# 的包装器叫 ohos-clang。用环境变量覆盖，无需往 /usr/local/bin 写 symlink。
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="ohos-clang"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_CXX="ohos-clang++"

# cc crate (freetype-sys 等 C 代码编译) 需要用正确的编译器/归档器
export CC_aarch64_unknown_linux_ohos="/usr/local/bin/ohos-clang"
export CXX_aarch64_unknown_linux_ohos="/usr/local/bin/ohos-clang++"
export AR_aarch64_unknown_linux_ohos="/usr/local/bin/llvm-ar"
export RANLIB_aarch64_unknown_linux_ohos="/usr/local/bin/llvm-ranlib"

# 某些 C crate（如 zstd-sys）需要额外的 C 宏定义
# OHOS libc 没有 qsort_r，需要 ZSTD_NOVMS
export CFLAGS_aarch64_unknown_linux_ohos="-DZSTD_NOVMS -D__ANDROID__ -Wno-incompatible-function-pointer-types"


# 确保 PATH 包含 /usr/local/bin (放置 ohos-clang、llvm-ar 等)
case ":$PATH:" in
    *:/usr/local/bin:*) ;;
    *) export PATH="/usr/local/bin:$PATH" ;;
esac

# ═══════════════════════════════════════════════════════════════════════════
# 5. 项目根目录
# ═══════════════════════════════════════════════════════════════════════════
PROJECT_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$PROJECT_ROOT"

# ═══════════════════════════════════════════════════════════════════════════
# 6. 解析参数
# ═══════════════════════════════════════════════════════════════════════════
IS_RELEASE=false
CARGO_ARGS=()
for arg in "$@"; do
    if [ "$arg" = "--release" ]; then
        IS_RELEASE=true
    else
        CARGO_ARGS+=("$arg")
    fi
done

# ═══════════════════════════════════════════════════════════════════════════
# 7. 并行代码生成单元
# ═══════════════════════════════════════════════════════════════════════════
if [ "$IS_RELEASE" = true ]; then
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
    PROFILE_DIR="release"
else
    export CARGO_PROFILE_DEV_CODEGEN_UNITS=16
    PROFILE_DIR="debug"
fi

# ═══════════════════════════════════════════════════════════════════════════
# 8. Cargo 交叉编译
# ═══════════════════════════════════════════════════════════════════════════
NPROC=$(nproc 2>/dev/null || echo 4)
timer "Rust compilation starting ($NPROC CPUs)..."
echo "[build-vm] $ cargo build $($IS_RELEASE && echo --release) --target $OHOS_TARGET ${CARGO_ARGS[*]+"${CARGO_ARGS[@]}"}"

# 全局 -A warnings 避免代码库已有警告阻塞编译（全量编译时触发，增量不触发）
RUSTFLAGS="${RUSTFLAGS:-} -A warnings" \
cargo build $($IS_RELEASE && echo --release) \
    --lib -p warp \
    --target "$OHOS_TARGET" \
    -j "$NPROC" \
    "${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"}"

echo "[build-vm] ✅ Rust cross-compilation succeeded"
timer "Rust compilation finished"

# ── 拷贝 libwarp.so 到 HAP 工程目录（共享文件系统，远程立即可见） ─────
HAP_LIBS_DIR="$PROJECT_ROOT/app/src/platform/ohos/hap/entry/libs/arm64-v8a"
mkdir -p "$HAP_LIBS_DIR"
timer "Copying libwarp.so (to HAP libs dir)..."
cp "$CARGO_TARGET_DIR/$OHOS_TARGET/$PROFILE_DIR/libwarp.so" "$HAP_LIBS_DIR/libwarp.so"
echo "[build-vm]   libwarp.so staged at $HAP_LIBS_DIR/libwarp.so"

# ═══════════════════════════════════════════════════════════════════════════
# 9. SSH 远程构建 HAP（远程机处理 CMake/Ninja/ArkTS 编译）
# ═══════════════════════════════════════════════════════════════════════════
REMOTE_HOST="172.16.100.1"
REMOTE_PORT="8022"
REMOTE_USER="edge"
REMOTE_HAP_DIR="/storage/Users/currentUser/workspace/warp-winit/app/src/platform/ohos/hap"
timer "Triggering remote HAP build (SSH)..."
if ssh -p "$REMOTE_PORT" "$REMOTE_USER@$REMOTE_HOST" \
    "cd $REMOTE_HAP_DIR && ./build-hap.sh 2>&1"; then
    echo "[build-vm] ✅ Remote HAP build succeeded"
    timer "Remote HAP build finished"
else
    echo "[build-vm] ⚠️  Remote HAP build failed — check remote for details"
    timer "Remote HAP build FAILED"
fi

TARGET_DIR="$CARGO_TARGET_DIR/$OHOS_TARGET/$PROFILE_DIR"
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  Build summary:"
echo "    Target:     $OHOS_TARGET"
echo "    Profile:    $PROFILE_DIR"
echo "    Rust libs:  $TARGET_DIR"
echo ""

if [ -d "$TARGET_DIR" ]; then
    echo "  Libraries:"
    find "$TARGET_DIR" -maxdepth 1 \( -name '*.so' -o -name '*.a' \) \
        -exec stat -c '    %n (%s bytes)' {} \; 2>/dev/null || true
fi

# 找 HAP 产物
HAP_OUTPUT=$(find "$PROJECT_ROOT/app/src/platform/ohos/hap" -name "*.hap" -type f 2>/dev/null | head -3)
if [ -n "$HAP_OUTPUT" ]; then
    echo ""
    echo "  HAP packages:"
    while IFS= read -r hap; do
        stat -c '    %n (%s bytes)' "$hap"
    done <<< "$HAP_OUTPUT"
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
