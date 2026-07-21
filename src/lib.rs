//! hover-dict — Zed 扩展壳
//!
//! 本扩展只负责一件事：告诉 Zed 我们的翻译语言服务器
//! (`hover-dict-ls`) 二进制在哪里、如何启动。
//! 真正的 LSP hover 逻辑全部在 `hover-dict-ls` 这个独立 Rust
//! 二进制里实现（见 crates/hover-dict-ls）。
//!
//! 设计参照 wakatime/zed-wakatime：扩展壳以 WASM 编译，仅做
//! `language_server_command` 适配；LS 逻辑放在外部二进制，首次从
//! GitHub release 下载并缓存路径。

use std::{
    fs,
    path::{Path, PathBuf},
};

use zed_extension_api::serde_json::{json, Value};
use zed_extension_api::{
    self as zed, settings::LspSettings, Command, LanguageServerId, Result, Worktree,
};

// 发布到 Zed 扩展商店时，在此仓库打 release 并附带
// hover-dict-ls-<版本>-<target>.zip，供非开发用户下载。
// 开发期（自用）完全不需要 GitHub：本地已编译的二进制会被优先使用，
// 因此不受 GitHub 匿名 API 限流影响。
const LS_REPO: &str = "Nahida-aa/hover-dict";

struct TranslateDictExtension {
    cached_ls_binary_path: Option<PathBuf>,
}

fn executable_name(binary: &str) -> String {
    match zed::current_platform() {
        (zed::Os::Windows, _) => format!("{binary}.exe"),
        _ => binary.to_string(),
    }
}

/// 根据当前平台拼出 GitHub release 的 asset 名（target triple）。
/// 仅发布期（从 GitHub 下载）使用；开发期不依赖。
#[allow(dead_code)]
fn target_triple() -> Result<String, String> {
    let (platform, arch) = zed::current_platform();
    let arch = match arch {
        zed::Architecture::Aarch64 => "aarch64",
        zed::Architecture::X8664 => "x86_64",
        _ => return Err(format!("unsupported architecture: {arch:?}")),
    };
    let os = match platform {
        zed::Os::Mac => "apple-darwin",
        zed::Os::Linux => "unknown-linux-gnu",
        zed::Os::Windows => "pc-windows-msvc",
    };
    Ok(format!("{arch}-{os}"))
}

/// 兜底：从 GitHub release 下载 LS 二进制（zip），返回解压后的可执行路径。
/// 仅发布期使用；开发期 `local_ls_binary` 命中本地即返回，不会走到这里。
#[allow(dead_code)]
fn download_ls(language_server_id: &LanguageServerId) -> Result<PathBuf> {
    let release = zed::latest_github_release(
        LS_REPO,
        zed::GithubReleaseOptions {
            require_assets: true,
            pre_release: false,
        },
    )
    .map_err(|e| {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "<unknown cwd>".to_string());
        format!(
            "本地未找到 hover-dict-ls 二进制，且无法从 GitHub 获取（{e}）。\
             [DIAG] cwd={cwd} pkg_ver={ver}",
            ver = env!("CARGO_PKG_VERSION"),
            e = e
        )
    })?;

    let triple = target_triple()?;
    // GitHub release 版本号自带 "v" 前缀（如 v0.0.1）。
    // 我们发布的 asset 命名为 hover-dict-ls-v<版本>-<target>.zip（保留 v），
    // 而本地解压目录命名为 hover-dict-ls-<版本>/（不带 v，与 CARGO_PKG_VERSION 对齐）。
    let version = release
        .version
        .strip_prefix('v')
        .unwrap_or(&release.version);
    let asset_name = format!("hover-dict-ls-v{version}-{triple}.zip");
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("no asset found matching {asset_name:?}"))?;

    let version_dir = format!("hover-dict-ls-{version}");
    let binary_path = Path::new(&version_dir).join(executable_name("hover-dict-ls"));

    if !fs::metadata(&binary_path).is_ok_and(|s| s.is_file()) {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::Downloading,
        );
        zed::download_file(
            &asset.download_url,
            &version_dir,
            zed::DownloadedFileType::Zip,
        )
        .map_err(|e| format!("failed to download file: {e}"))?;
    }

    zed::make_file_executable(
        binary_path
            .to_str()
            .ok_or_else(|| "binary path is not valid utf-8".to_string())?,
    )?;

    Ok(binary_path)
}

/// 开发模式下只在本地找 LS 二进制；找不到时返回 Err，并把 cwd /
/// worktree_root 拼进错误信息，方便从 Zed 报错弹窗 / 日志快速定位
/// （0.7.0 无 log 函数，借错误暴露）。
///
/// 实测：dev 扩展 wasm 的 cwd = ~/.local/share/zed/extensions/work/<id>/，
/// 这是唯一确定能被 wasm 的 fs::metadata 访问的目录。worktree 根 /
/// 绝对路径在 wasm 裸 fs 下读不到，故实际只搜 cwd（与 HOVER_DICT_LS_BIN
/// 环境变量）。二进制由 scripts/dev-install.sh 安置到 cwd 下的
/// hover-dict-ls-<版本>/ 中。
fn local_ls_binary(worktree_root: &str) -> Result<PathBuf, String> {
    let exe = executable_name("hover-dict-ls");
    let version_dir = format!("hover-dict-ls-{}", env!("CARGO_PKG_VERSION"));

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "<unknown cwd>".to_string());

    // 1. 显式环境变量（最高优先级，绝对路径）
    if let Ok(p) = std::env::var("HOVER_DICT_LS_BIN") {
        let pb = PathBuf::from(&p);
        if fs::metadata(&pb).is_ok_and(|s| s.is_file()) {
            return Ok(pb);
        }
    }

    // 2. wasm 运行时 cwd 下（唯一确定可访问的目录）
    for dir in [version_dir.as_str(), "target/release", "target/debug"] {
        let pb = Path::new(&cwd).join(dir).join(&exe);
        if fs::metadata(&pb).is_ok_and(|s| s.is_file()) {
            return Ok(pb);
        }
    }

    Err(format!(
        "[hover-dict dev] 本地未找到 LS 二进制。请运行 scripts/dev-install.sh 安置 LS 二进制。\
         cwd={cwd} worktree_root={worktree_root} exe={exe}"
    ))
}

impl TranslateDictExtension {
    fn language_server_binary_path(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree_root: &str,
    ) -> Result<PathBuf> {
        if let Some(path) = &self.cached_ls_binary_path {
            if fs::metadata(path).is_ok_and(|s| s.is_file()) {
                return Ok(path.clone());
            }
        }
        // 本地优先：命中即返回，不再触碰 GitHub（开发期完全离线）
        match local_ls_binary(worktree_root) {
            Ok(path) => {
                self.cached_ls_binary_path = Some(path.clone());
                return Ok(path);
            }
            Err(dev_err) => {
                // 开发期：本地找不到就直接把诊断错误抛出（含尝试过的路径），
                // 不再走 GitHub，避免匿名 API 限流。发布时如需回退下载，
                // 把下面这行替换为 download_ls(language_server_id)? 即可。
                return Err(dev_err);
            }
        }
    }
}

impl zed::Extension for TranslateDictExtension {
    fn new() -> Self {
        Self {
            cached_ls_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        let worktree_root = worktree.root_path();
        let ls_binary_path =
            self.language_server_binary_path(language_server_id, &worktree_root)?;

        Ok(Command {
            command: ls_binary_path
                .to_str()
                .ok_or_else(|| "ls binary path is not valid utf-8".to_string())?
                .to_owned(),
            args: vec![],
            env: worktree.shell_env(),
        })
    }

    /// 把默认配置与用户在 settings.json 写的 `lsp.hover-dict.initialization_options`
    /// 合并，作为 LSP `initialize` 的 initializationOptions 传给 LS。
    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<Value>> {
        let mut options = json!({
            "hover_dict.chinese_to_english_max_results": 10,
            "hover_dict.default_translate_platform": "google",
            "hover_dict.custom_translate_url": "",
        });

        if let Ok(lsp_settings) = LspSettings::for_worktree(language_server_id.as_ref(), worktree) {
            if let Some(user_opts) = lsp_settings.initialization_options {
                merge_json(user_opts, &mut options);
            }
        }

        Ok(Some(options))
    }

    /// 配置热更新通道：LS 声明了 didChangeConfiguration，Zed 改配置后会
    /// 通过 workspace/configuration 请求这里，返回最新配置。
    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<Value>> {
        let mut options = json!({
            "hover_dict.chinese_to_english_max_results": 10,
            "hover_dict.default_translate_platform": "google",
            "hover_dict.custom_translate_url": "",
        });

        if let Ok(lsp_settings) = LspSettings::for_worktree(language_server_id.as_ref(), worktree) {
            if let Some(user_opts) = lsp_settings.initialization_options {
                merge_json(user_opts, &mut options);
            }
        }

        Ok(Some(options))
    }
}

/// 深度合并 JSON（对象递归、数组追加、标量覆盖），参考 tsgo 的 merge_json_value_into
fn merge_json(source: Value, target: &mut Value) {
    match (source, target) {
        (Value::Object(src), Value::Object(tgt)) => {
            for (k, v) in src {
                if let Some(t) = tgt.get_mut(&k) {
                    merge_json(v, t);
                } else {
                    tgt.insert(k, v);
                }
            }
        }
        (Value::Array(src), Value::Array(tgt)) => {
            for v in src {
                tgt.push(v);
            }
        }
        (src, tgt) => *tgt = src,
    }
}

zed::register_extension!(TranslateDictExtension);
