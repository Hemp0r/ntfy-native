use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

pub const APP_NAME: &str = "ntfy-native";
pub const APP_ID: &str = "dev.hemp0r.ntfy-native";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    pub fn platform() -> Result<AppPaths> {
        let dirs = ProjectDirs::from("dev", "hemp0r", APP_NAME)
            .context("could not determine the user's home directory")?;
        let data_dir = dirs.data_dir().to_path_buf();
        Ok(AppPaths {
            config_dir: dirs.config_dir().to_path_buf(),
            log_dir: data_dir.join("logs"),
            data_dir,
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    /// Layout below an arbitrary root, for tests.
    pub fn under(root: &Path) -> AppPaths {
        let data_dir = root.join("data");
        AppPaths {
            config_dir: root.join("config"),
            log_dir: data_dir.join("logs"),
            data_dir,
            cache_dir: root.join("cache"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }

    pub fn default_import_file(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data_dir.join("instance.lock")
    }

    pub fn media_dir(&self) -> PathBuf {
        self.cache_dir.join("media")
    }

    pub fn ensure_dirs(&self) -> io::Result<()> {
        for d in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn paths_under_root() {
        let p = AppPaths::under(Path::new("/r"));
        assert_eq!(p.settings_file(), Path::new("/r/data/settings.json"));
        assert_eq!(p.default_import_file(), Path::new("/r/config/config.yaml"));
        assert_eq!(p.media_dir(), Path::new("/r/cache/media"));
    }
}
