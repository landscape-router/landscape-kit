use std::path::PathBuf;

use clap::Parser;
use lkit_publish::{PublishConfig, publish};

#[derive(Debug, Parser)]
#[command(
    version,
    disable_version_flag = true,
    about = "Publish a Landscape HTTP repository release"
)]
struct Args {
    #[arg(long)]
    version: String,
    #[arg(long)]
    directory: PathBuf,
    #[arg(long, env = "RUSTFS_ENDPOINT")]
    endpoint: String,
    #[arg(long, env = "RUSTFS_BUCKET")]
    bucket: String,
    #[arg(long, env = "RUSTFS_PUBLIC_BASE_URL")]
    public_base_url: Option<String>,
    #[arg(long, env = "AWS_REGION", default_value = "us-east-1")]
    region: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let config = PublishConfig {
        version: args.version,
        directory: args.directory,
        endpoint: args.endpoint,
        bucket: args.bucket,
        public_base_url: args.public_base_url,
        region: args.region,
    };
    if let Err(error) = publish(config).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
