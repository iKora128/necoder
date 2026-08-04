#!/bin/sh
# Shirushi の UI 検証用スクリーンショット（簡易版）
# ビルド → 起動 → 数秒待って画面全体を撮影。ウィンドウ単位の撮影は後続で改善する。
set -e
cd "$(dirname "$0")/.."
# フル Xcode の無い環境（Command Line Tools のみ = `metal` 不在）では、gpui のシェーダを
# 実行時コンパイルに切替える（bundle-mac.sh と同じ逃げ道）。Xcode があれば事前コンパイル。
SHADER_FEATURES=""
if ! xcrun -f metal >/dev/null 2>&1; then
    SHADER_FEATURES="--features runtime-shaders"
    echo "  metal コンパイラ無し → 実行時シェーダ（runtime-shaders）でビルド"
fi
cargo build -p shirushi $SHADER_FEATURES
./target/debug/shirushi &
APP_PID=$!
sleep 3
OUT="/tmp/shirushi-ui-$(date +%H%M%S).png"
screencapture -x "$OUT"
echo "saved: $OUT"
echo "app is still running (PID $APP_PID) — 確認が終わったら: kill $APP_PID"
