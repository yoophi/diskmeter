//! 세션 상태를 웹 대시보드와 Hammerspoon 패널용 JSON contract 로 투영한다.

use chrono::{DateTime, Local};
use serde::Serialize;

use crate::application::WatchState;
use crate::domain::disk::Range;

use super::chart::{self, Chart};
use super::model;

pub(crate) const INDEX: &str = include_str!("web.html");

#[derive(Debug, Serialize)]
pub(crate) struct Dashboard {
    pub timezone: String,
    pub generated_at: String,
    pub next_refresh_at: Option<String>,
    pub refreshing: bool,
    pub range: &'static str,
    pub ranges: Vec<&'static str>,
    pub panes: Vec<Pane>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Pane {
    pub path: String,
    pub display: String,
    pub origin: Option<String>,
    pub sampled_at: Option<String>,
    pub error: Option<String>,
    pub used_percent: Option<f64>,
    pub label: String,
    pub level: &'static str,
    pub used: Option<u64>,
    pub total: Option<u64>,
    pub available: Option<u64>,
    pub detail: Option<String>,
    pub delta: Option<String>,
    pub chart: Chart,
}

pub(crate) fn project(
    state: &WatchState,
    timezone: &str,
    now: DateTime<Local>,
    next_refresh_at: Option<DateTime<Local>>,
    refreshing: bool,
    range: Range,
) -> Dashboard {
    let panes = state
        .panes()
        .iter()
        .map(|pane| {
            let series = pane.series(range);
            let meter = pane.sample.as_ref().map(model::project);
            Pane {
                path: pane.path.clone(),
                display: pane.path.clone(),
                origin: pane.sample.map(|sample| model::origin_text(sample.at, now)),
                sampled_at: pane.sample.map(|sample| sample.at.to_rfc3339()),
                error: pane.error.clone(),
                used_percent: meter.as_ref().map(|meter| meter.used_percent),
                label: meter
                    .as_ref()
                    .map(|meter| meter.usage.label.clone())
                    .unwrap_or_else(|| "waiting for the first sample".to_string()),
                level: model::level_name(
                    meter
                        .as_ref()
                        .map(|meter| meter.level)
                        .unwrap_or(crate::domain::disk::Severity::Normal),
                ),
                used: pane.sample.map(|sample| sample.usage.used),
                total: pane.sample.map(|sample| sample.usage.total),
                available: pane.sample.map(|sample| sample.usage.available),
                detail: meter.map(|meter| meter.detail),
                delta: model::delta(series),
                chart: chart::project(series, range, now),
            }
        })
        .collect();

    Dashboard {
        timezone: timezone.to_string(),
        generated_at: now.to_rfc3339(),
        next_refresh_at: next_refresh_at.map(|at| at.to_rfc3339()),
        refreshing,
        range: range.name(),
        ranges: Range::ALL.iter().map(|range| range.name()).collect(),
        panes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{LiveSession, testing::application};
    use crate::domain::disk::DiskUsage;

    #[test]
    fn projects_each_watched_path_with_its_chart() {
        let (application, _) = application(DiskUsage::new(1_000, 905, 95));
        let session = LiveSession::new(application, vec!["/".into(), "/Volumes/Gone".into()]);
        session.refresh_blocking().unwrap();
        let state = session.read();
        let dashboard = project(
            &state.watch,
            "Asia/Seoul",
            Local::now(),
            None,
            false,
            Range::Week,
        );
        assert_eq!(dashboard.range, "7d");
        assert_eq!(dashboard.ranges, vec!["24h", "7d", "30d", "1y"]);
        let root = &dashboard.panes[0];
        assert_eq!(root.level, "critical");
        assert_eq!(root.chart.points.len(), 1);
        assert_eq!(root.chart.resolution, "1h");
        let gone = &dashboard.panes[1];
        assert!(gone.used_percent.is_none());
        assert!(gone.error.as_deref().unwrap().contains("not mounted"));
        assert!(gone.chart.points.is_empty());
        let json = serde_json::to_string(&dashboard).unwrap();
        assert!(json.contains("\"line_path\""));
    }
}
