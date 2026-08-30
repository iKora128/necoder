#!/usr/bin/env bash
# 自分で自分を更新する（ドッグフーディング用）。
# ビルド → /Applications/necoder.app を差し替え → 起動し直す、までを 1 コマンドで。
#
# necoder で necoder を書いている最中に走らせる前提なので、順番が大事:
#   重いビルドを「先に」終わらせてから終了 → 差し替え → 再起動 と繋ぎ、
#   エディタが落ちている時間を数秒に抑える。ビルドが失敗したら何も壊さず終わる。
#
# さらに、統合ターミナル（＝ necoder の子プロセス）から叩かれた場合は、自分を quit した
# 時点でこのスクリプトごと死んで差し替えが中途半端に終わる。祖先に necoder がいたら
# 差し替え以降を切り離したプロセスへ渡して生き延びさせる。
#
# 編集中バッファは hot exit（~/Library/Application Support/necoder/necoder.db）で
# 復元されるが、確実を期すなら走らせる前に保存しておくこと。
#
# 使い方: ./scripts/install-mac.sh [release|debug]   （既定 release）
set -euo pipefail

# cd する前に自分の絶対パスを取る（切り離し実行で自分を呼び直すため）。
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$0")/.."

# --swap-only は内部用（ビルド済み前提で差し替えだけやる）。
SWAP_ONLY=0
if [ "${1:-}" = "--swap-only" ]; then
    SWAP_ONLY=1
    shift
fi
PROFILE="${1:-release}"
SRC="target/necoder.app"
DEST="/Applications/necoder.app"
LOG="/tmp/necoder-install.log"

# 祖先を辿って necoder 本体がいるか見る（＝統合ターミナルから走っている）。
inside_necoder() {
    local pid="${PPID:-0}" hops=0 comm
    while [ "$pid" -gt 1 ] && [ "$hops" -lt 20 ]; do
        comm="$(ps -o comm= -p "$pid" 2>/dev/null)" || return 1
        [ -z "$comm" ] && return 1
        [ "${comm##*/}" = "necoder" ] && return 0
        pid="$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d '[:space:]')"
        [ -z "$pid" ] && return 1
        hops=$((hops + 1))
    done
    return 1
}

swap_and_relaunch() {
    # 動いていれば終了させる。強制 kill ではなく quit を送り、hot exit の保存を待つ。
    local relaunch=0
    if pgrep -x necoder >/dev/null 2>&1; then
        relaunch=1
        echo "動作中の necoder を終了中..."
        osascript -e 'quit app "necoder"' 2>/dev/null || true
        for _ in $(seq 1 50); do
            pgrep -x necoder >/dev/null 2>&1 || break
            sleep 0.2
        done
        if pgrep -x necoder >/dev/null 2>&1; then
            echo "終了しないので強制終了する" >&2
            pkill -x necoder || true
            sleep 0.5
        fi
    fi

    # 差し替え。--delete で旧ビルドの残骸（消えたリソース）を残さない。
    rsync -a --delete "$SRC/" "$DEST/"

    # 署名し直す。rsync は署名ごと運ぶので普通は有効なままだが、差分コピーで
    # _CodeSignature と中身がずれた時にアイコン解決が壊れる（bundle-mac.sh §4 と同じ罠）。
    codesign --force --sign - --identifier dev.necoder.editor "$DEST"
    local lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    [ -x "$lsregister" ] && "$lsregister" -f -R "$DEST"

    echo "インストール完了: $DEST"

    # ターミナル用 `ne` シム（/usr/local/bin/ne）: 旧実体（dev ビルド等）を指していたら
    # /Applications 版へ向け直す。未設置なら案内だけ — sudo が要る設置をスクリプトで
    # 勝手に求めない（設置は 設定 > コマンドライン のパスワードダイアログ経由が本線）。
    if [ -x /usr/local/bin/ne ]; then
        if ! grep -q "NECODER_BIN=\"$DEST/Contents/MacOS/necoder\"" /usr/local/bin/ne 2>/dev/null; then
            "$DEST/Contents/MacOS/necoder" install-cli \
                || echo "→ ne の向け直しは 設定 > コマンドライン から（または sudo \"$DEST/Contents/MacOS/necoder\" install-cli）"
        fi
    else
        echo "→ ターミナル用 ne コマンドは未設置。設定 > コマンドライン、または sudo \"$DEST/Contents/MacOS/necoder\" install-cli"
    fi

    # 元々動いていたなら開き直す。止まっていたなら黙って終わる（勝手に前面に出ない）。
    if [ "$relaunch" = "1" ]; then
        open -a "$DEST"
        echo "→ 再起動した"
    else
        echo "→ open -a \"$DEST\" で起動"
    fi
}

if [ "$SWAP_ONLY" = "1" ]; then
    swap_and_relaunch
    exit 0
fi

# 1) 先にビルド＆バンドル。ここで失敗したら動作中のアプリには一切触れない。
./scripts/bundle-mac.sh "$PROFILE"

# 2) 差し替え。necoder の中から走っているなら、自分の死に巻き込まれないよう切り離す。
if inside_necoder; then
    echo "necoder の統合ターミナルから実行されている → 差し替えを別プロセスへ渡す"
    nohup "$SELF" --swap-only "$PROFILE" >"$LOG" 2>&1 &
    disown 2>/dev/null || true
    echo "→ 数秒で終了・再起動します（ログ: $LOG）"
    exit 0
fi

swap_and_relaunch
