use std::fs::File;
use std::io::{BufReader, Cursor, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use lkit_test_fixture::{FIXTURE_CONFIG_FILE, LandscapeFixtureConfig, Scenario};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Architecture {
    #[value(name = "x86_64")]
    X86_64,
    #[value(name = "aarch64")]
    Aarch64,
}

impl Architecture {
    fn asset_name(self) -> &'static str {
        match self {
            Self::X86_64 => "landscape-webserver-x86_64.zst",
            Self::Aarch64 => "landscape-webserver-aarch64.zst",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::X86_64 => Self::Aarch64,
            Self::Aarch64 => Self::X86_64,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "lkit-fixture-release")]
struct Args {
    #[arg(long)]
    version: String,

    #[arg(long, value_enum)]
    scenario: Scenario,

    #[arg(long, value_enum)]
    native_architecture: Architecture,

    #[arg(long)]
    native_binary: PathBuf,

    /// 在 fixture ELF 尾部加入版本标记,使复用同一次编译的版本具有不同摘要。
    #[arg(long, default_value_t = false)]
    stamp_version: bool,

    /// `delayed_ready` 场景的启动延迟毫秒数;必须大于被测运行时配置的
    /// 启动轮询超时才能触发超时失败。
    #[arg(long, default_value_t = 750)]
    ready_delay_ms: u64,

    #[arg(long)]
    output: PathBuf,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fixture release: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<()> {
    let version = args
        .version
        .parse::<semver::Version>()
        .context("version must be stable semver")?;
    anyhow::ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "version must be stable semver"
    );
    anyhow::ensure!(
        args.native_binary.is_file(),
        "native binary {} is not a file",
        args.native_binary.display()
    );
    ensure_empty_output(&args.output)?;

    compress_file(
        &args.native_binary,
        &args.output.join(args.native_architecture.asset_name()),
        args.stamp_version.then_some(&version),
    )?;
    let placeholder = format!(
        "lkit fixture placeholder: architecture={} version={}\n",
        args.native_architecture.other().key(),
        version
    );
    compress_bytes(
        placeholder.as_bytes(),
        &args
            .output
            .join(args.native_architecture.other().asset_name()),
    )?;

    let config = LandscapeFixtureConfig {
        scenario: args.scenario,
        listen_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        dns_tcp_port: 53,
        dns_udp_port: 53,
        http_port: 6_300,
        https_port: 6_443,
        ready_delay_ms: args.ready_delay_ms,
        export_version: version.to_string(),
        export_content: format!("version = \"{version}\"\n"),
        ..LandscapeFixtureConfig::default()
    };
    config.validate()?;
    write_static_zip(&args.output.join("static.zip"), &config)?;
    Ok(())
}

fn ensure_empty_output(output: &Path) -> Result<()> {
    if output.exists() {
        anyhow::ensure!(
            output.is_dir(),
            "output {} is not a directory",
            output.display()
        );
        anyhow::ensure!(
            std::fs::read_dir(output)?.next().is_none(),
            "output directory {} is not empty",
            output.display()
        );
    } else {
        std::fs::create_dir_all(output)
            .with_context(|| format!("create output directory {}", output.display()))?;
    }
    Ok(())
}

fn compress_file(
    source: &Path,
    target: &Path,
    stamp_version: Option<&semver::Version>,
) -> Result<()> {
    let source_file =
        File::open(source).with_context(|| format!("open native binary {}", source.display()))?;
    let target_file = File::create(target)
        .with_context(|| format!("create compressed asset {}", target.display()))?;
    let mut encoder = zstd::stream::Encoder::new(target_file, 19)
        .with_context(|| format!("create zstd encoder for {}", target.display()))?;
    std::io::copy(&mut BufReader::new(source_file), &mut encoder)
        .with_context(|| format!("compress native binary {}", source.display()))?;
    if let Some(version) = stamp_version {
        writeln!(encoder, "\nlkit-fixture-version={version}")
            .context("append fixture version stamp")?;
    }
    encoder.finish().context("finish fixture compression")?;
    Ok(())
}

fn compress_bytes(content: &[u8], target: &Path) -> Result<()> {
    let target_file = File::create(target)
        .with_context(|| format!("create compressed asset {}", target.display()))?;
    zstd::stream::copy_encode(Cursor::new(content), target_file, 19)
        .with_context(|| format!("compress placeholder asset {}", target.display()))?;
    Ok(())
}

fn write_static_zip(target: &Path, config: &LandscapeFixtureConfig) -> Result<()> {
    let file = File::create(target)
        .with_context(|| format!("create static archive {}", target.display()))?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    archive.start_file("static/index.html", options)?;
    archive.write_all(b"<h1>Landscape fixture</h1>\n")?;
    archive.start_file(format!("static/{FIXTURE_CONFIG_FILE}"), options)?;
    archive.write_all(&serde_json::to_vec_pretty(config)?)?;
    archive.write_all(b"\n")?;
    archive.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lkit-fixture-release-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn creates_publishable_native_release_assets() {
        let root = temp_dir("assets");
        let native = root.join("landscape-webserver");
        let output = root.join("dist");
        std::fs::write(&native, b"native fixture binary\n").unwrap();

        run(Args {
            version: "1.2.3".into(),
            scenario: Scenario::Healthy,
            native_architecture: Architecture::X86_64,
            native_binary: native,
            stamp_version: false,
            ready_delay_ms: 750,
            output: output.clone(),
        })
        .unwrap();

        let native_content = zstd::stream::decode_all(
            File::open(output.join("landscape-webserver-x86_64.zst")).unwrap(),
        )
        .unwrap();
        assert_eq!(native_content, b"native fixture binary\n");
        let placeholder = zstd::stream::decode_all(
            File::open(output.join("landscape-webserver-aarch64.zst")).unwrap(),
        )
        .unwrap();
        assert!(
            String::from_utf8(placeholder)
                .unwrap()
                .contains("placeholder")
        );

        let mut archive =
            zip::ZipArchive::new(File::open(output.join("static.zip")).unwrap()).unwrap();
        assert!(archive.by_name("static/index.html").is_ok());
        let mut config_content = String::new();
        archive
            .by_name("static/lkit-fixture.json")
            .unwrap()
            .read_to_string(&mut config_content)
            .unwrap();
        let config: LandscapeFixtureConfig = serde_json::from_str(&config_content).unwrap();
        assert_eq!(config.export_version, "1.2.3");
        assert_eq!(config.scenario, Scenario::Healthy);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn version_stamp_changes_native_asset_without_changing_prefix() {
        let root = temp_dir("stamp");
        let native = root.join("landscape-webserver");
        let output = root.join("dist");
        std::fs::write(&native, b"native fixture binary\n").unwrap();

        run(Args {
            version: "2.0.0".into(),
            scenario: Scenario::Healthy,
            native_architecture: Architecture::X86_64,
            native_binary: native,
            stamp_version: true,
            ready_delay_ms: 750,
            output: output.clone(),
        })
        .unwrap();

        let native_content = zstd::stream::decode_all(
            File::open(output.join("landscape-webserver-x86_64.zst")).unwrap(),
        )
        .unwrap();
        assert!(native_content.starts_with(b"native fixture binary\n"));
        assert!(native_content.ends_with(b"lkit-fixture-version=2.0.0\n"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
