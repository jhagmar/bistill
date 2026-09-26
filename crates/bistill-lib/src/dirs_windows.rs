//! `%APPDATA%` and `%LOCALAPPDATA%`.

use crate::{ConfigFault, Error};
use std::path::PathBuf;

/// Directories used to find config and state on Windows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dirs {
    /// `%APPDATA%`.
    pub appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%`.
    pub local_appdata: Option<PathBuf>,
}

impl Dirs {
    pub(crate) fn new() -> Self {
        Self {
            appdata: None,
            local_appdata: None,
        }
    }

    pub(crate) fn from_process() -> Self {
        Self {
            appdata: super::path_from(super::nonempty(std::env::var("APPDATA"))),
            local_appdata: super::path_from(super::nonempty(std::env::var("LOCALAPPDATA"))),
        }
    }

    #[cfg(test)]
    pub(crate) fn sample() -> Self {
        Self {
            appdata: Some(PathBuf::from(r"C:\Users\jcitizen\AppData\Roaming")),
            local_appdata: Some(PathBuf::from(r"C:\Users\jcitizen\AppData\Local")),
        }
    }
}

pub(crate) fn config_path(dirs: &Dirs) -> Result<PathBuf, ConfigFault> {
    let dir = dirs
        .appdata
        .as_ref()
        .ok_or(ConfigFault::Missing { key: "config" })?;
    Ok(dir.join("bistill").join("config"))
}

pub(crate) fn state_dir(dirs: &Dirs) -> Result<PathBuf, Error> {
    let dir = dirs
        .local_appdata
        .as_ref()
        .ok_or(ConfigFault::Missing { key: "state_dir" })
        .map_err(super::config)?;
    Ok(dir.join("bistill"))
}

#[cfg(test)]
pub(crate) fn sample_state() -> PathBuf {
    PathBuf::from(r"C:\Users\jcitizen\AppData\Local").join("bistill")
}

#[cfg(test)]
mod tests {
    use crate::{Env, Flags, load};
    use std::fs;

    #[test]
    fn appdata_dirs_supply_config_and_state() {
        let dir = std::env::temp_dir().join(format!("bistill-appdata-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Roaming/bistill")).unwrap();
        fs::write(
            dir.join("Roaming/bistill/config"),
            "base_url = https://win.example.invalid\nusername = win-user\n",
        )
        .unwrap();
        let mut env = Env::new();
        env.token = Some("secret-token".to_owned());
        env.dirs.appdata = Some(dir.join("Roaming"));
        env.dirs.local_appdata = Some(dir.join("Local"));
        let loaded = load(&dir, &Flags::default(), &env).unwrap();
        assert_eq!(loaded.base_url, "https://win.example.invalid");
        assert_eq!(loaded.username, "win-user");
        assert_eq!(loaded.state_dir, dir.join("Local/bistill"));
        let _ = fs::remove_dir_all(&dir);
    }
}
