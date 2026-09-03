#!/usr/bin/env bash
# CHANGELOG.md から指定版の節を切り出して GitHub Release の本文（Markdown）を出力する。
#
#   scripts/release-notes.sh 0.1.9            # 標準出力へ
#   scripts/release-notes.sh 0.1.9 > body.md  # release.yml はこれを body_path で渡す
#
# 本文の正は CHANGELOG.md の 1 箇所だけ（RELEASE.md §1 の手順 1）。節が無ければ非 0 で落ちる
# ＝ CHANGELOG を書き忘れたまま空のリリースが公開されるのを CI で止める（v0.1.2〜v0.1.5 /
# v0.1.7〜v0.1.9 が空のまま公開された 2026-09-02 の反省）。
set -euo pipefail

version="${1:?usage: release-notes.sh <version>  (例: 0.1.9 / タグの v は付けない)}"
version="${version#v}"
changelog="$(cd "$(dirname "$0")/.." && pwd)/CHANGELOG.md"

section="$(awk -v ver="$version" '
  /^## \[/ {
    if (inside) exit
    if ($0 ~ "^## \\[" ver "\\]") { inside = 1; next }
  }
  inside { print }
' "$changelog")"

if [ -z "$(printf '%s' "$section" | tr -d '[:space:]')" ]; then
  echo "release-notes.sh: CHANGELOG.md に ## [$version] の節がありません（RELEASE.md §1 の手順 1）" >&2
  exit 1
fi

# 先頭・末尾の空行を落とす
printf '%s\n' "$section" | awk 'NF { seen = 1 } seen' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'

cat <<FOOTER

---

### インストール / Install

- **macOS 13+（Apple Silicon）**: \`necoder.dmg\` を開いて \`necoder.app\` を Applications へドラッグ。署名・公証済み（Gatekeeper の警告なし）
- **Windows 10+（x64）**: \`necoder-windows-x64.zip\` を展開して \`necoder.exe\` を起動。未署名のため初回は SmartScreen の「詳細情報 → 実行」が必要
- 全変更履歴: [CHANGELOG.md](https://github.com/iKora128/necoder/blob/v${version}/CHANGELOG.md) · License: AGPL-3.0 · https://necoder.com
FOOTER
