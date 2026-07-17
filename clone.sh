#!/bin/bash
# 下载 warp + winit + openharmony-ability + 三个修补过的外部 crate
# 自动打所有 patch，配置 cargo 路径覆盖
# 用法: cd <目标目录> && sh clone.sh
set -euo pipefail

BASE_DIR="$(pwd)"
PATCH_DIR="$(cd "$(dirname "$0")" && pwd)/patches"


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

# 三个 crate 统一从 crates.io 下载 tarball（比 git clone 更可靠）
dl_crate() {
    local name="$1" ver="$2"
    local dir="$BASE_DIR/$name"
    if [ -d "$dir" ]; then
        echo "  → $name 目录已存在，跳过"
    else
        echo "  → 下载 $name v$ver ..."
        local tmpf="/tmp/${name}_dl.tar.gz"
        curl -sL "https://crates.io/api/v1/crates/$name/$ver/download" -o "$tmpf"
        tar xzf "$tmpf" -C /tmp
        mv "/tmp/$name-$ver" "$dir"
        rm -f "$tmpf"
    fi
    cd "$dir"
    git apply "$PATCH_DIR/$name-$ver.patch"
    echo "  ✓ $name v$ver 已修补"
}

dl_crate "nix" "0.26.4"
dl_crate "interprocess" "1.2.1"
dl_crate "gettext-sys" "0.21.3"

cd "$BASE_DIR"
echo ""
echo "=========================================="
echo " 全部完成！"
echo "=========================================="
echo ""
echo "下一步: cd $BASE_DIR/warp && ./script/ohos/build-vm.sh"
