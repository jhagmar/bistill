//! `$XDG_CONFIG_HOME`, `$HOME`, and `$XDG_STATE_HOME`.

use crate::{ConfigFault, Error};
use std::path::PathBuf;

/// Directories used to find config and state on Unix.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dirs {
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<PathBuf>,
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// `$XDG_STATE_HOME`.
    pub xdg_state_home: Option<PathBuf>,
}

impl Dirs {
    pub(crate) fn new() -> Self {
        Self {
            xdg_config_home: None,
            home: None,
            xdg_state_home: None,
        }
    }

    pub(crate) fn from_process() -> Self {
        Self {
            xdg_config_home: super::path_from(super::nonempty(std::env::var("XDG_CONFIG_HOME"))),
            home: super::path_from(super::nonempty(std::env::var("HOME"))),
            xdg_state_home: super::path_from(super::nonempty(std::env::var("XDG_STATE_HOME"))),
        }
    }

    #[cfg(test)]
    pub(crate) fn sample() -> Self {
        Self {
            xdg_config_home: None,
            home: Some(PathBuf::from("/home/jcitizen")),
            xdg_state_home: None,
        }
    }
}

pub(crate) fn config_path(dirs: &Dirs) -> Result<PathBuf, ConfigFault> {
    if let Some(dir) = &dirs.xdg_config_home {
        return Ok(dir.join("bistill/config"));
    }
    let home = dirs
        .home
        .as_ref()
        .ok_or(ConfigFault::Missing { key: "config" })?;
    Ok(home.join(".config/bistill/config"))
}

pub(crate) fn state_dir(dirs: &Dirs) -> Result<PathBuf, Error> {
    if let Some(dir) = &dirs.xdg_state_home {
        return Ok(dir.join("bistill"));
    }
    let home = dirs
        .home
        .as_ref()
        .ok_or(ConfigFault::Missing { key: "state_dir" })
        .map_err(super::config)?;
    Ok(home.join(".local/state/bistill"))
}

#[cfg(test)]
pub(crate) fn sample_state() -> PathBuf {
    PathBuf::from("/home/jcitizen/.local/state/bistill")
}

#[cfg(test)]
mod tests {
    use crate::{Env, Flags, load};
    use std::fs;

    #[test]
    fn xdg_dirs_override_the_home_defaults() {
        let dir = std::env::temp_dir().join(format!("bistill-xdg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("config/bistill")).unwrap();
        fs::write(
            dir.join("config/bistill/config"),
            "base_url = https://xdg.example.invalid\nusername = xdg-user\npoll_seconds = 45\n",
        )
        .unwrap();
        let mut env = Env::new();
        env.token = Some("secret-token".to_owned());
        env.dirs.xdg_config_home = Some(dir.join("config"));
        env.dirs.xdg_state_home = Some(dir.join("state"));
        let loaded = load(&dir, &Flags::default(), &env).unwrap();
        assert_eq!(loaded.base_url, "https://xdg.example.invalid");
        assert_eq!(loaded.username, "xdg-user");
        assert_eq!(loaded.poll_seconds.get(), 45);
        assert_eq!(loaded.state_dir, dir.join("state/bistill"));
        let _ = fs::remove_dir_all(&dir);
    }
}
