//! 애플리케이션이 외부 세계에 요구하는 포트.

use crate::domain::disk::{Bucket, DiskUsage, Resolution, Sample};

use super::{ProbeError, Settings};

/// 마운트된 파일시스템의 용량을 재는 아웃바운드 포트.
pub trait UsageProbe: Send + Sync {
    fn measure(&self, path: &str) -> Result<DiskUsage, ProbeError>;
}

/// 표본을 해상도별로 보존하는 아웃바운드 포트.
///
/// 5분 버킷에 더하기, 시간·일 집계, 보존 기한 정리를 하나의 lifecycle 로 제공한다.
/// 어떤 스키마와 트랜잭션으로 하는지는 어댑터 안에 숨긴다.
pub trait HistoryRepository: Send + Sync {
    fn record(&self, path: &str, sample: Sample) -> anyhow::Result<()>;

    /// `[from, to]` 구간(epoch 초)의 버킷을 시작 시각 순으로 돌려준다.
    fn series(
        &self,
        path: &str,
        resolution: Resolution,
        from: i64,
        to: i64,
    ) -> anyhow::Result<Vec<Bucket>>;
}

/// 설정 저장소가 구현하는 아웃바운드 포트.
///
/// 파일이 없으면 `None`을 돌려주고 기본값 선택은 애플리케이션에 맡긴다.
pub trait SettingsRepository: Send + Sync {
    fn load(&self) -> anyhow::Result<Option<Settings>>;
    fn save(&self, settings: &Settings) -> anyhow::Result<()>;
}
