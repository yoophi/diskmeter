//! 사용량 도메인을 모든 출력 adapter 가 공유하는 화면 모델로 투영한다.

use chrono::{DateTime, Local};

use crate::domain::disk::{Bucket, Sample, Severity};

use super::format_bytes;

#[derive(Debug, Clone)]
pub(crate) struct Bar {
    pub fill: f64,
    pub label: String,
    pub level: Severity,
}

impl Bar {
    pub fn fill_clamped(&self) -> f64 {
        if self.fill.is_nan() {
            return 0.0;
        }
        self.fill.clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Meter {
    pub usage: Bar,
    /// `405.1 GB of 494.4 GB · 89.3 GB free`
    pub detail: String,
    pub level: Severity,
    pub used_percent: f64,
}

pub(crate) fn project(sample: &Sample) -> Meter {
    let usage = sample.usage;
    let used_percent = usage.used_percent();
    let level = usage.severity();
    Meter {
        usage: Bar {
            fill: usage.used_fraction(),
            label: format!("{used_percent:.0}% used"),
            level,
        },
        detail: format!(
            "{} of {} · {} free",
            format_bytes(usage.used),
            format_bytes(usage.total),
            format_bytes(usage.available)
        ),
        level,
        used_percent,
    }
}

pub(crate) fn level_name(level: Severity) -> &'static str {
    match level {
        Severity::Normal => "normal",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}

/// 값이 언제 기준인지 밝힌다 — `sampled 13:05 (just now)`.
pub(crate) fn origin_text(at: DateTime<Local>, now: DateTime<Local>) -> String {
    format!("sampled {} ({})", at.format("%H:%M"), relative(at, now))
}

fn relative(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let seconds = (now - at).num_seconds().max(0);
    let plural = |count: i64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    if seconds < 60 {
        "just now".to_string()
    } else if seconds < 3_600 {
        plural(seconds / 60, "minute")
    } else if seconds < 86_400 {
        plural(seconds / 3_600, "hour")
    } else {
        plural(seconds / 86_400, "day")
    }
}

/// 기간의 첫 표본부터 마지막 표본까지 소진율이 얼마나 움직였는지 — `+0.4%p`.
pub(crate) fn delta(buckets: &[Bucket]) -> Option<String> {
    if buckets.len() < 2 {
        return None;
    }
    let first = buckets.first()?.percent_avg();
    let last = buckets.last()?.percent_last();
    let difference = last - first;
    if difference.abs() < 0.05 {
        return None;
    }
    Some(format!("{difference:+.1}%p"))
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;
    use crate::domain::disk::DiskUsage;

    #[test]
    fn meter_rounds_the_label_like_df_and_keeps_the_exact_percent() {
        let meter = project(&Sample {
            at: Local::now(),
            usage: DiskUsage::new(494_384_795_648, 405_125_910_528, 89_258_885_120),
        });
        assert_eq!(meter.usage.label, "82% used");
        assert_eq!(meter.level, Severity::Warning);
        assert_eq!(meter.detail, "405.1 GB of 494.4 GB · 89.3 GB free");
        assert!((meter.used_percent - 81.94).abs() < 0.01);
    }

    #[test]
    fn origin_text_names_how_old_the_sample_is() {
        let now = Local::now();
        assert!(origin_text(now, now).ends_with("(just now)"));
        assert!(origin_text(now - TimeDelta::minutes(1), now).ends_with("(1 minute ago)"));
        assert!(origin_text(now - TimeDelta::hours(3), now).ends_with("(3 hours ago)"));
        assert!(origin_text(now - TimeDelta::days(2), now).ends_with("(2 days ago)"));
    }

    #[test]
    fn delta_compares_first_and_last_buckets() {
        let first = Bucket::from_usage(0, DiskUsage::new(1_000, 800, 200));
        let last = Bucket::from_usage(300, DiskUsage::new(1_000, 812, 188));
        assert_eq!(delta(&[first, last]).as_deref(), Some("+1.2%p"));
        assert!(delta(&[first]).is_none());
        assert!(delta(&[first, first]).is_none());
    }
}
