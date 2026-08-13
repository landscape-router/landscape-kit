//! lndp-server: the Landscape Router side of the layer-2 protocol.
//!
//! Linux only: on other platforms this binary is a stub that exits with an
//! error, and the server logic (`server`, `sniff`) is not compiled at all.

#[cfg(target_os = "linux")]
mod server;
#[cfg(target_os = "linux")]
mod sniff;

#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use clap::{Args, Parser, Subcommand};
#[cfg(target_os = "linux")]
use landscape_proto::cli::{parse_devs, parse_ethertype, parse_mac};
#[cfg(target_os = "linux")]
use landscape_proto::transport::Link;
#[cfg(target_os = "linux")]
use server::ServerConfig;

#[cfg(target_os = "linux")]
#[derive(Parser)]
#[command(
    name = "lndp-server",
    about = "The Landscape Router side of the layer-2 protocol; Linux only, listens on all interfaces by default"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[cfg(target_os = "linux")]
#[derive(Subcommand)]
enum Cmd {
    /// Run as the Landscape Router side (default)
    Serve(ServeArgs),
    /// Capture and decode LNDP frames, live or from a pcap file
    Sniff(SniffArgs),
}

#[cfg(target_os = "linux")]
#[derive(Args)]
struct ServeArgs {
    /// Shared secret used for challenge-response authentication
    #[arg(long, value_name = "SECRET")]
    psk: Option<String>,

    /// Device name announced in RESP
    #[arg(long, value_name = "NAME", default_value = "landscape-router")]
    device_name: String,

    /// Local MAC address override (auto-detected when omitted)
    #[arg(long, value_name = "AA:BB:CC:DD:EE:FF", value_parser = parse_mac)]
    mac: Option<[u8; 6]>,

    /// Devices to listen on: 'any', one device, or a comma-separated list
    #[arg(long, value_name = "DEV[,DEV...]", default_value = "any")]
    dev: String,

    /// The ethertype value configured in Landscape (must be 0x88B5-0x88B7)
    #[arg(long, value_name = "ETHERTYPE", value_parser = parse_ethertype, default_value = "0x88b6")]
    ethertype: u16,
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
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Args)]
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
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Cmd::Sniff(args)) => run_sniff(&args).await,
        Some(Cmd::Serve(args)) => run_serve(&args).await,
        None => run_serve(&ServeArgs::default()).await,
    }
}

#[cfg(target_os = "linux")]
async fn run_serve(args: &ServeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let psk = args.psk.as_deref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--psk is required to run as server",
        )
    })?;
    let devs = parse_devs(&args.dev)?;
    let cfg = ServerConfig {
        devs: &devs,
        ethertype: args.ethertype,
        mac: args.mac,
        psk,
        device_name: &args.device_name,
    };
    server::run(&cfg).await
}

#[cfg(target_os = "linux")]
async fn run_sniff(args: &SniffArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.list {
        return sniff::list_devices();
    }
    let filter = sniff::filter_expr(args.ethertype);
    if let Some(path) = &args.file {
        let mut cap = pcap::Capture::from_file(path)?;
        cap.filter(&filter, true)?;
        println!("reading pcap file (filter: {filter})");
        return sniff::run_offline(&mut cap, args.ethertype);
    }
    let devs = parse_devs(&args.dev)?;
    let mut link = Link::open(&devs, args.ethertype, None)?;
    println!("capturing on {} (filter: {filter})", devs.join(", "));
    sniff::run_live(&mut link, args.ethertype).await
}

#[cfg(not(target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("lndp-server is only supported on Linux".into())
}
