#!/usr/bin/env bash
# §4.1 — OHOS 构建编排脚本
#
# 1. Cargo build（build.rs 写入 ohos-ability-path.txt）
# 2. 读取路径，在 hap/deps/ 下创建 @ohos-rs/ability 软链
# 3. hvigor build 构建 HAP
#
# 使用方式:
#   ./script/ohos/build.sh [--release] [cargo args...]

set -euo pipefail

OHOS_TARGET="aarch64-unknown-linux-ohos"
export OHOS_NDK_HOME="/storage/Users/currentUser/.harmonybrew/Cellar/ohos-sdk/26.0.0.18_1"
OHOS_NDK="$OHOS_NDK_HOME/native"

OHOS_LLVM_DIR="$OHOS_NDK/llvm/bin"
export PATH="$OHOS_LLVM_DIR:$PATH"

export CC_aarch64_unknown_linux_ohos="aarch64-unknown-linux-ohos-clang"
export AR_aarch64_unknown_linux_ohos="llvm-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="aarch64-unknown-linux-ohos-clang"
export CMAKE_MAKE_PROGRAM="/storage/Users/currentUser/.harmonybrew/bin/make"

# 1. 强制限制编译器内部的工作线程数（关键！）
export RAYON_NUM_THREADS=1

# 2. 强制将代码生成单元设为 1（必须加！）
export RUSTFLAGS="-C codegen-units=1"

# --release → release profile, default → debug
IS_RELEASE=false
CARGO_ARGS=()
for arg in "$@"; do
    if [ "$arg" = "--release" ]; then
        IS_RELEASE=true
    else
        CARGO_ARGS+=("$arg")
    fi
done

if [ "$IS_RELEASE" = true ]; then
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
    PROFILE_DIR="release"
else
    export CARGO_PROFILE_DEV_CODEGEN_UNITS=1
    PROFILE_DIR="debug"
fi

if [ ! -d "$OHOS_NDK/llvm" ]; then
    echo "[ohos-build] ERROR: OHOS NDK not found at $OHOS_NDK"
    exit 1
fi

# OHOS: wrap /bin/id which returns "bad uid" for everything.
# GNU gettext configure calls id -u / id -un etc; without valid output the
# entire configure chain collapses.
_ID_WRAPPER_DIR="/storage/Users/currentUser/.tmp/_id_wrapper"
mkdir -p "$_ID_WRAPPER_DIR"
cat > "$_ID_WRAPPER_DIR/id" << 'IDEOF'
#!/bin/sh
# Override for HongMeng: /bin/id is a stub that always says "bad uid".
# Provide sensible defaults so GNU configure scripts can run.
case "${1:-}" in
  -u)        echo "20020208" ;;
  -un|-nu)   echo "currentUser" ;;
  -g)        echo "1006" ;;
  -gn|-ng)   echo "file_manager" ;;
  -G)        echo "1006" ;;
  -nG|-Gn)   echo "file_manager" ;;
  "-r"|"-ru"|"-ur")   echo "20020208" ;;
  "-rg"|"-gr")        echo "1006" ;;
  "")        echo "uid=20020208(currentUser) gid=1006(file_manager) groups=1006(file_manager)" ;;
  *)         /bin/id "$@" 2>/dev/null || echo "0" ;;
esac
IDEOF
chmod +x "$_ID_WRAPPER_DIR/id"
export PATH="$_ID_WRAPPER_DIR:$PATH"


#PARALLEL=$(nproc 2>/dev/null)
PARALLEL=1
echo "[ohos-build] Building Rust code for $OHOS_TARGET with $PARALLEL parallel jobs ($($IS_RELEASE && echo release || echo debug)) ..."
cargo build $($IS_RELEASE && echo --release) --lib -p warp --target "$OHOS_TARGET" -j "$PARALLEL" "${CARGO_ARGS[@]}"

echo "[ohos-build] Resolving @ohos-rs/ability path ..."
ABILITY_PATH_FILE=$(find target -name ohos-ability-path.txt -print -quit 2>/dev/null || true)

if [ -n "$ABILITY_PATH_FILE" ] && [ -f "$ABILITY_PATH_FILE" ]; then
    ABILITY_PATH=$(head -1 "$ABILITY_PATH_FILE")
    if [ -n "$ABILITY_PATH" ] && [ -d "$ABILITY_PATH" ]; then
        HAP_DIR="app/src/platform/ohos/hap"
        DEPS_DIR="$HAP_DIR/deps"
        mkdir -p "$DEPS_DIR"
        ln -sfn "$ABILITY_PATH" "$DEPS_DIR/@ohos-rs/ability"
        echo "[ohos-build] Linked @ohos-rs/ability -> $ABILITY_PATH"
    else
        echo "[ohos-build] WARNING: ohos-ability-path.txt exists but path '$ABILITY_PATH' is invalid"
    fi
else
    echo "[ohos-build] WARNING: ohos-ability-path.txt not found in target/ — run cargo build first"
fi

# ── 拷贝 libwarp.so 到 HAP 工程目录 ─────────────────────────────────────────
HAP_LIBS_DIR="app/src/platform/ohos/hap/entry/libs/arm64-v8a"
mkdir -p "$HAP_LIBS_DIR"
cp "target/$OHOS_TARGET/$PROFILE_DIR/libwarp.so" "$HAP_LIBS_DIR/libwarp.so"
echo "[ohos-build]   libwarp.so staged at $HAP_LIBS_DIR/libwarp.so"

echo "[ohos-build] Building HAP ..."
bash "app/src/platform/ohos/hap/build-hap.sh"
echo "[ohos-build] HAP build complete"
