#!/bin/sh
# Releases wtf from a clean, pushed master that passes CI's checks: asks,
# then sets the version, commits, tags and pushes. Saying no changes nothing.
# The tag starts the Release workflow, which also bumps the Homebrew cask.
#
# Usage: scripts/release.sh <major|minor|patch|X.Y.Z>
set -eu

cd "$(dirname "$0")/.."

die() {
    echo "$*" >&2
    exit 1
}

[ $# -eq 1 ] || die "Usage: scripts/release.sh <major|minor|patch|X.Y.Z>"

# app/Cargo.toml holds the version; the bundle and the workflow read it there.
current=$(sed -n 's/^version = "\(.*\)"/\1/p' app/Cargo.toml)
IFS=. read -r major minor patch <<EOF
$current
EOF

case $1 in
major) version="$((major + 1)).0.0" ;;
minor) version="$major.$((minor + 1)).0" ;;
patch) version="$major.$minor.$((patch + 1))" ;;
*)
    echo "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' ||
        die "Not a version or major, minor, patch: $1"
    version=$1
    ;;
esac
tag="v$version"

[ "$(git branch --show-current)" = master ] || die "Release from master."
[ -z "$(git status --porcelain)" ] || die "Commit or stash your changes first."
git fetch -q origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/master)" ] ||
    die "master and origin/master differ; pull or push first."
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "$tag exists."
git ls-remote --exit-code --tags origin "$tag" >/dev/null && die "$tag exists on origin."

# What CI runs, before anything changes.
echo "Checking format, lints and tests"
cargo fmt --all --check || die "Format is off; run cargo fmt --all."
cargo clippy -q --workspace --all-targets --locked -- -D warnings || die "Clippy failed."
cargo test -q --workspace --locked || die "Tests failed."

if [ "$version" = "$current" ]; then
    # Such as the first release: the version is already set.
    printf "Release %s as it is" "$version"
else
    printf "Release %s -> %s" "$current" "$version"
fi
printf ": commit, tag %s and push, starting the release? [y/N] " "$tag"
read -r answer
case $answer in
y | Y) ;;
*) die "Nothing changed." ;;
esac

if [ "$version" != "$current" ]; then
    sed "s/^version = \"$current\"/version = \"$version\"/" app/Cargo.toml >app/Cargo.toml.new
    mv app/Cargo.toml.new app/Cargo.toml
    # The build runs with --locked, so the lock file must name the new version.
    cargo update -q --workspace
    git commit -q -m "Release $version" app/Cargo.toml Cargo.lock
fi
git tag "$tag"
git push -q origin master "$tag"
echo "Pushed $tag. Follow it with: gh run watch"
