#!/bin/sh
# Builds the app and assembles build/wtf.app.
set -eu

cd "$(dirname "$0")/.."

cargo build --release -p wtf

# difftastic ships inside the app, built once per version and kept in build/.
difftastic=0.72.0
difft=build/difftastic-$difftastic
if [ ! -x "$difft/bin/difft" ]; then
    cargo install difftastic --version "$difftastic" --locked --root "$difft"
    strip "$difft/bin/difft"
    cp "$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/difftastic-$difftastic | head -1)/LICENSE" "$difft/LICENSE"
fi

app=build/wtf.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/wtf "$app/Contents/MacOS/wtf"
cp app/Info.plist "$app/Contents/Info.plist"
mkdir -p "$app/Contents/Resources"
cp app/icon/wtf.icns "$app/Contents/Resources/wtf.icns"
cp "$difft/bin/difft" "$app/Contents/MacOS/difft"
cp "$difft/LICENSE" "$app/Contents/Resources/difftastic-LICENSE"
codesign --force --sign - "$app"

echo "Built $app"
