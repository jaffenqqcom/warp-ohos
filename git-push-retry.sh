#!/bin/sh
# 重试 git push 直到成功
# 每次失败输出完整错误信息，间隔 30 秒后重试
set -u

RETRY_DELAY=30
attempt=0

while :; do
    attempt=$((attempt + 1))
    echo "=== git push (attempt $attempt) ==="

    # 捕获 git push 的输出和退出码
    output=$(git push 2>&1) || true
    exit_code=$?

    if [ $exit_code -eq 0 ]; then
        if [ -n "$output" ]; then
            echo "$output"
        else
            echo "（无输出，everything up-to-date）"
        fi
        echo ""
        echo "✅ git push 成功（attempt $attempt）"
        exit 0
    fi

    # 失败：原样输出错误信息
    echo "$output"
    echo ""
    echo "❌ git push 失败（attempt $attempt, exit code $exit_code）"
    echo "   等待 ${RETRY_DELAY} 秒后重试..."
    echo ""
    sleep "$RETRY_DELAY"
done
