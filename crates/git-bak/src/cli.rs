use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "git-bak")]
#[command(about = "Persona backup and rollback diagnostics")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Init { path: Option<PathBuf> },
    Run,
    Status,
}
