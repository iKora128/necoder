#!/usr/bin/env bash
# 自分で自分を更新する（ドッグフーディング用）。
# ビルド → /Applications/Shirushi.app を差し替え → 起動し直す、までを 1 コマンドで。
#
# Shirushi で Shirushi を書いている最中に走らせる前提なので、順番が大事:
#   重いビルドを「先に」終わらせてから終了 → 差し替え → 再起動 と繋ぎ、
#   エディタが落ちている時間を数秒に抑える。ビルドが失敗したら何も壊さず終わる。
#
# 編集中バッファは hot exit（~/Library/Application Support/Shirushi/shirushi.db）で
# 復元されるが、確実を期すなら走らせる前に保存しておくこと。
#
# 使い方: ./scripts/install-mac.sh [release|debug]   （既定 release）
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="${1:-release}"
SRC="target/Shirushi.app"
DEST="/Applications/Shirushi.app"

# 1) 先にビルド＆バンドル。ここで失敗したら動作中のアプリには一切触れない。
./scripts/bundle-mac.sh "$PROFILE"

# 2) 動いていれば終了させる。強制 kill ではなく quit を送り、hot exit の保存を待つ。
RELAUNCH=0
if pgrep -x shirushi >/dev/null 2>&1; then
    RELAUNCH=1
    echo "動作中の Shirushi を終了中..."
    osascript -e 'quit app "Shirushi"' 2>/dev/null || true
    for _ in $(seq 1 50); do
        pgrep -x shirushi >/dev/null 2>&1 || break
        sleep 0.2
    done
    if pgrep -x shirushi >/dev/null 2>&1; then
        echo "終了しないので強制終了する" >&2
        pkill -x shirushi || true
        sleep 0.5
    fi
fi

# 3) 差し替え。--delete で旧ビルドの残骸（消えたリソース）を残さない。
mkdir -p "$(dirname "$DEST")"
rsync -a --delete "$SRC/" "$DEST/"

# 4) 署名し直す。rsync は署名ごと運ぶので普通は有効なままだが、
#    差分コピーで _CodeSignature と中身がずれた時にアイコン解決が壊れる（bundle-mac.sh §4 と同じ罠）。
codesign --force --sign - --identifier dev.shirushi.editor "$DEST"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$LSREGISTER" ] && "$LSREGISTER" -f -R "$DEST"

echo "インストール完了: $DEST"

# 5) 元々動いていたなら開き直す。止まっていたなら黙って終わる（勝手に前面に出ない）。
if [ "$RELAUNCH" = "1" ]; then
    open -a "$DEST"
    echo "→ 再起動した"
else
    echo "→ open -a \"$DEST\" で起動"
fi
