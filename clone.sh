#!/bin/bash
# 下载 warp + winit + openharmony-ability + 三个修补过的外部 crate
# 自动打所有 patch，配置 cargo 路径覆盖
# 用法: cd <目标目录> && bash clone.sh
set -euo pipefail

BASE_DIR="$(pwd)"
PATCH_DIR="$(cd "$(dirname "$0")" && pwd)/patches"

download_crate() {
    local name="$1" version="$2"
    local dir="$BASE_DIR/$name"
    if [ -d "$dir" ]; then
        echo "  → $name 目录已存在，跳过下载"
    else
        echo "  → 下载 $name v$version 从 crates.io ..."
        local tmpdir="/tmp/crate_dl_$$"
        mkdir -p "$tmpdir"
        curl -sL "https://crates.io/api/v1/crates/$name/$version/download" -o "$tmpdir/crate.tar.gz"
        tar xzf "$tmpdir/crate.tar.gz" -C "$tmpdir"
        mv "$tmpdir/$name-$version" "$dir"
        rm -rf "$tmpdir"
    fi
}

echo "=========================================="
echo " 步骤 1/5: 克隆 warp (主仓库)"
echo "=========================================="
if [ -d "$BASE_DIR/warp" ]; then
    echo "  → warp 目录已存在，跳过克隆"
else
    git clone https://github.com/warpdotdev/warp.git "$BASE_DIR/warp"
fi
cd "$BASE_DIR/warp"
git checkout 51ba262fdc6ce2b1b3ba134a6a2da9128257e998
echo "  → 应用 patch: 00-tracked.patch"
git apply "$PATCH_DIR/warp/00-tracked.patch"
echo "  → 应用 patch: 01-new-files.patch"
git apply "$PATCH_DIR/warp/01-new-files.patch"
echo "  ✓ warp 完成"

echo ""
echo "=========================================="
echo " 步骤 2/5: 克隆 winit (OHOS fork)"
echo "=========================================="
if [ -d "$BASE_DIR/winit" ]; then
    echo "  → winit 目录已存在，跳过克隆"
else
    git clone https://github.com/warpdotdev/winit.git "$BASE_DIR/winit"
fi
cd "$BASE_DIR/winit"
git checkout a4e0ecb5f9626ccac9445a73dc28354b52423abc
echo "  → 应用 patch: winit/full.patch"
git apply "$PATCH_DIR/winit/full.patch"
echo "  ✓ winit 完成"

echo ""
echo "=========================================="
echo " 步骤 3/5: 克隆 openharmony-ability"
echo "=========================================="
if [ -d "$BASE_DIR/openharmony-ability" ]; then
    echo "  → openharmony-ability 目录已存在，跳过克隆"
else
    git clone https://github.com/harmony-contrib/openharmony-ability.git "$BASE_DIR/openharmony-ability"
fi
cd "$BASE_DIR/openharmony-ability"
git checkout 6c52bb44164ea2d6d7f573c090a75142f0dbd2ef
echo "  → 应用 patch: openharmony-ability/full.patch"
git apply "$PATCH_DIR/openharmony-ability/full.patch"
echo "  ✓ openharmony-ability 完成"

echo ""
echo "=========================================="
echo " 步骤 4/5: 下载修补过的外部 crate"
echo "=========================================="
download_crate "nix" "0.26.4"
cd "$BASE_DIR/nix"
git apply "$PATCH_DIR/nix-0.26.4.patch"
echo "  ✓ nix v0.26.4 已修补"

download_crate "interprocess" "1.2.1"
cd "$BASE_DIR/interprocess"
git apply "$PATCH_DIR/interprocess-1.2.1.patch"
echo "  ✓ interprocess v1.2.1 已修补"

download_crate "gettext-sys" "0.21.3"
cd "$BASE_DIR/gettext-sys"
git apply "$PATCH_DIR/gettext-sys-0.21.3.patch"
echo "  ✓ gettext-sys v0.21.3 已修补"

echo ""
echo "=========================================="
echo " 步骤 5/5: 配置 cargo 路径覆盖"
echo "=========================================="
CARGO_CONFIG="$BASE_DIR/warp/.cargo/config.toml"
PATCH_LINE='nix = { path = "../nix" }'
if [ -f "$CARGO_CONFIG" ] && grep -q "$PATCH_LINE" "$CARGO_CONFIG" 2>/dev/null; then
    echo "  → cargo 配置中已有 patch 覆盖，跳过"
else
    mkdir -p "$BASE_DIR/warp/.cargo"
    cat >> "$CARGO_CONFIG" << 'CONFIGEOF'

# === OHOS patched crates (自动添加) ===
[patch.crates-io]
nix = { path = "../nix" }
interprocess = { path = "../interprocess" }
gettext-sys = { path = "../gettext-sys" }
CONFIGEOF
    echo "  ✓ 已添加 cargo patch 配置"
fi

cd "$BASE_DIR"
echo ""
echo "=========================================="
echo " 全部完成！"
echo "=========================================="
echo "  $BASE_DIR/warp                (commit 51ba262)"
echo "  $BASE_DIR/winit               (commit a4e0ecb5)"
echo "  $BASE_DIR/openharmony-ability  (commit 6c52bb4)"
echo "  $BASE_DIR/nix                 (v0.26.4, patched)"
echo "  $BASE_DIR/interprocess         (v1.2.1, patched)"
echo "  $BASE_DIR/gettext-sys          (v0.21.3, patched)"
echo ""
echo "下一步: cd $BASE_DIR/warp && ./script/ohos/build-vm.sh"
