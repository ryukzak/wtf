#!/bin/sh
# Builds the app and assembles build/wtf.app.
set -eu

cd "$(dirname "$0")/.."

cargo build --release -p wtf

app=build/wtf.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/wtf "$app/Contents/MacOS/wtf"
cp app/Info.plist "$app/Contents/Info.plist"
mkdir -p "$app/Contents/Resources"
cp app/icon/wtf.icns "$app/Contents/Resources/wtf.icns"
codesign --force --sign - "$app"

echo "Built $app"
