//! 표본과 이력을 plain·TUI·JSON·웹 화면으로 투영한다.

pub(crate) mod chart;
pub(crate) mod history;
pub(crate) mod model;
pub(crate) mod plain;
pub(crate) mod tui;
pub(crate) mod web;

use chrono::{DateTime, Local};

use crate::application::{ProbeError, VolumeResult};
use crate::domain::disk::{Bucket, Range, Sample, Severity};

/// 색 사용 여부. `--no-color`, `NO_COLOR`, 비-TTY 를 모두 고려한다.
pub(crate) fn use_color(flag_no_color: bool, is_tty: bool) -> bool {
    if flag_no_color || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    is_tty
}

pub(crate) fn ansi_for(level: Severity) -> &'static str {
    match level {
        Severity::Normal => "\x1b[38;5;147m",   // 연보라
        Severity::Warning => "\x1b[38;5;179m",  // 호박색
        Severity::Critical => "\x1b[38;5;203m", // 적색
    }
}

pub(crate) const DIM: &str = "\x1b[38;5;239m";
pub(crate) const MUTED: &str = "\x1b[38;5;245m";
pub(crate) const SPARK: &str = "\x1b[38;5;109m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const RESET: &str = "\x1b[0m";

/// 일회성 출력이 한 경로에 대해 아는 전부: 측정 결과와 선택한 기간의 이력.
pub(crate) struct VolumeView<'a> {
    pub path: &'a str,
    pub result: &'a Result<Sample, ProbeError>,
    pub history: &'a [Bucket],
}

impl<'a> VolumeView<'a> {
    pub(crate) fn new(volume: &'a VolumeResult, history: &'a [Bucket]) -> Self {
        Self {
            path: &volume.path,
            result: &volume.result,
            history,
        }
    }
}

/// SI 단위(1 GB = 10⁹ B). macOS Finder 와 시스템 설정이 쓰는 단위라 그 숫자와 맞는다.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `--json` 출력. 경로가 하나여도 `volumes` 배열이라 스크립트가 분기하지 않아도 된다.
pub(crate) fn to_json(
    views: &[VolumeView<'_>],
    range: Range,
    timezone: &str,
    now: DateTime<Local>,
) -> anyhow::Result<String> {
    #[derive(serde::Serialize)]
    struct Volume {
        path: String,
        sampled_at: Option<String>,
        total: Option<u64>,
        used: Option<u64>,
        available: Option<u64>,
        used_percent: Option<f64>,
        label: String,
        level: &'static str,
        detail: Option<String>,
        /// 선택한 기간의 첫 표본 대비 변화량. 이력이 없으면 없다.
        delta: Option<String>,
        error: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Out<'a> {
        generated_at: String,
        timezone: &'a str,
        range: &'static str,
        volumes: Vec<Volume>,
    }

    let volumes = views
        .iter()
        .map(|view| match view.result {
            Ok(sample) => {
                let meter = model::project(sample);
                Volume {
                    path: view.path.to_string(),
                    sampled_at: Some(sample.at.to_rfc3339()),
                    total: Some(sample.usage.total),
                    used: Some(sample.usage.used),
                    available: Some(sample.usage.available),
                    used_percent: Some((meter.used_percent * 10.0).round() / 10.0),
                    label: meter.usage.label,
                    level: model::level_name(meter.level),
                    detail: Some(meter.detail),
                    delta: model::delta(view.history),
                    error: None,
                }
            }
            Err(error) => Volume {
                path: view.path.to_string(),
                sampled_at: None,
                total: None,
                used: None,
                available: None,
                used_percent: None,
                label: "unavailable".to_string(),
                level: model::level_name(Severity::Critical),
                detail: None,
                delta: None,
                error: Some(error.to_string()),
            },
        })
        .collect();
    let out = Out {
        generated_at: now.to_rfc3339(),
        timezone,
        range: range.name(),
        volumes,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::disk::DiskUsage;

    #[test]
    fn bytes_use_si_units_with_one_decimal() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_000), "1.0 kB");
        assert_eq!(format_bytes(494_384_795_648), "494.4 GB");
        assert_eq!(format_bytes(2_000_000_000_000), "2.0 TB");
    }

    #[test]
    fn json_keeps_the_public_level_words_and_reports_errors_per_volume() {
        let ok = VolumeResult {
            path: "/".into(),
            result: Ok(Sample {
                at: Local::now(),
                usage: DiskUsage::new(1_000, 905, 95),
            }),
        };
        let gone = VolumeResult {
            path: "/Volumes/Gone".into(),
            result: Err(ProbeError::Missing("/Volumes/Gone".into())),
        };
        let views = [VolumeView::new(&ok, &[]), VolumeView::new(&gone, &[])];
        let json: serde_json::Value =
            serde_json::from_str(&to_json(&views, Range::Day, "Asia/Seoul", Local::now()).unwrap())
                .unwrap();
        assert_eq!(json["range"], "24h");
        assert_eq!(json["volumes"][0]["level"], "critical");
        assert_eq!(json["volumes"][0]["used_percent"], 90.5);
        assert!(
            json["volumes"][1]["error"]
                .as_str()
                .unwrap()
                .contains("not mounted")
        );
        assert!(json["volumes"][1]["used_percent"].is_null());
    }
}
