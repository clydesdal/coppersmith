#!/bin/sh
# Renders the app's picture at each size the launcher asks for. The simpler
# drawing takes over at small sizes, where the fine detail would be mush.
# Only needed after changing icon.svg; the results are kept in assets/icons.
set -e
cd "$(dirname "$0")"
mkdir -p icons
for size in 512 256 128 64 48; do
    rsvg-convert -w "$size" -h "$size" icon.svg -o "icons/$size.png"
done
for size in 32 24 16; do
    rsvg-convert -w "$size" -h "$size" icon-small.svg -o "icons/$size.png"
done
