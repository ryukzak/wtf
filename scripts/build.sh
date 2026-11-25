#!/bin/sh
# Builds the app and assembles build/wtf.app.
set -eu

cd "$(dirname "$0")/.."

cargo build --release --locked -p wtf

# The Homebrew cask builds with its own target folder.
target=${CARGO_TARGET_DIR:-target}
# app/Cargo.toml holds the version; the bundle gets it from there.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' app/Cargo.toml)

app=build/wtf.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$target/release/wtf" "$app/Contents/MacOS/wtf"
cp app/Info.plist "$app/Contents/Info.plist"
plist="$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleShortVersionString string $version" "$plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleVersion string $version" "$plist"
mkdir -p "$app/Contents/Resources"
cp app/icon/wtf.icns "$app/Contents/Resources/wtf.icns"
# The wtf command, for Homebrew to link onto the PATH.
cp scripts/wtf "$app/Contents/Resources/wtf"
codesign --force --sign - "$app"

echo "Built $app $version"
