mod commands;

use clap::Parser;
use commands::Commands;

#[derive(Debug, Parser)]
#[command(name = "lkit")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Check(_) => {
            println!("check");
        }
    }
}
