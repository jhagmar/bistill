//! Load `key = value` config, then let environment variables and flags override it.

use crate::{ConfigFault, Error};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[path = "dirs_unix.rs"]
mod dirs;
#[cfg(windows)]
#[path = "dirs_windows.rs"]
mod dirs;

pub use dirs::Dirs;
#[cfg(test)]
pub(crate) use dirs::sample_state;

/// Process environment the loader reads. Empty values count as unset.
#[derive(Clone, Eq, PartialEq)]
pub struct Env {
    /// `BISTILL_URL`.
    pub base_url: Option<String>,
    /// `BISTILL_USER`.
    pub username: Option<String>,
    /// `BISTILL_TOKEN`. When set, the token file is not read.
    pub token: Option<String>,
    /// `BISTILL_TOKEN_FILE`.
    pub token_file: Option<PathBuf>,
    /// Config and state directories for this operating system.
    pub dirs: Dirs,
}

impl std::fmt::Debug for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Env")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("token_file", &self.token_file)
            .field("dirs", &self.dirs)
            .finish()
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// An empty environment. Tests start here and set the fields they need.
    pub fn new() -> Self {
        Self {
            base_url: None,
            username: None,
            token: None,
            token_file: None,
            dirs: Dirs::new(),
        }
    }

    /// Read `BISTILL_*` and this operating system's directory variables.
    pub fn from_process() -> Self {
        Self {
            base_url: nonempty(std::env::var("BISTILL_URL")),
            username: nonempty(std::env::var("BISTILL_USER")),
            token: nonempty(std::env::var("BISTILL_TOKEN")),
            token_file: path_from(nonempty(std::env::var("BISTILL_TOKEN_FILE"))),
            dirs: Dirs::from_process(),
        }
    }
}

/// CLI overrides. A set field wins over the environment and the file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Flags {
    /// `--config`. This path is the only file consulted.
    pub config: Option<PathBuf>,
    /// `--url`.
    pub base_url: Option<String>,
    /// `--user`.
    pub username: Option<String>,
    /// `--token-file`.
    pub token_file: Option<PathBuf>,
}

/// Loaded settings. `token` is omitted from [`Debug`].
#[derive(Clone, Eq, PartialEq)]
pub struct Config {
    /// Bitbucket origin, including a context path, with no trailing slash.
    pub base_url: String,
    /// Bitbucket username / slug.
    pub username: String,
    token: String,
    /// Poll interval. Values below 15 become 15.
    pub poll_seconds: PollSeconds,
    /// Age in days after which a row is stale.
    pub stale_days: u32,
    /// Optional `--cacert` path.
    pub ca_file: Option<PathBuf>,
    /// Optional log path.
    pub log_file: Option<PathBuf>,
    /// Directory for `snapshot.json` and `poll.lock`.
    pub state_dir: PathBuf,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("token", &"***")
            .field("poll_seconds", &self.poll_seconds)
            .field("stale_days", &self.stale_days)
            .field("ca_file", &self.ca_file)
            .field("log_file", &self.log_file)
            .field("state_dir", &self.state_dir)
            .finish()
    }
}

impl Config {
    /// The token from `BISTILL_TOKEN` or the token file.
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Poll interval in seconds, never below [`PollSeconds::FLOOR`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PollSeconds(u64);

impl PollSeconds {
    /// Lowest accepted interval.
    pub const FLOOR: u64 = 15;
    /// Interval when `poll_seconds` is absent.
    pub const DEFAULT: Self = Self(60);

    /// Build an interval. `seconds` below [`Self::FLOOR`] becomes the floor.
    pub fn new(seconds: u64) -> Self {
        Self(seconds.max(Self::FLOOR))
    }

    /// The interval in seconds.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Load config for `cwd`.
///
/// File search: `flags.config`; else `cwd/bistill.conf`; else this operating
/// system's config directory when that file exists. `#` starts a comment.
/// Paths are literal. Flags win over `env`, which wins over the file.
/// `env.token` wins over a token file and the file is not read.
pub fn load(cwd: &Path, flags: &Flags, env: &Env) -> Result<Config, Error> {
    let text = read_config_file(cwd, flags, env)?;
    let keys = match &text {
        Some(text) => parse_keys(text)?,
        None => BTreeMap::new(),
    };
    let base_url = require(
        "base_url",
        pick(
            flags.base_url.as_deref(),
            env.base_url.as_deref(),
            keys.get("base_url").map(String::as_str),
        ),
    )?;
    if base_url.ends_with('/') {
        return Err(config(ConfigFault::Value {
            key: "base_url".to_owned(),
            message: "trailing slash",
        }));
    }
    let username = require(
        "username",
        pick(
            flags.username.as_deref(),
            env.username.as_deref(),
            keys.get("username").map(String::as_str),
        ),
    )?;
    let token = match env.token.as_deref() {
        Some(token) => require("BISTILL_TOKEN", Some(token))?,
        None => {
            let path = pick_path(
                flags.token_file.as_deref(),
                env.token_file.as_deref(),
                keys.get("token_file").map(String::as_str),
            );
            read_token(&require_path("token_file", path)?)?
        }
    };
    let poll_seconds = match keys.get("poll_seconds") {
        Some(text) => PollSeconds::new(parse_u64("poll_seconds", text)?),
        None => PollSeconds::DEFAULT,
    };
    let stale_days = match keys.get("stale_days") {
        Some(text) => parse_u32("stale_days", text)?,
        None => 7,
    };
    let state_dir = match keys.get("state_dir") {
        Some(path) => PathBuf::from(path),
        None => dirs::state_dir(&env.dirs)?,
    };
    Ok(Config {
        base_url,
        username,
        token,
        poll_seconds,
        stale_days,
        ca_file: keys.get("ca_file").map(PathBuf::from),
        log_file: keys.get("log_file").map(PathBuf::from),
        state_dir,
    })
}

fn read_config_file(cwd: &Path, flags: &Flags, env: &Env) -> Result<Option<String>, Error> {
    if let Some(path) = &flags.config {
        if !path.is_file() {
            return Err(config(ConfigFault::Read {
                path: path.clone(),
                message: "not a file".to_owned(),
            }));
        }
        return Ok(Some(read_utf8(path)?));
    }
    let local = cwd.join("bistill.conf");
    if local.is_file() {
        return Ok(Some(read_utf8(&local)?));
    }
    match dirs::config_path(&env.dirs) {
        Ok(path) if path.is_file() => Ok(Some(read_utf8(&path)?)),
        Ok(_) => Ok(None),
        Err(_) => Ok(None),
    }
}

fn parse_keys(text: &str) -> Result<BTreeMap<String, String>, Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut keys = BTreeMap::new();
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(config(ConfigFault::Value {
                key: line.to_owned(),
                message: "missing '='",
            }));
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            return Err(config(ConfigFault::Value {
                key: "key".to_owned(),
                message: "empty",
            }));
        }
        if value.is_empty() {
            return Err(config(ConfigFault::Value {
                key: key.to_owned(),
                message: "empty",
            }));
        }
        if !known(key) {
            return Err(config(ConfigFault::Unknown {
                key: key.to_owned(),
            }));
        }
        if keys.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(config(ConfigFault::Duplicate {
                key: key.to_owned(),
            }));
        }
    }
    Ok(keys)
}

fn known(key: &str) -> bool {
    matches!(
        key,
        "base_url"
            | "username"
            | "token_file"
            | "poll_seconds"
            | "stale_days"
            | "ca_file"
            | "log_file"
            | "state_dir"
    )
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

fn read_token(path: &Path) -> Result<String, Error> {
    ensure_private(path)?;
    let text = read_utf8(path)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(config(ConfigFault::Value {
            key: "token_file".to_owned(),
            message: "empty",
        }));
    }
    Ok(text.to_owned())
}

fn ensure_private(path: &Path) -> Result<(), Error> {
    let meta = match path.metadata() {
        Ok(meta) => meta,
        Err(err) => {
            return Err(config(ConfigFault::Read {
                path: path.to_owned(),
                message: err.to_string(),
            }));
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(config(ConfigFault::TokenMode {
                path: path.to_owned(),
            }));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
    }
    Ok(())
}

fn read_utf8(path: &Path) -> Result<String, Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Err(config(ConfigFault::Read {
                path: path.to_owned(),
                message: err.to_string(),
            }));
        }
    };
    String::from_utf8(bytes).map_err(|_| {
        config(ConfigFault::Utf8 {
            path: path.to_owned(),
        })
    })
}

fn require(key: &'static str, value: Option<&str>) -> Result<String, Error> {
    match value {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        Some(_) => Err(config(ConfigFault::Value {
            key: key.to_owned(),
            message: "empty",
        })),
        None => Err(config(ConfigFault::Missing { key })),
    }
}

fn require_path(key: &'static str, path: Option<PathBuf>) -> Result<PathBuf, Error> {
    match path {
        Some(path) if !path.as_os_str().is_empty() => Ok(path),
        Some(_) => Err(config(ConfigFault::Value {
            key: key.to_owned(),
            message: "empty",
        })),
        None => Err(config(ConfigFault::Missing { key })),
    }
}

fn pick<'a>(flag: Option<&'a str>, env: Option<&'a str>, file: Option<&'a str>) -> Option<&'a str> {
    if flag.is_some() {
        return flag;
    }
    if env.is_some() {
        return env;
    }
    file
}

fn pick_path(flag: Option<&Path>, env: Option<&Path>, file: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = flag {
        return Some(path.to_owned());
    }
    if let Some(path) = env {
        return Some(path.to_owned());
    }
    file.map(PathBuf::from)
}

fn parse_u64(key: &str, text: &str) -> Result<u64, Error> {
    text.parse().map_err(|_| {
        config(ConfigFault::Value {
            key: key.to_owned(),
            message: "not an integer",
        })
    })
}

fn parse_u32(key: &str, text: &str) -> Result<u32, Error> {
    text.parse().map_err(|_| {
        config(ConfigFault::Value {
            key: key.to_owned(),
            message: "not an integer",
        })
    })
}

fn config(fault: ConfigFault) -> Error {
    Error::Config(fault)
}

pub(crate) fn nonempty(value: Result<String, std::env::VarError>) -> Option<String> {
    match value {
        Ok(value) if !value.is_empty() => Some(value),
        Ok(_) | Err(_) => None,
    }
}

pub(crate) fn path_from(value: Option<String>) -> Option<PathBuf> {
    value.map(PathBuf::from)
}
