//! 실제 포트 구현을 애플리케이션에 연결하는 조립 지점(composition root).

use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::outbound::config::FileSettingsRepository;
use crate::adapters::outbound::history::SqliteHistoryRepository;
use crate::adapters::outbound::statfs::StatfsProbe;
use crate::application::{SettingsApplication, UsageApplication};

pub(crate) struct Runtime {
    pub(crate) usage: UsageApplication,
    pub(crate) settings: SettingsApplication,
    pub(crate) settings_path: PathBuf,
    pub(crate) history_path: PathBuf,
}

/// 프로덕션 어댑터는 오직 이곳에서 애플리케이션 포트에 연결한다.
pub(crate) fn production() -> anyhow::Result<Runtime> {
    let history = SqliteHistoryRepository::production()?;
    let history_path = history.path().to_path_buf();
    let usage = UsageApplication::new(Arc::new(StatfsProbe), Arc::new(history));

    let repository = FileSettingsRepository;
    let settings_path = repository.path()?;
    let settings = SettingsApplication::new(Arc::new(repository));

    Ok(Runtime {
        usage,
        settings,
        settings_path,
        history_path,
    })
}
