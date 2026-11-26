#!/usr/bin/env bash
# Retakes the README screenshots in .github/assets from the UI's browser fixtures.
# Every screen is shot twice, dark and light, for the README's <picture> tags.
#
#   cd apps/desktop && npm run dev    # in another terminal, serves the fixtures on :1420
#   tools/screenshots.sh
#
# Needs Google Chrome (headless) and `cwebp` (brew install webp).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/.github/assets"
URL="${PITLANE_DEV_URL:-http://localhost:1420}"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
PROFILE="$(mktemp -d)"
trap 'rm -rf "$PROFILE"' EXIT

curl -sf -o /dev/null "$URL" || { echo "Start the fixtures first: cd apps/desktop && npm run dev" >&2; exit 1; }
mkdir -p "$OUT"

# name:hash. The run id is the one in apps/desktop/src/dev/run-meta.json.
SHOTS=(
  "board:board"
  "inbox:inbox"
  "run:run/9-1795200919-2688"
  "review:review/4"
  "lanes:lanes"
  "terminal:terminal"
)

for shot in "${SHOTS[@]}"; do
  name="${shot%%:*}"
  hash="${shot#*:}"
  # Chrome's preferredColorScheme: 0 is dark, 1 is light.
  for scheme in 0:dark 1:light; do
    raw="$PROFILE/$name.png"
    file="$OUT/$name-${scheme#*:}.webp"
    "$CHROME" --headless=new --disable-gpu --hide-scrollbars \
      --user-data-dir="$PROFILE" --window-size=1600,1000 --force-device-scale-factor=2 \
      --virtual-time-budget=6000 --blink-settings=preferredColorScheme="${scheme%%:*}" \
      --screenshot="$raw" "$URL/#$hash" >/dev/null 2>&1
    # Lossless WebP at 2400px wide: sharp on a retina screen, half the size of a PNG.
    cwebp -quiet -lossless -z 9 -resize 2400 0 "$raw" -o "$file"
    echo "$file"
  done
done
