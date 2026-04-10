use std::path::Path;

use anyhow::{Context, Result, anyhow};

use crate::cli::{BotWebhookArgs, RunArgs, TestApiArgs};
use crate::config::{AppConfig, LaunchInstanceConfig, LaunchMode, TelegramMode};
use crate::i18n::{I18nCatalog, locales_dir};
use crate::logging::initialize_logging;
use crate::oci::{CreateInstanceRequest, LaunchPlanner, OciClient};

pub struct App {
    pub config_path: std::path::PathBuf,
}

impl App {
    pub fn new(config_path: std::path::PathBuf) -> Self {
        Self { config_path }
    }

    pub async fn run_command(&self, args: RunArgs) -> Result<()> {
        let config = self.load_config()?;
        initialize_logging(&config.app.log_dir)?;
        let i18n = I18nCatalog::initialize(&locales_dir(), config.app.locale.clone())?;
        let credentials = config.oci.resolve_credentials()?;
        let client = OciClient::new(credentials.clone());
        let launch_config = resolve_launch_config(&config, &client).await?;
        let payload = CreateInstanceRequest::from_launch_config(&launch_config)?;

        println!(
            "{}",
            i18n.t(
                &config.app.locale,
                "cli.config.loaded",
                &[("path", &self.config_path.display().to_string())]
            )
        );
        println!(
            "{}",
            i18n.t(
                &config.app.locale,
                "cli.region.current",
                &[("region", &credentials.region)]
            )
        );
        println!(
            "{}",
            i18n.t(
                &config.app.locale,
                "cli.shape.current",
                &[("shape", &payload.shape)]
            )
        );
        println!(
            "{}",
            i18n.t(
                &config.app.locale,
                "cli.telegram.mode",
                &[("mode", telegram_mode_label(&config.telegram.mode))]
            )
        );

        if args.dry_run {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            return Ok(());
        }

        println!(
            "{}",
            i18n.t(&config.app.locale, "cli.runtime.bootstrap", &[])
        );
        Ok(())
    }

    pub async fn test_api_command(&self, args: TestApiArgs) -> Result<()> {
        let config = self.load_config()?;
        initialize_logging(&config.app.log_dir)?;
        let i18n = I18nCatalog::initialize(&locales_dir(), config.app.locale.clone())?;
        let credentials = config.oci.resolve_credentials()?;
        let client = OciClient::new(credentials);
        let launch_config = resolve_launch_config(&config, &client).await?;
        client
            .test_auth(&launch_config.compartment_id)
            .await
            .context("OCI auth test request failed")?;
        println!(
            "{}",
            i18n.t(&config.app.locale, "cli.test_api.success", &[])
        );

        if args.dump_launch_payload {
            let payload = CreateInstanceRequest::from_launch_config(&launch_config)?;
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }

        Ok(())
    }

    pub async fn bot_webhook_command(&self, args: BotWebhookArgs) -> Result<()> {
        let mut config = self.load_config()?;
        initialize_logging(&config.app.log_dir)?;
        let i18n = I18nCatalog::initialize(&locales_dir(), config.app.locale.clone())?;
        match (args.set, args.clear) {
            (Some(url), false) => {
                config.telegram.mode = TelegramMode::Webhook;
                config.telegram.webhook_url = Some(url.clone());
                config.save_to_path(&self.config_path)?;
                println!(
                    "{}",
                    i18n.t(&config.app.locale, "cli.bot.webhook_set", &[("url", &url)])
                );
            }
            (None, true) => {
                config.telegram.mode = TelegramMode::Polling;
                config.telegram.webhook_url = None;
                config.save_to_path(&self.config_path)?;
                println!(
                    "{}",
                    i18n.t(&config.app.locale, "cli.bot.webhook_cleared", &[])
                );
            }
            _ => return Err(anyhow!("either --set <URL> or --clear must be specified")),
        }
        Ok(())
    }

    fn load_config(&self) -> Result<AppConfig> {
        AppConfig::load_from_path(Path::new(&self.config_path))
    }
}

async fn resolve_launch_config(
    config: &AppConfig,
    client: &OciClient,
) -> Result<LaunchInstanceConfig> {
    match config.instance.effective_launch_config() {
        LaunchMode::Explicit(config) => Ok(config),
        LaunchMode::FreeTierFallback(defaults) => {
            LaunchPlanner::new(defaults).resolve_defaults(client).await
        }
    }
}

fn telegram_mode_label(mode: &TelegramMode) -> &'static str {
    match mode {
        TelegramMode::Polling => "polling",
        TelegramMode::Webhook => "webhook",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bot_webhook_clear_switches_back_to_polling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let locales = dir.path().join("locales");
        fs::create_dir_all(&locales).unwrap();
        fs::write(
            locales.join("en.json"),
            r#"{"cli.bot.webhook_cleared":"Webhook cleared; fallback to polling."}"#,
        )
        .unwrap();
        fs::write(
            locales.join("zh-CN.json"),
            r#"{"cli.bot.webhook_cleared":"Webhook 已清除，已回退到 polling 模式。"}"#,
        )
        .unwrap();
        fs::write(
            locales.join("zh-TW.json"),
            r#"{"cli.bot.webhook_cleared":"Webhook 已清除，已回退至 polling 模式。"}"#,
        )
        .unwrap();
        fs::write(
            &path,
            r#"
[app]
locale = "en"

[oci]
config_file = "/tmp/oci-config"

[telegram]
mode = "webhook"
webhook_url = "https://example.com/hook"
"#,
        )
        .unwrap();

        let app = App::new(path.clone());
        std::env::set_current_dir(dir.path()).unwrap();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(app.bot_webhook_command(BotWebhookArgs {
                set: None,
                clear: true,
            }))
            .unwrap();

        let updated = AppConfig::load_from_path(&path).unwrap();
        assert_eq!(updated.telegram.mode, TelegramMode::Polling);
        assert!(updated.telegram.webhook_url.is_none());
    }
}
