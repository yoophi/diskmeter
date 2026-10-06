//! 이력 버킷을 표·CSV·JSON 행으로 투영한다. `diskmeter history` 와 `/api/history` 가 같은 행을 쓴다.

use chrono::{Local, TimeZone};
use serde::Serialize;

use crate::domain::disk::{Bucket, Range};

use super::format_bytes;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HistoryRow {
    pub start: i64,
    pub at: String,
    pub used_percent: f64,
    pub used_avg: u64,
    pub used_min: u64,
    pub used_max: u64,
    pub used_last: u64,
    pub total: u64,
    pub available: u64,
    pub samples: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct HistorySeries {
    pub path: String,
    pub range: &'static str,
    pub resolution: &'static str,
    pub rows: Vec<HistoryRow>,
}

pub(crate) fn series(path: &str, range: Range, buckets: &[Bucket]) -> HistorySeries {
    HistorySeries {
        path: path.to_string(),
        range: range.name(),
        resolution: range.resolution().name(),
        rows: buckets
            .iter()
            .map(|bucket| HistoryRow {
                start: bucket.start,
                at: Local
                    .timestamp_opt(bucket.start, 0)
                    .single()
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_default(),
                used_percent: (bucket.percent_avg() * 10.0).round() / 10.0,
                used_avg: bucket.used_avg(),
                used_min: bucket.used_min,
                used_max: bucket.used_max,
                used_last: bucket.used_last,
                total: bucket.total,
                available: bucket.available_last,
                samples: bucket.samples,
            })
            .collect(),
    }
}

fn local_label(start: i64) -> String {
    Local
        .timestamp_opt(start, 0)
        .single()
        .map(|at| at.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| start.to_string())
}

pub(crate) fn render_table(series: &HistorySeries) -> String {
    let mut out = format!(
        "# {} · {} · {} buckets · {} rows\n",
        series.path,
        series.range,
        series.resolution,
        series.rows.len()
    );
    out.push_str(&format!(
        "{:<17} {:>6}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>3}\n",
        "time", "used%", "avg", "min", "max", "last", "total", "n"
    ));
    for row in &series.rows {
        out.push_str(&format!(
            "{:<17} {:>6.1}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>3}\n",
            local_label(row.start),
            row.used_percent,
            format_bytes(row.used_avg),
            format_bytes(row.used_min),
            format_bytes(row.used_max),
            format_bytes(row.used_last),
            format_bytes(row.total),
            row.samples
        ));
    }
    out
}

pub(crate) fn render_csv(all: &[HistorySeries]) -> String {
    let mut out = String::from(
        "path,resolution,start,time,used_percent,used_avg,used_min,used_max,used_last,total,available,samples\n",
    );
    for series in all {
        for row in &series.rows {
            out.push_str(&format!(
                "{},{},{},{},{:.1},{},{},{},{},{},{},{}\n",
                csv_field(&series.path),
                series.resolution,
                row.start,
                local_label(row.start),
                row.used_percent,
                row.used_avg,
                row.used_min,
                row.used_max,
                row.used_last,
                row.total,
                row.available,
                row.samples
            ));
        }
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::disk::DiskUsage;

    #[test]
    fn rows_carry_the_rounded_percent_and_local_time() {
        let bucket = Bucket::from_usage(1_700_000_000, DiskUsage::new(1_000, 819, 181));
        let series = series("/", Range::Week, &[bucket]);
        assert_eq!(series.resolution, "1h");
        assert_eq!(series.rows[0].used_percent, 81.9);
        assert!(series.rows[0].at.starts_with("2023-11-1"));
        let table = render_table(&series);
        assert!(table.starts_with("# / · 7d · 1h buckets · 1 rows"));
        assert!(table.contains("81.9"));
        let csv = render_csv(&[series]);
        assert_eq!(csv.lines().count(), 2);
        assert!(csv.lines().nth(1).unwrap().starts_with("/,1h,1700000000,"));
    }

    #[test]
    fn csv_quotes_paths_with_commas() {
        assert_eq!(csv_field("/Volumes/a,b"), "\"/Volumes/a,b\"");
        assert_eq!(csv_field("/plain"), "/plain");
    }
}
