#!/usr/bin/env bash
# necoder.app（macOS アプリバンドル）を組み立てる。アイコン＝マスコット（猫耳コーダー娘）。
#
# gpui はアプリアイコンをコード設定できない（Zed 同様 .app の .icns で決まる）。
# `cargo run` の素のバイナリは Dock に汎用アイコンが出るだけなので、Dock/Finder に
# マスコットを出すにはこのバンドルを使う（or ビルド済み .app を /Applications に置く）。
#
# 使い方: ./scripts/bundle-mac.sh [release|debug]   （既定 release）
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="${1:-release}"
# バージョンの唯一の出所 = workspace の Cargo.toml（[workspace.package] version）。
# updater は CARGO_PKG_VERSION（= 同じ値）と比較し、タグとの一致は release.yml が検証する
# ＝「Cargo.toml / Info.plist / タグ」三重手動同期の廃止（不一致だと更新チップが無限に出る）。
APP_VERSION="$(awk -F '"' '/^\[workspace\.package\]/{flag=1; next} /^\[/{flag=0} flag && /^version = /{print $2; exit}' Cargo.toml)"
if [ -z "$APP_VERSION" ]; then
    echo "Cargo.toml から version を読めない（[workspace.package] の version 行を確認）" >&2
    exit 1
fi
# アイコン原画 = necoder（pixel art・2026-07-27 に 01-neko-coder.png から差し替え）。
# 小サイズで読めるバストアップ。全身の neko-art.png は 32px で潰れるため不採用。
ICON_SRC="lp/assets/img/necoder-mark.png"
ICON_DIR="crates/necoder/assets/icon"
APP="target/necoder.app"

# 1) アイコン（.icns）を生成（角丸マスク → iconset → iconutil）。
python3 scripts/make-icon.py "$ICON_SRC" "$ICON_DIR"

# 2) バイナリをビルド。
# フル Xcode の無い環境（Command Line Tools のみ = `metal` コンパイラ不在）では、gpui の
# シェーダを実行時コンパイルに切替える（`runtime-shaders` feature）。Xcode があれば従来どおり
# 事前コンパイル（起動が僅かに速い）。CI/出荷は Xcode 前提なので影響しない。
SHADER_FEATURES=""
if ! xcrun -f metal >/dev/null 2>&1; then
    SHADER_FEATURES="--features runtime-shaders"
    echo "  metal コンパイラ無し → 実行時シェーダ（runtime-shaders）でビルド"
fi
if [ "$PROFILE" = "debug" ]; then
    cargo build -p necoder $SHADER_FEATURES
    BIN="target/debug/necoder"
else
    cargo build --release -p necoder $SHADER_FEATURES
    BIN="target/release/necoder"
fi

# 3) .app を組み立て。
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/necoder"
cp "$ICON_DIR/necoder.icns" "$APP/Contents/Resources/necoder.icns"
cp LICENSE "$APP/Contents/Resources/LICENSE-AGPL-3.0.txt"
cp THIRD_PARTY_NOTICES.md "$APP/Contents/Resources/THIRD_PARTY_NOTICES.md"
mkdir -p "$APP/Contents/Resources/licenses"
cp third_party/licenses/*.txt "$APP/Contents/Resources/licenses/"
# ターミナル用 `ne` コマンドはバンドル同梱物ではなく、本体の `necoder install-cli` /
# 設定 > コマンドライン が /usr/local/bin へ委譲シムを生成する（crates/cli_shim）。

# 3b) remote SSH サーバーバイナリを同梱（#1・旧 M9）。インストール版でも配備できるように。
#  - 同 OS 用（mac→mac / ssh://localhost）: MacOS/ の隣に置く（find_local_remote_server の sibling 探索先）。
#  - 別 OS 用（mac→Linux）: CI 生成の musl artifact が target/<triple>/release/ にあれば
#    Resources/remote/<triple>/ へ（find_remote_server_for の .app 同梱探索先）。
if [ "$PROFILE" = "debug" ]; then
    cargo build -p host --bin necoder-remote-server
    SERVER_BIN="target/debug/necoder-remote-server"
else
    cargo build --release -p host --bin necoder-remote-server
    SERVER_BIN="target/release/necoder-remote-server"
fi
cp "$SERVER_BIN" "$APP/Contents/MacOS/necoder-remote-server"
for triple in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    artifact="target/$triple/release/necoder-remote-server"
    if [ -f "$artifact" ]; then
        mkdir -p "$APP/Contents/Resources/remote/$triple"
        cp "$artifact" "$APP/Contents/Resources/remote/$triple/necoder-remote-server"
        echo "  remote server 同梱: $triple"
    fi
done

# LSMinimumSystemVersion は 13.0（Ventura）。10.15 は未検証の空約束だった — §4 のアイコン挙動
# はじめ動作確認は macOS 13+ のみ。CFBundleDocumentTypes で「このアプリケーションで開く」に出る
# （LSHandlerRank=Alternate = 既定ハンドラは奪わない）。フォルダは Dock アイコンへの D&D 用。
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>necoder</string>
  <key>CFBundleDisplayName</key><string>necoder</string>
  <key>CFBundleIdentifier</key><string>dev.necoder.editor</string>
  <key>CFBundleVersion</key><string>${APP_VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${APP_VERSION}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>necoder</string>
  <key>CFBundleIconFile</key><string>necoder</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHumanReadableCopyright</key><string>Copyright © necoder contributors. AGPL-3.0-or-later.</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>NSAppleEventsUsageDescription</key><string>Finder 経由でファイルをゴミ箱へ移動するために使います。/ Used to move files to the Trash via Finder.</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Text / Source Code</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.text</string>
        <string>public.plain-text</string>
        <string>public.utf8-plain-text</string>
        <string>public.source-code</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Folder</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.folder</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
PLIST

# 4) ad-hoc 署名（2026-07-27 追加）。cargo が吐くバイナリにはリンカの ad-hoc 署名が付いており、
#    その Identifier は `necoder-<hash>` で Info.plist の CFBundleIdentifier と食い違う。
#    macOS 13+ はこの不一致でアイコン解決/Launch Services の登録がおかしくなる（Dock に
#    マスコットが出ない実例）。組み立て後に bundle 全体を署名し直して identifier を揃える。
codesign --force --sign - --identifier dev.necoder.editor \
    --entitlements crates/necoder/resources/necoder.entitlements "$APP"

# 5) Finder / Dock のアイコンキャッシュを更新させる。
#    バンドル dir だけ touch しても効かないことがあるので Info.plist も進め、
#    Launch Services へ明示的に再登録する（同一 bundle ID の別コピーがあると特に必要）。
touch "$APP/Contents/Info.plist" "$APP"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$LSREGISTER" ] && "$LSREGISTER" -f -R "$PWD/$APP"

echo "組み立て完了: $APP"
echo "→ open \"$APP\" で起動（Dock にマスコットが出る）"
echo "   アイコンが古いままなら: killall Dock"
