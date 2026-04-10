use anyhow::{Context, Result};
use teloxide::prelude::*;
use teloxide::types::{InputFile, ParseMode};
use url::Url;

use crate::config::AppConfig;
use crate::i18n::I18nCatalog;
use crate::logging::{latest_log_tail, zip_logs};

const TELEGRAM_ARCHIVE_LIMIT: u64 = 50 * 1024 * 1024;

pub async fn run_bot(config_path: std::path::PathBuf) -> Result<()> {
    let config = AppConfig::load_from_path(&config_path)?;
    let token = config
        .telegram
        .bot_token
        .clone()
        .context("telegram.bot_token is required to run the bot")?;
    let default_locale = config.app.locale.clone();
    let log_dir = config.app.log_dir.clone();
    let bot = Bot::new(token);

    teloxide::repl(bot, move |bot: Bot, message: Message| {
        let config_path = config_path.clone();
        let default_locale = default_locale.clone();
        let log_dir = log_dir.clone();
        async move {
            if let Err(error) =
                handle_message(bot, message, config_path, default_locale, log_dir).await
            {
                eprintln!("telegram bot handler error: {error:#}");
            }
            respond(())
        }
    })
    .await;

    Ok(())
}

pub async fn configure_webhook(config: &AppConfig, webhook_url: &str) -> Result<()> {
    let token = config
        .telegram
        .bot_token
        .clone()
        .context("telegram.bot_token is required to set a webhook")?;
    let url = Url::parse(webhook_url).context("invalid webhook URL")?;
    Bot::new(token)
        .set_webhook(url)
        .send()
        .await
        .context("failed to set Telegram webhook")?;
    Ok(())
}

pub async fn clear_webhook(config: &AppConfig) -> Result<()> {
    let token = config
        .telegram
        .bot_token
        .clone()
        .context("telegram.bot_token is required to clear a webhook")?;
    Bot::new(token)
        .delete_webhook()
        .send()
        .await
        .context("failed to clear Telegram webhook")?;
    Ok(())
}

async fn handle_message(
    bot: Bot,
    message: Message,
    config_path: std::path::PathBuf,
    default_locale: String,
    log_dir: std::path::PathBuf,
) -> Result<()> {
    let Some(text) = message.text() else {
        return Ok(());
    };

    let i18n = I18nCatalog::global();
    let mut config = AppConfig::load_from_path(&config_path)?;
    let chat_id = message.chat.id.0;
    let locale = config.telegram.preferred_locale(chat_id, &default_locale);

    match parse_command(text) {
        ParsedCommand::Start | ParsedCommand::Help => {
            bot.send_message(message.chat.id, i18n.t(&locale, "bot.command.help", &[]))
                .await?;
        }
        ParsedCommand::Language(Some(next_locale)) => {
            config.telegram.set_preferred_locale(chat_id, &next_locale);
            config.save_to_path(&config_path)?;
            bot.send_message(
                message.chat.id,
                i18n.t(
                    &config.telegram.preferred_locale(chat_id, &default_locale),
                    "bot.command.language.updated",
                    &[("locale", &next_locale)],
                ),
            )
            .await?;
        }
        ParsedCommand::Language(None) => {
            bot.send_message(
                message.chat.id,
                i18n.t(&locale, "bot.command.language.usage", &[]),
            )
            .await?;
        }
        ParsedCommand::Logs(limit) => {
            let archive_path = zip_logs(&log_dir, limit)?;
            let file_size = std::fs::metadata(&archive_path)
                .with_context(|| format!("failed to stat archive {}", archive_path.display()))?
                .len();
            if file_size > TELEGRAM_ARCHIVE_LIMIT {
                bot.send_message(
                    message.chat.id,
                    i18n.t(&locale, "bot.command.logs.archive_too_large", &[]),
                )
                .await?;
                return Ok(());
            }

            bot.send_document(message.chat.id, InputFile::file(archive_path.clone()))
                .await?;
            let count = limit.unwrap_or(count_log_files(&log_dir)?).to_string();
            bot.send_message(
                message.chat.id,
                i18n.t(&locale, "bot.command.logs.sent", &[("count", &count)]),
            )
            .await?;
        }
        ParsedCommand::LogLatestTail(lines) => {
            let tail = latest_log_tail(&log_dir, lines.unwrap_or(100), 3800)?;
            match tail {
                Some(contents) if !contents.is_empty() => {
                    bot.send_message(
                        message.chat.id,
                        format!("<pre>{}</pre>", escape_html(&contents)),
                    )
                    .parse_mode(ParseMode::Html)
                    .await?;
                }
                Some(_) => {
                    bot.send_message(
                        message.chat.id,
                        i18n.t(&locale, "bot.command.logs.latest_empty", &[]),
                    )
                    .await?;
                }
                None => {
                    bot.send_message(
                        message.chat.id,
                        i18n.t(&locale, "bot.command.log.empty", &[]),
                    )
                    .await?;
                }
            }
        }
        ParsedCommand::Unknown => {
            bot.send_message(message.chat.id, i18n.t(&locale, "bot.command.unknown", &[]))
                .await?;
        }
    }

    Ok(())
}

fn count_log_files(log_dir: &std::path::Path) -> Result<usize> {
    Ok(std::fs::read_dir(log_dir)
        .map(|entries| entries.filter_map(|entry| entry.ok()).count())
        .unwrap_or_default())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedCommand {
    Start,
    Help,
    Language(Option<String>),
    Logs(Option<usize>),
    LogLatestTail(Option<usize>),
    Unknown,
}

fn parse_command(input: &str) -> ParsedCommand {
    let mut parts = input.split_whitespace();
    match parts.next().unwrap_or_default() {
        "/start" => ParsedCommand::Start,
        "/help" => ParsedCommand::Help,
        "/language" => ParsedCommand::Language(parts.next().map(ToString::to_string)),
        "/logs" => ParsedCommand::Logs(parts.next().and_then(|value| value.parse().ok())),
        "/log_latest_tail" => {
            ParsedCommand::LogLatestTail(parts.next().and_then(|value| value.parse().ok()))
        }
        _ => ParsedCommand::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_language_command() {
        assert_eq!(
            parse_command("/language zh-CN"),
            ParsedCommand::Language(Some("zh-CN".to_string()))
        );
    }

    #[test]
    fn parses_logs_with_optional_limit() {
        assert_eq!(parse_command("/logs 3"), ParsedCommand::Logs(Some(3)));
        assert_eq!(parse_command("/logs"), ParsedCommand::Logs(None));
    }

    #[test]
    fn escapes_html_for_tail_messages() {
        assert_eq!(escape_html("<tag>&"), "&lt;tag&gt;&amp;");
    }
}
