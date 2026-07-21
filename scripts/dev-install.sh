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
# Zed dev 模式会热加载：重跑本脚本覆盖二进制后即生效，
# 无需手动 zed: install dev extension / rebuild extensions / restart language server。
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

# 注意：扩展壳 wasm 不能手动 cp 到 extension.wasm——Zed 需要的是
# component 格式（由 zed: rebuild extensions / install dev extension 自动
# 从 target/wasm32-wasip1/release/hover_dict.wasm 包装生成）。手动 cp 裸
# module 会导致 "attempted to parse a wasm module with a component parser" 错误。

echo "==> 安置 LS 二进制到扩展运行时 cwd: $CACHE_DIR"
mkdir -p "$CACHE_DIR"

# LS 正在运行时无法覆盖二进制（text file busy），需先退出 Zed 或重启语言服务器
if pgrep -f "$CACHE_DIR/$LS_BIN_NAME" >/dev/null 2>&1; then
    echo "错误：检测到 hover-dict-ls 正在运行，无法覆盖二进制（text file busy）。" >&2
    echo "请先在 Zed 中执行 'zed: restart language server' 或退出 Zed，再重跑本脚本。" >&2
    exit 1
fi

cp "target/release/$LS_BIN_NAME" "$CACHE_DIR/$LS_BIN_NAME"
chmod +x "$CACHE_DIR/$LS_BIN_NAME"

echo "==> 完成。"
echo "    扩展壳 wasm(module): target/wasm32-wasip1/release/hover_dict.wasm"
echo "    LS 二进制(cwd):      $CACHE_DIR/$LS_BIN_NAME"
echo "    dict 目录(LS 运行时 cwd=仓库根, 自动可访问): $(pwd)/dict"
echo ""
echo "    说明："
echo "    - 改 LS 逻辑后：重跑本脚本覆盖二进制即生效（Zed 会自动重拉 LS）。"
echo "    - 改扩展壳(src/lib.rs)后：必须在本脚本完成后，于 Zed 中执行"
echo "      'zed: rebuild extensions'，让 Zed 把 wasm 重新包装为 component 并加载。"
echo "      切勿手动 cp 裸 wasm 到 extension.wasm（会导致加载失败）。"
