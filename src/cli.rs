use std::path::PathBuf;

use anyhow::Result;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{
    ArgAction, Args, ColorChoice, Command, CommandFactory, FromArgMatches, Parser, Subcommand,
};

use crate::i18n::{I18nCatalog, locales_dir};

#[derive(Debug, Parser)]
#[command(
    name = "oci-sniper",
    version,
    about = "Oracle Cloud CLI and bot runner"
)]
pub struct Cli {
    #[arg(long, short, global = true, default_value = "config.toml")]
    pub config: PathBuf,
    #[arg(long, short = 'l', global = true, default_value = "en")]
    pub lang: String,
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
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct BotWebhookArgs {
    #[arg(long)]
    pub set: Option<String>,
    #[arg(long)]
    pub clear: bool,
}

pub fn parse_cli() -> Result<Cli> {
    let lang = detect_lang(std::env::args());
    let mut command = localized_command(&lang)?;
    let matches = command.get_matches_mut();
    Ok(Cli::from_arg_matches(&matches)?)
}

fn localized_command(lang: &str) -> Result<Command> {
    let i18n = I18nCatalog::load_from_dir(&locales_dir(), "en")?;
    let mut command = Cli::command()
        .styles(cli_styles())
        .color(ColorChoice::Auto)
        .about(i18n.t(lang, "cli.help.about", &[]))
        .after_help(i18n.t(lang, "cli.help.after", &[]));

    command = command.mut_arg("config", |arg| {
        arg.help(i18n.t(lang, "cli.help.arg.config", &[]))
    });
    command = command.mut_arg("lang", |arg| {
        arg.help(i18n.t(lang, "cli.help.arg.lang", &[]))
            .action(ArgAction::Set)
            .value_parser(["en", "zh-CN", "zh-TW"])
    });

    command = command.mut_subcommand("run", |subcmd| {
        subcmd
            .about(i18n.t(lang, "cli.help.cmd.run", &[]))
            .mut_arg("dry_run", |arg| {
                arg.help(i18n.t(lang, "cli.help.arg.run.dry_run", &[]))
            })
    });
    command = command.mut_subcommand("test-api", |subcmd| {
        subcmd
            .about(i18n.t(lang, "cli.help.cmd.test_api", &[]))
            .mut_arg("dump_launch_payload", |arg| {
                arg.help(i18n.t(lang, "cli.help.arg.test_api.dump_launch_payload", &[]))
            })
            .mut_arg("json", |arg| {
                arg.help(i18n.t(lang, "cli.help.arg.test_api.json", &[]))
            })
    });
    command = command.mut_subcommand("bot-webhook", |subcmd| {
        subcmd
            .about(i18n.t(lang, "cli.help.cmd.bot_webhook", &[]))
            .mut_arg("set", |arg| {
                arg.help(i18n.t(lang, "cli.help.arg.bot_webhook.set", &[]))
            })
            .mut_arg("clear", |arg| {
                arg.help(i18n.t(lang, "cli.help.arg.bot_webhook.clear", &[]))
            })
    });

    Ok(command)
}

fn detect_lang<I>(args: I) -> String
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--lang=") {
            return value.to_string();
        }
        if let Some(value) = arg.strip_prefix("-l=") {
            return value.to_string();
        }
        if matches!(arg.as_str(), "--lang" | "-l")
            && let Some(next) = iter.next()
        {
            return next.to_string();
        }
    }
    "en".to_string()
}

fn cli_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::BrightBlue.on_default())
        .error(AnsiColor::BrightRed.on_default() | Effects::BOLD)
        .valid(AnsiColor::BrightGreen.on_default() | Effects::BOLD)
        .invalid(AnsiColor::BrightRed.on_default() | Effects::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lang_from_long_form() {
        let lang = detect_lang(["oci-sniper", "--lang", "zh-CN", "run"]);
        assert_eq!(lang, "zh-CN");
    }

    #[test]
    fn localizes_root_about() {
        let command = localized_command("zh-CN").unwrap();
        assert_eq!(
            command.get_about().map(|about| about.to_string()),
            Some("Oracle Cloud 本地 CLI 与 Telegram Bot 运行器".to_string())
        );
    }
}
