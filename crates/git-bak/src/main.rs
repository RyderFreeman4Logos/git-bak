mod cli;
mod commands;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Init { path } => commands::init::execute(path),
        Commands::Run => commands::run::execute().await,
        Commands::Status => commands::status::execute(),
        Commands::Hook { event } => commands::hook::execute(event),
        Commands::Checkpoint => commands::checkpoint::execute(),
        Commands::Bisect { good, bad, run } => commands::bisect::execute(good, bad, run),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
