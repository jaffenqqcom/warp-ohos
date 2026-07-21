#!/bin/sh
# 下载 warp + winit + openharmony-ability + 三个修补过的外部 crate
# 自动打所有 patch
# 用法: cd <目标目录> && sh clone.sh
set -eu

BASE_DIR="$(pwd)"
DEPS_DIR="$BASE_DIR/depends"
PATCH_DIR="$(cd "$(dirname "$0")" && pwd)/patches"

# 将 .complete 加入 .gitignore（如已存在且包含则跳过）
add_complete_to_gitignore() {
    dir="$1"
    gi="$dir/.gitignore"
    if [ -f "$gi" ]; then
        if ! grep -qxF '.complete' "$gi" 2>/dev/null; then
            echo '.complete' >> "$gi"
        fi
    else
        echo '.complete' > "$gi"
    fi
}

# 对所有剩余参数对应的 patch 文件执行 git apply
apply_patches() {
    label="$1"
    shift
    for patch in "$@"; do
        patch_file="$PATCH_DIR/$patch"
        if [ ! -s "$patch_file" ]; then
            echo "  → patch $patch（空文件，跳过）"
            continue
        fi

        # 自动检测 -p 剥离层数
        first_line=$(grep '^--- ' "$patch_file" | head -1 | sed 's/^--- //;s/[[:space:]].*//')
        case "$first_line" in
            a/*) p_strip=1 ;;              # git diff 格式: "a/src/lib.rs"
            /*)                             # diff -ruN 绝对路径: 数所有路径分量
                rest="${first_line#/}"
                p_strip=$(echo "$rest" | tr -cd '/' | wc -c)
                ;;
            *) p_strip=1 ;;                 # 其它情况默认 -p1
        esac

        echo "  → 应用 patch: $patch（-p$p_strip）"
        if ! git apply --ignore-whitespace --whitespace=nowarn -p"$p_strip" "$patch_file"; then
            echo "  ✗ [$label] patch $patch 应用失败"
            return 1
        fi
        echo "  ✓ patch $patch 应用成功"
    done
    # 取消暂存（patch 中新文件可能被 git apply 自动加入 index）
    git reset HEAD -- . > /dev/null 2>&1 || true
}

# ── git 仓库任务（从 GitHub 下载源码压缩包，替代 git clone） ──
# 下载 tarball → 解压 → git init（用于 patch 管理） → 打 patch
# 支持 tag 和 commit SHA 两种引用
do_git_archive() {
    dir="$1" base_url="$2" ref="$3" label="$4"
    shift 4

    echo "──────────────────────────────────────────"
    echo "  [$label] 开始"
    echo "──────────────────────────────────────────"

    if [ -f "$dir/.complete" ]; then
        echo "  → [$label] 已完成（.complete 存在），跳过"
        return 0
    fi

    if [ -d "$dir" ]; then
        echo "  → [$label] 目录已存在，删除重新下载"
        rm -rf "$dir"
    fi

    # 构造 tarball URL：tag 或 commit
    case "$ref" in
        v*) archive_url="$base_url/archive/refs/tags/$ref.tar.gz" ;;
        *)  archive_url="$base_url/archive/$ref.tar.gz" ;;
    esac

    echo "  → 下载 $label 源码压缩包..."
    echo "     URL: $archive_url"
    tmpf="$BASE_DIR/.${label}_dl.tmp"
    if ! curl -SL "$archive_url" -o "$tmpf"; then
        echo "  ✗ [$label] 下载失败（curl 错误）"
        rm -f "$tmpf"
        return 1
    fi
    echo "  ✓ 下载完成"

    # 解压到临时目录
    echo "  → 解压..."
    extract_dir="$BASE_DIR/.${label}_extract"
    rm -rf "$extract_dir"
    mkdir -p "$extract_dir"
    if ! tar xzf "$tmpf" -C "$extract_dir"; then
        echo "  ✗ [$label] 解压失败"
        rm -f "$tmpf"
        rm -rf "$extract_dir"
        return 1
    fi
    rm -f "$tmpf"

    # GitHub tarball 顶层目录名为 {repo}-{ref}，重命名为目标目录
    extracted_name=$(ls "$extract_dir" | head -1)
    mkdir -p "$(dirname "$dir")"
    mv "$extract_dir/$extracted_name" "$dir"
    rm -rf "$extract_dir"
    echo "  ✓ 解压完成 → $dir"

    cd "$dir" || { echo "  ✗ [$label] 无法进入目录"; return 1; }

    # tarball 不含 .git，初始化 git 以便 git apply 打 patch
    echo "  → 初始化 git 仓库..."
    git init > /dev/null 2>&1
    git add -A > /dev/null 2>&1
    git commit -m "base: $label $ref" --allow-empty > /dev/null 2>&1
    echo "  ✓ git 初始化完成（已创建初始 commit）"

    apply_patches "$label" "$@"

    add_complete_to_gitignore "$dir"
    touch "$dir/.complete"
    echo "  ✓ [$label] 完成"
    return 0
}

# ── git 仓库任务（有 tag 的仓库，用 --depth 1 加速） ─────
do_git_repo_tag() {
    dir="$1" url="$2" tag="$3" label="$4"
    shift 4

    echo "──────────────────────────────────────────"
    echo "  [$label] 开始"
    echo "──────────────────────────────────────────"

    if [ -f "$dir/.complete" ]; then
        echo "  → [$label] 已完成（.complete 存在），跳过"
        return 0
    fi

    if [ -d "$dir" ]; then
        echo "  → [$label] 目录已存在，续传中..."
    else
        echo "  → 浅克隆 $url（tag: $tag）"
        git clone --branch "$tag" --depth 1 --progress "$url" "$dir"
    fi

    cd "$dir" || { echo "  ✗ [$label] 无法进入目录"; return 1; }

    if ! git describe --exact-match --tags HEAD 2>/dev/null | grep -q "$tag"; then
        echo "  → 当前不是目标 tag，续传..."
        git fetch origin tag "$tag" --depth 1 --progress
        git checkout --force "$tag"
    fi
    echo "  → 已在 tag $tag"

    apply_patches "$label" "$@"

    add_complete_to_gitignore "$dir"
    touch "$dir/.complete"
    echo "  ✓ [$label] 完成"
    return 0
}

# ── git 仓库任务（无 tag 的仓库，完整克隆） ──────────────
do_git_repo() {
    dir="$1" url="$2" commit="$3" label="$4"
    shift 4

    echo "──────────────────────────────────────────"
    echo "  [$label] 开始"
    echo "──────────────────────────────────────────"

    if [ -f "$dir/.complete" ]; then
        echo "  → [$label] 已完成（.complete 存在），跳过"
        return 0
    fi

    if [ ! -d "$dir" ]; then
        mkdir -p "$dir"
    fi
    cd "$dir" || return 1
    # 注意：必须检测本地 .git 目录，不能 git rev-parse --git-dir（会向上查到父 repo）
    if [ ! -d ".git" ]; then
        # 没有 .git，重新初始化
        echo "  → 初始化并浅克隆 $url（commit: $(echo "$commit" | cut -c1-8)）"
        git init
        git remote add origin "$url"
        git fetch --depth 1 --progress origin "$commit"
    elif [ ! -f "$dir/.complete" ]; then
        # 有 .git 但不完整，续传
        echo "  → [$label] 续传中..."
        git fetch --depth 1 --progress origin "$commit"
    fi

    if ! git checkout --force "$commit"; then
        echo "  ✗ [$label] checkout 失败"
        return 1
    fi
    echo "  → checkout $commit 成功"

    apply_patches "$label" "$@"

    add_complete_to_gitignore "$dir"
    touch "$dir/.complete"
    echo "  ✓ [$label] 完成"
    return 0
}

# ── crate 下载任务 ──────────────────────────────────────
do_crate() {
    name="$1" ver="$2"
    base_dir="${3:-$BASE_DIR}"
    shift 3

    echo "──────────────────────────────────────────"
    echo "  [$name] 开始"
    echo "──────────────────────────────────────────"

    dir="$base_dir/$name"
    if [ -f "$dir/.complete" ]; then
        echo "  → [$name] 已完成（.complete 存在），跳过"
        return 0
    fi

    if [ -d "$dir" ]; then
        echo "  → [$name] 目录不完整，删除重下"
        rm -rf "$dir"
    fi

    url="https://static.crates.io/crates/$name/$name-$ver.crate"
    echo "  → 下载 $name v$ver ..."
    echo "     URL: $url"
    tmpf="$base_dir/.${name}_dl.tmp"
    if ! curl -SL "$url" -o "$tmpf"; then
        echo "  ✗ [$name] 下载失败（curl 错误）"
        rm -f "$tmpf"
        return 1
    fi
    echo "  ✓ 下载完成"

    if ! tar xzf "$tmpf" -C "$base_dir"; then
        echo "  ✗ [$name] 解压失败"
        rm -f "$tmpf"
        return 1
    fi
    mv "$base_dir/$name-$ver" "$dir"
    rm -f "$tmpf"
    echo "  ✓ 解压完成"

    cd "$dir" || { echo "  ✗ [$name] 无法进入目录"; return 1; }

    # 初始化 git 仓库（后续 git apply 需要）
    echo "  → 初始化 git 仓库..."
    git init > /dev/null 2>&1
    git add -A > /dev/null 2>&1
    git commit -m "base: $name $ver" --allow-empty > /dev/null 2>&1
    echo "  ✓ git 初始化完成"

    apply_patches "$name" "$@"

    add_complete_to_gitignore "$dir"
    touch "$dir/.complete"
    echo "  ✓ [$name] 完成"
    return 0
}

# ── 通用 tarball 下载任务 ─────────────────────────────────
do_archive() {
    filename="$1" url="$2"
    dest="$DEPS_DIR/$filename"

    echo "──────────────────────────────────────────"
    echo "  [$filename] 开始"
    echo "──────────────────────────────────────────"

    if [ -f "$dest" ]; then
        echo "  → [$filename] 已存在，跳过"
        return 0
    fi

    tmpf="$DEPS_DIR/.${filename}_dl.tmp"
    echo "  → 下载 $filename ..."
    echo "     URL: $url"
    if ! curl -SL "$url" -o "$tmpf"; then
        echo "  ✗ [$filename] 下载失败（curl 错误）"
        rm -f "$tmpf"
        return 1
    fi
    echo "  ✓ 下载完成"

    mv "$tmpf" "$dest"
    echo "  ✓ [$filename] 已保存到 $dest"
    return 0
}

# ════════════════════════════════════════════════════════
#  执行所有任务
# ════════════════════════════════════════════════════════

RESULTS=""
run() {
    label="$1"
    shift
    if "$@" 2>&1; then
        RESULTS="$RESULTS  ✓ $label\n"
    else
        RESULTS="$RESULTS  ✗ $label\n"
    fi
}

echo ""
echo "=========================================="
echo " 开始下载所有仓库和 crate"
echo "=========================================="
echo ""

run "warp"                do_git_archive "$BASE_DIR/warp"                "https://github.com/warpdotdev/warp"                "v0.2026.06.03.09.49.stable_00"             "warp"                "warp/00-tracked.patch" "warp/01-new-files.patch"
run "winit"               do_git_archive "$DEPS_DIR/winit"               "https://github.com/warpdotdev/winit"               "a4e0ecb5f9626ccac9445a73dc28354b52423abc" "winit"               "winit/00-tracked.patch" "winit/01-new-files.patch"
run "wgpu"                do_git_archive "$DEPS_DIR/wgpu"                "https://github.com/zed-industries/wgpu"             "357a0c56e0070480ad9daea5d2eaa83150b79e88" "wgpu"                "wgpu/00-tracked.patch" "wgpu/01-new-files.patch"
run "openharmony-ability" do_git_archive "$DEPS_DIR/openharmony-ability" "https://github.com/harmony-contrib/openharmony-ability" "6c52bb44164ea2d6d7f573c090a75142f0dbd2ef" "openharmony-ability" "openharmony-ability/00-tracked.patch" "openharmony-ability/01-new-files.patch"
run "nix"                 do_crate "nix" "0.26.4" "$DEPS_DIR" "nix/00-tracked.patch" "nix/01-new-files.patch"
run "interprocess"        do_crate "interprocess" "1.2.1" "$DEPS_DIR" "interprocess/00-tracked.patch" "interprocess/01-new-files.patch"
run "gettext-sys"         do_crate "gettext-sys" "0.21.3" "$DEPS_DIR" "gettext-sys/00-tracked.patch" "gettext-sys/01-new-files.patch"
run "mbedtls"             do_archive "third_party_mbedtls-OpenHarmony-v3.2-Release.tar.gz" "https://github.com/openharmony/third_party_mbedtls/archive/refs/tags/OpenHarmony-v3.2-Release.tar.gz"
run "libssh2"             do_archive "libssh2-1.11.0.tar.gz" "https://libssh2.org/download/libssh2-1.11.0.tar.gz"
run "ohos-ime-binding"    do_crate "ohos-ime-binding" "0.2.1" "$DEPS_DIR" "ohos-ime-binding/00-tracked.patch" "ohos-ime-binding/01-new-files.patch"

echo ""
echo "=========================================="
echo " 结果汇总"
echo "=========================================="
printf "$RESULTS"
echo "=========================================="

cd "$BASE_DIR"
echo "下一步: cd $BASE_DIR/warp && ./script/ohos/build-vm.sh"
