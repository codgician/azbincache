use anyhow::Result;
use azbincache::cli::{self, Cli, Command};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    cli.init_tracing();

    match cli.command {
        Command::Push(args) => cli::push::run(args).await,
        Command::Gc(args) => cli::gc::run(args).await,
        Command::Info(args) => cli::info::run(args).await,
        Command::Pubkey(args) => cli::pubkey::run(args),
        Command::Doctor(args) => cli::doctor::run(args).await,
    }
}
