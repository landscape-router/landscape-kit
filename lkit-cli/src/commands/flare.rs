//! `lkit flare`：Landscape Terrain 协议的 Router 侧(L2 防失联通道)。
//!
//! 子命令:`serve`(前台服务端)、`sniff`(抓包诊断)、`setup`(显示/更新 daemon
//! 托管的 `[flare]` 配置段)。Linux only:on other platforms the command is a
//! stub that exits with an error, and the server/sniff modules are not compiled
//! at all.

#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use clap::{Args, Subcommand};

#[cfg(target_os = "linux")]
use crate::flare::server::ServerConfig;
#[cfg(target_os = "linux")]
use landscape_terrain_proto::cli::{parse_devs, parse_ethertype, parse_mac, parse_port_list};
#[cfg(target_os = "linux")]
use landscape_terrain_proto::transport::Link;

#[cfg(target_os = "linux")]
#[derive(Debug, Args)]
pub struct Flare {
    #[command(subcommand)]
    cmd: Option<FlareCmd>,
}

/// 非 Linux 平台占位:clap 接受 `lkit flare` 但 run 总是报错。
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Args)]
pub struct Flare {}

#[cfg(target_os = "linux")]
#[derive(Debug, Subcommand)]
enum FlareCmd {
    /// Run as the Landscape Router side (default)
    Serve(ServeArgs),
    /// Capture and decode Terrain frames, live or from a pcap file
    Sniff(SniffArgs),
    /// Show or update the daemon-hosted `[flare]` config.toml section
    Setup(SetupArgs),
}

#[cfg(target_os = "linux")]
#[derive(Debug, Args)]
pub(crate) struct ServeArgs {
    /// Shared secret used for challenge-response authentication; when
    /// omitted, the LANDSCAPE_FLARE_PSK environment variable is used
    #[arg(long, value_name = "SECRET")]
    pub(crate) psk: Option<String>,

    /// Device name announced in RESP
    #[arg(long, value_name = "NAME", default_value = "landscape-router")]
    pub(crate) device_name: String,

    /// Local MAC address override (auto-detected when omitted)
    #[arg(long, value_name = "AA:BB:CC:DD:EE:FF", value_parser = parse_mac)]
    pub(crate) mac: Option<[u8; 6]>,

    /// Devices to listen on: 'any', one device, or a comma-separated list
    #[arg(long, value_name = "DEV[,DEV...]", default_value = "any")]
    pub(crate) dev: String,

    /// The ethertype value configured in Landscape (must be 0x88B5-0x88B7)
    #[arg(long, value_name = "ETHERTYPE", value_parser = parse_ethertype, default_value = "0x88b6")]
    pub(crate) ethertype: u16,

    /// Local ports the server may be asked to forward to 127.0.0.1
    #[arg(long, value_name = "P1,P2...", default_value = "22,6443")]
    pub(crate) forward_ports: String,

    /// Discovery token: only respond to DISCOVER frames carrying it
    #[arg(long, value_name = "TOKEN")]
    pub(crate) token: Option<String>,
}

#[cfg(target_os = "linux")]
impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            psk: None,
            device_name: "landscape-router".to_string(),
            mac: None,
            dev: "any".to_string(),
            ethertype: 0x88B6,
            forward_ports: "22,6443".to_string(),
            token: None,
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Args)]
struct SniffArgs {
    /// Devices to capture on: 'any', one device, or a comma-separated list
    #[arg(long, value_name = "DEV[,DEV...]", default_value = "any")]
    dev: String,

    /// The ethertype value to capture (must be 0x88B5-0x88B7)
    #[arg(long, value_name = "ETHERTYPE", value_parser = parse_ethertype, default_value = "0x88b6")]
    ethertype: u16,

    /// Read packets from a pcap file instead of a live device
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,

    /// List available capture devices and exit
    #[arg(long)]
    list: bool,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Args)]
pub(crate) struct SetupArgs {
    /// Recovery psk; when omitted the current value is kept
    #[arg(long, value_name = "SECRET")]
    pub(crate) psk: Option<String>,

    /// Device name announced in RESP
    #[arg(long, value_name = "NAME")]
    pub(crate) device_name: Option<String>,

    /// Local MAC address override (auto-detected when omitted)
    #[arg(long, value_name = "AA:BB:CC:DD:EE:FF", value_parser = parse_mac)]
    pub(crate) mac: Option<[u8; 6]>,

    /// Devices to listen on: 'any', one device, or a comma-separated list
    #[arg(long, value_name = "DEV[,DEV...]")]
    pub(crate) dev: Option<String>,

    /// The ethertype value configured in Landscape (must be 0x88B5-0x88B7)
    #[arg(long, value_name = "ETHERTYPE", value_parser = parse_ethertype)]
    pub(crate) ethertype: Option<u16>,

    /// Local ports the server may be asked to forward to 127.0.0.1
    #[arg(long, value_name = "P1,P2...")]
    pub(crate) forward_ports: Option<String>,

    /// Discovery token: only respond to DISCOVER frames carrying it
    #[arg(long, value_name = "TOKEN")]
    pub(crate) token: Option<String>,
}

#[cfg(target_os = "linux")]
pub(crate) async fn run(args: &Flare) -> std::process::ExitCode {
    match run_inner(args).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lkit flare: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
async fn run_inner(args: &Flare) -> Result<(), Box<dyn std::error::Error>> {
    match &args.cmd {
        Some(FlareCmd::Sniff(args)) => run_sniff(args).await,
        Some(FlareCmd::Serve(args)) => run_serve(args, None).await,
        Some(FlareCmd::Setup(args)) => run_setup(args),
        None => run_serve(&ServeArgs::default(), None).await,
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) async fn run(_args: &Flare) -> std::process::ExitCode {
    eprintln!("lkit flare: only supported on Linux");
    std::process::ExitCode::FAILURE
}

/// 以 daemon 托管方式启动 flare 服务端:由外部通过 oneshot 控制退出,
/// 不在进程内安装任何信号处理。
#[cfg(target_os = "linux")]
pub(crate) async fn run_serve(
    args: &ServeArgs,
    shutdown: Option<tokio::sync::oneshot::Receiver<()>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let psk = match &args.psk {
        Some(p) => p.clone(),
        None => std::env::var("LANDSCAPE_FLARE_PSK").map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--psk or the LANDSCAPE_FLARE_PSK environment variable is required",
            )
        })?,
    };
    let devs = parse_devs(&args.dev)?;
    let forward_ports = parse_port_list(&args.forward_ports)?;
    let cfg = ServerConfig {
        devs: &devs,
        ethertype: args.ethertype,
        mac: args.mac,
        psk: &psk,
        device_name: &args.device_name,
        forward_ports: &forward_ports,
        discover_token: args.token.as_deref().unwrap_or(""),
    };
    crate::flare::server::run(&cfg, shutdown).await
}

#[cfg(target_os = "linux")]
async fn run_sniff(args: &SniffArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.list {
        return crate::flare::sniff::list_devices();
    }
    let filter = crate::flare::sniff::filter_expr(args.ethertype);
    if let Some(path) = &args.file {
        println!("reading pcap file: {}", path.display());
        return crate::flare::sniff::run_offline(path, args.ethertype);
    }
    let devs = parse_devs(&args.dev)?;
    let mut link = Link::open(&devs, args.ethertype, None)?;
    println!("capturing on {} (filter: {filter})", devs.join(", "));
    crate::flare::sniff::run_live(&mut link, args.ethertype).await
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use crate::deployment::config::{FlareSection, default_flare_section, load_flare, save_flare};
    use crate::deployment::layout;

    use super::*;

    fn territory(name: &str) -> (layout::TerritoryOverride, std::path::PathBuf) {
        let temp =
            std::env::temp_dir().join(format!("lkit-flare-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let guard = layout::test_territory(&temp);
        (guard, temp)
    }

    #[test]
    fn setup_without_options_only_reports_the_current_configuration() {
        let (guard, temp) = territory("view");
        save_flare(&FlareSection {
            psk: Some("a-configured-recovery-secret".into()),
            ..default_flare_section()
        })
        .unwrap();
        let args = SetupArgs {
            psk: None,
            device_name: None,
            mac: None,
            dev: None,
            ethertype: None,
            forward_ports: None,
            token: None,
        };
        run_setup(&args).unwrap();
        let section = load_flare().unwrap();
        assert_eq!(
            section.psk.as_deref(),
            Some("a-configured-recovery-secret"),
            "a bare setup must not modify the configuration"
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn setup_updates_only_the_given_fields() {
        let (guard, temp) = territory("update");
        save_flare(&FlareSection {
            psk: Some("an-existing-secret".into()),
            devices: Some("eth0".into()),
            token: Some("old-token".into()),
            ..default_flare_section()
        })
        .unwrap();
        let args = SetupArgs {
            psk: Some("a-rotated-recovery-secret".into()),
            device_name: None,
            mac: None,
            dev: None,
            ethertype: None,
            forward_ports: None,
            token: Some("new-token".into()),
        };
        run_setup(&args).unwrap();
        let section = load_flare().unwrap();
        assert_eq!(section.psk.as_deref(), Some("a-rotated-recovery-secret"));
        assert_eq!(section.token.as_deref(), Some("new-token"));
        assert_eq!(
            section.devices.as_deref(),
            Some("eth0"),
            "unset fields must keep their current values"
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn setup_rejects_a_short_psk_without_writing() {
        let (guard, temp) = territory("short");
        save_flare(&FlareSection {
            psk: Some("an-existing-secret".into()),
            ..default_flare_section()
        })
        .unwrap();
        let args = SetupArgs {
            psk: Some("short".into()),
            device_name: None,
            mac: None,
            dev: None,
            ethertype: None,
            forward_ports: None,
            token: None,
        };
        assert!(run_setup(&args).is_err());
        assert_eq!(
            load_flare().unwrap().psk.as_deref(),
            Some("an-existing-secret"),
            "a rejected setup must not touch the configuration"
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn setup_creates_a_minimal_section_when_missing() {
        let (guard, temp) = territory("create");
        let args = SetupArgs {
            psk: Some("a-brand-new-recovery-secret".into()),
            device_name: None,
            mac: None,
            dev: None,
            ethertype: None,
            forward_ports: None,
            token: None,
        };
        run_setup(&args).unwrap();
        let section = load_flare().unwrap();
        assert_eq!(section.psk.as_deref(), Some("a-brand-new-recovery-secret"));
        assert_eq!(section.device_name, "landscape-router");
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn setup_validates_devices_and_forward_ports() {
        let (guard, temp) = territory("validate");
        let invalid_dev = SetupArgs {
            psk: Some("a-recovery-secret".into()),
            device_name: None,
            mac: None,
            dev: Some("any,eth0".into()),
            ethertype: None,
            forward_ports: None,
            token: None,
        };
        assert!(run_setup(&invalid_dev).is_err());
        let invalid_ports = SetupArgs {
            psk: Some("a-recovery-secret".into()),
            device_name: None,
            mac: None,
            dev: None,
            ethertype: None,
            forward_ports: Some("22,not-a-port".into()),
            token: None,
        };
        assert!(run_setup(&invalid_ports).is_err());
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }
}

/// 显示或更新 daemon 托管的 `[flare]` 配置段。带任一修改选项时,在既有配置
/// 基础上覆盖对应字段并写回 `config.toml`(daemon 下一个周期自动拾取);
/// 不带选项时打印当前有效配置,psk 一并显示,供分发给 `lflare` 恢复客户端。
#[cfg(target_os = "linux")]
fn run_setup(args: &SetupArgs) -> Result<(), Box<dyn std::error::Error>> {
    use crate::deployment::config::{
        FLARE_PSK_MIN_LENGTH, default_flare_section, load_flare, save_flare,
    };

    let mut section = load_flare().unwrap_or_else(default_flare_section);
    let modifying = args.psk.is_some()
        || args.device_name.is_some()
        || args.mac.is_some()
        || args.dev.is_some()
        || args.ethertype.is_some()
        || args.forward_ports.is_some()
        || args.token.is_some();
    if !modifying {
        let psk = section.psk.as_deref().unwrap_or("<not configured>");
        println!(
            "flare: device {} (ethertype 0x{:04x}), forward ports: {}",
            section.device_name, section.ethertype, section.forward_ports
        );
        println!(
            "flare: listening on {}",
            section.devices.clone().unwrap_or_else(|| "any".into())
        );
        println!("flare: psk {psk}");
        match section.token.as_deref() {
            Some(token) => println!("flare: discovery token {token}"),
            None => println!("flare: discovery token <unset>"),
        }
        return Ok(());
    }
    if let Some(psk) = &args.psk {
        if psk.len() < FLARE_PSK_MIN_LENGTH {
            return Err(format!(
                "the flare psk must be at least {FLARE_PSK_MIN_LENGTH} characters"
            )
            .into());
        }
        section.psk = Some(psk.clone());
    }
    if let Some(name) = &args.device_name {
        section.device_name = name.clone();
    }
    if let Some(mac) = args.mac {
        section.mac = Some(landscape_terrain_proto::transport::fmt_mac(&mac));
    }
    if let Some(dev) = &args.dev {
        let _ = parse_devs(dev)?;
        section.devices = Some(dev.clone());
    }
    if let Some(ethertype) = args.ethertype {
        section.ethertype = ethertype;
    }
    if let Some(ports) = &args.forward_ports {
        let _ = parse_port_list(ports)?;
        section.forward_ports = ports.clone();
    }
    if let Some(token) = &args.token {
        section.token = Some(token.clone());
    }
    save_flare(&section)?;
    println!("flare: daemon-hosted flare configuration updated (written to config.toml)");
    Ok(())
}
