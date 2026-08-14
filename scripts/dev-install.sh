#!/usr/bin/env bash
# Local development script (for personal use, not for publishing).
#
# Purpose: one-shot local deployment of the Zed extension:
#   1. Build the extension shell (wasm32-wasip1)
#   2. Build the language server binary (translate-dict-lsp)
#   3. Place the LS binary under the Zed extension runtime cwd so the shell
#      finds it locally first - fully offline, no GitHub rate-limit issues.
#
# Key fact (verified): in dev mode the extension shell wasm runs with cwd =
#   ~/.local/share/zed/extensions/work/<extension-id>/   (id = translate-dict-lsp)
# This is the only directory the wasm's fs::metadata can reliably access;
# the worktree root / absolute paths are not readable from raw wasm fs.
# So the LS binary must live at <cwd>/translate-dict-lsp-<version>/.
#
# Usage:
#   ./scripts/dev-install.sh            # build wasm + LS and install the LS binary
#
# Zed dev mode hot-reloads: re-running this script overwrites the binary and
# takes effect immediately - no need to manually run
# `zed: install dev extension` / rebuild extensions / restart language server.
# Note: reinstalling the dev extension may clear work/<extension-id>/;
# re-run this script afterwards to restore the LS binary.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PKG_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')"
LS_BIN_NAME="translate-dict-lsp"
EXT_ID="$(grep -m1 '^id' extension.toml | sed -E 's/id *= *"([^"]+)"/\1/')"
CACHE_REL="$LS_BIN_NAME-$PKG_VERSION"
WORK_DIR="${ZED_WORK_DIR:-$HOME/.local/share/zed/extensions/work/$EXT_ID}"
CACHE_DIR="$WORK_DIR/$CACHE_REL"

echo "==> Building extension shell (wasm32-wasip1 --release)"
cargo build --target wasm32-wasip1 --release

echo "==> Building LS binary (--release)"
cargo build --release -p translate-dict-lsp

# Note: the extension shell wasm must NOT be manually copied to
# extension.wasm - Zed needs the component format (wrapped automatically by
# `zed: rebuild extensions` / `install dev extension` from
# target/wasm32-wasip1/release/translate_dict.wasm). Manually copying the bare
# module causes "attempted to parse a wasm module with a component parser".

echo "==> Installing LS binary + dict directory to extension runtime cwd: $CACHE_DIR"
mkdir -p "$CACHE_DIR"

# Cannot overwrite the binary while the LS is running (text file busy); quit
# Zed or restart the language server first.
if pgrep -f "$CACHE_DIR/$LS_BIN_NAME" >/dev/null 2>&1; then
    echo "Error: translate-dict-lsp is running; cannot overwrite the binary (text file busy)." >&2
    echo "Run 'zed: restart language server' in Zed or quit Zed, then re-run this script." >&2
    exit 1
fi

cp "target/release/$LS_BIN_NAME" "$CACHE_DIR/$LS_BIN_NAME"
chmod +x "$CACHE_DIR/$LS_BIN_NAME"

# Also copy dict/ next to the binary so the dictionary is found regardless of
# the LS cwd (dict_dir() prefers a dict/ directory next to the binary).
echo "==> Copying dict/ directory to $CACHE_DIR/dict"
rm -rf "$CACHE_DIR/dict"
cp -r "$ROOT/dict" "$CACHE_DIR/dict"

echo "==> Done."
echo "    Extension shell wasm (module): target/wasm32-wasip1/release/translate_dict.wasm"
echo "    LS binary:                     $CACHE_DIR/$LS_BIN_NAME"
echo "    dict directory:                $CACHE_DIR/dict"
echo "    (copied next to the LS binary; works in any workspace)"
echo ""
echo "    Notes:"
echo "    - After changing LS logic: re-run this script to overwrite the binary"
echo "      (Zed will pick up the new LS automatically)."
echo "    - After changing the dict: re-run this script to re-copy dict/."
echo "    - After changing the extension shell (src/lib.rs): run"
echo "      'zed: rebuild extensions' in Zed afterwards so it rewraps the wasm as"
echo "      a component and loads it. Never manually copy the raw wasm to"
echo "      extension.wasm (it will fail to load)."
