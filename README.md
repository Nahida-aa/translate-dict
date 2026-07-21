# Hover Dict

Offline hover translation for [Zed](https://zed.dev). Hover over any code identifier and get its meaning in Chinese — no network required.

## Features

- **Identifier splitting**: `getUserProfile` → `get` + `user` + `profile`; also handles `snake_case`, `kebab-case`, `PascalCase`, abbreviation chains (`HTTPService` → `HTTP` + `Service`), and lowercase compound words (`redblacktree` → `red` + `black` + `tree`).
- **Built-in dictionary**: ~760k English words (674 JSON files) loaded entirely into memory at startup.
- **Two-way lookup**:
  - Hover English → Chinese translation.
  - Select Chinese text and hover → English candidates (reverse query).
- **Fully local**: works offline. No API calls, no telemetry.

## Install

### From the Zed extension store

Search for **Hover Dict** in Zed's extension panel and install. The extension downloads the matching language-server binary (with bundled dictionary) from the GitHub release on first run.

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

## License

[MIT](LICENSE) © 2026 Nahida-aa
