mod client;

use clap::Parser;
use client::ClientConfig;
use landscape_proto::cli::{parse_devs, parse_ethertype, parse_mac};

#[derive(Parser)]
#[command(name = "lndp-client", about = "Connect to a Landscape Router over layer-2")]
struct Cli {
    /// Shared secret used for challenge-response authentication
    #[arg(long, value_name = "SECRET")]
    psk: String,

    /// Username sent in AUTH_REQ
    #[arg(long, value_name = "NAME", default_value = "admin")]
    user: String,

    /// Client identity announced in DISCOVER
    #[arg(long, value_name = "NAME", default_value = "pc")]
    client_name: String,

    /// Local MAC address override (auto-detected when omitted)
    #[arg(long, value_name = "AA:BB:CC:DD:EE:FF", value_parser = parse_mac)]
    mac: Option<[u8; 6]>,

    /// Device to send and receive on (default: interface with the default route)
    #[arg(long, value_name = "DEVICE")]
    dev: Option<String>,

    /// The ethertype value configured in Landscape (must be 0x88B5-0x88B7)
    #[arg(long, value_name = "ETHERTYPE", value_parser = parse_ethertype, default_value = "0x88b6")]
    ethertype: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let devs = match cli.dev {
        Some(ref dev) => parse_devs(dev)?,
        None => Vec::new(),
    };
    if devs.iter().any(|d| d == "any") {
        return Err("client cannot run on 'any' (all interfaces), specify one device with --dev"
            .into());
    }
    if devs.len() > 1 {
        return Err("client runs on a single device, --dev accepts one device".into());
    }
    let cfg = ClientConfig {
        devs: &devs,
        ethertype: cli.ethertype,
        mac: cli.mac,
        user: &cli.user,
        psk: &cli.psk,
        client_name: &cli.client_name,
    };
    client::run(&cfg).await
}
