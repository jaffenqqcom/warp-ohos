#!/bin/sh
# 将各仓库当前代码相对于 git 干净代码的差异，提取为 patch
# 自动处理：已有文件修改 + 新增文件（未跟踪）
# 用法: cd <工作目录> && sh update-patches.sh
set -eu

BASE_DIR="$(pwd)"
DEPS_DIR="$BASE_DIR/depends"
PATCH_DIR="$(cd "$(dirname "$0")" && pwd)/patches"

# ── 函数：提取已有文件的修改 ──────────────────────────────
# 参数: <仓库路径> <输出 patch 文件> [额外的 git pathspec 排除规则...]
make_modified_patch() {
    dir="$1" patch="$2"
    shift 2

    cd "$dir" || { echo "  ✗ 无法进入 $dir"; return 1; }
    # 过滤掉隐藏文件/目录（路径以 . 开头或其任意父目录以 . 开头），
    # 同时对 warp 额外排除指定的路径模式
    files=$(git diff HEAD --numstat -- "$@" | awk '$1+$2 > 0 {print $3}' | grep -v '/\.\|^\.' || true)
    if [ -n "$files" ]; then
        git diff HEAD -- $files > "$patch"
        echo "      $(wc -l < "$patch") 行"
    else
        echo "      0 行（无修改）"
        # 确保空 patch 文件存在，避免下游 wc 报错
        : > "$patch"
    fi
}

# ── 函数：提取新增文件（未跟踪，排除隐藏目录） ────────────
# 参数: <仓库路径> <输出 patch 文件> [额外的 grep -v 排除规则]
make_newfile_patch() {
    dir="$1" patch="$2"
    extra_exclude="${3:-}"

    cd "$dir" || { echo "  ✗ 无法进入 $dir"; return 1; }

    # 构造过滤命令
    # 用 grep -v '^\.' || true 防止无匹配时 set -eu 终止脚本
    cmd="git ls-files --others --exclude-standard | grep -v '^\.' || true"
    if [ -n "$extra_exclude" ]; then
        cmd="$cmd | grep -v -E '$extra_exclude' || true"
    fi

    eval "$cmd" | while IFS= read -r f; do
        git diff --no-index --binary --src-prefix=a/ --dst-prefix=b/ /dev/null "$f" >> "$patch" || true
    done
}

# ════════════════════════════════════════════════════════════
#  warp
# ════════════════════════════════════════════════════════════
echo "=========================================="
echo " warp"
echo "=========================================="

echo "  → 00-tracked.patch (已有文件的修改)..."
make_modified_patch "$BASE_DIR/warp" "$PATCH_DIR/warp/00-tracked.patch" \
  .cargo/config.toml . \
  ':!.agents/*' ':!.claude/*' ':!.github/*' ':!.vscode/*' ':!.warp/*' \
  ':!.PSScriptAnalyzerSettings.psd1' ':!.clippy.toml' ':!.config/*' \
  ':!.dockerignore' ':!.gitattributes' ':!.gitignore' ':!.mcp.json' \
  ':!.rustfmt.toml' ':!.warpindexingignore'

echo "  → 01-new-files.patch (新增的文件)..."
make_newfile_patch "$BASE_DIR/warp" "$PATCH_DIR/warp/01-new-files.patch" \
  '\.cxx|build|oh_modules|\.hvigor|\.bitfun|target|\.bak$|libwarp\.so|local\.properties|code-linter|oh-package-lock|This PC|\.gitignore|deps/test'

# ════════════════════════════════════════════════════════════
#  winit
# ════════════════════════════════════════════════════════════
echo ""
echo "=========================================="
echo " winit"
echo "=========================================="

echo "  → 00-tracked.patch (已有文件的修改)..."
make_modified_patch "$DEPS_DIR/winit" "$PATCH_DIR/winit/00-tracked.patch"

echo "  → 01-new-files.patch (新增的文件)..."
make_newfile_patch "$DEPS_DIR/winit" "$PATCH_DIR/winit/01-new-files.patch" '\.bak$'

# ════════════════════════════════════════════════════════════
#  wgpu
# ════════════════════════════════════════════════════════════
echo ""
echo "=========================================="
echo " wgpu"
echo "=========================================="

echo "  → 00-tracked.patch (已有文件的修改)..."
make_modified_patch "$DEPS_DIR/wgpu" "$PATCH_DIR/wgpu/00-tracked.patch"

echo "  → 01-new-files.patch (新增的文件)..."
make_newfile_patch "$DEPS_DIR/wgpu" "$PATCH_DIR/wgpu/01-new-files.patch" '\.bak$'

# ════════════════════════════════════════════════════════════
#  openharmony-ability
# ════════════════════════════════════════════════════════════
echo ""
echo "=========================================="
echo " openharmony-ability"
echo "=========================================="

echo "  → 00-tracked.patch (已有文件的修改)..."
make_modified_patch "$DEPS_DIR/openharmony-ability" "$PATCH_DIR/openharmony-ability/00-tracked.patch"

echo "  → 01-new-files.patch (新增的文件)..."
make_newfile_patch "$DEPS_DIR/openharmony-ability" "$PATCH_DIR/openharmony-ability/01-new-files.patch"

# ════════════════════════════════════════════════════════════
echo ""
echo "=========================================="
echo " 新patches 制作完成！"
echo "=========================================="
ls -lh "$PATCH_DIR/warp/"*.patch "$PATCH_DIR/winit/"*.patch "$PATCH_DIR/wgpu/"*.patch "$PATCH_DIR/openharmony-ability/"*.patch "$PATCH_DIR/"*.patch 2>/dev/null
echo "提交指令：git add fetch-full-code.sh  update-patches.sh patches/ && git commit -m "更新脚本与补丁目录" && git push"
