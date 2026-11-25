#!/bin/sh
# Points the wtf cask in a tap checkout at a new release. When the release
# starts a new line, the cask of the old one is kept as wtf@<line>, so
# `brew install --cask ryukzak/tap/wtf@0.1` still gets the last 0.1.x.
#
# Usage: scripts/bump-cask.sh <tap dir> <version> <sha256>
set -eu

[ $# -eq 3 ] || {
    echo "Usage: scripts/bump-cask.sh <tap dir> <version> <sha256>" >&2
    exit 1
}
cask="$1/Casks/wtf.rb"
version=$2
sha=$3

# A line is what users stay on: 0.1 before 1.0, as minors may break; 1 after.
line() {
    case $1 in
    0.*) echo "$1" | cut -d. -f1,2 ;;
    *) echo "$1" | cut -d. -f1 ;;
    esac
}

old=$(sed -n 's/^  version "\(.*\)"/\1/p' "$cask")
old_sha=$(sed -n 's/^  sha256 "\(.*\)"/\1/p' "$cask")
old_line=$(line "$old")

# A cask that was never released, with no checksum yet, is not worth keeping.
if [ "$old_line" != "$(line "$version")" ] && echo "$old_sha" | grep -Eq '^[0-9a-f]{64}$'; then
    sed -e "s/^cask \"wtf\" do/cask \"wtf@$old_line\" do/" \
        -e "s/^  depends_on formula: \"rust\"/  conflicts_with cask: \"wtf\"\\
  depends_on formula: \"rust\"/" \
        "$cask" >"$1/Casks/wtf@$old_line.rb"
    echo "Kept $old as wtf@$old_line"
fi

sed -e "s/^  version \".*\"/  version \"$version\"/" \
    -e "s/^  sha256 \".*\"/  sha256 \"$sha\"/" \
    "$cask" >"$cask.new"
mv "$cask.new" "$cask"
echo "wtf is $version"
