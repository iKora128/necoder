#!/bin/sh
# idle メモリ実測（M13・terminal-stack-2026 §4「RAM で買う層への武器」）。
# release バイナリでファイルを 1 枚開いて idle 数秒 → RSS を読む。
# 使い方: scripts/memory-usage.sh [待ち秒=6]
set -eu
cd "$(dirname "$0")/.."

WAIT="${1:-6}"
BIN=./target/release/shirushi
PROBE=/tmp/shirushi-memory-probe.rs
[ -x "$BIN" ] || { echo "先に: cargo build --release -p shirushi"; exit 1; }
printf 'fn main() {\n    println!("hello");\n}\n' > "$PROBE"

SHIRUSHI_NO_UPDATE_CHECK=1 "$BIN" "$PROBE" >/dev/null 2>&1 &
PID=$!
sleep "$WAIT"
RSS_KB=$(ps -o rss= -p "$PID" | tr -d ' ')
kill "$PID" 2>/dev/null || true
echo "idle RSS: $((RSS_KB / 1024)) MB（起動 ${WAIT}s 後・${PROBE} を 1 枚表示）"
