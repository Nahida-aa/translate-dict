# Translate Dict

Zed 编辑器的离线悬停翻译插件。悬停代码标识符即可获取中文释义，无需网络。

## 功能

- **标识符拆分**：`getUserProfile` → `get` + `user` + `profile`；支持 `snake_case`、`kebab-case`、`PascalCase`、缩写链（`HTTPService` → `HTTP` + `Service`）以及小写复合词（`redblacktree` → `red` + `black` + `tree`）。
- **内置词典**：约 76 万英文词条（674 个 JSON 文件），启动时一次性加载到内存。
- **双向查询**：
  - 悬停英文 → 中文释义。
  - 选中中文文本并悬停 → 英文候选词（反向查询）。
- **完全本地**：离线可用，无 API 调用，无遥测。

## 配置

所有设置写在 Zed `settings.json` 的 `lsp.translate-dict-lsp.initialization_options` 下（这是 Zed 扩展标准的 LSP 配置通道——本扩展没有独立的设置面板）：

```jsonc
// ~/.config/zed/settings.json
{
  "lsp": {
    "translate-dict-lsp": {
      "initialization_options": {
        "translate_dict_lsp.chinese_to_english_max_results": 10, // 1..50
        "translate_dict_lsp.default_translate_platform": "google",
        "translate_dict_lsp.custom_translate_url": ""        // 当 platform = "custom" 时使用
      }
    }
  }
}
```

| 键 | 类型 | 默认值 | 说明 |
|-----|------|---------|-------------|
| `translate_dict_lsp.chinese_to_english_max_results` | number | `10` | 中文反查返回的最大候选数（限制在 1..50）。 |
| `translate_dict_lsp.default_translate_platform` | string (enum) | `"google"` | 单词链接跳转的平台。可选：`google`、`baidu`、`deepl`、`bing`、`yandex`、`custom`。 |
| `translate_dict_lsp.custom_translate_url` | string | `""` | 当 `default_translate_platform` 为 `custom` 时的 URL 模板。用 `{word}` 作为占位符（如 `https://fanyi.baidu.com/#en/zh/{word}`）。 |

> **按语言启用/禁用**：使用 Zed 原生的 `languages` 设置而非内置白名单/黑名单。例如，要在 Markdown 中禁用悬停翻译，在 `languages.Markdown.language_servers` 中添加 `"!translate-dict-lsp"`。设置更改后实时生效（无需重启）。

## 安装

### 从 Zed 扩展商店安装

在 Zed 的扩展面板中搜索 **Translate Dict** 并安装。首次运行时，扩展会从 GitHub Release 下载匹配您平台的 language-server 二进制文件。**约 76 万词条内置于二进制文件中**——无需额外下载或配置，完全离线可用。

### 从 GitHub Release 手动安装

从最新的 GitHub Release 下载 `translate-dict-lsp-<version>-<your-platform>.zip`，解压出 `translate-dict-lsp` 即可。适用于不使用 Zed 扩展商店的情况。

### 开发安装（本地开发）

1. 通过 [rustup](https://rustup.rs) 安装 Rust（编译 WASM 扩展需要）。
2. 添加 WASM 目标：

   ```sh
   rustup target add wasm32-wasip1
   ```

3. 打开命令面板（`ctrl/cmd+shift+p`）运行 **`zed: install dev extension`**，选择本仓库根目录。
4. 扩展页面会显示 **"Overridden by dev extension"**。

修改代码后，重新运行 `zed: install dev extension` 即可重载。如需重新编译 language-server 二进制：

```sh
cargo build --release -p translate-dict-lsp
```

扩展优先使用本地编译的 LS 二进制文件（`target/release/translate-dict-lsp` 或 `scripts/dev-install.sh` 安置的二进制），找不到时才回退到下载的 Release 二进制。

## 项目结构

```
extension.toml          # Zed 扩展清单
src/lib.rs              # WASM 扩展外壳（下载/定位 LS 二进制）
crates/translate-dict-lsp/   # Language server（tower-lsp）
  src/dict.rs           # 词典加载与查询
  src/query.rs          # 单词变体生成与词典查询
  src/reverse_query.rs  # 中文 → 英文反向查询
  src/word.rs           # 取词（标识符边界 + 中文 FMM 分词）
  src/utils/format.rs   # 标识符拆分（camelCase / snake_case / ...）
  src/main.rs           # LSP 入口、hover 处理器、Markdown 格式化
dict/                   # 674 个内置词典 JSON 文件（aa.json .. zz.json）
```

## 开发

```sh
# 编译扩展外壳（WASM）
cargo build --target wasm32-wasip1 --release

# 编译 language server
cargo build --release -p translate-dict-lsp

# 运行单元测试
cargo test -p translate-dict-lsp

# 运行端到端测试（通过 JSON-RPC 启动真实 LS 二进制）
cargo test -p translate-dict-lsp --test e2e
```

## 词典

内置的约 76 万英文词库源自 **[ECDICT](https://github.com/skywind3000/ECDICT)**（skywind3000），从 `ecdict.csv` 提取并按首字母拆分为 JSON 文件（`dict/aa.json` … `dict/zz.json`）。词条字段：`w` = 单词，`p` = 音标，`t` = 翻译。

- **词典数据许可**：ECDICT 基于 **[CC BY-NC 4.0](https://creativecommons.org/licenses/by-nc/4.0/)**（署名-非商业性使用）。本扩展免费开源，未对词典数据做任何商业化使用。
- **扩展代码许可**：[MIT](LICENSE) © 2026 Nahida-aa（见下方）。

## 许可

[MIT](LICENSE) © 2026 Nahida-aa

## 贡献

参见 [CONTRIBUTING.md](CONTRIBUTING.md) 了解如何将本扩展发布到 Zed 扩展商店。
