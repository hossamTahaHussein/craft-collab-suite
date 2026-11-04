#!/usr/bin/env bash
# Build Craft Docs', Craft Sheets' and Craft Slides' browser apps and zip them for a release:
#   $DIST/wordcraft-web-<version>.zip      Craft Docs   (apps/wordcraft-web)
#   $DIST/craft-sheets-web-<version>.zip   Craft Sheets (sheets/apps/gridcraft-web)
#   $DIST/craft-slides-web-<version>.zip   Craft Slides (slides/apps/deckcraft-web)
#   $DIST/SHA256SUMS.txt                   their checksums, which craft-collab-embed's
#                                          `npm run fetch-editor` checks
#
# Usage: packaging/web/suite-release.sh [--skip-build]
#
# Needs trunk and the wasm32-unknown-unknown target. Set CRAFT_FONTS_DIR (the pinned craft-fonts
# checkout) so the builds embed the Arabic, Hebrew and CJK faces and the zips carry their licences.
# Each zip is a self-contained static site in <name>-<version>/ that works from any URL path and
# inside an <iframe>, with its app's licences, NOTICE and ATTRIBUTION and the hosting notes.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

command -v trunk >/dev/null || { echo "error: trunk not found (cargo install trunk --locked)" >&2; exit 1; }

# One app: its web crate, its release name and the folder whose README, licences and packaging
# notes go with it.
package() {
  local crate="$1" name="$2" home="$3"
  local site="$ROOT/dist/web-$name"
  if [[ "${SKIP_BUILD:-}" != 1 ]]; then
    (cd "$ROOT/$crate" && trunk build --release --dist "$site" --public-url ./)
  fi
  [[ -f "$site/index.html" ]] || { echo "error: $site/index.html missing; run without --skip-build" >&2; exit 1; }
  if grep -Eq '(src|href)="/[^/]' "$site/index.html"; then
    echo "error: $site/index.html has root-absolute URLs; it would break when served from a sub-path" >&2
    exit 1
  fi
  local dir="$name-$VERSION" work="$CARGO_TARGET_DIR/web-package"
  rm -rf "${work:?}/$dir"
  mkdir -p "$work/$dir"
  cp -R "$site/." "$work/$dir/"
  cp "$home/packaging/web/_headers" "$home/packaging/web/.htaccess" "$work/$dir/"
  cp "$home/packaging/web/README.md" "$work/$dir/HOSTING.md"
  local f
  for f in README.md LICENSE-MIT LICENSE-APACHE NOTICE ATTRIBUTION.md; do
    if [[ -f "$home/$f" ]]; then cp "$home/$f" "$work/$dir/"; fi
  done
  copy_font_licences "$work/$dir"
  rm -f "$DIST/$dir.zip"
  (cd "$work" && zip -qr9 "$DIST/$dir.zip" "$dir")
  echo "wrote $DIST/$dir.zip"
}

[[ "${1:-}" == "--skip-build" ]] && SKIP_BUILD=1
package apps/wordcraft-web wordcraft-web "$ROOT"
package sheets/apps/gridcraft-web craft-sheets-web "$ROOT/sheets"
package slides/apps/deckcraft-web craft-slides-web "$ROOT/slides"
(cd "$DIST" && sha256sum "wordcraft-web-$VERSION.zip" "craft-sheets-web-$VERSION.zip" "craft-slides-web-$VERSION.zip" > SHA256SUMS.txt)
echo "wrote $DIST/SHA256SUMS.txt"
cat "$DIST/SHA256SUMS.txt"
