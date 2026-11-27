#!/bin/sh
# Renders app/icon/icon.svg into app/icon/wtf.icns, and icon.png for About. Needs librsvg (brew install librsvg).
set -eu

cd "$(dirname "$0")/../app/icon"

set=$(mktemp -d)/wtf.iconset
mkdir -p "$set"
for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" icon.svg -o "$set/icon_${size}x${size}.png"
    rsvg-convert -w $((size * 2)) -h $((size * 2)) icon.svg -o "$set/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$set" -o wtf.icns
cp "$set/icon_128x128@2x.png" icon.png
rm -rf "$(dirname "$set")"

echo "Built app/icon/wtf.icns and app/icon/icon.png"
