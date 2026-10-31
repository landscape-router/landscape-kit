mod task_names;

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

fn main() {
    let sequence = std::sync::atomic::AtomicUsize::new(0);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name_fn(move || {
            format!(
                "{}-{}",
                task_names::RUNTIME,
                sequence.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        })
        .build()
        .expect("build publisher runtime")
        .block_on(async_main());
}

async fn async_main() {
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
