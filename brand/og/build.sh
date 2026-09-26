#!/usr/bin/env bash
# Renders the social preview card to passband-site/og.png.
#
#   ./brand/og/build.sh
#
# The trace is the live analyzer after a few seconds of animation, and its
# transient spikes are random, so every render differs slightly. Look at the
# result before shipping.
set -euo pipefail
cd "$(dirname "$0")"
site=../../passband-site

[[ -d node_modules ]] || bun install >/dev/null
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

bun build card.ts --outfile "$tmp/card.js" >/dev/null
cp card.html "$site/mark.svg" "$site/newsreader-var.woff2" "$tmp/"
bun run shoot.ts "$tmp" "$site/og.png"
echo "wrote passband-site/og.png"
