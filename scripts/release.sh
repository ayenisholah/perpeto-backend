#!/usr/bin/env bash
set -euo pipefail

# Cut a SemVer release of the Perpeto backend workspace.
#
# Bumps the single shared [workspace.package] version, rolls the
# docs/CHANGELOG.md [Unreleased] section into a dated release section,
# commits, and creates an annotated vX.Y.Z tag. Pushing the tag triggers
# .github/workflows/RELEASE.yml, which publishes the GitHub Release.
#
# Usage: scripts/release.sh <patch|minor|major|X.Y.Z>

level="${1:?usage: scripts/release.sh <patch|minor|major|X.Y.Z>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

[ -z "$(git status --porcelain)" ] || { echo "working tree is dirty; commit or stash first" >&2; exit 1; }
branch="$(git rev-parse --abbrev-ref HEAD)"
[ "$branch" = "main" ] || { echo "release from main (currently on '$branch')" >&2; exit 1; }

current="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ -n "$current" ] || { echo "could not read [workspace.package] version from Cargo.toml" >&2; exit 1; }
IFS=. read -r major minor patch <<EOF
$current
EOF

case "$level" in
  major) next="$((major + 1)).0.0" ;;
  minor) next="${major}.$((minor + 1)).0" ;;
  patch) next="${major}.${minor}.$((patch + 1))" ;;
  [0-9]*.[0-9]*.[0-9]*) next="$level" ;;
  *) echo "invalid level: $level (expected patch|minor|major|X.Y.Z)" >&2; exit 1 ;;
esac

echo "releasing $current -> $next"

# 1. Bump the shared workspace version (all crates inherit version.workspace = true).
sed -i "s/^version = \"$current\"/version = \"$next\"/" Cargo.toml
# 2. Refresh Cargo.lock entries for the workspace members.
cargo update --workspace --offline >/dev/null 2>&1 || cargo check --workspace >/dev/null

# 3. Roll the changelog: rename [Unreleased] to the dated release and open a fresh [Unreleased].
today="$(date -u +%Y-%m-%d)"
awk -v ver="$next" -v date="$today" '
  !done && /^## \[Unreleased\]/ {
    print "## [Unreleased]"
    print ""
    print "## [" ver "] - " date
    done = 1
    next
  }
  { print }
' docs/CHANGELOG.md > docs/CHANGELOG.md.tmp && mv docs/CHANGELOG.md.tmp docs/CHANGELOG.md

# 4. Commit and tag.
git add Cargo.toml Cargo.lock docs/CHANGELOG.md
git commit -m "release: v$next"
git tag -a "v$next" -m "Perpeto backend v$next"

echo
echo "Created release commit and tag v$next."
echo "Publish it with:"
echo "  git push origin $branch && git push origin v$next"
