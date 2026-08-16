//! `lkit self`:管理 lkit 自身——安装/升级/移除全局常驻 daemon。
//!
//! lkit 自身与 landscape 安装完全解耦:daemon 固定读取 lkit 地盘,不绑定任何
//! landscape 根,`self remove` 不要求 landscape 已卸载。`self` 命令都不接收
//! `--install-dir`,不创建业务事务、不创建保护备份,退出码只定义 `0/1/2`。
//!
//! `self install` 把 lkit 注册为全局常驻服务(unit 原件
//! `/usr/local/lib/lkit/lkit.service`,`ExecStart=/usr/local/bin/lkit daemon`,
//! 注册链接指向全局原件),重复执行时 restart 刷新;`self upgrade` 从 GitHub
//! Release 下载对应架构二进制与 `SHA256SUMS` 校验,`lkit --version` 自检后
//! 原子替换 `/usr/local/bin/lkit`;`self remove` 停止、注销并删除全局原件,
//! 幂等可重复,不删除 CLI 二进制。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Args, Subcommand};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use semver::Version;
use serde::Deserialize;
use url::Url;

use crate::deployment::lock;
use crate::deployment::plan::InstallError;
use crate::deployment::runtime::InstallRuntime;
use crate::release::repository::Architecture;
use crate::release::repository::download::{DownloadClient, validate_network_url};
use crate::release::repository::{Asset, AssetEncoding, RepositoryError};
use crate::service::manager::{Availability, ManagedService, ServiceManager, SystemRegistration};
use crate::service::systemd::{self, Systemd};

/// lkit 自身的 GitHub 仓库(与 landscape-webserver 仓库不同)。
pub(crate) const LKIT_REPOSITORY: &str = "landscape-router/landscape-kit";
/// CLI 二进制固定位置(由 install.sh 安装,self 命令只读不删)。
pub(crate) const LKIT_BINARY: &str = "/usr/local/bin/lkit";
/// 全局 unit 原件目录。
pub(crate) const LKIT_UNIT_ORIGIN_DIR: &str = "/usr/local/lib/lkit";

/// 测试钩子:环境变量 `LKIT_GLOBAL_DIR` 存在时把 `/usr/local` 重映射到该目录
/// (fixture 世界用,文档不公开),否则用真实的全局位置。
fn global_dir() -> Option<&'static Path> {
    let value = std::env::var("LKIT_GLOBAL_DIR").ok()?;
    if value.is_empty() {
        return None;
    }
    Some(Path::new(Box::leak(value.into_boxed_str())))
}

/// CLI 二进制固定位置(由 install.sh 安装,self 命令只读不删)。
fn lkit_binary() -> PathBuf {
    match global_dir() {
        Some(dir) => dir.join("bin/lkit"),
        None => PathBuf::from(LKIT_BINARY),
    }
}

/// 全局 unit 原件目录。
fn unit_origin_dir() -> PathBuf {
    match global_dir() {
        Some(dir) => dir.join("lib/lkit"),
        None => PathBuf::from(LKIT_UNIT_ORIGIN_DIR),
    }
}

const GITHUB_API_ROOT: &str = "https://api.github.com";
const RELEASES_DOWNLOAD_ROOT: &str =
    "https://github.com/landscape-router/landscape-kit/releases/download";

#[derive(Debug, Args)]
pub struct SelfCommand {
    #[command(subcommand)]
    pub action: SelfAction,
}

#[derive(Debug, Subcommand)]
pub enum SelfAction {
    /// 注册全局常驻 daemon 并启动
    Install(SelfArgs),
    /// 升级 /usr/local/bin/lkit 与常驻 daemon
    Upgrade(UpgradeArgs),
    /// 停止、注销并删除常驻 daemon(幂等)
    Remove(SelfArgs),
}

#[derive(Debug, Args)]
pub struct SelfArgs {
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// 目标版本 tag,如 v0.2.0-rc.1;缺省为 GitHub 最新 stable
    #[arg(long, value_name = "TAG")]
    pub version: Option<String>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &SelfCommand) -> ExitCode {
    match run_inner(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("self: {error}");
            exit_code(&error)
        }
    }
}

fn exit_code(error: &InstallError) -> ExitCode {
    match error {
        // 参数错误与「请求 systemd 但环境不可用」均属于使用错误。
        InstallError::ParameterUsage(_)
        | InstallError::Systemd(_)
        | InstallError::UnsupportedPlatform(_) => ExitCode::from(2),
        _ => ExitCode::FAILURE,
    }
}

async fn run_inner(args: &SelfCommand) -> Result<(), InstallError> {
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(InstallError::UnsupportedPlatform(
            "self commands require root".into(),
        ));
    }
    let _lock = lock::acquire_install_lock()?;
    match &args.action {
        SelfAction::Install(_) => install(&runtime),
        SelfAction::Upgrade(args) => upgrade(&runtime, args).await,
        SelfAction::Remove(_) => remove(&runtime),
    }
}

/// `self` 固定使用 systemd:unit 原件、注册链接、MainPID 校验都是 systemd 语义。
fn require_systemd(runtime: &InstallRuntime) -> Result<&Systemd, InstallError> {
    let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
    match systemd.probe() {
        Availability::Available { .. } => Ok(systemd),
        _ => Err(InstallError::UnsupportedPlatform(
            "lkit self requires systemd on this host".into(),
        )),
    }
}

fn install(runtime: &InstallRuntime) -> Result<(), InstallError> {
    let systemd = require_systemd(runtime)?;
    let service = ManagedService::LkitDaemon;
    let binary = lkit_binary();
    if !is_executable(&binary) {
        return Err(InstallError::ParameterUsage(format!(
            "{} is missing or not executable; install lkit through install.sh first",
            binary.display()
        )));
    }
    let origin_dir = unit_origin_dir();
    std::fs::create_dir_all(&origin_dir).map_err(InstallError::Io)?;
    let origin = origin_dir.join(systemd.service_name(service));
    let content = systemd.render_definition(service, &binary)?;
    systemd.validate_definition(service, &content, &binary)?;
    write_unit_origin(&origin, &content)?;
    let result = (|| -> Result<(), InstallError> {
        systemd.register(service, &origin)?;
        systemd.enable(service)?;
        if systemd.is_active(service)? {
            // 旧 daemon 仍在运行:重启使其加载当前二进制。
            systemd.restart(service)?;
        } else {
            systemd.start(service)?;
        }
        let pid = systemd.main_pid(service)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "lkit daemon did not produce a main pid after start".into(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        // 注册/启动失败:尽力恢复现场,避免遗留已启用但未运行的注册服务。
        cleanup_partial_install(systemd, service, &origin);
        return Err(error);
    }
    println!("self: {}", crate::tr!(crate::keys::SELF_INSTALLED));
    Ok(())
}

/// 注册/启动失败后的尽力清理:停止、注销并删除定义原件。
fn cleanup_partial_install(systemd: &Systemd, service: ManagedService, origin: &Path) {
    let _ = systemd.stop(service);
    if systemd.is_enabled(service).unwrap_or(false) {
        let _ = systemd.disable(service);
    }
    let _ = systemd.unregister(service, origin);
    let _ = systemd.refresh();
    let _ = remove_file_if_present(origin);
}

fn remove(runtime: &InstallRuntime) -> Result<(), InstallError> {
    let systemd = require_systemd(runtime)?;
    let service = ManagedService::LkitDaemon;
    if systemd.is_active(service)? {
        systemd.stop_and_wait(
            service,
            &(|| {
                systemd
                    .active_state(service)
                    .map(|value| value != "active")
                    .unwrap_or(true)
            }),
        )?;
    }
    if systemd.is_enabled(service).unwrap_or(false) {
        let _ = systemd.disable(service);
    }
    let origin = unit_origin_dir().join(systemd.service_name(service));
    if let Err(error) = systemd.unregister(service, &origin) {
        eprintln!("self: {error}");
    }
    remove_file_if_present(&origin)?;
    remove_empty_dir_if_present(&unit_origin_dir())?;
    println!("self: {}", crate::tr!(crate::keys::SELF_REMOVED));
    Ok(())
}

async fn upgrade(runtime: &InstallRuntime, args: &UpgradeArgs) -> Result<(), InstallError> {
    let architecture = Architecture::host().ok_or_else(|| {
        InstallError::UnsupportedPlatform(format!(
            "lkit self upgrade only supports x86_64 and aarch64, not {}",
            std::env::consts::ARCH
        ))
    })?;
    // 解析目标版本:默认 GitHub releases/latest 的 stable;--version 指定版本
    // (候选版必须用带 tag 的版本,例如 v0.2.0-rc.1)。
    let release = fetch_release(args.version.as_deref()).await?;
    let version = match args.version.as_deref() {
        Some(tag) => parse_upgrade_version(tag)?,
        None => {
            if release.draft || release.prerelease {
                return Err(InstallError::ParameterUsage(format!(
                    "the latest GitHub release {} is a draft or prerelease",
                    release.tag_name
                )));
            }
            parse_release_tag(&release.tag_name).ok_or_else(|| {
                InstallError::ParameterUsage(format!(
                    "the latest GitHub release has an invalid tag {:?}",
                    release.tag_name
                ))
            })?
        }
    };
    // 版本相同 → 输出提示并返回 0,不修改任何文件。
    if current_version()? == version {
        println!(
            "self: {}",
            crate::tr!(crate::keys::SELF_ALREADY_LATEST, version = version)
        );
        return Ok(());
    }
    let asset_name = match architecture {
        Architecture::X86_64 => "lkit-x86_64",
        Architecture::Aarch64 => "lkit-aarch64",
    };
    let asset = unique_asset(&release.assets, asset_name).ok_or_else(|| {
        InstallError::Repository(RepositoryError::InvalidRelease(format!(
            "release {} is missing the {asset_name} asset",
            release.tag_name
        )))
    })?;
    let client = DownloadClient::new()?;
    let checksums_url = Url::parse(&format!(
        "{RELEASES_DOWNLOAD_ROOT}/{}/SHA256SUMS",
        release.tag_name
    ))
    .map_err(RepositoryError::InvalidUrl)?;
    let Some((_, body)) = client
        .get_metadata(checksums_url, github_headers()?, false)
        .await
        .map_err(InstallError::Repository)?
    else {
        unreachable!("必填元数据不会返回 None")
    };
    let checksum = manifest_checksum(&body, asset_name).ok_or_else(|| {
        InstallError::Repository(RepositoryError::ChecksumParse(format!(
            "SHA256SUMS does not contain exactly one valid entry for {asset_name}"
        )))
    })?;
    let asset_url = Url::parse(&asset.browser_download_url).map_err(RepositoryError::InvalidUrl)?;
    validate_network_url(&asset_url)?;
    let asset = Asset::checked(asset_url, checksum, asset.size, AssetEncoding::Identity)?;

    let binary = lkit_binary();
    let parent = binary.parent().ok_or_else(|| {
        InstallError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", binary.display()),
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
    let staged = parent.join(format!(".lkit.upgrade.{}.tmp", std::process::id()));
    let outcome = async {
        client
            .download_asset(&version, &asset, "lkit", &staged)
            .await
            .map_err(InstallError::Repository)?;
        // 自检:替换前对下载的二进制执行 `lkit --version`。
        let output = Command::new(&staged).arg("--version").output()?;
        verify_version_output(&output)?;
        install_staged_binary(&staged, &binary)?;
        Ok::<(), InstallError>(())
    }
    .await;
    if let Err(error) = outcome {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    // 刷新 daemon:已注册且运行中 → restart 加载新二进制;已注册未运行 → 不启动;
    // 未注册 → 仅更新 CLI 并提示。
    refresh_daemon(runtime)?;
    println!(
        "self: {}",
        crate::tr!(crate::keys::SELF_UPGRADED, version = version)
    );
    Ok(())
}

/// 下载、校验、自检或替换失败时保留原二进制;成功时以暂存文件原子替换目标。
fn install_staged_binary(staged: &Path, target: &Path) -> Result<(), InstallError> {
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
        .map_err(InstallError::Io)?;
    std::fs::rename(staged, target).map_err(InstallError::Io)
}

fn verify_version_output(output: &std::process::Output) -> Result<(), InstallError> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !stdout.trim_start().starts_with("lkit ") {
        return Err(InstallError::Repository(RepositoryError::InvalidRelease(
            "the downloaded lkit failed its version check".into(),
        )));
    }
    Ok(())
}

/// daemon 刷新是尽力而为:systemd 不可用时跳过(CLI 升级仍然成功)。
fn refresh_daemon(runtime: &InstallRuntime) -> Result<(), InstallError> {
    let Ok(systemd) = systemd::downcast(runtime.service_manager.as_ref()) else {
        return Ok(());
    };
    if !matches!(systemd.probe(), Availability::Available { .. }) {
        return Ok(());
    }
    let service = ManagedService::LkitDaemon;
    match systemd.query_registration(service)? {
        SystemRegistration::Symlink { .. } => {
            if systemd.is_active(service)? {
                systemd.restart(service)?;
            }
        }
        SystemRegistration::Missing => {
            println!(
                "self: {}",
                crate::tr!(crate::keys::SELF_DAEMON_NOT_INSTALLED_HINT)
            );
        }
        SystemRegistration::Conflict { .. } => {}
    }
    Ok(())
}

async fn fetch_release(version: Option<&str>) -> Result<GithubRelease, InstallError> {
    let client = DownloadClient::new()?;
    let url = match version {
        None => Url::parse(&format!(
            "{GITHUB_API_ROOT}/repos/{LKIT_REPOSITORY}/releases/latest"
        ))
        .map_err(RepositoryError::InvalidUrl)?,
        Some(tag) => Url::parse(&format!(
            "{GITHUB_API_ROOT}/repos/{LKIT_REPOSITORY}/releases/tags/{tag}"
        ))
        .map_err(RepositoryError::InvalidUrl)?,
    };
    let Some((_, body)) = client
        .get_metadata(url, github_headers()?, false)
        .await
        .map_err(InstallError::Repository)?
    else {
        unreachable!("必填元数据不会返回 None")
    };
    serde_json::from_slice(&body)
        .map_err(|error| InstallError::Repository(RepositoryError::InvalidJson(error)))
}

fn github_headers() -> Result<HeaderMap, InstallError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(concat!("lkit/", env!("CARGO_PKG_VERSION"))),
    );
    headers.insert(
        HeaderName::from_static("x-github-api-version"),
        HeaderValue::from_static("2022-11-28"),
    );
    if let Some(token) = std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
    {
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            InstallError::Repository(RepositoryError::UnsafeUrl(
                "GITHUB_TOKEN contains invalid characters".into(),
            ))
        })?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

/// 解析 `lkit self upgrade --version` 的目标版本:接受可选 `v` 前缀,
/// 候选版必须带完整 prerelease 后缀(如 `v0.2.0-rc.1`)。
fn parse_upgrade_version(value: &str) -> Result<Version, InstallError> {
    let stripped = value.strip_prefix('v').unwrap_or(value);
    let version = Version::parse(stripped).map_err(|error| {
        InstallError::ParameterUsage(format!("invalid lkit version {value:?}: {error}"))
    })?;
    if version.to_string() != stripped {
        return Err(InstallError::ParameterUsage(format!(
            "lkit version must be a canonical semver like 0.2.0 or v0.2.0-rc.1, got {value:?}"
        )));
    }
    // 候选版必须带数字发布段(v0.2.0-rc.1 这类 tag 约定);裸 `-rc` 虽是合法
    // SemVer,但不属于可解析的发布 tag。
    if !version.pre.is_empty()
        && version
            .pre
            .split('.')
            .next_back()
            .is_none_or(|segment| !segment.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(InstallError::ParameterUsage(format!(
            "lkit prerelease versions must carry a numeric segment like v0.2.0-rc.1, got {value:?}"
        )));
    }
    Ok(version)
}

fn parse_release_tag(tag: &str) -> Option<Version> {
    let value = tag.strip_prefix('v').unwrap_or(tag);
    let version = Version::parse(value).ok()?;
    (version.to_string() == value).then_some(version)
}

fn current_version() -> Result<Version, InstallError> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        InstallError::CorruptedState(format!(
            "invalid built-in lkit version {:?}: {error}",
            env!("CARGO_PKG_VERSION")
        ))
    })
}

fn unique_asset<'a>(assets: &'a [GithubAsset], name: &str) -> Option<&'a GithubAsset> {
    let mut matches = assets.iter().filter(|asset| asset.name == name);
    let asset = matches.next();
    if matches.next().is_some() {
        return None;
    }
    asset
}

/// 从 `SHA256SUMS` 文本中解析指定资产的校验和,与 install.sh 同规则:
/// 恰好一个 64 位小写十六进制条目,分隔符为空格或星号。
fn manifest_checksum(body: &[u8], asset_name: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let mut found: Option<String> = None;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.len() < 67
            || !bytes[..64]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            || bytes[64] != b' '
            || !matches!(bytes[65], b' ' | b'*')
        {
            return None;
        }
        let name = &line[66..];
        if name == asset_name {
            if found.is_some() {
                return None;
            }
            found = Some(String::from_utf8_lossy(&bytes[..64]).into_owned());
        }
    }
    found
}

fn write_unit_origin(path: &Path, content: &str) -> Result<(), InstallError> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let tmp = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(content.as_bytes())
        .map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, path).map_err(InstallError::Io)?;
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<(), InstallError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn remove_empty_dir_if_present(path: &Path) -> Result<(), InstallError> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        // 目录非空时不视为失败:原件已删除,空目录清理是尽力而为。
        Err(_) => Ok(()),
    }
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && (metadata.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

fn resolve_runtime(_args: &SelfCommand) -> Result<InstallRuntime, InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = test_runtime(_args) {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(feature = "test-support")]
fn test_runtime(args: &SelfCommand) -> Option<&Path> {
    match &args.action {
        SelfAction::Install(args) | SelfAction::Remove(args) => args.test_runtime.as_deref(),
        SelfAction::Upgrade(args) => args.test_runtime.as_deref(),
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[cfg(test)]
mod tests {
    use clap::{Command, FromArgMatches};

    use super::*;

    fn parse(args: &[&str]) -> Result<SelfCommand, clap::Error> {
        let command = <SelfCommand as Args>::augment_args(Command::new("self"));
        let matches = command.try_get_matches_from(args)?;
        SelfCommand::from_arg_matches(&matches)
    }

    #[test]
    fn parses_subcommands() {
        assert!(matches!(
            parse(&["self", "install"]).unwrap().action,
            SelfAction::Install(_)
        ));
        assert!(matches!(
            parse(&["self", "remove"]).unwrap().action,
            SelfAction::Remove(_)
        ));
        let upgrade = parse(&["self", "upgrade"]).unwrap();
        match upgrade.action {
            SelfAction::Upgrade(args) => assert!(args.version.is_none()),
            _ => panic!("expected upgrade"),
        }
        let upgrade = parse(&["self", "upgrade", "--version", "v0.2.0-rc.1"]).unwrap();
        match upgrade.action {
            SelfAction::Upgrade(args) => {
                assert_eq!(args.version.as_deref(), Some("v0.2.0-rc.1"))
            }
            _ => panic!("expected upgrade"),
        }
    }

    #[test]
    fn rejects_install_dir() {
        assert!(parse(&["self", "install", "--install-dir", "/srv/x"]).is_err());
        assert!(parse(&["self", "remove", "--install-dir", "/srv/x"]).is_err());
        assert!(parse(&["self", "upgrade", "--install-dir", "/srv/x"]).is_err());
    }

    #[test]
    fn rejects_unknown_subcommands() {
        assert!(parse(&["self", "status"]).is_err());
        assert!(parse(&["self"]).is_err());
    }

    #[test]
    fn parses_upgrade_versions() {
        assert_eq!(
            parse_upgrade_version("0.2.0").unwrap(),
            Version::parse("0.2.0").unwrap()
        );
        assert_eq!(
            parse_upgrade_version("v0.2.0").unwrap(),
            Version::parse("0.2.0").unwrap()
        );
        assert_eq!(
            parse_upgrade_version("v0.2.0-rc.1").unwrap(),
            Version::parse("0.2.0-rc.1").unwrap()
        );
        for invalid in ["latest", "", "0.19", "release-0.19.2", "v0.2.0-rc"] {
            assert!(parse_upgrade_version(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn parses_release_tags() {
        assert_eq!(
            parse_release_tag("v0.2.0").unwrap(),
            Version::parse("0.2.0").unwrap()
        );
        assert_eq!(
            parse_release_tag("0.2.0").unwrap(),
            Version::parse("0.2.0").unwrap()
        );
        assert_eq!(
            parse_release_tag("v0.2.0-rc.1").unwrap(),
            Version::parse("0.2.0-rc.1").unwrap()
        );
        assert!(parse_release_tag("latest").is_none());
        assert!(parse_release_tag("0.19").is_none());
    }

    #[test]
    fn verifies_version_output() {
        let ok = std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: b"lkit 0.2.0\n".to_vec(),
            stderr: Vec::new(),
        };
        assert!(verify_version_output(&ok).is_ok());

        let wrong_prefix = std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: b"not lkit 0.2.0\n".to_vec(),
            stderr: Vec::new(),
        };
        assert!(verify_version_output(&wrong_prefix).is_err());
    }

    #[test]
    fn installs_staged_binary_atomically() {
        let dir =
            std::env::temp_dir().join(format!("lkit-self-test-staged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join(".lkit.tmp");
        std::fs::write(&staged, b"binary").unwrap();
        let target = dir.join("lkit");
        install_staged_binary(&staged, &target).unwrap();
        assert!(!staged.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"binary");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "installed binary must be executable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_exactly_one_manifest_checksum() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let body = format!("{digest}  lkit-x86_64\n{digest} *lkit-aarch64\n");
        assert_eq!(
            manifest_checksum(body.as_bytes(), "lkit-x86_64").as_deref(),
            Some(digest)
        );
        assert!(manifest_checksum(body.as_bytes(), "lkit-aarch64").is_some());
        assert!(manifest_checksum(body.as_bytes(), "missing").is_none());

        let duplicate = format!("{digest}  lkit-x86_64\n{digest} *lkit-x86_64\n");
        assert!(manifest_checksum(duplicate.as_bytes(), "lkit-x86_64").is_none());
    }

    #[test]
    fn rejects_malformed_manifest_lines() {
        let upper =
            "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF  lkit-x86_64\n";
        assert!(manifest_checksum(upper.as_bytes(), "lkit-x86_64").is_none());
        let no_separator =
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/lkit-x86_64\n";
        assert!(manifest_checksum(no_separator.as_bytes(), "lkit-x86_64").is_none());
        let path = format!(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  {}\n",
            "sub/dir/lkit-x86_64"
        );
        assert!(manifest_checksum(path.as_bytes(), "lkit-x86_64").is_none());
    }
}
