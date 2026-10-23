#!/bin/sh
# 動画に使う実画面を ja / en の両方で撮る（隔離 offscreen・課金なし）。
#   ISO=/tmp/ncp promo/scripts/capture-all.sh
set -u
ISO=${ISO:-/tmp/ncp}
HERE=$(cd "$(dirname "$0")" && pwd)
SHOOT="$HERE/shoot.sh"
ALL="$ISO/necoder $ISO/api $ISO/web $ISO/infra $ISO/ml"
TABS="NECODER_OPEN_TABS=crates/editor_core/src/editor_core.rs,crates/agent_panel/src/agent_panel.rs,crates/workspace/src/workspace/rail_view.rs NECODER_TYPE_PROBE=214:0: NECODER_TYPE_PROBE_DELAY_MS=3000"
for LOCALE in ja en; do
  # shellcheck disable=SC2086
  "$SHOOT" hero "$LOCALE" "$TABS NECODER_TABS_PROBE=3 NECODER_ACTIVITY_PROBE=1" $ALL
  "$SHOOT" project1 "$LOCALE" "NECODER_ACTIVE_PROJECT=1 NECODER_ACTIVE_OPEN=src/auth.rs NECODER_ACTIVITY_PROBE=1" $ALL
  "$SHOOT" project2 "$LOCALE" "NECODER_ACTIVE_PROJECT=2 NECODER_ACTIVE_OPEN=src/hero.tsx NECODER_ACTIVITY_PROBE=1" $ALL
  "$SHOOT" project3 "$LOCALE" "NECODER_ACTIVE_PROJECT=3 NECODER_ACTIVE_OPEN=main.tf NECODER_ACTIVITY_PROBE=1" $ALL
  "$SHOOT" project4 "$LOCALE" "NECODER_ACTIVE_PROJECT=4 NECODER_ACTIVE_OPEN=rerank/train.py NECODER_ACTIVITY_PROBE=1" $ALL
  "$SHOOT" flash "$LOCALE" "NECODER_ACTIVE_PROJECT=2:flash NECODER_ACTIVE_OPEN=src/hero.tsx NECODER_SCREENSHOT_DELAY_MS=1350" $ALL
  "$SHOOT" transcript "$LOCALE" "NECODER_AGENT_FULLSCREEN=1 NECODER_DEMO_THREADS=1 NECODER_PLAN_PROBE=1" $ALL
  "$SHOOT" switcher "$LOCALE" "NECODER_SWITCHER_PROBE=2600 NECODER_ACTIVITY_PROBE=1 NECODER_TABS_PROBE=3" $ALL
  "$SHOOT" finder "$LOCALE" "NECODER_FINDER_PROBE=1" $ALL
  "$SHOOT" palette "$LOCALE" "NECODER_PALETTE_PROBE=theme" $ALL
  "$SHOOT" todos "$LOCALE" "NECODER_TODOS_PROBE=1 NECODER_TERMINAL=1 NECODER_TERM_ECHO=necoder·terminal" $ALL
  "$SHOOT" formation "$LOCALE" "NECODER_CONTROL_PROBE=1 NECODER_FLEET_PROBE=graph;formation NECODER_GRAPH=fan" $ALL
  for THEME in solarized-dark gruvbox-dark catppuccin-mocha necoder-light high-contrast-dark; do
    "$SHOOT" "theme-$THEME" "$LOCALE" "$TABS NECODER_TABS_PROBE=3 NECODER_ACTIVITY_PROBE=1 NECODER_THEME=$THEME" $ALL
  done
done
# ここから先は差分が要る（rail_view.rs に小さな編集を入れる）
sed -i '' 's/\.size(px(30\.))/.size(px(32.))/; s/\.rounded(px(8\.))/.rounded(px(9.))/' "$ISO/necoder/crates/workspace/src/workspace/rail_view.rs"
for LOCALE in ja en; do
  "$SHOOT" fleet "$LOCALE" "NECODER_CONTROL_PROBE=1 NECODER_FLEET_PROBE=graph;proposal:3;origin;task:1;side:diff" $ALL
  "$SHOOT" diff "$LOCALE" "NECODER_OPEN_TABS=crates/workspace/src/workspace/rail_view.rs NECODER_DIFF_PROBE=1 NECODER_GIT_PANEL=1" $ALL
done
echo "done"
