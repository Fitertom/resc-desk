#!/bin/sh
# A release for the in-system updater (update.rs): the program built for x86_64 Linux and
# its SHA-256, tagged r<commit time>-<commit>, notes from the commits since the last one.
# Run on Windows (Git Bash) with the Arch WSL distro and an authorised gh; the tree must be
# committed and pushed.
set -eu
cd "$(dirname "$0")/.."
DISTRO=${DISTRO:-archlinux}
if [ -n "$(git status --porcelain)" ]; then
    echo "uncommitted changes: commit and push first" >&2
    exit 1
fi
VER="r$(git log -1 --format=%ct-%h)"
SHA=$(git rev-parse HEAD)
if gh release view "$VER" >/dev/null 2>&1; then
    echo "$VER is out already"
    exit 0
fi
WIN=$(pwd -W 2>/dev/null || pwd)
SRC="/mnt/$(echo "$WIN" | sed -E 's|^([A-Za-z]):|\L\1|; s|\|/|g')"
echo "building $VER in $DISTRO ($SRC)"
wsl -d "$DISTRO" -- bash -lc "cd '$SRC' && RESC_BUILD_OVERRIDE=$VER CARGO_TARGET_DIR=\$HOME/target cargo build --release -q -p desk && \$HOME/target/release/resc-desk version && mkdir -p /tmp/resc-release && cp \$HOME/target/release/resc-desk /tmp/resc-release/resc-desk-x86_64 && cd /tmp/resc-release && sha256sum resc-desk-x86_64 > resc-desk-x86_64.sha256 && cat resc-desk-x86_64.sha256"
OUT=$(mktemp -d)
wsl -d "$DISTRO" -- sh -c "cat /tmp/resc-release/resc-desk-x86_64" > "$OUT/resc-desk-x86_64"
wsl -d "$DISTRO" -- sh -c "cat /tmp/resc-release/resc-desk-x86_64.sha256" > "$OUT/resc-desk-x86_64.sha256"
# The copy must be the very file that was hashed.
WANT=$(cut -d' ' -f1 "$OUT/resc-desk-x86_64.sha256")
GOT=$(sha256sum "$OUT/resc-desk-x86_64" | cut -d' ' -f1)
[ "$WANT" = "$GOT" ] || { echo "copy differs from the build" >&2; exit 1; }
PREV=$(gh release list -L 1 --json tagName -q '.[0].tagName' 2>/dev/null || true)
PREV_SHA=""
if [ -n "$PREV" ]; then
    PREV_SHA=$(git rev-list -n 1 "$PREV" 2>/dev/null || gh api "repos/{owner}/{repo}/commits/$PREV" -q .sha 2>/dev/null || true)
fi
if [ -n "$PREV_SHA" ]; then
    NOTES=$(git log --format='- %s' "$PREV_SHA..$SHA")
else
    NOTES=$(git log -10 --format='- %s')
fi
gh release create "$VER" "$OUT/resc-desk-x86_64" "$OUT/resc-desk-x86_64.sha256" --target "$SHA" --title "$VER" --notes "$NOTES"
rm -rf "$OUT"
echo "released $VER"
