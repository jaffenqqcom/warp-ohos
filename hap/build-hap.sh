#!/usr/bin/env bash
# *****严禁自行修改*****
# *****严禁自行修改*****
# *****严禁自行修改*****
# *****严禁自行修改*****
# *****严禁自行修改*****
# ============================================================================
# build-hap.sh — hvigor HAP 打包（hvigor 自动管理 cmake/ninja 编译）
# ============================================================================
# 用法: ./build-hap.sh [-m mode] [-p product]
#   默认模式: debug
#   默认 product: default
#
# 前置条件：先运行 cargo build --target aarch64-unknown-linux-ohos
#           确保 libwarp.so 已编译到目标目录。
#
# 环境变量（可选覆盖自动检测）:
#   NODE_CMD, HVIGOR_DIR, DEVECO_SDK_HOME
# ============================================================================
set -euo pipefail

# ==================== 修复 SDK 编译器包装器 ==================================
# VM（build-vm.sh）和本机共用同一份 SDK，VM 会把 clang/clang++ 重写为指向
# /usr/local/bin/ohos-clang（仅 VM 环境存在）。每次构建前修复。
OHOS_CLANG_DIR="/storage/Users/currentUser/workspace/openharmony/hvigor/SDK/openharmony/native/llvm/bin"
if [ -f "$OHOS_CLANG_DIR/clang++" ] && grep -q "/usr/local/bin/ohos-clang" "$OHOS_CLANG_DIR/clang++" 2>/dev/null; then
  cat > "$OHOS_CLANG_DIR/clang" << 'CLANGEOF'
#!/bin/sh
SOURCE=$(dirname -- "$( readlink -f -- "$0"; )")
exec "$SOURCE/clang-15" \
  -target aarch64-linux-ohos \
  --sysroot="$SOURCE/../../sysroot" \
  -D__MUSL__ \
  "$@"
CLANGEOF
  cat > "$OHOS_CLANG_DIR/clang++" << 'CLANGXXEOF'
#!/bin/sh
SOURCE=$(dirname -- "$( readlink -f -- "$0"; )")
exec "$SOURCE/clang-15" \
  -target aarch64-linux-ohos \
  --sysroot="$SOURCE/../../sysroot" \
  -D__MUSL__ \
  "$@"
CLANGXXEOF
  chmod +x "$OHOS_CLANG_DIR/clang" "$OHOS_CLANG_DIR/clang++"
  echo "[build-hap.sh] fixed SDK clang/clang++ wrappers (were pointing to /usr/local/bin/ohos-clang)"
fi

# ===========================================================================
# =========================== 从环境变量读取配置 ==============================
NODE_CMD="${NODE_CMD:-}"
HVIGOR_DIR="${HVIGOR_DIR:-}"
DEVECO_SDK_HOME="${DEVECO_SDK_HOME:-}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR"
BUILD_MODE="debug"
PRODUCT="default"

# ===========================================================================
# =========================== 自动检测路径 ====================================
if [ -z "$NODE_CMD" ] || [ -z "$HVIGOR_DIR" ] || [ -z "$DEVECO_SDK_HOME" ]; then
  HARMONYBREW_NODE="/storage/Users/currentUser/.harmonybrew/bin/node"
  PATH_NODE="$(command -v node 2>/dev/null || true)"
  HVIGOR_CANDIDATES=(
    "/storage/Users/currentUser/workspace/openharmony/hvigor"
    "$HOME/workspace/openharmony/hvigor"
    "/opt/harmony/hvigor"
    "/usr/lib/hvigor"
  )
  HARMONYBREW_SDK="/storage/Users/currentUser/workspace/openharmony/hvigor/SDK"
fi

if [ -z "$NODE_CMD" ]; then
  if [ -x "$HARMONYBREW_NODE" ]; then
    NODE_CMD="$HARMONYBREW_NODE"
  elif [ -n "$PATH_NODE" ]; then
    NODE_CMD="$PATH_NODE"
  fi
fi

if [ -z "$HVIGOR_DIR" ]; then
  for candidate in "${HVIGOR_CANDIDATES[@]}"; do
    if [ -f "$candidate/bin/hvigorw.js" ]; then
      HVIGOR_DIR="$candidate"
      break
    fi
  done
fi

# Always re-detect: override any pre-set DEVECO_SDK_HOME with the workspace SDK
# path, so the build uses the SDK that has both openharmony/ and hms/ components.
# The brew cellar SDK (ohos-sdk/26.0.0.18_1) lacks hms/ and causes hvigor
# to reject the value.
echo "[build-hap.sh] DEVECO_SDK_HOME before auto-detect: ${DEVECO_SDK_HOME:-<unset>}"
if [ -d "$HARMONYBREW_SDK/openharmony" ]; then
  export DEVECO_SDK_HOME="$HARMONYBREW_SDK"
  echo "[build-hap.sh] DEVECO_SDK_HOME after auto-detect (HARMONYBREW_SDK): $HARMONYBREW_SDK"
elif [ -n "$HVIGOR_DIR" ] && [ -d "$HVIGOR_DIR/SDK/openharmony" ]; then
  export DEVECO_SDK_HOME="$HVIGOR_DIR/SDK"
  echo "[build-hap.sh] DEVECO_SDK_HOME after auto-detect (HVIGOR_DIR): $HVIGOR_DIR/SDK"
else
  echo "[build-hap.sh] auto-detect failed, keeping existing value: ${DEVECO_SDK_HOME:-<unset>}"
fi

# ===========================================================================
# =========================== 参数解析 ======================================
while [ $# -gt 0 ]; do
  case "$1" in
    -m|--mode)     BUILD_MODE="$2"; shift 2 ;;
    -p|--product)  PRODUCT="$2"; shift 2 ;;
    release|debug) BUILD_MODE="$1"; shift ;;
    *) echo "Unknown: $1"; exit 1 ;;
  esac
done

# ===========================================================================
# =========================== 校验 ==========================================
HVIGORW_JS="${HVIGOR_DIR}/bin/hvigorw.js"
errors=0
[ -z "$NODE_CMD" ] && echo "ERROR: NODE_CMD not set" && errors=1
[ -z "$HVIGOR_DIR" ] && echo "ERROR: HVIGOR_DIR not set" && errors=1
[ -z "$DEVECO_SDK_HOME" ] && echo "ERROR: DEVECO_SDK_HOME not set" && errors=1
[ ! -f "$NODE_CMD" ] && echo "ERROR: Node not found: $NODE_CMD" && errors=1
[ ! -f "$HVIGORW_JS" ] && echo "ERROR: hvigorw.js not found: $HVIGORW_JS" && errors=1
[ ! -d "$DEVECO_SDK_HOME" ] && echo "ERROR: SDK not found: $DEVECO_SDK_HOME" && errors=1
[ $errors -ne 0 ] && exit 1

# ===========================================================================
cd "$PROJECT_DIR"

# =========================== 检查 libwarp.so ==================================
LIBWARP_PATH="$PROJECT_DIR/entry/libs/arm64-v8a/libwarp.so"
if [ ! -f "$LIBWARP_PATH" ]; then
  echo "ERROR: libwarp.so not found at $LIBWARP_PATH" >&2
  exit 1
fi

# ======================= 清理 stale cmake/ninja 状态 ============================
# cmake 重配置会清理 CMakeCache.txt 和输出目录，但 .ninja_log 残留导致 ninja
# 误认为输出已最新、跳过链接步骤。每次构建前强制清理，确保完整重编。
CXX_DIR="$PROJECT_DIR/entry/.cxx/default/default/debug/arm64-v8a"
rm -f "$CXX_DIR/.ninja_log" "$CXX_DIR/.ninja_deps" 2>/dev/null || true

# ===========================================================================
# ===========================================================================
echo "=== HAP Build ==="
echo "Project : $PROJECT_DIR"
echo "Node    : $("$NODE_CMD" --version)"
echo "SDK     : $DEVECO_SDK_HOME"
echo "Mode    : $BUILD_MODE"
echo "Product : $PRODUCT"
echo ""

# =========================== hvigor HAP 打包 =================================
# cmake+ninja 编译 libentry.so + hvigor 打包 HAP 合一完成。
# hvigor 自动管理 cmake 配置和 ninja 构建流程。
echo "--- hvigor assembleHap ---"
"$NODE_CMD" "$HVIGORW_JS" \
  --mode module \
  -p product="$PRODUCT" \
  -p buildMode="$BUILD_MODE" \
  assembleHap \
  --info \
  --analyze=normal \
  --parallel \
  --incremental \
  --no-daemon

echo ""
echo "=== HAP BUILD SUCCESSFUL ==="

# ==================== 清理 cmake staging ====================================
# CMAKE_LIBRARY_OUTPUT_DIRECTORY 被设为项目外的 .cmake_staging 目录以绕开
# hvigor syncLibOutputs 的源==目标 bug。构建完成后清理即可。
CMAKE_STAGING_DIR="$PROJECT_DIR/entry/.cmake_staging"
if [ -d "$CMAKE_STAGING_DIR" ]; then
  rm -rf "$CMAKE_STAGING_DIR"
  echo "[build-hap.sh] cleaned up .cmake_staging directory"
fi

# ===========================================================================
