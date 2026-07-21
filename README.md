# Hover Dict

Offline hover translation for [Zed](https://zed.dev). Hover over any code identifier and get its meaning in Chinese — no network required.

## Features

- **Identifier splitting**: `getUserProfile` → `get` + `user` + `profile`; also handles `snake_case`, `kebab-case`, `PascalCase`, abbreviation chains (`HTTPService` → `HTTP` + `Service`), and lowercase compound words (`redblacktree` → `red` + `black` + `tree`).
- **Built-in dictionary**: ~760k English words (674 JSON files) loaded entirely into memory at startup.
- **Two-way lookup**:
  - Hover English → Chinese translation.
  - Select Chinese text and hover → English candidates (reverse query).
- **Fully local**: works offline. No API calls, no telemetry.

## Configuration

All settings are written under the `lsp.hover-dict.initialization_options` key in your Zed `settings.json` (this is the standard LSP configuration channel for Zed extensions — the extension has no in-UI settings panel):

```jsonc
// ~/.config/zed/settings.json
{
  "lsp": {
    "hover-dict": {
      "initialization_options": {
        "hover_dict.chinese_to_english_max_results": 10, // 1..50
        "hover_dict.default_translate_platform": "google",
        "hover_dict.custom_translate_url": ""        // used when platform = "custom"
      }
    }
  }
}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `hover_dict.chinese_to_english_max_results` | number | `10` | Max candidates returned for Chinese → English reverse query (clamped to 1..50). |
| `hover_dict.default_translate_platform` | string (enum) | `"google"` | Platform the word link jumps to. One of: `google`, `baidu`, `deepl`, `bing`, `yandex`, `custom`. |
| `hover_dict.custom_translate_url` | string | `""` | URL template used when `default_translate_platform` is `custom`. Use `{word}` as the placeholder (e.g. `https://fanyi.baidu.com/#en/zh/{word}`). |

> **Enabling / disabling per language**: use Zed's native `languages` setting instead of a built-in allow/deny list — e.g. to disable the hover translation for Markdown, add `"!hover-dict"` to `languages.Markdown.language_servers`. Changes to settings are picked up live (no restart needed).

## Install

### From the Zed extension store

Search for **Hover Dict** in Zed's extension panel and install. On first run the extension downloads the matching language-server binary for your platform from the GitHub release. **The ~760k-word dictionary is bundled inside the binary** — no extra download or setup needed, and it works fully offline.

### From GitHub release (manual)

Download the `hover-dict-ls-<version>-<your-platform>.zip` asset from the latest GitHub release, extract `hover-dict-ls`, and point the extension at it via the `HOVER_DICT_LS_BIN` environment variable (absolute path). Useful if you don't use the Zed extension store.

### Dev install (local development)

1. Install Rust via [rustup](https://rustup.rs) (needed to compile the WASM extension shell).
2. Add the WASM target:

   ```sh
   rustup target add wasm32-wasip1
   ```

3. Open the command palette (`ctrl/cmd+shift+p`) and run **`zed: install dev extension`**, then select this repository's root directory.
4. The Extensions page will show **"Overridden by dev extension"**.

After changing code, re-run `zed: install dev extension` to reload. To rebuild the language-server binary locally:

```sh
cargo build --release -p hover-dict-ls
```

The extension shell prefers a locally built LS binary (`target/release/hover-dict-ls` or the `HOVER_DICT_LS_BIN` env var) before falling back to the downloaded release binary.

## Project layout

```
extension.toml          # Zed extension manifest
src/lib.rs              # WASM extension shell (downloads / locates the LS binary)
crates/hover-dict-ls/   # The language server (tower-lsp)
  src/dict.rs           # Dictionary loading & lookup
  src/query.rs          # Word variant generation & dict query
  src/reverse_query.rs  # Chinese -> English reverse query
  src/utils/format.rs   # Identifier splitting (camelCase / snake_case / ...)
  src/main.rs           # LSP entry, hover handler, Markdown formatting
dict/                   # 674 built-in dictionary JSON files (aa.json .. zz.json)
```

## Development

```sh
# Build the extension shell (WASM)
cargo build --target wasm32-wasip1 --release

# Build the language server
cargo build --release -p hover-dict-ls

# Run unit tests
cargo test -p hover-dict-ls

# Run end-to-end tests (spawns the real LS binary over JSON-RPC)
cargo test -p hover-dict-ls --test e2e
```

## Dictionary

The built-in ~760k-word English dictionary is derived from **[ECDICT](https://github.com/skywind3000/ECDICT)** (skywind3000), fetched from its `ecdict.csv` source and split into per-prefix JSON files (`dict/aa.json` … `dict/zz.json`). Word fields map directly: `w` = word, `p` = phonetic, `t` = translation.

- **Dictionary data license**: ECDICT is released under **[CC BY-NC 4.0](https://creativecommons.org/licenses/by-nc/4.0/)** (Attribution-NonCommercial). This extension is free and open-source and does not commercialize the dictionary data.
- **Extension code license**: [MIT](LICENSE) © 2026 Nahida-aa (see below).

## License

[MIT](LICENSE) © 2026 Nahida-aa

## Publishing (上架 Zed 扩展商店)

Zed 扩展**不**支持作者自发布命令；所有扩展都经官方中央仓库审核分发。流程：

1. Fork [zed-industries/extensions](https://github.com/zed-industries/extensions)。
2. 在 fork 里把本仓库作为 submodule 加入（用 **main 分支最新 commit**，不要用 tag/detached commit）：

   ```sh
   git submodule add https://github.com/Nahida-aa/hover-dict.git extensions/hover-dict
   git add extensions/hover-dict
   ```

3. 在顶层 `extensions.toml` 追加（version 与 extension.toml 的 version 保持一致）：

   ```toml
   [hover-dict]
   submodule = "extensions/hover-dict"
   version = "0.1.0"
   ```

4. 运行 `pnpm sort-extensions` 排序后开 PR 到 `zed-industries/extensions`。
5. 合并后官方自动打包 `extension.wasm` 并发布到 Zed 扩展商店。

**前置约束（Zed 审核规则）**：
- 扩展 ID/名称不得含 `zed`/`extension`（本扩展 `hover-dict` 符合）。
- 语言服务器**不得**随扩展打包，必须运行时下载——本扩展从 GitHub release 拉取
  `hover-dict-ls-<version>-<target>.zip`（见 `src/lib.rs` 的 `download_ls`），符合。
- 仓库根须有被接受的 LICENSE（MIT，已具备）。

**LS 二进制如何就位**：打 `v*` tag 后，本仓库的 GitHub Actions（`.github/workflows/release.yml`，
由 cargo-dist 生成）会自动为 6 个平台编译 `hover-dict-ls` 并上传到对应 GitHub release，
扩展壳运行时按平台下载。

