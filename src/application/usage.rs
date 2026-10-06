//! 측정·기록·이력 조회 사용 사례.

use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Local};

use crate::domain::disk::{Bucket, Range, Sample};

use super::{HistoryRepository, ProbeError, UsageProbe};

/// 경로 하나의 측정 결과. 경로 하나가 실패해도 나머지는 그대로 보여준다.
#[derive(Debug)]
pub(crate) struct VolumeResult {
    pub path: String,
    pub result: Result<Sample, ProbeError>,
}

pub(crate) struct UsageApplication {
    probe: Arc<dyn UsageProbe>,
    history: Arc<dyn HistoryRepository>,
}

impl UsageApplication {
    pub(crate) fn new(probe: Arc<dyn UsageProbe>, history: Arc<dyn HistoryRepository>) -> Self {
        Self { probe, history }
    }

    /// 모든 경로를 측정만 한다.
    pub(crate) fn measure(&self, paths: &[String], now: DateTime<Local>) -> Vec<VolumeResult> {
        paths
            .iter()
            .map(|path| VolumeResult {
                path: path.clone(),
                result: self
                    .probe
                    .measure(path)
                    .map(|usage| Sample { at: now, usage }),
            })
            .collect()
    }

    /// 측정하고 성공한 표본을 이력에 남긴다.
    ///
    /// 저장이 실패하면 오류로 돌려준다 — 조용히 빠진 표본은 나중에 공백으로 보여서
    /// 수집이 멈춘 것처럼 오해하게 만든다.
    pub(crate) fn record(
        &self,
        paths: &[String],
        now: DateTime<Local>,
    ) -> Result<Vec<VolumeResult>> {
        let results = self.measure(paths, now);
        for volume in &results {
            if let Ok(sample) = &volume.result {
                self.history
                    .record(&volume.path, *sample)
                    .with_context(|| format!("could not record a sample for {}", volume.path))?;
            }
        }
        Ok(results)
    }

    /// 한 기간의 이력을 그 기간의 해상도로 읽는다.
    ///
    /// 기간 시작이 걸친 버킷도 포함해 차트 왼쪽 끝이 비지 않게 한다.
    pub(crate) fn history(
        &self,
        path: &str,
        range: Range,
        now: DateTime<Local>,
    ) -> Result<Vec<Bucket>> {
        let resolution = range.resolution();
        let from = resolution.bucket_start(range.started_at(now));
        self.history
            .series(path, resolution, from, now.timestamp())
            .with_context(|| format!("could not read the {} history of {path}", range.name()))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use crate::domain::disk::{DiskUsage, Resolution, rollup};

    use super::*;

    /// 경로별 고정 응답을 돌려주는 측정기. 모르는 경로는 `Missing`.
    pub(crate) struct FixedProbe(pub BTreeMap<String, DiskUsage>);

    impl UsageProbe for FixedProbe {
        fn measure(&self, path: &str) -> Result<DiskUsage, ProbeError> {
            self.0
                .get(path)
                .copied()
                .ok_or_else(|| ProbeError::Missing(path.to_string()))
        }
    }

    /// 표본을 메모리에 쌓고 조회 때 즉석에서 집계하는 이력 저장소.
    #[derive(Default)]
    pub(crate) struct MemoryHistory {
        pub samples: Mutex<Vec<(String, Sample)>>,
    }

    impl HistoryRepository for MemoryHistory {
        fn record(&self, path: &str, sample: Sample) -> Result<()> {
            self.samples
                .lock()
                .unwrap()
                .push((path.to_string(), sample));
            Ok(())
        }

        fn series(
            &self,
            path: &str,
            resolution: Resolution,
            from: i64,
            to: i64,
        ) -> Result<Vec<Bucket>> {
            let fine: Vec<Bucket> = self
                .samples
                .lock()
                .unwrap()
                .iter()
                .filter(|(stored, _)| stored == path)
                .map(|(_, sample)| {
                    Bucket::from_usage(
                        Resolution::FiveMinutes.bucket_start(sample.at),
                        sample.usage,
                    )
                })
                .collect();
            Ok(rollup(resolution, &fine)
                .into_iter()
                .filter(|bucket| bucket.start >= from && bucket.start <= to)
                .collect())
        }
    }

    pub(crate) fn application(usage: DiskUsage) -> (UsageApplication, Arc<MemoryHistory>) {
        let history = Arc::new(MemoryHistory::default());
        let probe = FixedProbe(BTreeMap::from([("/".to_string(), usage)]));
        (
            UsageApplication::new(
                Arc::new(probe),
                Arc::clone(&history) as Arc<dyn HistoryRepository>,
            ),
            history,
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::testing::application;
    use super::*;
    use crate::domain::disk::DiskUsage;

    #[test]
    fn a_missing_path_fails_alone_and_the_rest_are_still_recorded() {
        let (application, history) = application(DiskUsage::new(1_000, 800, 200));
        let now = Local::now();
        let results = application
            .record(&["/".into(), "/Volumes/Gone".into()], now)
            .unwrap();
        assert!(results[0].result.is_ok());
        assert!(
            results[1]
                .result
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("not mounted")
        );
        assert_eq!(history.samples.lock().unwrap().len(), 1);
    }

    #[test]
    fn history_includes_the_bucket_the_range_starts_in() {
        let (application, _) = application(DiskUsage::new(1_000, 800, 200));
        let now = Local::now();
        // 25시간 전 표본은 24시간 보기의 시작 버킷 바깥이지만 23시간 59분 전은 안이다.
        application
            .record(&["/".into()], now - TimeDelta::hours(25))
            .unwrap();
        application
            .record(&["/".into()], now - TimeDelta::minutes(23 * 60 + 59))
            .unwrap();
        let buckets = application.history("/", Range::Day, now).unwrap();
        assert_eq!(buckets.len(), 1);
    }
}
