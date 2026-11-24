#!/bin/sh
# Assembles the project site in $1 (default: target/site) from site/, the screenshots in
# docs/ and the icons in assets/icons. Used by .github/workflows/pages.yml.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/target/site"}
rm -rf "$out"
mkdir -p "$out/img"
cp "$root/site/index.html" "$out/"
for icon in o-at o-nec o-nr o-aud-tx o-aud-rx o-aud-a; do
  cp "$root/assets/icons/$icon.png" "$out/img/$icon.png"
done
cp "$root/docs/screenshot.png" "$out/img/o-nec-editor.png"
cp "$root/docs/o-nr.png" "$out/img/o-nr-editor.png"
cp "$root/docs/o-aud-tx.png" "$out/img/o-aud-tx-plugin.png"
cp "$root/docs/o-aud-tx-app.png" "$out/img/o-aud-tx-app.png"
cp "$root/docs/o-aud-rx.png" "$out/img/o-aud-rx-editor.png"
for view in overview spectral spectrum sonogram stereo loudness compare 3d; do
  cp "$root/docs/o-aud-a/$view.png" "$out/img/o-aud-a-$view.png"
done
# Every image the page references must exist.
grep -o 'src="img/[^"]*"' "$out/index.html" | sed 's/src="//; s/"$//' | sort -u | while read -r image; do
  [ -f "$out/$image" ] || { echo "missing $image" >&2; exit 1; }
done
echo "site assembled in $out"
