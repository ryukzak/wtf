#!/bin/sh
# Builds the app and installs it to /Applications (or $1).
set -eu

cd "$(dirname "$0")/.."

dest=${1:-/Applications}

scripts/build.sh
rm -rf "$dest/wtf.app"
cp -R build/wtf.app "$dest/wtf.app"

echo "Installed $dest/wtf.app"
