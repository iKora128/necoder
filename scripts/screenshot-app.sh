#!/bin/sh
# Shirushi の UI 検証用スクリーンショット（簡易版）
# ビルド → 起動 → 数秒待って画面全体を撮影。ウィンドウ単位の撮影は後続で改善する。
set -e
cd "$(dirname "$0")/.."
cargo build -p shirushi
./target/debug/shirushi &
APP_PID=$!
sleep 3
OUT="/tmp/shirushi-ui-$(date +%H%M%S).png"
screencapture -x "$OUT"
echo "saved: $OUT"
echo "app is still running (PID $APP_PID) — 確認が終わったら: kill $APP_PID"
