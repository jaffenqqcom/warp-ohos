#!/bin/bash
# 将各仓库当前代码相对于 git 干净代码的差异，提取为 patch 更新到 patches/ 目录。
#
# 适用场景：修改代码（warp/winit/openharmony-ability/三个 crate）后，
# 运行此脚本自动更新对应的 patch 文件。
#
# 用法: cd <工作目录> && bash update-patches.sh
set -euo pipefail

BASE_DIR="$(pwd)"
PATCH_DIR="$(cd "$(dirname "$0")" && pwd)/patches"

echo "=========================================="
echo " 更新 warp patches"
echo "=========================================="
cd "$BASE_DIR/warp"

echo "  → 更新 00-tracked.patch (已有文件的修改)..."
git diff HEAD > "$PATCH_DIR/warp/00-tracked.patch"
echo "      $(wc -l < "$PATCH_DIR/warp/00-tracked.patch") 行"

echo "  → 更新 01-new-files.patch (新增文件的修改)..."
# 找出所有未被 git 跟踪的源文件（排除构建产物）
NEW_FILES=$(git ls-files --others --exclude-standard | \
  grep -v -E '\.cxx|build|oh_modules|\.hvigor|\.bitfun|target|deps/@ohos-rs|\.bak$|libwarp\.so|local\.properties|code-linter|oh-package-lock|This PC|hvigorfile|oh-package\.json5$|\.gitignore|hvigor-config|deps/test' || true)
if [ -n "$NEW_FILES" ]; then
    echo "$NEW_FILES" | tr '\n' '\0' | xargs -0 git add
    git diff --cached HEAD --binary > "$PATCH_DIR/warp/01-new-files.patch"
    git reset HEAD -- . > /dev/null 2>&1
    echo "      $(wc -l < "$PATCH_DIR/warp/01-new-files.patch") 行"
else
    echo "      (无新增文件)"
fi

echo ""
echo "=========================================="
echo " 更新 winit patch"
echo "=========================================="
cd "$BASE_DIR/winit"
git diff HEAD > "$PATCH_DIR/winit/full.patch"
echo "  $(wc -l < "$PATCH_DIR/winit/full.patch") 行"

echo ""
echo "=========================================="
echo " 更新 openharmony-ability patch"
echo "=========================================="
cd "$BASE_DIR/openharmony-ability"
git diff HEAD > "$PATCH_DIR/openharmony-ability/full.patch"
echo "  $(wc -l < "$PATCH_DIR/openharmony-ability/full.patch") 行"


echo ""
echo "=========================================="
echo " 新patches 制作完成！"
echo "=========================================="
ls -lh "$PATCH_DIR/warp/"*.patch "$PATCH_DIR/winit/"*.patch "$PATCH_DIR/openharmony-ability/"*.patch "$PATCH_DIR/"*.patch 2>/dev/null
echo ""
echo "提示: 提交到 warp-ohos 仓库保存：git add clone.sh README.md update-patches.sh patches/ && git commit -m "更新脚本与补丁目录" && git push"
