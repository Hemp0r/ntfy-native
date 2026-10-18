use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, PartialEq)]
pub enum Loaded<T> {
    Missing,
    Ok(T),
    Corrupt { backup: PathBuf },
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = sibling_with_suffix(path, ".tmp");
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(value)?;
    let mut file = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(&json)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

pub fn load_json<T: DeserializeOwned>(path: &Path) -> Result<Loaded<T>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => Ok(Loaded::Ok(v)),
        Err(_) => {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup = sibling_with_suffix(path, &format!(".corrupt-{secs}"));
            fs::rename(path, &backup)
                .with_context(|| format!("back up corrupt {}", path.display()))?;
            Ok(Loaded::Corrupt { backup })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{AuthKind, Instance, Settings, Topic};

    #[test]
    fn roundtrip_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings {
            instances: vec![Instance {
                id: "a".into(),
                url: "https://x".into(),
                auth: AuthKind::Basic {
                    username: "u".into(),
                },
                topics: vec![Topic {
                    name: "t".into(),
                    muted: true,
                    urgent_breaks_dnd: Some(true),
                }],
                ..Instance::default()
            }],
            paused_until: Some(i64::MAX),
            ..Settings::default()
        };
        save_json(&path, &s).unwrap();
        assert_eq!(load_json::<Settings>(&path).unwrap(), Loaded::Ok(s));
    }

    #[test]
    fn missing_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let r = load_json::<Settings>(&dir.path().join("nope.json")).unwrap();
        assert_eq!(r, Loaded::Missing);
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{nicht json").unwrap();
        let Loaded::Corrupt { backup } = load_json::<Settings>(&path).unwrap() else {
            panic!("expected Corrupt");
        };
        assert_eq!(fs::read_to_string(&backup).unwrap(), "{nicht json");
        assert!(!path.exists());
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("settings.json.corrupt-")
        );
    }

    #[test]
    fn save_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("state.json");
        save_json(&path, &Settings::default()).unwrap();
        assert!(!sibling_with_suffix(&path, ".tmp").exists());
        let back: Settings = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, Settings::default());
    }
}
