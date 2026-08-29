#!/usr/bin/env bash
# Keep strong-copyleft dependencies explicit. cargo-deny ignores publish=false packages when
# licenses.private.ignore=true, which includes both this workspace and Zed's git-only crates.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
metadata_file="$(mktemp)"
trap 'rm -f "$metadata_file"' EXIT

cargo metadata --format-version 1 >"$metadata_file"

zed_source='git+https://github.com/zed-industries/zed?rev=b2d9c2e122fbc408d42276b4456243ba4f90f181#b2d9c2e122fbc408d42276b4456243ba4f90f181'

unexpected="$(
  jq -r --arg root "$repo_root" --arg zed_source "$zed_source" '
    .packages[]
    | . as $package
    # cargo-deny handles SPDX alternatives such as `Apache-2.0 OR GPL-2.0-only` by selecting an
    # allowed branch. This guard is for packages whose declared license is the strong-copyleft
    # license itself and which private.ignore could otherwise hide.
    | select((.license // "") as $license
        | $license == "GPL-3.0-only"
          or $license == "GPL-3.0-or-later"
          or $license == "AGPL-3.0-only"
          or $license == "AGPL-3.0-or-later")
    | select(
        (
          .source == null
          and .license == "AGPL-3.0-or-later"
          and (.manifest_path | startswith($root + "/crates/"))
        )
        or
        (
          (["ztracing", "ztracing_macro", "zlog"] | index($package.name)) != null
          and $package.source == $zed_source
          and $package.license == "GPL-3.0-or-later"
        )
        | not
      )
    | "\(.name)\t\(.license // "<none>")\t\(.source // "workspace")"
  ' "$metadata_file"
)"

if [[ -n "$unexpected" ]]; then
  echo "Unexpected GPL/AGPL dependency boundary:" >&2
  echo "$unexpected" >&2
  exit 1
fi

for package in ztracing ztracing_macro zlog; do
  if ! jq -e --arg package "$package" --arg zed_source "$zed_source" '
    any(.packages[];
      .name == $package
      and .license == "GPL-3.0-or-later"
      and .source == $zed_source
    )
  ' "$metadata_file" >/dev/null; then
    echo "Documented GPL dependency is missing or changed: $package" >&2
    echo "Review the new GPUI graph and update THIRD_PARTY_NOTICES.md intentionally." >&2
    exit 1
  fi
done

echo "copyleft boundary ok: workspace AGPL + documented Zed GPL transitive crates"
