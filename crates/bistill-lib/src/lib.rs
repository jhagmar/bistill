//! Config and the process error type.
//!
//! This crate owns `key = value` config, environment and flag overrides, and
//! [`Error`]. It does not draw a terminal and it does not call Bitbucket.

#![deny(unsafe_code)]

mod config;

pub use config::{Config, Dirs, Env, Flags, PollSeconds, load};

use std::ffi::OsString;

/// Why a bistill operation failed.
#[derive(Debug)]
pub enum Error {
    /// `curl` or another host program failed. Timeout and a missing binary stay distinct.
    Curl(CurlFault),
    /// An HTTP status the caller treats as failure.
    Http(u16),
    /// TLS verification or the TLS handshake failed.
    Tls(String),
    /// JSON text failed to parse. The payload keeps offset, line, and column.
    Json(json::Error),
    /// Config text, paths, or the token file failed a check.
    Config(ConfigFault),
    /// The token does not match the configured user.
    Auth(String),
    /// A filesystem or process error outside config parsing.
    Io(std::io::Error),
}

/// A host failure that is not TLS.
#[derive(Debug, Eq, PartialEq)]
pub enum CurlFault {
    /// `program` is not on `PATH`.
    Missing { program: String },
    /// The process exceeded its timeout.
    Timeout { program: String },
    /// The process failed before it produced a usable result.
    Failed { program: String, message: String },
}

/// What was wrong with config text or the token file.
#[derive(Debug, Eq, PartialEq)]
pub enum ConfigFault {
    /// A required key was absent after flags, environment, and the file.
    Missing { key: &'static str },
    /// A line named a key this version does not accept.
    Unknown { key: String },
    /// The same key appeared twice.
    Duplicate { key: String },
    /// The value failed a check. `message` is a fixed phrase.
    Value { key: String, message: &'static str },
    /// The token file is group- or world-accessible.
    TokenMode { path: std::path::PathBuf },
    /// The path could not be read.
    Read {
        path: std::path::PathBuf,
        message: String,
    },
    /// The file was not UTF-8.
    Utf8 { path: std::path::PathBuf },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Curl(fault) => write!(f, "{fault}"),
            Error::Http(status) => write!(f, "HTTP {status}"),
            Error::Tls(message) => write!(f, "TLS failed: {message}"),
            Error::Json(err) => write!(f, "{err}"),
            Error::Config(fault) => write!(f, "{fault}"),
            Error::Auth(message) => write!(f, "{message}"),
            Error::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::fmt::Display for CurlFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CurlFault::Missing { program } => write!(f, "{program} is not on PATH"),
            CurlFault::Timeout { program } => write!(f, "{program} timed out"),
            CurlFault::Failed { program, message } => write!(f, "{program} failed: {message}"),
        }
    }
}

impl std::fmt::Display for ConfigFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigFault::Missing { key } => write!(f, "missing {key}"),
            ConfigFault::Unknown { key } => write!(f, "unknown key {key}"),
            ConfigFault::Duplicate { key } => write!(f, "duplicate key {key}"),
            ConfigFault::Value { key, message } => write!(f, "{key} {message}"),
            ConfigFault::TokenMode { path } => {
                write!(
                    f,
                    "token file {} is group or world accessible",
                    path.display()
                )
            }
            ConfigFault::Read { path, message } => {
                write!(f, "cannot read {}: {message}", path.display())
            }
            ConfigFault::Utf8 { path } => write!(f, "{} is not UTF-8", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Json(err) => Some(err),
            Error::Io(err) => Some(err),
            Error::Curl(_) | Error::Http(_) | Error::Tls(_) | Error::Config(_) | Error::Auth(_) => {
                None
            }
        }
    }
}

impl std::error::Error for CurlFault {}

impl std::error::Error for ConfigFault {}

impl From<host::Error> for Error {
    fn from(err: host::Error) -> Self {
        match err {
            host::Error::Missing { program } => Error::Curl(CurlFault::Missing { program }),
            host::Error::Timeout { program } => Error::Curl(CurlFault::Timeout { program }),
            host::Error::Failed { program, message } => {
                Error::Curl(CurlFault::Failed { program, message })
            }
            host::Error::Tls { message } => Error::Tls(message),
        }
    }
}

impl From<json::Error> for Error {
    fn from(err: json::Error) -> Self {
        Error::Json(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

/// Copy `args` for a verbose log. `Bearer` values become `***`.
pub fn redact_argv(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|arg| redact_bearer(&arg.to_string_lossy()))
        .collect()
}

fn redact_bearer(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(index) = rest.find("Bearer ") {
        let start = index + "Bearer ".len();
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 {
            break;
        }
        out.push_str("***");
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
