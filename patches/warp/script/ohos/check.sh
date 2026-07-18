#!/bin/sh
set -e

cd "$(git rev-parse --show-toplevel 2>/dev/null || echo .)"

OHOS_NDK_ROOT="/storage/Users/currentUser/.harmonybrew/Cellar/ohos-sdk/26.0.0.18_1"
export OHOS_NDK_HOME="${OHOS_NDK_ROOT}/native"
export CC_aarch64_unknown_linux_ohos="ohos-clang"
export AR_aarch64_unknown_linux_ohos="llvm-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="ohos-clang"
export CMAKE_TOOLCHAIN_FILE_aarch64_unknown_linux_ohos="${OHOS_NDK_ROOT}/native/build/cmake/ohos.toolchain.cmake"

NPROC=$(nproc 2>/dev/null || echo 4)
exec cargo check --target "aarch64-unknown-linux-ohos" -j "$NPROC" "$@"
