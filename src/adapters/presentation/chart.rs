//! 버킷 이력을 차트 좌표로 투영한다.
//!
//! 웹과 Hammerspoon 은 이 좌표를 그대로 그리고, TUI 는 Sparkline 셀로 접는다.
//! 세로축은 기간 안의 실제 변화 폭에 맞춘다 — 디스크는 하루에 1%p 도 안 움직이는
//! 날이 많아 0~100% 고정 축에서는 선이 평평해 아무것도 읽을 수 없다. 대신 축 눈금을
//! 항상 함께 보내 독자가 배율을 알 수 있게 한다.

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike, Weekday};
use serde::Serialize;

use crate::domain::disk::{Bucket, Range, Resolution, local_midnight};

pub(crate) const VIEW_WIDTH: f64 = 1000.0;
pub(crate) const VIEW_HEIGHT: f64 = 100.0;

/// 버킷 간격이 버킷 길이의 이 배수를 넘으면 기록이 끊긴 것으로 보고 선을 끊는다.
/// 수집이 멈춘 시간을 직선으로 이으면 없는 측정을 있는 것처럼 보이게 한다.
const GAP_FACTOR: i64 = 3;

/// 세로축의 최소 폭(%p). 변화가 거의 없는 기간에 잡음을 절벽처럼 키우지 않는다.
const MIN_SPAN: f64 = 6.0;

#[derive(Debug, Default, Serialize)]
pub(crate) struct Chart {
    pub from: i64,
    pub to: i64,
    pub resolution: &'static str,
    pub y_min: f64,
    pub y_max: f64,
    pub y_ticks: Vec<Tick>,
    pub markers: Vec<Marker>,
    pub points: Vec<Point>,
    pub line_path: String,
    pub area_path: String,
    /// 성긴 해상도에서 버킷 안의 최소~최대 범위. 모든 버킷이 표본 하나면 비어 있다.
    pub band_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
    pub at: i64,
    pub percent: f64,
    pub percent_min: f64,
    pub percent_max: f64,
    pub used: u64,
    pub total: u64,
    pub available: u64,
    pub samples: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Marker {
    pub x: f64,
    pub kind: &'static str,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Tick {
    pub y: f64,
    pub label: String,
}

pub(crate) fn project(buckets: &[Bucket], range: Range, now: DateTime<Local>) -> Chart {
    let to = now.timestamp();
    let from = range.started_at(now).timestamp();
    let span = (to - from).max(1);
    let (y_min, y_max) = y_domain(buckets);
    let resolution = range.resolution();
    let inside: Vec<Bucket> = buckets
        .iter()
        .copied()
        .filter(|bucket| bucket.start <= to)
        .collect();

    let mut points = Vec::new();
    let mut lines = Vec::new();
    let mut areas = Vec::new();
    let mut bands = Vec::new();
    for run in segments(&inside, resolution) {
        let run_points: Vec<Point> = run
            .iter()
            .map(|bucket| point_for(bucket, from, span, y_min, y_max))
            .collect();
        match run_points.as_slice() {
            [] => {}
            // 공백 뒤 첫 표본처럼 구간에 점이 하나뿐이면 길이 0 subpath 로 남긴다.
            // `stroke-linecap: round` 가 점으로 그려 준다.
            [only] => lines.push(format!(
                "M{:.1} {:.1} L{:.1} {:.1}",
                only.x, only.y, only.x, only.y
            )),
            many => {
                let line = path_of(many.iter().map(|point| (point.x, point.y)));
                let first_x = many[0].x;
                let last_x = many[many.len() - 1].x;
                areas.push(format!(
                    "M{first_x:.1} {VIEW_HEIGHT:.0} L{} L{last_x:.1} {VIEW_HEIGHT:.0} Z",
                    line.strip_prefix('M').unwrap_or(&line)
                ));
                if many
                    .iter()
                    .any(|point| point.samples > 1 && point.percent_max > point.percent_min)
                {
                    let upper = many
                        .iter()
                        .map(|point| (point.x, y_of(point.percent_max, y_min, y_max)));
                    let lower = many
                        .iter()
                        .rev()
                        .map(|point| (point.x, y_of(point.percent_min, y_min, y_max)));
                    bands.push(format!("{} Z", path_of(upper.chain(lower))));
                }
                lines.push(line);
            }
        }
        points.extend(run_points);
    }

    Chart {
        from,
        to,
        resolution: resolution.name(),
        y_min,
        y_max,
        y_ticks: ticks(y_min, y_max),
        markers: markers(range, from, to),
        points,
        line_path: lines.join(" "),
        area_path: areas.join(" "),
        band_path: bands.join(" "),
    }
}

fn point_for(bucket: &Bucket, from: i64, span: i64, y_min: f64, y_max: f64) -> Point {
    let x = ((bucket.start - from) as f64 / span as f64).clamp(0.0, 1.0) * VIEW_WIDTH;
    Point {
        x,
        y: y_of(bucket.percent_avg(), y_min, y_max),
        at: bucket.start,
        percent: bucket.percent_avg(),
        percent_min: bucket.percent_min(),
        percent_max: bucket.percent_max(),
        used: bucket.used_avg(),
        total: bucket.total,
        available: bucket.available_last,
        samples: bucket.samples,
    }
}

fn y_of(percent: f64, y_min: f64, y_max: f64) -> f64 {
    let span = (y_max - y_min).max(f64::EPSILON);
    VIEW_HEIGHT - ((percent - y_min) / span).clamp(0.0, 1.0) * VIEW_HEIGHT
}

fn path_of(points: impl IntoIterator<Item = (f64, f64)>) -> String {
    points
        .into_iter()
        .enumerate()
        .map(|(index, (x, y))| format!("{}{x:.1} {y:.1}", if index == 0 { "M" } else { "L" }))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 기간 안의 최소~최대에 여백을 더한 세로축. 항상 0~100 안이고 폭은 `MIN_SPAN` 이상이다.
pub(crate) fn y_domain(buckets: &[Bucket]) -> (f64, f64) {
    let Some(first) = buckets.first() else {
        return (0.0, 100.0);
    };
    let (mut low, mut high) = buckets.iter().fold(
        (first.percent_min(), first.percent_max()),
        |(low, high), bucket| {
            (
                low.min(bucket.percent_min()),
                high.max(bucket.percent_max()),
            )
        },
    );
    let pad = ((high - low) * 0.3).max(1.0);
    low = (low - pad).floor();
    high = (high + pad).ceil();
    if high - low < MIN_SPAN {
        let center = (low + high) / 2.0;
        low = (center - MIN_SPAN / 2.0).floor();
        high = (center + MIN_SPAN / 2.0).ceil();
    }
    if high > 100.0 {
        low -= high - 100.0;
        high = 100.0;
    }
    if low < 0.0 {
        high = (high - low).min(100.0);
        low = 0.0;
    }
    (low, high)
}

/// `80–85%` 처럼 세로축이 어디서 어디까지인지.
pub(crate) fn domain_label(buckets: &[Bucket]) -> String {
    let (low, high) = y_domain(buckets);
    format!("{low:.0}–{high:.0}%")
}

fn ticks(low: f64, high: f64) -> Vec<Tick> {
    let span = high - low;
    let step = [1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0]
        .into_iter()
        .find(|step| span / step <= 5.0)
        .unwrap_or(50.0);
    let mut out = Vec::new();
    let mut value = (low / step).ceil() * step;
    while value <= high + 1e-9 {
        out.push(Tick {
            y: y_of(value, low, high),
            label: format!("{value:.0}%"),
        });
        value += step;
    }
    out
}

/// 기록이 끊긴 자리에서 버킷을 끊어 구간별로 나눈다.
pub(crate) fn segments(buckets: &[Bucket], resolution: Resolution) -> Vec<&[Bucket]> {
    if buckets.is_empty() {
        return Vec::new();
    }
    let limit = resolution.seconds() * GAP_FACTOR;
    let mut runs = Vec::new();
    let mut start = 0;
    for index in 1..buckets.len() {
        if buckets[index].start - buckets[index - 1].start > limit {
            runs.push(&buckets[start..index]);
            start = index;
        }
    }
    runs.push(&buckets[start..]);
    runs
}

/// 시간 격자. 라벨이 있는 것만 축 아래에 글자가 붙는다.
fn markers(range: Range, from: i64, to: i64) -> Vec<Marker> {
    let span = (to - from).max(1) as f64;
    let x = |at: i64| (at - from) as f64 / span * VIEW_WIDTH;
    let mut out = Vec::new();
    match range {
        Range::Day => {
            let mut hour = from.div_euclid(3_600) * 3_600 + 3_600;
            while hour < to {
                if let Some(at) = Local.timestamp_opt(hour, 0).single() {
                    let h = at.hour();
                    if h == 0 {
                        out.push(Marker {
                            x: x(hour),
                            kind: "midnight",
                            label: Some(at.format("%b %-d").to_string()),
                        });
                    } else if h % 3 == 0 {
                        out.push(Marker {
                            x: x(hour),
                            kind: "hour",
                            label: (h % 6 == 0).then(|| at.format("%H:%M").to_string()),
                        });
                    }
                }
                hour += 3_600;
            }
        }
        Range::Week => {
            for midnight in midnights(from, to) {
                out.push(Marker {
                    x: x(midnight.timestamp()),
                    kind: "midnight",
                    label: Some(midnight.format("%a %-d").to_string()),
                });
            }
        }
        Range::Month => {
            for midnight in midnights(from, to) {
                let (kind, label) = if midnight.weekday() == Weekday::Mon {
                    ("week", Some(midnight.format("%b %-d").to_string()))
                } else {
                    ("day", None)
                };
                out.push(Marker {
                    x: x(midnight.timestamp()),
                    kind,
                    label,
                });
            }
        }
        Range::Year => {
            for first in month_starts(from, to) {
                out.push(Marker {
                    x: x(first.timestamp()),
                    kind: "month",
                    label: Some(first.format("%b").to_string()),
                });
            }
        }
    }
    out
}

fn midnights(from: i64, to: i64) -> Vec<DateTime<Local>> {
    let Some(start) = Local.timestamp_opt(from, 0).single() else {
        return Vec::new();
    };
    let mut date = start.date_naive();
    let mut out = Vec::new();
    while let Some(midnight) = local_midnight(date) {
        let at = midnight.timestamp();
        if at >= to {
            break;
        }
        if at > from {
            out.push(midnight);
        }
        match date.succ_opt() {
            Some(next) => date = next,
            None => break,
        }
    }
    out
}

fn month_starts(from: i64, to: i64) -> Vec<DateTime<Local>> {
    let Some(start) = Local.timestamp_opt(from, 0).single() else {
        return Vec::new();
    };
    let (mut year, mut month) = (start.year(), start.month());
    let mut out = Vec::new();
    while let Some(midnight) = NaiveDate::from_ymd_opt(year, month, 1).and_then(local_midnight) {
        let at = midnight.timestamp();
        if at >= to {
            break;
        }
        if at > from {
            out.push(midnight);
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    out
}

/// TUI Sparkline 셀. 기간 전체를 `width` 칸에 펴고, 값은 세로축 범위를 0~100 으로 편 것이다.
pub(crate) fn sparkline_cells(
    buckets: &[Bucket],
    range: Range,
    now: DateTime<Local>,
    width: usize,
) -> Vec<Option<u64>> {
    if width == 0 {
        return Vec::new();
    }
    let to = now.timestamp();
    let from = range.started_at(now).timestamp();
    let span = (to - from).max(1) as f64;
    let (low, high) = y_domain(buckets);
    let mut cells = vec![None; width];
    for bucket in buckets {
        if bucket.start > to {
            continue;
        }
        let offset = ((bucket.start - from) as f64 / span).clamp(0.0, 1.0);
        let index = ((offset * width as f64) as usize).min(width - 1);
        let scaled =
            ((bucket.percent_avg() - low) / (high - low).max(f64::EPSILON)).clamp(0.0, 1.0);
        cells[index] = Some((scaled * 100.0).round() as u64);
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::disk::DiskUsage;

    fn bucket(start: i64, used: u64) -> Bucket {
        Bucket::from_usage(start, DiskUsage::new(1_000, used, 1_000 - used))
    }

    #[test]
    fn empty_history_uses_the_full_axis() {
        assert_eq!(y_domain(&[]), (0.0, 100.0));
    }

    #[test]
    fn the_axis_hugs_the_data_but_never_leaves_zero_to_one_hundred() {
        let (low, high) = y_domain(&[bucket(0, 820), bucket(300, 824)]);
        assert!(low <= 81.0 && high >= 83.0, "{low}..{high}");
        assert!(high - low >= MIN_SPAN);
        let (low, high) = y_domain(&[bucket(0, 995)]);
        assert_eq!(high, 100.0);
        assert!(low >= 90.0, "{low}");
        let (low, _) = y_domain(&[bucket(0, 2)]);
        assert_eq!(low, 0.0);
    }

    #[test]
    fn ticks_stay_within_the_axis_and_are_few() {
        let ticks = ticks(78.0, 86.0);
        assert!(ticks.len() <= 6 && ticks.len() >= 3, "{ticks:?}");
        assert_eq!(ticks[0].label, "78%");
        assert!(ticks.iter().all(|tick| (0.0..=100.0).contains(&tick.y)));
    }

    #[test]
    fn a_long_gap_splits_the_series_but_a_missed_sample_does_not() {
        let joined = [bucket(0, 1), bucket(600, 1), bucket(900, 1)];
        assert_eq!(segments(&joined, Resolution::FiveMinutes).len(), 1);
        let split = [bucket(0, 1), bucket(300, 1), bucket(3_600, 1)];
        assert_eq!(segments(&split, Resolution::FiveMinutes).len(), 2);
        let daily = [bucket(0, 1), bucket(86_400 * 2, 1)];
        assert_eq!(segments(&daily, Resolution::Daily).len(), 1);
    }

    #[test]
    fn projection_draws_dots_areas_and_bands() {
        let now = Local::now();
        let start = Range::Day.started_at(now).timestamp();
        let mut wide = bucket(start + 1_200, 800);
        wide.absorb(&bucket(start + 1_200, 840));
        let buckets = [
            bucket(start + 600, 810),
            bucket(start + 900, 812),
            wide,
            // 세 시간 뒤 홀로 남은 표본.
            bucket(start + 20_000, 830),
        ];
        let chart = project(&buckets, Range::Day, now);
        assert_eq!(chart.points.len(), 4);
        assert_eq!(
            chart.line_path.matches('M').count(),
            2,
            "{}",
            chart.line_path
        );
        assert_eq!(
            chart.area_path.matches('Z').count(),
            1,
            "{}",
            chart.area_path
        );
        assert!(chart.band_path.ends_with('Z'), "{}", chart.band_path);
        assert_eq!(chart.resolution, "5m");
        assert!(
            chart
                .points
                .iter()
                .all(|point| (0.0..=1000.0).contains(&point.x))
        );
        assert!(!chart.y_ticks.is_empty());
    }

    #[test]
    fn a_week_marks_every_local_midnight_and_a_day_marks_hours() {
        let now = Local::now();
        let week = markers(
            Range::Week,
            Range::Week.started_at(now).timestamp(),
            now.timestamp(),
        );
        let midnights = week.iter().filter(|m| m.kind == "midnight").count();
        assert!((6..=7).contains(&midnights), "{week:?}");
        assert!(week.iter().all(|m| m.label.is_some()));

        let day = markers(
            Range::Day,
            Range::Day.started_at(now).timestamp(),
            now.timestamp(),
        );
        assert!(day.iter().any(|m| m.kind == "hour"), "{day:?}");
        assert!(day.iter().filter(|m| m.kind == "midnight").count() <= 1);

        let year = markers(
            Range::Year,
            Range::Year.started_at(now).timestamp(),
            now.timestamp(),
        );
        assert!((11..=12).contains(&year.len()), "{year:?}");
    }

    #[test]
    fn sparkline_cells_cover_the_whole_range() {
        let now = Local::now();
        let start = Range::Day.started_at(now).timestamp();
        let cells = sparkline_cells(
            &[bucket(start + 300, 500), bucket(now.timestamp(), 600)],
            Range::Day,
            now,
            20,
        );
        assert_eq!(cells.len(), 20);
        assert!(cells[0].is_some());
        assert!(cells[19].is_some());
        assert!(cells[1..19].iter().all(Option::is_none));
        assert!(sparkline_cells(&[], Range::Day, now, 0).is_empty());
    }
}
