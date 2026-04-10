use anyhow::Result;
use clap::Parser;
use oci_sniper_rs::app::App;
use oci_sniper_rs::cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let app = App::new(cli.config.clone());

    match cli.command {
        Commands::Run(args) => app.run_command(args).await?,
        Commands::TestApi(args) => app.test_api_command(args).await?,
        Commands::BotWebhook(args) => app.bot_webhook_command(args).await?,
    }

    Ok(())
}
