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

# 3b) remote SSH サーバーバイナリを同梱（#1・旧 M9）。インストール版でも配備できるように。
#  - 同 OS 用（mac→mac / ssh://localhost）: MacOS/ の隣に置く（find_local_remote_server の sibling 探索先）。
#  - 別 OS 用（mac→Linux）: CI 生成の musl artifact が target/<triple>/release/ にあれば
#    Resources/remote/<triple>/ へ（find_remote_server_for の .app 同梱探索先）。
if [ "$PROFILE" = "debug" ]; then
    cargo build -p host --bin shirushi-remote-server
    SERVER_BIN="target/debug/shirushi-remote-server"
else
    cargo build --release -p host --bin shirushi-remote-server
    SERVER_BIN="target/release/shirushi-remote-server"
fi
cp "$SERVER_BIN" "$APP/Contents/MacOS/shirushi-remote-server"
for triple in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    artifact="target/$triple/release/shirushi-remote-server"
    if [ -f "$artifact" ]; then
        mkdir -p "$APP/Contents/Resources/remote/$triple"
        cp "$artifact" "$APP/Contents/Resources/remote/$triple/shirushi-remote-server"
        echo "  remote server 同梱: $triple"
    fi
done

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
