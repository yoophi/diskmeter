//! TOML 파일로 설정 포트를 구현한다.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::application::{Settings, SettingsRepository};

const DIR: &str = "diskmeter";
const FILE: &str = "config.toml";

#[derive(Debug, Default, Clone, Copy)]
pub struct FileSettingsRepository;

impl FileSettingsRepository {
    pub(crate) fn path(&self) -> Result<PathBuf> {
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
            _ => {
                PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config")
            }
        };
        Ok(base.join(DIR).join(FILE))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSettings {
    paths: Option<Vec<String>>,
}

impl SettingsRepository for FileSettingsRepository {
    fn load(&self) -> Result<Option<Settings>> {
        let path = self.path()?;
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("could not open {}", path.display()));
            }
        };
        parse(&raw).with_context(|| format!("could not read {}", path.display()))
    }

    fn save(&self, settings: &Settings) -> Result<()> {
        let path = self.path()?;
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("could not create {}", directory.display()))?;
        }
        let stored = StoredSettings {
            paths: Some(settings.paths.clone()),
        };
        let body = toml::to_string_pretty(&stored)
            .context("could not render the configuration as TOML")?;
        std::fs::write(&path, body)
            .with_context(|| format!("could not write to {}", path.display()))?;
        Ok(())
    }
}

fn parse(raw: &str) -> Result<Option<Settings>> {
    let stored: StoredSettings = toml::from_str(raw).context("could not parse the TOML")?;
    Ok(stored.paths.map(|paths| Settings { paths }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_written_config_and_keeps_order() {
        let settings = parse("paths = [\"/Volumes/Data\", \"/\"]")
            .unwrap()
            .unwrap();
        assert_eq!(settings.paths, vec!["/Volumes/Data", "/"]);
    }

    #[test]
    fn missing_field_delegates_defaulting_to_the_application() {
        assert!(parse("").unwrap().is_none());
    }

    #[test]
    fn path_follows_xdg_shape() {
        let path = FileSettingsRepository.path().unwrap();
        assert!(
            path.ends_with("diskmeter/config.toml"),
            "{}",
            path.display()
        );
    }
}
