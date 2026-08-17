//! `lkit flare`：Landscape Terrain 协议的 Router 侧(L2 防失联通道)。
//!
//! Linux only:on other platforms the command is a stub that exits with an
//! error, and the server/sniff modules are not compiled at all.

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
        let mut cap = pcap::Capture::from_file(path)?;
        cap.filter(&filter, true)?;
        println!("reading pcap file (filter: {filter})");
        return crate::flare::sniff::run_offline(&mut cap, args.ethertype);
    }
    let devs = parse_devs(&args.dev)?;
    let mut link = Link::open(&devs, args.ethertype, None)?;
    println!("capturing on {} (filter: {filter})", devs.join(", "));
    crate::flare::sniff::run_live(&mut link, args.ethertype).await
}
