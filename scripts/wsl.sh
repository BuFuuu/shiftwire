#!/usr/bin/env bash
# Build and run shiftWire inside WSL. Windows Smart App Control can block unsigned
# compilers and binaries, so all building happens in Linux.
#
# Usage (from Git Bash on Windows):
#   scripts/wsl.sh                   # sync + cargo build
#   scripts/wsl.sh cargo test        # sync + any command, run from the repo root in WSL
#   scripts/wsl.sh ui                # sync + npm install/build the UI, then cargo build
#
# Env: SHIFTWIRE_WSL_DISTRO (default kali-linux), SHIFTWIRE_WSL_DIR (default ~/shiftwire).
set -euo pipefail
export MSYS_NO_PATHCONV=1 # stop Git Bash rewriting /dev/null etc. into Windows paths

DISTRO="${SHIFTWIRE_WSL_DISTRO:-kali-linux}"
DEST="${SHIFTWIRE_WSL_DIR:-shiftwire}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Scripts go to WSL on stdin: wsl.exe mangles quotes in command-line arguments.
in_wsl() { wsl.exe -d "$DISTRO" -- bash -ls; }

# 1. Mirror the source tree, keeping WSL-side caches (target/, node_modules/, dist/) and lockfiles.
in_wsl <<EOF
mkdir -p ~/$DEST/ui && cd ~/$DEST
find . -mindepth 1 -maxdepth 1 ! -name target ! -name ui ! -name Cargo.lock -exec rm -rf {} +
find ui -mindepth 1 -maxdepth 1 ! -name node_modules ! -name dist ! -name package-lock.json -exec rm -rf {} +
EOF
tar -C "$ROOT" --exclude=./target --exclude=./ui/node_modules --exclude=./ui/dist --exclude=./.git -cf - . |
  wsl.exe -d "$DISTRO" --cd "~/$DEST" -- tar -xf -

# 2. Run the command.
case "${1:-build}" in
  build) CMD="cargo build" ;;
  ui) CMD="cd ui && npm install --no-fund --no-audit && npm run build && cd .. && cargo build" ;;
  *) CMD="$*" ;;
esac
status=0
printf 'cd ~/%s && %s\n' "$DEST" "$CMD" | in_wsl || status=$?

# 3. Bring back what tools change in place: formatted sources and lockfiles.
printf 'cd ~/%s && tar -cf - crates $(ls Cargo.lock ui/package-lock.json 2>/dev/null)\n' "$DEST" |
  in_wsl | tar -C "$ROOT" -xf -
exit $status
