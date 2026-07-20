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

# 计算 patch -pN 的正确层级
# 从 patch 的第一条 --- 路径中，数出到 crate 根需要去掉的层数
detect_p_strip() {
    patch_file="$1"
    line=$(grep '^--- ' "$patch_file" | head -1 | sed 's/^--- //;s/[[:space:]].*//')
    # 去掉开头的 /，然后数还剩几个 /
    rest="${line#/}"
    slash_count=$(echo "$rest" | tr -cd '/' | wc -c)
    # 去掉的层数 = 斜杠数（把 crate-version/ 及其前面的路径都去掉）
    # 例如: home/user/.../nix-0.26.4/src/fcntl.rs
    #       去掉 home=1 user=2 ...=7 nix-0.26.4=8 → src/fcntl.rs
    echo "$slash_count"
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

    for patch in "$@"; do
        if [ ! -s "$PATCH_DIR/$patch" ]; then
            echo "  → patch $patch（空文件，跳过）"
            continue
        fi
        echo "  → 应用 patch: $patch"
        if ! git apply --ignore-whitespace --whitespace=nowarn "$PATCH_DIR/$patch"; then
            echo "  ✗ [$label] patch $patch 应用失败"
            return 1
        fi
        echo "  ✓ patch $patch 应用成功"
    done

    # git apply --3way 处理新文件时会自动加入 index，这里取消暂存
    # 新文件应为 untracked 状态，只由人手动 git add
    git reset HEAD -- . > /dev/null 2>&1 || true

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

    for patch in "$@"; do
        if [ ! -s "$PATCH_DIR/$patch" ]; then
            echo "  → patch $patch（空文件，跳过）"
            continue
        fi
        echo "  → 应用 patch: $patch"
        if ! git apply --ignore-whitespace --whitespace=nowarn "$PATCH_DIR/$patch"; then
            echo "  ✗ [$label] patch $patch 应用失败"
            return 1
        fi
        echo "  ✓ patch $patch 应用成功"
    done

    # git apply --3way 处理新文件时会自动加入 index，这里取消暂存
    git reset HEAD -- . > /dev/null 2>&1 || true

    add_complete_to_gitignore "$dir"
    touch "$dir/.complete"
    echo "  ✓ [$label] 完成"
    return 0
}

# ── crate 下载任务 ──────────────────────────────────────
do_crate() {
    name="$1" ver="$2"
    base_dir="${3:-$BASE_DIR}"

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
    patch_file="$PATCH_DIR/$name-$ver.patch"
    if [ -f "$patch_file" ] && [ -s "$patch_file" ]; then
        echo "  → 应用 patch: $name-$ver.patch"
        # 基于 $name-$ver 在 patch 路径中的位置计算 strip 层数
        # cargo registry 的 hash 段长度可变，不能简单数斜杠
        p_strip=1
        first_line=$(grep '^--- ' "$patch_file" | head -1 | sed 's/^--- //;s/[[:space:]].*//')
        rest="${first_line#/}"
        prefix="${rest%%/$name-$ver/*}"
        if [ "$prefix" != "$rest" ]; then
            slash_count=$(echo "$prefix" | tr -cd '/' | wc -c)
            p_strip=$((slash_count + 1))
        else
            p_strip=$(detect_p_strip "$patch_file")
        fi
        # 使用 git apply 而非 patch（兼容 toybox/GNU 的 -p 差异）
        # --ignore-whitespace 避免 CRLF/LF 换行符不一致导致 patch 失败
        if ! git apply --ignore-whitespace -p"$p_strip" "$patch_file"; then
            echo "  ✗ [$name] patch 应用失败"
            return 1
        fi
        echo "  ✓ patch 应用成功"
    else
        echo "  → 无 patch 文件"
    fi

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

run "warp"                do_git_repo_tag "$BASE_DIR/warp"                "https://github.com/warpdotdev/warp.git"                "v0.2026.06.03.09.49.stable_00"             "warp"                "warp/00-tracked.patch" "warp/01-new-files.patch"
run "winit"               do_git_repo "$DEPS_DIR/winit"    "https://github.com/warpdotdev/winit.git"               "a4e0ecb5f9626ccac9445a73dc28354b52423abc" "winit"               "winit/00-tracked.patch" "winit/01-new-files.patch"
run "wgpu"                do_git_repo_tag "$DEPS_DIR/wgpu"  "https://github.com/gfx-rs/wgpu.git"                    "v29.0.1"            "wgpu"                "wgpu/00-tracked.patch" "wgpu/01-new-files.patch"
run "openharmony-ability" do_git_repo "$DEPS_DIR/openharmony-ability" "https://github.com/harmony-contrib/openharmony-ability.git" "6c52bb44164ea2d6d7f573c090a75142f0dbd2ef" "openharmony-ability" "openharmony-ability/00-tracked.patch" "openharmony-ability/01-new-files.patch"
run "nix"                 do_crate "nix" "0.26.4" "$DEPS_DIR"
run "interprocess"        do_crate "interprocess" "1.2.1" "$DEPS_DIR"
run "gettext-sys"         do_crate "gettext-sys" "0.21.3" "$DEPS_DIR"
run "mbedtls"             do_archive "third_party_mbedtls-OpenHarmony-v3.2-Release.tar.gz" "https://github.com/openharmony/third_party_mbedtls/archive/refs/tags/OpenHarmony-v3.2-Release.tar.gz"
run "libssh2"             do_archive "libssh2-1.11.0.tar.gz" "https://libssh2.org/download/libssh2-1.11.0.tar.gz"
run "ohos-ime-binding"    do_crate "ohos-ime-binding" "0.1.2" "$DEPS_DIR"

echo ""
echo "=========================================="
echo " 结果汇总"
echo "=========================================="
printf "$RESULTS"
echo "=========================================="

cd "$BASE_DIR"
echo "下一步: cd $BASE_DIR/warp && ./script/ohos/build-vm.sh"
