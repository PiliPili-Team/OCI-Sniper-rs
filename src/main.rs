use anyhow::Result;
use oci_sniper_rs::app::App;
use oci_sniper_rs::cli::{Commands, parse_cli};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = parse_cli()?;
    let app = App::new(cli.config.clone(), Some(cli.lang.clone()));

    match cli.command {
        Commands::Run(args) => app.run_command(args).await?,
        Commands::TestApi(args) => app.test_api_command(args).await?,
        Commands::BotWebhook(args) => app.bot_webhook_command(args).await?,
    }

    Ok(())
}
