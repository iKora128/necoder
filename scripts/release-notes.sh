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

### Install

- **macOS 13+ (Apple Silicon)**: open \`necoder.dmg\` and drag \`necoder.app\` into Applications. Signed and notarized, so Gatekeeper shows no warning.
- **Windows 10+ (x64)**: unzip \`necoder-windows-x64.zip\` and run \`necoder.exe\`. The build is not signed yet, so on first launch SmartScreen asks you to choose "More info → Run anyway".
- Full history: [CHANGELOG.md](https://github.com/iKora128/necoder/blob/main/CHANGELOG.md) · License: AGPL-3.0 · https://necoder.com

### インストール

- **macOS 13 以降（Apple Silicon）**：\`necoder.dmg\` を開き、\`necoder.app\` を「アプリケーション」へドラッグします。署名と公証を済ませてあるので、Gatekeeper の警告は出ません。
- **Windows 10 以降（x64）**：\`necoder-windows-x64.zip\` を展開して \`necoder.exe\` を起動します。まだ署名していないため、初回は SmartScreen で「詳細情報」→「実行」を選んでください。
- 全変更履歴：[CHANGELOG.md](https://github.com/iKora128/necoder/blob/main/CHANGELOG.md) · ライセンス：AGPL-3.0 · https://necoder.com
FOOTER
