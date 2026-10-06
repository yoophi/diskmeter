//! 감시할 경로를 관리하는 설정 사용 사례.

use std::sync::Arc;

use anyhow::{Result, bail};

use super::SettingsRepository;

/// 설정 파일이 없을 때 감시하는 경로.
pub(crate) const DEFAULT_PATH: &str = "/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// 표시 순서이기도 하므로 입력 순서를 보존한다.
    pub paths: Vec<String>,
}

/// 기본값 선택과 검증을 파일 형식·저장 위치에서 분리한다.
pub(crate) struct SettingsApplication {
    repository: Arc<dyn SettingsRepository>,
}

impl SettingsApplication {
    pub(crate) fn new(repository: Arc<dyn SettingsRepository>) -> Self {
        Self { repository }
    }

    /// 저장된 설정을 읽고, 파일이 없으면 루트 볼륨 하나를 감시한다.
    pub(crate) fn load(&self) -> Result<Settings> {
        let settings = self.repository.load()?.unwrap_or_else(|| Settings {
            paths: vec![DEFAULT_PATH.to_string()],
        });
        validate(&settings)?;
        Ok(settings)
    }

    /// 경로 목록을 검증한 뒤 저장한다.
    pub(crate) fn replace_paths(&self, paths: Vec<String>) -> Result<Settings> {
        let settings = Settings { paths };
        validate(&settings)?;
        self.repository.save(&settings)?;
        Ok(settings)
    }
}

/// 마운트 여부는 여기서 보지 않는다 — 외장 디스크는 꽂혀 있지 않은 시간이 더 길다.
fn validate(settings: &Settings) -> Result<()> {
    if settings.paths.is_empty() {
        bail!("paths is empty. for example: paths = [\"/\", \"/Volumes/Backup\"]");
    }
    for (index, path) in settings.paths.iter().enumerate() {
        if !path.starts_with('/') {
            bail!("path must be absolute: {path}");
        }
        if settings.paths[..index].contains(path) {
            bail!("path is listed twice: {path}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemorySettings {
        value: Mutex<Option<Settings>>,
    }

    impl SettingsRepository for MemorySettings {
        fn load(&self) -> Result<Option<Settings>> {
            Ok(self.value.lock().unwrap().clone())
        }

        fn save(&self, settings: &Settings) -> Result<()> {
            *self.value.lock().unwrap() = Some(settings.clone());
            Ok(())
        }
    }

    fn application() -> SettingsApplication {
        SettingsApplication::new(Arc::new(MemorySettings::default()))
    }

    #[test]
    fn missing_file_watches_the_root_volume() {
        assert_eq!(application().load().unwrap().paths, vec!["/"]);
    }

    #[test]
    fn relative_and_duplicate_paths_are_rejected_before_saving() {
        let application = application();
        assert!(application.replace_paths(vec!["Volumes/X".into()]).is_err());
        assert!(
            application
                .replace_paths(vec!["/".into(), "/".into()])
                .is_err()
        );
        assert!(application.replace_paths(vec![]).is_err());
        assert_eq!(application.load().unwrap().paths, vec!["/"]);
    }

    #[test]
    fn saved_paths_keep_their_order() {
        let application = application();
        application
            .replace_paths(vec!["/Volumes/B".into(), "/".into()])
            .unwrap();
        assert_eq!(application.load().unwrap().paths, vec!["/Volumes/B", "/"]);
    }
}
