#!/bin/sh
# 撮影用の隔離環境を作る（稼働中の necoder 本体・DB・ソケットには一切触れない）。
# 課金の起きる設定（agent_prewarm / tier2_summaries / agent_auto_name / captain_agent）は全部切る。
#   ISO=/tmp/ncp promo/scripts/setup-capture.sh /tmp/necoder-promo-src
set -eu
ISO=${ISO:-/tmp/ncp}
SOURCE=${1:?necoder のソース（git archive で書き出したもの）を渡す}
rm -rf "$ISO"
mkdir -p "$ISO/home" "$ISO/docs" "$ISO/shots"
cat > "$ISO/home/settings.json" <<'JSON'
{"onboarded":true,"agent_prewarm":false,"reduce_motion":true,"tier2_summaries":false,"agent_auto_name":false,"sound_done":"off","sound_waiting":"off"}
JSON

commit() { git -C "$1" add -A && git -C "$1" -c user.name=necoder -c user.email=promo@necoder.local commit -qm "$2"; }

# 1. necoder 自身（実コード）— 赤
mkdir -p "$ISO/necoder"
(cd "$SOURCE" && tar -cf - .gitignore crates docs locales Cargo.toml Cargo.lock README.md FEATURES.md LICENSE rust-toolchain.toml) | tar -xf - -C "$ISO/necoder"
color_of() { mkdir -p "$ISO/$1/.necoder" && printf '{"color":"%s"}\n' "$2" > "$ISO/$1/.necoder/settings.json"; }
# 識別色はアプリの巡回 5 色（theme_core::IDENTITY_PALETTE_HEXES の先頭）に合わせる
color_of necoder '#ef4444'; color_of api '#22c55e'; color_of web '#3b82f6'; color_of infra '#f97316'; color_of ml '#a855f7'
printf '# Today\n- [x] rope buffer\n- [x] undo groups\n- [ ] ship v0.2\n- [ ] fleet: captain split\n' > "$ISO/necoder/.necoder/todos.md"
git -C "$ISO/necoder" init -q -b main && commit "$ISO/necoder" init

# 2〜5. 架空のプロジェクト（言語を変えて、切り替えたときに別物だと分かるようにする）
mkdir -p "$ISO/api/src" "$ISO/web/src" "$ISO/infra/modules" "$ISO/ml/rerank"
echo 'target/' > "$ISO/api/.gitignore"
cat > "$ISO/api/Cargo.toml" <<'T'
[package]
name = "api"
version = "0.4.0"
edition = "2021"
T
cat > "$ISO/api/src/auth.rs" <<'T'
use axum::http::StatusCode;

pub async fn verify(token: &str, keys: &KeySet) -> Result<Claims, StatusCode> {
    let header = decode_header(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let key = keys.find(&header.kid).ok_or(StatusCode::UNAUTHORIZED)?;
    decode::<Claims>(token, key, &VALIDATION)
        .map(|data| data.claims)
        .map_err(|_| StatusCode::UNAUTHORIZED)
}
T
echo 'fn main() {}' > "$ISO/api/src/main.rs"
git -C "$ISO/api" init -q -b main && commit "$ISO/api" init && git -C "$ISO/api" checkout -q -b fix/auth
cat > "$ISO/web/package.json" <<'T'
{ "name": "web", "private": true, "scripts": { "dev": "vite" } }
T
cat > "$ISO/web/src/hero.tsx" <<'T'
export function Hero({ title }: { title: string }) {
  return <section className="hero"><h1>{title}</h1></section>;
}
T
git -C "$ISO/web" init -q -b main && commit "$ISO/web" init && git -C "$ISO/web" checkout -q -b feat/landing
cat > "$ISO/infra/main.tf" <<'T'
module "cluster" {
  source = "./modules/cluster"
  region = "ap-northeast-1"
}
T
git -C "$ISO/infra" init -q -b main && commit "$ISO/infra" init
cat > "$ISO/ml/rerank/train.py" <<'T'
def train(model, loader, epochs: int = 3):
    for epoch in range(epochs):
        for batch in loader:
            loss = model(**batch).loss
            loss.backward()
T
git -C "$ISO/ml" init -q -b main && commit "$ISO/ml" init && git -C "$ISO/ml" checkout -q -b exp/rerank

# LSP は何もしないスタブに差し替える（rust-analyzer が隔離側で cargo check を始めないように）
mkdir -p "$ISO/bin"
printf '#!/bin/sh\nexec cat > /dev/null\n' > "$ISO/bin/lsp-stub" && chmod +x "$ISO/bin/lsp-stub"
echo "ready: $ISO"
