use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "oci-sniper",
    version,
    about = "Oracle Cloud CLI and bot runner"
)]
pub struct Cli {
    #[arg(long, short, global = true, default_value = "config.toml")]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Run(RunArgs),
    TestApi(TestApiArgs),
    BotWebhook(BotWebhookArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct TestApiArgs {
    #[arg(long)]
    pub dump_launch_payload: bool,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct BotWebhookArgs {
    #[arg(long)]
    pub set: Option<String>,
    #[arg(long)]
    pub clear: bool,
}
