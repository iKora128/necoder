#!/bin/sh
# 起動時間計測（cold start → 初回描画）。予算: Zed 比 ~80%（docs/ARCHITECTURE §8 / CLAUDE.md 性能予算）。
# アプリは NECODER_STARTUP_LOG=1 のとき初回 render で `startup_ms=<n>` を stdout に出す。
# 使い方: scripts/startup-time.sh [試行回数=5]
set -eu
cd "$(dirname "$0")/.."

RUNS="${1:-5}"
BIN=./target/release/necoder
PROBE=/tmp/necoder-startup-probe.txt

echo "リリースビルド中..."
cargo build --release -p necoder >/dev/null 2>&1
printf 'necoder 起動計測用プローブ\n%s\n' "$(seq 1 200 | tr '\n' ' ')" > "$PROBE"

run_once() {
  log=$(mktemp)
  NECODER_STARTUP_LOG=1 "$BIN" "$PROBE" >"$log" 2>/dev/null &
  pid=$!
  # 初回描画のログ行が出るまで待つ（最大 ~10s）
  waited=0
  while [ "$waited" -lt 400 ]; do
    if grep -q startup_ms "$log" 2>/dev/null; then break; fi
    sleep 0.025
    waited=$((waited + 1))
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  grep -o 'startup_ms=[0-9.]*' "$log" 2>/dev/null | head -1 | cut -d= -f2
  rm -f "$log"
}

echo "起動時間 (cold start → first render, ${RUNS} 回):"
total=0
count=0
for run in $(seq 1 "$RUNS"); do
  ms=$(run_once)
  if [ -n "${ms:-}" ]; then
    printf '  run %s: %s ms\n' "$run" "$ms"
    total=$(awk -v t="$total" -v m="$ms" 'BEGIN { print t + m }')
    count=$((count + 1))
  else
    printf '  run %s: (計測失敗)\n' "$run"
  fi
done

if [ "$count" -gt 0 ]; then
  awk -v t="$total" -v c="$count" 'BEGIN { printf "平均: %.1f ms (%d/%d 回成功)\n", t / c, c, '"$RUNS"' }'
fi
