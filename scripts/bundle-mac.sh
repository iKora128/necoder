#!/usr/bin/env bash
# Shirushi.app（macOS アプリバンドル）を組み立てる。アイコン＝マスコット（猫耳コーダー娘）。
#
# gpui はアプリアイコンをコード設定できない（Zed 同様 .app の .icns で決まる）。
# `cargo run` の素のバイナリは Dock に汎用アイコンが出るだけなので、Dock/Finder に
# マスコットを出すにはこのバンドルを使う（or ビルド済み .app を /Applications に置く）。
#
# 使い方: ./scripts/bundle-mac.sh [release|debug]   （既定 release）
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="${1:-release}"
ICON_SRC="mock/mascot/01-neko-coder.png"
ICON_DIR="crates/shirushi/assets/icon"
APP="target/Shirushi.app"

# 1) アイコン（.icns）を生成（角丸マスク → iconset → iconutil）。
python3 scripts/make-icon.py "$ICON_SRC" "$ICON_DIR"

# 2) バイナリをビルド。
if [ "$PROFILE" = "debug" ]; then
    cargo build -p shirushi
    BIN="target/debug/shirushi"
else
    cargo build --release -p shirushi
    BIN="target/release/shirushi"
fi

# 3) .app を組み立て。
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/shirushi"
cp "$ICON_DIR/Shirushi.icns" "$APP/Contents/Resources/Shirushi.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Shirushi</string>
  <key>CFBundleDisplayName</key><string>Shirushi</string>
  <key>CFBundleIdentifier</key><string>dev.shirushi.editor</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>shirushi</string>
  <key>CFBundleIconFile</key><string>Shirushi</string>
  <key>LSMinimumSystemVersion</key><string>10.15</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
</dict>
</plist>
PLIST

# Finder のアイコンキャッシュを更新させる（mtime を進める）。
touch "$APP"
echo "組み立て完了: $APP"
echo "→ open \"$APP\" で起動（Dock にマスコットが出る）"
