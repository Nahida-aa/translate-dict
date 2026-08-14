//! translate-dict — Zed extension shell
//!
//! This extension does one thing: tell Zed where our translation language
//! server (`translate-dict-lsp`) binary lives and how to launch it.
//! All the actual LSP hover logic lives in the standalone Rust binary
//! `translate-dict-lsp` (see crates/translate-dict-lsp).
//!
//! Modeled after wakatime/zed-wakatime: the extension shell compiles to WASM
//! and only adapts `language_server_command`; the LS logic lives in an external
//! binary that is first downloaded from a GitHub release and cached.

use std::{
    fs,
    path::{Path, PathBuf},
};

use zed_extension_api::serde_json::{json, Value};
use zed_extension_api::{
    self as zed, settings::LspSettings, Command, LanguageServerId, Result, Worktree,
};

// When publishing to the Zed extension store, cut a release in this repo and
// attach translate-dict-lsp-<version>-<target>.zip for non-developers.
// During development (personal use) GitHub is not needed at all: a local
// binary is preferred, avoiding GitHub anonymous API rate limits.
const LS_REPO: &str = "Nahida-aa/translate-dict";

struct TranslateDictExtension {
    cached_ls_binary_path: Option<PathBuf>,
}

fn executable_name(binary: &str) -> String {
    match zed::current_platform() {
        (zed::Os::Windows, _) => format!("{binary}.exe"),
        _ => binary.to_string(),
    }
}

/// Build the GitHub release asset name (target triple) for the current platform.
/// Only used at release time (downloading from GitHub); not needed during development.
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

/// Fallback: download the LS binary (zip) from a GitHub release, returning the extracted executable path.
/// Only used at release time; during development `local_ls_binary` returns early.
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
            "No local translate-dict-lsp binary found, and failed to fetch it from GitHub ({e}). \
             [DIAG] cwd={cwd} pkg_ver={ver}",
            ver = env!("CARGO_PKG_VERSION"),
            e = e
        )
    })?;

    let triple = target_triple()?;
    // cargo-dist produces the asset translate-dict-lsp-<version>-<target>.zip
    // (no "v" prefix; version is release.tag minus the leading "v", e.g. 0.0.1).
    // The local extraction dir is translate-dict-lsp-<version>/ (aligned with CARGO_PKG_VERSION).
    let version = release
        .version
        .strip_prefix('v')
        .unwrap_or(&release.version);
    let asset_name = format!("translate-dict-lsp-{version}-{triple}.zip");
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("no asset found matching {asset_name:?}"))?;

    let version_dir = format!("translate-dict-lsp-{version}");
    let binary_path = Path::new(&version_dir).join(executable_name("translate-dict-lsp"));

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

/// In dev mode only look for the LS binary locally; if missing return Err and
/// embed cwd / worktree_root in the message for quick diagnosis from Zed's
/// error dialog / logs (0.7.0 has no log fn, so errors surface it).
///
/// Measured: the dev extension wasm cwd = ~/.local/share/zed/extensions/work/<id>/,
/// the only dir that wasm fs::metadata can reliably access. The worktree root /
/// absolute paths are unreadable under wasm's bare fs, so only cwd is searched.
/// The binary and dict/ are placed by scripts/dev-install.sh into
/// translate-dict-lsp-<version>/ under cwd.
fn local_ls_binary(worktree_root: &str) -> Result<PathBuf, String> {
    let exe = executable_name("translate-dict-lsp");
    let version_dir = format!("translate-dict-lsp-{}", env!("CARGO_PKG_VERSION"));

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "<unknown cwd>".to_string());

    // Under the wasm runtime cwd (the only reliably accessible directory)
    for dir in [version_dir.as_str(), "target/release", "target/debug"] {
        let pb = Path::new(&cwd).join(dir).join(&exe);
        if fs::metadata(&pb).is_ok_and(|s| s.is_file()) {
            return Ok(pb);
        }
    }

    Err(format!(
        "[translate-dict dev] No local language server binary found. Run scripts/dev-install.sh \
         to place it. cwd={cwd} worktree_root={worktree_root} exe={exe}"
    ))
}

impl TranslateDictExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree_root: &str,
    ) -> Result<PathBuf> {
        if let Some(path) = &self.cached_ls_binary_path {
            if fs::metadata(path).is_ok_and(|s| s.is_file()) {
                return Ok(path.clone());
            }
        }
        // Local first: return on hit, never touching GitHub (fully offline in dev)
        match local_ls_binary(worktree_root) {
            Ok(path) => {
                self.cached_ls_binary_path = Some(path.clone());
                return Ok(path);
            }
            Err(dev_err) => {
                // If no local binary (dev / user-placed) is found, fall back to
                // downloading a prebuilt LS binary from a GitHub release. The
                // release path goes through Zed's proxy, unaffected by the
                // api.github.com anonymous 60 req/h limit; the binary is cached
                // once in translate-dict-lsp-<ver>/, so hovers stay local afterwards.
                match download_ls(language_server_id) {
                    Ok(path) => {
                        self.cached_ls_binary_path = Some(path.clone());
                        return Ok(path);
                    }
                    Err(dl_err) => Err(format!(
                        "{dev_err}\n[translate-dict] No local binary found and GitHub download failed: {dl_err}"
                    )),
                }
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

    /// Merge defaults with the user's `lsp.translate-dict-lsp.initialization_options`
    /// from settings.json, passing them to the LS as LSP `initialize` initializationOptions.
    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<Value>> {
        let mut options = json!({
            "translate_dict_lsp.chinese_to_english_max_results": 10,
            "translate_dict_lsp.default_translate_platform": "google",
            "translate_dict_lsp.custom_translate_url": "",
        });

        if let Ok(lsp_settings) = LspSettings::for_worktree(language_server_id.as_ref(), worktree) {
            if let Some(user_opts) = lsp_settings.initialization_options {
                merge_json(user_opts, &mut options);
            }
        }

        Ok(Some(options))
    }

    /// Config hot-reload channel: the LS declares didChangeConfiguration, so
    /// after Zed settings change it queries this via workspace/configuration for the latest config.
    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<Value>> {
        let mut options = json!({
            "translate_dict_lsp.chinese_to_english_max_results": 10,
            "translate_dict_lsp.default_translate_platform": "google",
            "translate_dict_lsp.custom_translate_url": "",
        });

        if let Ok(lsp_settings) = LspSettings::for_worktree(language_server_id.as_ref(), worktree) {
            if let Some(user_opts) = lsp_settings.initialization_options {
                merge_json(user_opts, &mut options);
            }
        }

        Ok(Some(options))
    }
}

/// Deep-merge JSON (recurse objects, append arrays, overwrite scalars); ported from tsgo's merge_json_value_into
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
