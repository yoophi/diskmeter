//! TUI 와 웹이 함께 보는 하나의 상주 표본 세션.
//!
//! 화면이 둘이어도 디스크 측정과 기록은 한 번만 일어나야 한다. 세션이 표본 상태와
//! 기간별 이력을 소유하고, 화면 어댑터는 그 상태를 읽어 그리기만 한다.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, RwLock, RwLockReadGuard};
use std::time::Duration;

use chrono::{DateTime, Local, TimeDelta, TimeZone};

use super::{UsageApplication, VolumeResult};
use crate::domain::disk::{Bucket, Range, Sample};

/// 경로 하나의 화면 상태. 측정이 실패해도 직전 표본은 남겨 둔다.
#[derive(Debug, Clone)]
pub(crate) struct WatchPane {
    pub path: String,
    pub sample: Option<Sample>,
    pub error: Option<String>,
    series: BTreeMap<Range, Vec<Bucket>>,
}

impl WatchPane {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            sample: None,
            error: None,
            series: BTreeMap::new(),
        }
    }

    pub(crate) fn series(&self, range: Range) -> &[Bucket] {
        self.series.get(&range).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WatchState {
    panes: Vec<WatchPane>,
}

impl WatchState {
    pub(crate) fn new(paths: &[String]) -> Self {
        Self {
            panes: paths.iter().map(|path| WatchPane::new(path)).collect(),
        }
    }

    pub(crate) fn panes(&self) -> &[WatchPane] {
        &self.panes
    }

    pub(crate) fn pane(&self, path: &str) -> Option<&WatchPane> {
        self.panes.iter().find(|pane| pane.path == path)
    }

    pub(crate) fn apply(&mut self, results: Vec<VolumeResult>) {
        for volume in results {
            let Some(pane) = self.panes.iter_mut().find(|pane| pane.path == volume.path) else {
                continue;
            };
            match volume.result {
                Ok(sample) => {
                    pane.sample = Some(sample);
                    pane.error = None;
                }
                Err(error) => pane.error = Some(error.to_string()),
            }
        }
    }

    pub(crate) fn set_series(&mut self, path: &str, range: Range, buckets: Vec<Bucket>) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.path == path) {
            pane.series.insert(range, buckets);
        }
    }

    pub(crate) fn any_failed(&self) -> bool {
        self.panes.iter().any(|pane| pane.error.is_some())
    }

    /// 가장 최근 표본 시각. footer 의 "sampled …" 에 쓴다.
    pub(crate) fn latest_sample_at(&self) -> Option<DateTime<Local>> {
        self.panes
            .iter()
            .filter_map(|pane| pane.sample.map(|sample| sample.at))
            .max()
    }
}

/// 화면 어댑터가 한 프레임을 그리기 위해 읽는 상태.
pub(crate) struct SessionState {
    pub watch: WatchState,
    pub refreshing: bool,
    pub next_refresh_at: Option<DateTime<Local>>,
}

impl SessionState {
    /// 다음 자동 표본까지 남은 초. 이미 지났으면 0으로 붙인다.
    pub(crate) fn seconds_until_refresh(&self, now: DateTime<Local>) -> Option<u64> {
        let remaining = self.next_refresh_at? - now;
        Some(remaining.num_seconds().max(0) as u64)
    }
}

pub(crate) struct LiveSession {
    application: UsageApplication,
    paths: Vec<String>,
    state: RwLock<SessionState>,
    /// 측정·기록을 직렬화한다. HTTP 요청과 주기 루프가 동시에 들어와도 한 번에 하나다.
    gate: Mutex<()>,
    /// 루프를 깨우는 요청 큐. 루프가 하나뿐이라 Mutex로 충분하다.
    requests: Mutex<Receiver<()>>,
    sender: Sender<()>,
}

impl LiveSession {
    pub(crate) fn new(application: UsageApplication, paths: Vec<String>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let session = Self {
            state: RwLock::new(SessionState {
                watch: WatchState::new(&paths),
                refreshing: false,
                next_refresh_at: None,
            }),
            application,
            paths,
            gate: Mutex::new(()),
            requests: Mutex::new(receiver),
            sender,
        };
        // 첫 측정 전에도 저장된 이력은 보여 준다. 읽기 실패는 첫 refresh 가 다시 알린다.
        let _ = session.reload_series(Local::now());
        session
    }

    pub(crate) fn read(&self) -> RwLockReadGuard<'_, SessionState> {
        self.state.read().expect("session state lock")
    }

    /// 지금 표본을 떠 달라고 요청한다. 실행은 루프가 맡으므로 호출자는 막히지 않는다.
    pub(crate) fn request(&self) {
        // Receiver를 세션이 들고 있어서 send는 실패하지 않는다.
        let _ = self.sender.send(());
    }

    /// 호출한 스레드에서 측정·기록·이력 갱신을 끝까지 실행한다.
    pub(crate) fn refresh_blocking(&self) -> anyhow::Result<()> {
        let _serialized = self.gate.lock().expect("refresh gate lock");
        self.set_refreshing(true);
        let now = Local::now();
        let recorded = match self.application.record(&self.paths, now) {
            Ok(results) => {
                self.state
                    .write()
                    .expect("session state lock")
                    .watch
                    .apply(results);
                Ok(())
            }
            Err(error) => Err(error),
        };
        let reloaded = self.reload_series(now);
        self.set_refreshing(false);
        recorded.and(reloaded)
    }

    /// 주기 표본 루프. 전용 스레드 하나에서만 돌려야 한다.
    ///
    /// 표본 시각을 주기의 배수에 맞춘다 — 5분 버킷 경계(:00, :05 …)에 표본이 놓여야
    /// 어느 프로세스가 떴든 같은 버킷을 채운다.
    pub(crate) fn run_refresh_loop(&self, interval: Duration) {
        loop {
            // 경로별 실패는 pane 상태로 이미 남으므로 여기서는 삼킨다.
            let _ = self.refresh_blocking();
            let next = next_tick(Local::now(), interval);
            self.state
                .write()
                .expect("session state lock")
                .next_refresh_at = Some(next);
            let wait = (next - Local::now()).to_std().unwrap_or(Duration::ZERO);
            let requests = self.requests.lock().expect("refresh request queue lock");
            match requests.recv_timeout(wait) {
                Ok(()) => {
                    // 측정 중에 쌓인 요청은 한 번으로 합친다.
                    while requests.try_recv().is_ok() {}
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn reload_series(&self, now: DateTime<Local>) -> anyhow::Result<()> {
        for path in &self.paths {
            for range in Range::ALL {
                let buckets = self.application.history(path, range, now)?;
                self.state
                    .write()
                    .expect("session state lock")
                    .watch
                    .set_series(path, range, buckets);
            }
        }
        Ok(())
    }

    fn set_refreshing(&self, refreshing: bool) {
        self.state.write().expect("session state lock").refreshing = refreshing;
    }
}

/// `now` 다음에 오는 주기 경계.
pub(crate) fn next_tick(now: DateTime<Local>, interval: Duration) -> DateTime<Local> {
    let step = interval.as_secs().max(1) as i64;
    let next = (now.timestamp().div_euclid(step) + 1) * step;
    Local
        .timestamp_opt(next, 0)
        .single()
        .unwrap_or(now + TimeDelta::seconds(step))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::usage::testing::application;
    use crate::domain::disk::DiskUsage;

    #[test]
    fn ticks_land_on_interval_boundaries() {
        let now = Local.timestamp_opt(1_000_000_123, 0).single().unwrap();
        let next = next_tick(now, Duration::from_secs(300));
        assert_eq!(next.timestamp(), 1_000_000_200);
        let on_boundary = Local.timestamp_opt(1_000_000_200, 0).single().unwrap();
        assert_eq!(
            next_tick(on_boundary, Duration::from_secs(300)).timestamp(),
            1_000_000_500
        );
    }

    #[test]
    fn refresh_records_and_reloads_every_range() {
        let (application, history) = application(DiskUsage::new(1_000, 820, 180));
        let session = LiveSession::new(application, vec!["/".into()]);
        session.refresh_blocking().unwrap();
        let state = session.read();
        let pane = &state.watch.panes()[0];
        assert_eq!(pane.sample.unwrap().usage.used, 820);
        assert!(pane.error.is_none());
        for range in Range::ALL {
            assert_eq!(pane.series(range).len(), 1, "{}", range.name());
        }
        assert_eq!(history.samples.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_failed_probe_keeps_the_previous_sample_and_shows_the_error() {
        let (application, _) = application(DiskUsage::new(1_000, 820, 180));
        let session = LiveSession::new(application, vec!["/".into(), "/Volumes/Gone".into()]);
        session.refresh_blocking().unwrap();
        let state = session.read();
        assert!(state.watch.any_failed());
        let gone = state.watch.pane("/Volumes/Gone").unwrap();
        assert!(gone.sample.is_none());
        assert!(gone.error.as_deref().unwrap().contains("not mounted"));
        assert!(state.watch.latest_sample_at().is_some());
    }
}
