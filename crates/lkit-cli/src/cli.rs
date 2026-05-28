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

#[derive(Subcommand, Clone, Copy)]
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
    Backup(BackupArgs),
    /// 升级管理
    Upgrade(UpgradeArgs),
    /// 回滚管理
    Rollback(RollbackArgs),
    /// 配置管理
    Config(ConfigArgs),
    /// 自身管理
    #[command(name = "self")]
    SelfCmd(SelfArgs),
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

#[derive(Args, Clone, Copy)]
pub struct InstallArgs {}

#[derive(Args, Clone, Copy)]
pub struct BackupArgs {}

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
}
