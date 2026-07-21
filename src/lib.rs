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

use zed_extension_api::{self as zed, Command, LanguageServerId, Result, Worktree};

// 改成你自己的 GitHub 仓库（owner/repo），发布时在该仓库打 release
// 并附带 hover-dict-ls-{target}.zip（见 dist-workspace.toml）。
// 当前为占位符，发布前必须替换，否则 Zed 会去错误仓库下载 LS 二进制。
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

/// 从 GitHub release 下载 LS 二进制（zip），返回解压后的可执行路径。
fn download_ls(language_server_id: &LanguageServerId) -> Result<PathBuf> {
    let release = zed::latest_github_release(
        LS_REPO,
        zed::GithubReleaseOptions {
            require_assets: true,
            pre_release: false,
        },
    )?;

    let triple = target_triple()?;
    // cargo-dist 0.30.2 的产物命名：<二进制名>-v<版本>-<target>.zip
    let asset_name = format!("hover-dict-ls-v{}-{triple}.zip", release.version);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("no asset found matching {asset_name:?}"))?;

    let version_dir = format!("hover-dict-ls-{}", release.version);
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

/// 本地开发回退：优先使用本地已编译的 LS 二进制，避免每次都从 GitHub 下载。
/// 查找顺序：
///   1. 环境变量 HOVER_DICT_LS_BIN（显式指定）
///   2. 当前目录下的 target/release 与 target/debug
///   3. 仓库根（CARGO_MANIFEST_DIR 不适用 wasm，故用 cwd 推断）
/// 找不到返回 None，由调用方回退到 GitHub release 下载。
fn local_ls_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HOVER_DICT_LS_BIN") {
        let pb = PathBuf::from(&p);
        if fs::metadata(&pb).is_ok_and(|s| s.is_file()) {
            return Some(pb);
        }
    }
    for dir in ["target/release", "target/debug"] {
        let pb = Path::new(dir).join(executable_name("hover-dict-ls"));
        if fs::metadata(&pb).is_ok_and(|s| s.is_file()) {
            return Some(pb);
        }
    }
    None
}

impl TranslateDictExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
    ) -> Result<PathBuf> {
        if let Some(path) = &self.cached_ls_binary_path {
            if fs::metadata(path).is_ok_and(|s| s.is_file()) {
                return Ok(path.clone());
            }
        }
        // 本地开发：优先用本地编译好的 LS 二进制
        if let Some(path) = local_ls_binary() {
            self.cached_ls_binary_path = Some(path.clone());
            return Ok(path);
        }
        let path = download_ls(language_server_id)?;
        self.cached_ls_binary_path = Some(path.clone());
        Ok(path)
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
        let ls_binary_path = self.language_server_binary_path(language_server_id)?;

        Ok(Command {
            command: ls_binary_path
                .to_str()
                .ok_or_else(|| "ls binary path is not valid utf-8".to_string())?
                .to_owned(),
            args: vec![],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(TranslateDictExtension);
