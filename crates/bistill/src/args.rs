//! Hand-rolled argv for `bistill ping`.

use bistill_lib::Flags;
use std::ffi::OsString;
use std::path::PathBuf;

const USAGE: &str = "\
Usage: bistill ping [--json] [--url URL] [--user SLUG] [--token-file PATH] [--config PATH] [--verbose]
";

/// What to run.
pub(crate) enum Command {
    /// `--help`.
    Help,
    /// `ping`.
    Ping(Ping),
}

/// Flags for `ping`.
#[derive(Default)]
pub(crate) struct Ping {
    /// `--json`.
    pub json: bool,
    /// `--verbose`.
    pub verbose: bool,
    /// Config overrides.
    pub flags: Flags,
}

/// Why argv was rejected.
pub(crate) enum Usage {
    /// No subcommand.
    Bare,
    /// A name this binary does not accept.
    Unknown,
    /// A flag that takes a value had none.
    NeedsValue(&'static str),
    /// An argument was not UTF-8.
    NotUtf8,
}

pub(crate) fn parse(args: &[OsString]) -> Result<Command, Usage> {
    let mut iter = args.iter().skip(1);
    let Some(first) = iter.next() else {
        return Err(Usage::Bare);
    };
    if first == "--help" || first == "-h" {
        return Ok(Command::Help);
    }
    if first != "ping" {
        return Err(Usage::Unknown);
    }
    let mut ping = Ping::default();
    while let Some(arg) = iter.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(Command::Help);
        }
        let name = arg.to_str().ok_or(Usage::NotUtf8)?;
        match name {
            "--json" => ping.json = true,
            "--verbose" => ping.verbose = true,
            "--url" => ping.flags.base_url = Some(value(&mut iter, "--url")?),
            "--user" => ping.flags.username = Some(value(&mut iter, "--user")?),
            "--token-file" => {
                ping.flags.token_file = Some(PathBuf::from(value(&mut iter, "--token-file")?));
            }
            "--config" => {
                ping.flags.config = Some(PathBuf::from(value(&mut iter, "--config")?));
            }
            _ => return Err(Usage::Unknown),
        }
    }
    Ok(Command::Ping(ping))
}

fn value<'a>(
    iter: &mut impl Iterator<Item = &'a OsString>,
    flag: &'static str,
) -> Result<String, Usage> {
    let Some(arg) = iter.next() else {
        return Err(Usage::NeedsValue(flag));
    };
    match arg.to_str() {
        Some(text) if !text.is_empty() && !text.starts_with('-') => Ok(text.to_owned()),
        Some(_) => Err(Usage::NeedsValue(flag)),
        None => Err(Usage::NotUtf8),
    }
}

pub(crate) fn help_text() -> String {
    format!(
        "\
{USAGE}\
--json prints the raw response bodies.
--url sets the Bitbucket origin.
--user sets the username.
--token-file sets the token file.
--config sets the config file.
--verbose prints the curl arguments with the token redacted.
--help prints this text.
"
    )
}

pub(crate) fn usage_text(usage: &Usage) -> String {
    let reason = match usage {
        Usage::Bare => "Run ping.".to_owned(),
        Usage::Unknown => "Unknown argument.".to_owned(),
        Usage::NeedsValue(flag) => format!("{flag} needs a value."),
        Usage::NotUtf8 => "The argument must be UTF-8.".to_owned(),
    };
    format!("{reason}\n{USAGE}")
}
