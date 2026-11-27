#!/usr/bin/env bash
# Builds the OpenBuds app bundle + DMG with local paths scrubbed.
#
# rustc embeds absolute source paths (panic locations etc.) in the binary,
# which would leak the builder's home directory and username. The remaps
# below rewrite them to neutral prefixes. Later --remap-path-prefix flags
# take precedence, so the generic $HOME rule comes first.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
RUSTUP_HOME_DIR="${RUSTUP_HOME:-$HOME/.rustup}"

export RUSTFLAGS="${RUSTFLAGS:-} \
  --remap-path-prefix=$HOME=/home \
  --remap-path-prefix=$RUSTUP_HOME_DIR=/rustup \
  --remap-path-prefix=$CARGO_HOME_DIR=/cargo \
  --remap-path-prefix=$ROOT=/openbuds"

# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

cd "$ROOT/buds-tray"
npm run tauri build

BIN="src-tauri/target/release/bundle/macos/OpenBuds.app/Contents/MacOS/buds-tray"
if strings -a "$BIN" | grep -q -F "$HOME"; then
  echo "WARNING: binary still contains $HOME - path remapping missed something" >&2
  exit 1
fi
echo "OK: no local paths found in the binary."
