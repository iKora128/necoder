#!/bin/sh
# 隔離 offscreen で necoder の実画面を 1 枚撮る。窓は出ず、撮ったらアプリは自分で終了する。
#   shoot.sh <名前> <ja|en> "<追加の環境変数…>" <プロジェクト…>
# 例: shoot.sh hero ja "NECODER_ACTIVITY_PROBE=1" /tmp/ncp/necoder /tmp/ncp/api
set -eu
ISO=${ISO:-/tmp/ncp}
BINARY=${BINARY:-/tmp/necoder-promo-src/target/debug/necoder}
NAME=$1; LOCALE=$2; EXTRA=$3; shift 3
OUTPUT="$ISO/shots/$NAME-$LOCALE.png"
rm -f "$OUTPUT"
# 毎回まっさらな DB から撮る（前の撮影のスレッドやタブを持ち越さない）
rm -f "$ISO"/home/necoder.db*
# shellcheck disable=SC2086
env NECODER_HOME="$ISO/home" NECODER_GUI_SOCK="$ISO/gui.sock" NECODER_DOCUMENTS_DIR="$ISO/docs" \
  NECODER_NO_UPDATE_CHECK=1 NECODER_RUST_ANALYZER="$ISO/bin/lsp-stub" NECODER_WINDOW_SIZE=1680x1000 NECODER_LOCALE="$LOCALE" \
  NECODER_SCREENSHOT="$OUTPUT" NECODER_SCREENSHOT_DELAY_MS=5500 $EXTRA \
  "$BINARY" "$@" > "$ISO/shots/$NAME-$LOCALE.log" 2>&1 &
PID=$!
WAITED=0
while kill -0 "$PID" 2>/dev/null; do
  sleep 1; WAITED=$((WAITED + 1))
  if [ "$WAITED" -ge 40 ]; then kill "$PID"; echo "timeout: $NAME"; break; fi
done
ls -la "$OUTPUT" 2>/dev/null || echo "no output: $NAME (see $ISO/shots/$NAME-$LOCALE.log)"
