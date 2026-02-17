use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "git-bak")]
#[command(about = "Persona backup and rollback diagnostics")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init { path: Option<String> },
    Run,
    Status,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let _cli = Cli::parse();
}
