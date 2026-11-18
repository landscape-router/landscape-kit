mod client;
mod i18n;
mod task_names;
mod tui;

use clap::{Args, Parser, Subcommand};
use client::{ClientConfig, Forward, LogSink};
use landscape_terrain_proto::cli::{parse_devs, parse_ethertype, parse_forward, parse_mac};
use landscape_terrain_proto::protocol::v6;

rust_i18n::i18n!("locales", fallback = "en");

#[derive(Parser)]
#[command(
    name = "lflare",
    about = "Connect to a Landscape Router over layer-2",
    long_about = concat!(
        "Landscape L2 client.\n\n",
        "With no subcommand lflare lists available devices. Select a router,\n",
        "confirm its fingerprint, sign in, then manage port mappings.\n",
        "Scripts and automation should use the `cli` subcommand with flags."
    )
)]
struct Cli {
    /// TUI language: en or zh; omitted uses LKIT_LANG and the system locale
    #[arg(long, value_name = "LANG", value_parser = i18n::parse_arg)]
    lang: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Command-line mode: connect with flags (script-friendly)
    #[command(name = "cli")]
    Cli(CliArgs),
    /// List Terrain v6 devices on a single interface
    Discover(DiscoveryArgs),
}

#[derive(Args)]
struct DiscoveryArgs {
    #[arg(long)]
    dev: Option<String>,
    #[arg(long, default_value="0x88b6", value_parser=parse_ethertype)]
    ethertype: u16,
    /// Discovery PSK (also accepted via LANDSCAPE_FLARE_PSK)
    #[arg(long)]
    psk: Option<String>,
    #[arg(long)]
    token: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct CliArgs {
    /// Full device fingerprint, confirmed for this connection
    #[arg(long, conflicts_with = "legacy")]
    target: Option<String>,
    /// Use the old recovery-code protocol to connect to a v5 daemon
    #[arg(long)]
    legacy: bool,
    /// Password authority for this Flare session
    #[arg(long, requires="target",value_parser=["root","landscape"])]
    auth: Option<String>,
    /// Read the login password from a private file (otherwise prompt at terminal)
    #[arg(long)]
    password_file: Option<std::path::PathBuf>,

    /// Private discovery PSK, or the old recovery code with --legacy; when
    /// omitted, the LANDSCAPE_FLARE_PSK environment variable is used
    #[arg(long, value_name = "SECRET")]
    psk: Option<String>,

    /// Username sent in AUTH_REQ
    #[arg(long, value_name = "NAME", default_value = "admin")]
    user: String,

    /// Client name in legacy v5 discovery
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

    /// Forward a local port to the router's 127.0.0.1, repeatable:
    /// --forward 2222:22 listens on 127.0.0.1:2222 and connects to the
    /// router's 127.0.0.1:22
    #[arg(long, value_name = "LOCAL:DST", value_parser = parse_forward)]
    forward: Vec<Forward>,

    /// Discovery token carried in DISCOVER; the server stays silent
    /// without it (must match the server's --token)
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name_fn(
            landscape_terrain_proto::concurrency::runtime_thread_name_fn(task_names::RUNTIME),
        )
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    i18n::preconfigure(std::env::args_os());
    let cli = Cli::parse();
    i18n::configure(i18n::resolve(cli.lang.as_deref()));
    match cli.command {
        None => tui::run().await,
        Some(Command::Discover(args)) => {
            let devs = args
                .dev
                .as_deref()
                .map(parse_devs)
                .transpose()?
                .unwrap_or_default();
            let psk = args
                .psk
                .or_else(|| std::env::var("LANDSCAPE_FLARE_PSK").ok());
            let gate = landscape_terrain_proto::protocol::v6::Discovery::new(psk.as_deref());
            let found = client::rescue::discover(
                &devs,
                args.ethertype,
                None,
                &gate,
                args.token.as_deref().unwrap_or(""),
            )
            .await?;
            if args.json {
                let list: Vec<_> = found
                    .iter()
                    .map(|device| {
                        serde_json::json!({
                            "name": device.name,
                            "fingerprint": device.fingerprint,
                            "mac": landscape_terrain_proto::transport::fmt_mac(&device.mac),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string(&list)?);
            } else {
                for d in found {
                    println!("{}  {}", d.fingerprint, d.name);
                }
            }
            Ok(())
        }
        Some(Command::Cli(args)) => run_cli(&args).await,
    }
}

async fn run_cli(cli: &CliArgs) -> Result<(), Box<dyn std::error::Error>> {
    let psk = cli
        .psk
        .clone()
        .or_else(|| std::env::var("LANDSCAPE_FLARE_PSK").ok());
    let devs = match cli.dev {
        Some(ref dev) => parse_devs(dev)?,
        None => Vec::new(),
    };
    if devs.iter().any(|d| d == "any") {
        return Err(
            "client cannot run on 'any' (all interfaces), specify one device with --dev".into(),
        );
    }
    if devs.len() > 1 {
        return Err("client runs on a single device, --dev accepts one device".into());
    }
    let cfg = ClientConfig {
        devs: &devs,
        ethertype: cli.ethertype,
        mac: cli.mac,
        user: if cli.auth.as_deref() == Some("root") {
            "root"
        } else {
            &cli.user
        },
        psk: psk.as_deref().unwrap_or(""),
        client_name: &cli.client_name,
        forwards: &cli.forward,
        token: cli.token.as_deref().unwrap_or(""),
        log: LogSink::Stdio,
        shutdown: None,
        forward_control: None,
        events: None,
    };
    if let Some(target) = &cli.target {
        let gate = landscape_terrain_proto::protocol::v6::Discovery::new(psk.as_deref());
        let device = client::rescue::discover(&devs, cli.ethertype, cli.mac, &gate, cfg.token)
            .await?
            .into_iter()
            .find(|d| d.fingerprint.eq_ignore_ascii_case(target))
            .ok_or("selected fingerprint not found")?;
        let conn = client::rescue::connect(&devs, cli.ethertype, cli.mac, &gate, device).await?;
        let password = match &cli.password_file {
            Some(path) => read_login_password(path)?,
            None => zeroize::Zeroizing::new(rpassword::prompt_password("Flare login password: ")?),
        };
        conn.run(
            cfg,
            if cli.auth.as_deref() == Some("root") {
                v6::AUTH_ROOT
            } else {
                v6::AUTH_LANDSCAPE
            },
            password,
        )
        .await
    } else {
        if cli.password_file.is_some() || (!cli.legacy && cli.psk.is_none() && psk.is_none()) {
            return Err(
                "select a device with --target, or use --legacy with an old recovery code".into(),
            );
        }
        if psk.is_none() {
            return Err("legacy connections require the old recovery code".into());
        }
        client::run(cfg).await
    }
}

fn read_login_password(path: &std::path::Path) -> std::io::Result<zeroize::Zeroizing<String>> {
    use std::io::{Error, Read};
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Error::other("password file must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(Error::other(
                "password file must be private and owned by the current user",
            ));
        }
    }
    let limit = v6::MAX_CREDENTIAL_BYTES + 2;
    let mut password = zeroize::Zeroizing::new(String::new());
    file.take((limit + 1) as u64)
        .read_to_string(&mut password)?;
    if password.len() > limit {
        return Err(Error::other("password file too large"));
    }
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    if password.is_empty() || password.contains(['\r', '\n', '\0']) {
        return Err(Error::other("password file must contain one nonempty line"));
    }
    Ok(password)
}
