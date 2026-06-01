//! CLI argument definitions using clap derive.

use clap::{Args, Parser, Subcommand};

/// Landscape local CLI management and rescue tool.
#[derive(Parser)]
#[command(name = "lkit", version, about = "Landscape 本机管理与救援工具")]
pub struct Cli {
    /// Increase verbosity (-v for INFO, -vv for DEBUG).
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Subcommand to execute. If omitted, launches the interactive menu.
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// 查看服务状态
    Status(StatusArgs),
    /// 管理服务
    Service(ServiceArgs),
    /// 查看日志
    Logs(LogsArgs),
    /// 诊断检查
    Diagnose(DiagnoseArgs),
    /// 安装 Landscape
    Install(InstallArgs),
    /// 备份管理
    Backup(BackupCmd),
    /// (hidden) Detached restore phase
    #[command(hide = true)]
    DoRestore(BackupRestoreArgs),
    /// 升级管理
    Upgrade(UpgradeArgs),
    /// 回滚管理
    Rollback(RollbackArgs),
    /// 配置管理
    Config(ConfigArgs),
    /// 自身管理
    #[command(name = "self")]
    SelfCmd(SelfArgs),
    /// 镜像管理
    Mirror(MirrorArgs),
}

#[derive(Args, Clone, Copy)]
pub struct StatusArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Clone, Copy)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Subcommand, Clone, Copy)]
pub enum ServiceAction {
    /// 启动服务
    Start,
    /// 停止服务
    Stop,
    /// 重启服务
    Restart,
}

#[derive(Args, Clone, Copy)]
pub struct LogsArgs {
    /// Number of log lines to show
    #[arg(short = 'n', default_value_t = 50)]
    pub lines: usize,
}

#[derive(Args, Clone, Copy)]
pub struct DiagnoseArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Clone)]
pub struct InstallArgs {
    /// Path to an existing landscape_init.toml (skip wizard, non-interactive mode).
    #[arg(long)]
    pub init_file: Option<std::path::PathBuf>,

    /// Source name to use (skip interactive source selection).
    #[arg(long)]
    pub source: Option<String>,

    /// Version tag to install (e.g. v0.19.2).
    #[arg(long)]
    pub version: Option<String>,

    /// Web UI port (used in non-interactive mode).
    #[arg(long, default_value_t = 6300)]
    pub web_port: u16,

    /// HTTPS listen port.
    #[arg(long, default_value_t = 6443)]
    pub https_port: u16,

    /// Force reinstall even if already installed.
    #[arg(long)]
    pub force: bool,
}

/// 备份命令包装，允许无子命令时回退到交互菜单。
#[derive(Parser, Clone)]
pub struct BackupCmd {
    /// 子命令；省略时进入交互菜单。
    #[command(subcommand)]
    pub action: Option<BackupAction>,
}

/// 备份子命令。
#[derive(Subcommand, Clone)]
pub enum BackupAction {
    /// 创建备份
    Create(BackupCreateArgs),
    /// 列出备份
    List(BackupListArgs),
    /// 恢复备份
    Restore(BackupRestoreArgs),
    /// 解压备份到指定目录
    Extract(BackupExtractArgs),
    /// 删除备份
    Delete(BackupDeleteArgs),
}

/// 创建备份参数。
#[derive(Args, Clone)]
pub struct BackupCreateArgs {
    /// Remark for the backup
    #[arg(long)]
    pub remark: Option<String>,

    /// Full backup (entire LANDSCAPE_HOME)
    #[arg(long)]
    pub all: bool,
}

/// 列出备份参数。
#[derive(Args, Clone, Copy)]
pub struct BackupListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// 恢复备份参数。
#[derive(Args, Clone)]
pub struct BackupRestoreArgs {
    /// Backup ID or path to .lkb file
    pub id_or_path: String,
}

/// 解压备份参数。
#[derive(Args, Clone)]
pub struct BackupExtractArgs {
    /// Backup ID or path to .lkb file
    pub id_or_path: String,

    /// Target directory
    #[arg(long)]
    pub target: std::path::PathBuf,

    /// Force overwrite if target is non-empty
    #[arg(long)]
    pub force: bool,
}

/// 删除备份参数。
#[derive(Args, Clone)]
pub struct BackupDeleteArgs {
    /// Backup ID or path to .lkb file
    pub id_or_path: String,
}

#[derive(Args, Clone, Copy)]
pub struct UpgradeArgs {}

#[derive(Args, Clone, Copy)]
pub struct RollbackArgs {}

#[derive(Args, Clone, Copy)]
pub struct ConfigArgs {}

#[derive(Args, Clone, Copy)]
pub struct SelfArgs {
    #[command(subcommand)]
    pub action: SelfAction,
}

#[derive(Subcommand, Clone, Copy)]
pub enum SelfAction {
    /// 显示版本
    Version,
    /// 检查更新
    UpgradeCheck,
}

#[derive(Args, Clone)]
pub struct MirrorArgs {
    #[command(subcommand)]
    pub action: MirrorAction,
}

#[derive(Subcommand, Clone)]
pub enum MirrorAction {
    /// 从上游同步 release 制品
    Sync(MirrorSyncArgs),
    /// 启动 HTTP 文件服务
    Serve(MirrorServeArgs),
    /// 校验镜像完整性
    Verify(MirrorVerifyArgs),
    /// 列出已同步版本
    List(MirrorListArgs),
}

#[derive(Args, Clone)]
pub struct MirrorSyncArgs {
    /// 同步源类型
    #[arg(long, value_enum, default_value_t = SyncSourceType::Github)]
    pub source: SyncSourceType,
    /// HTTP 镜像源地址 (source=http)
    #[arg(long)]
    pub source_url: Option<String>,
    /// 本地源路径 (source=local)
    #[arg(long)]
    pub source_path: Option<String>,
    /// S3 bucket (source=s3)
    #[arg(long)]
    pub source_bucket: Option<String>,
    /// S3 endpoint (source=s3)
    #[arg(long)]
    pub source_endpoint: Option<String>,
    /// GitHub 仓库 (owner/repo)
    #[arg(long, default_value = "ThisSeanZhang/landscape")]
    pub repo: String,
    /// 目标产品目录
    #[arg(long)]
    pub prefix: Option<String>,
    /// 目标类型
    #[arg(long, value_enum)]
    pub target: MirrorTargetType,
    /// 本地路径 (target=local)
    #[arg(long)]
    pub path: Option<String>,
    /// S3 bucket (target=s3)
    #[arg(long)]
    pub bucket: Option<String>,
    /// S3 endpoint (target=s3)
    #[arg(long)]
    pub endpoint: Option<String>,
    /// S3 bucket 内的 key 前缀 (target=s3)
    #[arg(long, default_value = "")]
    pub s3_prefix: String,
    /// 同步指定版本
    #[arg(long)]
    pub tag: Option<String>,
    /// 同步最近 N 个版本
    #[arg(long)]
    pub latest: Option<u32>,
    /// 同步某版本之后的所有版本
    #[arg(long)]
    pub since: Option<String>,
    /// 同步全部历史版本
    #[arg(long)]
    pub all: bool,
    /// 强制重新同步
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Clone)]
pub struct MirrorServeArgs {
    /// 本地镜像路径
    #[arg(long)]
    pub path: String,
    /// 监听端口
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
    /// 绑定地址
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: String,
}

#[derive(Args, Clone)]
pub struct MirrorVerifyArgs {
    /// 目标类型
    #[arg(long, value_enum)]
    pub target: MirrorTargetType,
    /// 产品目录前缀
    #[arg(long, default_value = "landscape")]
    pub prefix: String,
    /// 本地路径
    #[arg(long)]
    pub path: Option<String>,
    /// S3 bucket
    #[arg(long)]
    pub bucket: Option<String>,
    /// S3 endpoint
    #[arg(long)]
    pub endpoint: Option<String>,
    /// S3 bucket 内的 key 前缀 (target=s3)
    #[arg(long, default_value = "")]
    pub s3_prefix: String,
}

#[derive(Args, Clone)]
pub struct MirrorListArgs {
    /// 目标类型
    #[arg(long, value_enum)]
    pub target: MirrorTargetType,
    /// 产品目录前缀
    #[arg(long, default_value = "landscape")]
    pub prefix: String,
    /// 本地路径
    #[arg(long)]
    pub path: Option<String>,
    /// S3 bucket
    #[arg(long)]
    pub bucket: Option<String>,
    /// S3 endpoint
    #[arg(long)]
    pub endpoint: Option<String>,
    /// S3 bucket 内的 key 前缀 (target=s3)
    #[arg(long, default_value = "")]
    pub s3_prefix: String,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub enum MirrorTargetType {
    Local,
    S3,
}

/// 同步源类型。
#[derive(clap::ValueEnum, Clone, Debug)]
pub enum SyncSourceType {
    /// 从 GitHub Releases 同步
    Github,
    /// 从 HTTP(S) 镜像同步
    Http,
    /// 从本地目录同步
    Local,
    /// 从 S3 兼容存储同步
    S3,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_status() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "status"])?;
        assert!(matches!(cli.command, Some(Commands::Status(_))));
        Ok(())
    }

    #[test]
    fn parse_status_json() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "status", "--json"])?;
        if let Some(Commands::Status(args)) = cli.command {
            assert!(args.json);
        } else {
            return Err("expected Status".into());
        }
        Ok(())
    }

    #[test]
    fn parse_service_start() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "service", "start"])?;
        if let Some(Commands::Service(args)) = cli.command {
            assert!(matches!(args.action, ServiceAction::Start));
        } else {
            return Err("expected Service".into());
        }
        Ok(())
    }

    #[test]
    fn parse_logs_custom_lines() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "logs", "-n", "100"])?;
        if let Some(Commands::Logs(args)) = cli.command {
            assert_eq!(args.lines, 100);
        } else {
            return Err("expected Logs".into());
        }
        Ok(())
    }

    #[test]
    fn parse_no_command() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit"])?;
        assert!(cli.command.is_none());
        Ok(())
    }

    #[test]
    fn parse_self_version() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "self", "version"])?;
        if let Some(Commands::SelfCmd(args)) = cli.command {
            assert!(matches!(args.action, SelfAction::Version));
        } else {
            return Err("expected SelfCmd".into());
        }
        Ok(())
    }

    #[test]
    fn parse_mirror_sync() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "mirror", "sync", "--target", "local"])?;
        if let Some(Commands::Mirror(args)) = cli.command {
            assert!(matches!(args.action, MirrorAction::Sync(_)));
        } else {
            return Err("expected Mirror".into());
        }
        Ok(())
    }

    #[test]
    fn parse_backup_no_subcommand() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "backup"])?;
        if let Some(Commands::Backup(cmd)) = cli.command {
            assert!(cmd.action.is_none());
        } else {
            return Err("expected Backup".into());
        }
        Ok(())
    }

    #[test]
    fn parse_backup_create() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "backup", "create", "--remark", "test"])?;
        if let Some(Commands::Backup(cmd)) = cli.command {
            if let Some(BackupAction::Create(args)) = cmd.action {
                assert_eq!(args.remark.as_deref(), Some("test"));
                assert!(!args.all);
            } else {
                return Err("expected Create".into());
            }
        } else {
            return Err("expected Backup".into());
        }
        Ok(())
    }

    #[test]
    fn parse_backup_list_json() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "backup", "list", "--json"])?;
        if let Some(Commands::Backup(cmd)) = cli.command {
            if let Some(BackupAction::List(args)) = cmd.action {
                assert!(args.json);
            } else {
                return Err("expected List".into());
            }
        } else {
            return Err("expected Backup".into());
        }
        Ok(())
    }

    #[test]
    fn parse_backup_restore() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "backup", "restore", "abc123"])?;
        if let Some(Commands::Backup(cmd)) = cli.command {
            if let Some(BackupAction::Restore(args)) = cmd.action {
                assert_eq!(args.id_or_path, "abc123");
            } else {
                return Err("expected Restore".into());
            }
        } else {
            return Err("expected Backup".into());
        }
        Ok(())
    }

    #[test]
    fn parse_do_restore() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::try_parse_from(["lkit", "do-restore", "abc123"])?;
        if let Some(Commands::DoRestore(args)) = cli.command {
            assert_eq!(args.id_or_path, "abc123");
        } else {
            return Err("expected DoRestore".into());
        }
        Ok(())
    }
}
