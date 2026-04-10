use std::path::Path;

use anyhow::{Context, Result, anyhow};

use crate::bot::{clear_webhook, configure_webhook, run_bot};
use crate::cli::{BotWebhookArgs, RunArgs, TestApiArgs};
use crate::config::{AppConfig, LaunchMode, TelegramMode};
use crate::i18n::{I18nCatalog, locales_dir};
use crate::lock::ProcessLock;
use crate::logging::initialize_logging;
use crate::oci::{CreateInstanceRequest, LaunchPlanner, LaunchStrategy, OciClient, ResolvedLaunch};

pub struct App {
    pub config_path: std::path::PathBuf,
    pub cli_lang: Option<String>,
}

impl App {
    pub fn new(config_path: std::path::PathBuf, cli_lang: Option<String>) -> Self {
        Self {
            config_path,
            cli_lang,
        }
    }

    pub async fn run_command(&self, args: RunArgs) -> Result<()> {
        let config = self.load_config()?;
        let locale = self.effective_locale(&config);
        initialize_logging(&config.app.log_dir)?;
        let i18n = I18nCatalog::initialize(&locales_dir(), locale.clone())?;
        let credentials = config.oci.resolve_credentials()?;
        let client = OciClient::new(credentials.clone());
        let resolved = resolve_launch_config(&config, &client).await?;
        let payload = CreateInstanceRequest::from_launch_config(&resolved.launch_config)?;

        println!(
            "{}",
            i18n.t(
                &locale,
                "cli.config.loaded",
                &[("path", &self.config_path.display().to_string())]
            )
        );
        println!(
            "{}",
            i18n.t(
                &locale,
                "cli.region.current",
                &[("region", &credentials.region)]
            )
        );
        println!(
            "{}",
            i18n.t(&locale, "cli.shape.current", &[("shape", &payload.shape)])
        );
        print_launch_diagnostics(&locale, i18n, &resolved);
        println!(
            "{}",
            i18n.t(
                &locale,
                "cli.telegram.mode",
                &[("mode", telegram_mode_label(&config.telegram.mode))]
            )
        );
        print_runtime_summary(&locale, i18n, &config);

        if args.dry_run {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            return Ok(());
        }

        let runtime_lock = ProcessLock::acquire(&config.app.lock_file).map_err(|_| {
            anyhow!(i18n.t(
                &locale,
                "cli.runtime.lock_busy",
                &[("path", &config.app.lock_file.display().to_string())]
            ))
        })?;
        println!(
            "{}",
            i18n.t(
                &locale,
                "cli.runtime.lock_acquired",
                &[("path", &runtime_lock.path().display().to_string())]
            )
        );
        println!("{}", i18n.t(&locale, "cli.runtime.bootstrap", &[]));
        if config.telegram.bot_token.is_some() {
            match config.telegram.mode {
                TelegramMode::Polling => {
                    println!("{}", i18n.t(&locale, "cli.bot.starting_polling", &[]));
                    run_bot(self.config_path.clone(), locale).await?;
                }
                TelegramMode::Webhook => {
                    let url = config.telegram.webhook_url.clone().ok_or_else(|| {
                        anyhow!(i18n.t(&locale, "cli.bot.webhook_missing_url", &[]))
                    })?;
                    println!(
                        "{}",
                        i18n.t(&locale, "cli.bot.starting_webhook", &[("url", &url)])
                    );
                    run_bot(self.config_path.clone(), locale).await?;
                }
            }
        } else {
            println!("{}", i18n.t(&locale, "cli.bot.disabled", &[]));
        }
        Ok(())
    }

    pub async fn test_api_command(&self, args: TestApiArgs) -> Result<()> {
        let config = self.load_config()?;
        let locale = self.effective_locale(&config);
        initialize_logging(&config.app.log_dir)?;
        let i18n = I18nCatalog::initialize(&locales_dir(), locale.clone())?;
        println!("{}", i18n.t(&locale, "cli.test_api.phase.credentials", &[]));
        let credentials = config
            .oci
            .resolve_credentials()
            .map_err(|error| classify_test_api_error(&locale, i18n, &error))?;
        let client = OciClient::new(credentials);
        println!("{}", i18n.t(&locale, "cli.test_api.phase.discovery", &[]));
        let resolved = resolve_launch_config(&config, &client)
            .await
            .map_err(|error| classify_test_api_error(&locale, i18n, &error))?;
        let launch_config = &resolved.launch_config;
        print_launch_diagnostics(&locale, i18n, &resolved);
        println!("{}", i18n.t(&locale, "cli.test_api.phase.auth", &[]));
        client
            .test_auth(&launch_config.compartment_id)
            .await
            .map_err(|error| classify_test_api_error(&locale, i18n, &error))
            .context("OCI auth test request failed")?;
        println!("{}", i18n.t(&locale, "cli.test_api.success", &[]));

        if args.dump_launch_payload {
            let payload = CreateInstanceRequest::from_launch_config(launch_config)?;
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }

        Ok(())
    }

    pub async fn bot_webhook_command(&self, args: BotWebhookArgs) -> Result<()> {
        let mut config = self.load_config()?;
        let locale = self.effective_locale(&config);
        initialize_logging(&config.app.log_dir)?;
        let i18n = I18nCatalog::initialize(&locales_dir(), locale.clone())?;
        match (args.set, args.clear) {
            (Some(url), false) => {
                config.telegram.mode = TelegramMode::Webhook;
                config.telegram.webhook_url = Some(url.clone());
                if config.telegram.bot_token.is_some() {
                    configure_webhook(&config, &url).await?;
                }
                config.save_to_path(&self.config_path)?;
                println!(
                    "{}",
                    i18n.t(&locale, "cli.bot.webhook_set", &[("url", &url)])
                );
            }
            (None, true) => {
                config.telegram.mode = TelegramMode::Polling;
                config.telegram.webhook_url = None;
                if config.telegram.bot_token.is_some() {
                    clear_webhook(&config).await?;
                }
                config.save_to_path(&self.config_path)?;
                println!("{}", i18n.t(&locale, "cli.bot.webhook_cleared", &[]));
            }
            _ => return Err(anyhow!("either --set <URL> or --clear must be specified")),
        }
        Ok(())
    }

    fn load_config(&self) -> Result<AppConfig> {
        AppConfig::load_from_path(Path::new(&self.config_path))
    }

    fn effective_locale(&self, config: &AppConfig) -> String {
        self.cli_lang
            .clone()
            .unwrap_or_else(|| config.app.locale.clone())
    }
}

async fn resolve_launch_config(config: &AppConfig, client: &OciClient) -> Result<ResolvedLaunch> {
    match config.instance.effective_launch_config() {
        LaunchMode::Explicit(config) => {
            let selected_shape_ocpus = config.shape_config.as_ref().map(|shape| shape.ocpus);
            let selected_shape_memory_in_gbs = config
                .shape_config
                .as_ref()
                .map(|shape| shape.memory_in_gbs);
            Ok(ResolvedLaunch {
                strategy: LaunchStrategy::ExplicitConfig,
                launch_config: config,
                selected_shape_ocpus,
                selected_shape_memory_in_gbs,
            })
        }
        LaunchMode::FreeTierFallback(defaults) => {
            LaunchPlanner::new(defaults).resolve_defaults(client).await
        }
    }
}

fn print_launch_diagnostics(locale: &str, i18n: &I18nCatalog, resolved: &ResolvedLaunch) {
    println!(
        "{}",
        i18n.t(
            locale,
            "cli.launch.strategy",
            &[("strategy", launch_strategy_label(resolved.strategy))]
        )
    );
    println!(
        "{}",
        i18n.t(
            locale,
            "cli.launch.ad",
            &[(
                "availability_domain",
                &resolved.launch_config.availability_domain
            )]
        )
    );
    println!(
        "{}",
        i18n.t(
            locale,
            "cli.launch.subnet",
            &[("subnet_id", &resolved.launch_config.subnet_id)]
        )
    );
    println!(
        "{}",
        i18n.t(
            locale,
            "cli.launch.image",
            &[("image_id", &resolved.launch_config.image_id)]
        )
    );

    if let (Some(ocpus), Some(memory)) = (
        resolved.selected_shape_ocpus,
        resolved.selected_shape_memory_in_gbs,
    ) {
        let ocpus = ocpus.to_string();
        let memory = memory.to_string();
        println!(
            "{}",
            i18n.t(
                locale,
                "cli.launch.shape_config",
                &[("ocpus", &ocpus), ("memory_in_gbs", &memory)]
            )
        );
    }
}

fn print_runtime_summary(locale: &str, i18n: &I18nCatalog, config: &AppConfig) {
    println!(
        "{}",
        i18n.t(
            locale,
            "cli.runtime.log_dir",
            &[("path", &config.app.log_dir.display().to_string())]
        )
    );
    println!(
        "{}",
        i18n.t(
            locale,
            "cli.runtime.lock_file",
            &[("path", &config.app.lock_file.display().to_string())]
        )
    );
    println!(
        "{}",
        i18n.t(
            locale,
            "cli.runtime.bot_mode",
            &[("mode", telegram_mode_label(&config.telegram.mode))]
        )
    );

    if let TelegramMode::Webhook = config.telegram.mode {
        if let Some(url) = &config.telegram.webhook_url {
            println!(
                "{}",
                i18n.t(locale, "cli.runtime.webhook_url", &[("url", url)])
            );
        }
        if let Some(address) = &config.telegram.webhook_listen {
            println!(
                "{}",
                i18n.t(
                    locale,
                    "cli.runtime.webhook_listen",
                    &[("address", address)]
                )
            );
        }
        if let Some(path) = &config.telegram.webhook_path {
            println!(
                "{}",
                i18n.t(locale, "cli.runtime.webhook_path", &[("path", path)])
            );
        }
    }
}

fn telegram_mode_label(mode: &TelegramMode) -> &'static str {
    match mode {
        TelegramMode::Polling => "polling",
        TelegramMode::Webhook => "webhook",
    }
}

fn launch_strategy_label(strategy: LaunchStrategy) -> &'static str {
    match strategy {
        LaunchStrategy::ExplicitConfig => "explicit",
        LaunchStrategy::FreeTierFallback => "free-tier-fallback",
    }
}

fn classify_test_api_error(
    locale: &str,
    i18n: &I18nCatalog,
    error: &anyhow::Error,
) -> anyhow::Error {
    let details = error.to_string();
    let key = match classify_error_kind(&details) {
        TestApiErrorKind::Configuration => "cli.test_api.error.configuration",
        TestApiErrorKind::Discovery => "cli.test_api.error.discovery",
        TestApiErrorKind::Auth => "cli.test_api.error.auth",
        TestApiErrorKind::Network => "cli.test_api.error.network",
        TestApiErrorKind::Unknown => "cli.test_api.error.unknown",
    };
    anyhow!(i18n.t(locale, key, &[("details", &details)]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestApiErrorKind {
    Configuration,
    Discovery,
    Auth,
    Network,
    Unknown,
}

fn classify_error_kind(details: &str) -> TestApiErrorKind {
    let lower = details.to_ascii_lowercase();
    if lower.contains("failed to read config file")
        || lower.contains("failed to parse config file")
        || lower.contains("failed to read oci config file")
        || lower.contains("oci profile")
        || lower.contains("missing 'user'")
        || lower.contains("missing 'fingerprint'")
        || lower.contains("missing 'tenancy'")
        || lower.contains("missing 'region'")
        || lower.contains("missing 'key_file'")
    {
        return TestApiErrorKind::Configuration;
    }
    if lower.contains("no oci availability domain found")
        || lower.contains("no available subnet found")
        || lower.contains("no oracle linux image found")
        || lower.contains("no free-tier shape candidates configured")
        || lower.contains("failed to resolve default ssh public key")
    {
        return TestApiErrorKind::Discovery;
    }
    if lower.contains("oci request failed with status")
        || lower.contains("oci auth test request failed")
    {
        return TestApiErrorKind::Auth;
    }
    if lower.contains("failed to execute oci get request")
        || lower.contains("failed to execute oci post request")
        || lower.contains("failed to build oci get url")
    {
        return TestApiErrorKind::Network;
    }
    TestApiErrorKind::Unknown
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn classifies_resource_discovery_errors() {
        assert_eq!(
            classify_error_kind("no available subnet found in tenancy compartment"),
            TestApiErrorKind::Discovery
        );
    }

    #[test]
    fn classifies_network_errors() {
        assert_eq!(
            classify_error_kind("failed to execute OCI GET request"),
            TestApiErrorKind::Network
        );
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

        let app = App::new(path.clone(), None);
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

    #[test]
    fn app_section_defaults_include_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[oci]\nconfig_file = \"/tmp/oci-config\"\n").unwrap();

        let config = AppConfig::load_from_path(&path).unwrap();

        assert_eq!(
            config.app.lock_file,
            std::path::PathBuf::from(".oci-sniper.lock")
        );
    }
}
