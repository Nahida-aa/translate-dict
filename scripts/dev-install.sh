#!/usr/bin/env bash
# 本地开发安装脚本（自用，非发布用）。
#
# 作用：编译扩展壳 wasm，并同步到 Zed 的本地扩展目录，
# 这样改完代码跑一条命令就能在 Zed 里看到效果，无需发布 release。
#
# 用法：
#   ./scripts/dev-install.sh          # 编译并安装到 Zed 扩展目录
#   ./scripts/dev-install.sh --ls     # 同时编译 LS 二进制（供本地回退使用）
#
# 之后重启 Zed 即可生效。

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ZED_EXT_DIR="${ZED_EXT_DIR:-$HOME/.local/share/zed/extensions/installed}"
DEST="$ZED_EXT_DIR/hover-dict"

echo "==> 编译扩展壳 (wasm32-wasip1 --release)"
cargo build --target wasm32-wasip1 --release

echo "==> 同步到 $DEST"
mkdir -p "$DEST"
cp extension.toml "$DEST/extension.toml"
cp "target/wasm32-wasip1/release/hover_dict.wasm" "$DEST/extension.wasm"

if [[ "${1:-}" == "--ls" ]]; then
    echo "==> 编译 LS 二进制 (本地回退用)"
    cargo build -p hover-dict-ls
    echo "    LS 二进制: target/debug/hover-dict-ls"
fi

echo "==> 完成。重启 Zed 后生效。"
echo "    本地 LS 回退: lib.rs 会优先使用 target/{debug,release}/hover-dict-ls"
