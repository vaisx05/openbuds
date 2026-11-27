#!/usr/bin/env bash
# Sets the project version everywhere it lives, in one step:
#   buds-core, buds-cli (root workspace), buds-tray (Cargo + npm + Tauri).
# Usage: scripts/set-version.sh 0.3.0
set -euo pipefail

if [[ $# -ne 1 || ! $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 MAJOR.MINOR.PATCH   (e.g. $0 0.3.0)" >&2
  exit 1
fi
V="$1"
cd "$(dirname "$0")/.."

# First `version = "..."` in each Cargo manifest is the package version.
for f in buds-core/Cargo.toml buds-cli/Cargo.toml buds-tray/src-tauri/Cargo.toml; do
  perl -0pi -e 's/^version = "[^"]*"/version = "'"$V"'"/m' "$f"
done
# First "version" key in the JSON files is the app version.
for f in buds-tray/package.json buds-tray/src-tauri/tauri.conf.json; do
  perl -0pi -e 's/"version": "[^"]*"/"version": "'"$V"'"/' "$f"
done

# Refresh lockfiles so they match (workspace members only, no dep upgrades).
cargo update --workspace --quiet
(cd buds-tray/src-tauri && cargo update --workspace --quiet)
(cd buds-tray && npm install --package-lock-only --silent)

echo "Version set to $V."
echo "Next: add a CHANGELOG.md entry, commit, then:"
echo "  git tag v$V && git push --follow-tags"
