#!/usr/bin/env bash
# 本地开发脚本（自用，非发布用）。
#
# 作用：一键完成 Zed 扩展的本地开发部署：
#   1. 编译扩展壳 (wasm32-wasip1)
#   2. 编译 LS 二进制 (hover-dict-ls)
#   3. 把 LS 二进制放到 Zed 扩展运行时的 cwd 下，让扩展壳能本地优先命中，
#      完全离线、不受 GitHub 匿名 API 限流影响。
#
# 关键事实（实测）：dev 模式下扩展壳 wasm 的 cwd =
#   ~/.local/share/zed/extensions/work/hover-dict/
# 这是唯一确定能被 wasm 的 fs::metadata 访问的目录；
# worktree 根 / 绝对路径在 wasm 裸 fs 下读不到。
# 所以 LS 二进制必须放在该 cwd 下的 hover-dict-ls-<版本>/ 里。
#
# 用法：
#   ./scripts/dev-install.sh            # 编译 wasm + LS，并安置 LS 二进制
#
# 之后在 Zed 里：
#   - 首次：zed: install dev extension → 选本仓库根目录
#   - 改代码后：zed: rebuild extensions（重编 wasm）
#   - 改 LS 后：重跑本脚本（刷新 cwd 下的 LS 二进制）→ zed: restart language server
# 注意：zed: install dev extension 重装时可能清空 work/hover-dict/，
#       届时重跑本脚本即可恢复 LS 二进制。

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PKG_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')"
LS_BIN_NAME="hover-dict-ls"
CACHE_REL="$LS_BIN_NAME-$PKG_VERSION"
WORK_DIR="${ZED_WORK_DIR:-$HOME/.local/share/zed/extensions/work/hover-dict}"
CACHE_DIR="$WORK_DIR/$CACHE_REL"

echo "==> 编译扩展壳 (wasm32-wasip1 --release)"
cargo build --target wasm32-wasip1 --release

echo "==> 编译 LS 二进制 (--release)"
cargo build --release -p hover-dict-ls

echo "==> 安置 LS 二进制到扩展运行时 cwd: $CACHE_DIR"
mkdir -p "$CACHE_DIR"
cp "target/release/$LS_BIN_NAME" "$CACHE_DIR/$LS_BIN_NAME"
chmod +x "$CACHE_DIR/$LS_BIN_NAME"

echo "==> 完成。"
echo "    扩展壳 wasm:    $(pwd)/extension.wasm"
echo "    LS 二进制(cwd): $CACHE_DIR/$LS_BIN_NAME"
echo "    dict 目录(LS 运行时 cwd=仓库根, 自动可访问): $(pwd)/dict"
echo ""
echo "    后续："
echo "    1) Zed 里 zed: install dev extension → 选 $(pwd)"
echo "    2) 改扩展壳后 zed: rebuild extensions"
echo "    3) 改 LS 后重跑本脚本 → zed: restart language server"
