//! 파일시스템 호출과 화면 표현에 독립적인 디스크 사용량 도메인.
//!
//! 바이트 수, 버킷 집계, 보관 해상도와 조회 기간의 규칙만 둔다. 어떤 경로를 재는지,
//! 어디에 저장하는지, 어떻게 그리는지는 바깥 계층이 정한다.

use chrono::{DateTime, Local, NaiveDate, TimeDelta, TimeZone};

/// 소진율로 판정한 심각도. 색과 문구는 presentation 이 정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    /// 이 소진율부터 경고로 본다.
    pub const WARNING_PERCENT: f64 = 80.0;
    /// 이 소진율부터 위험으로 본다. 여유가 10% 아래로 내려가면 APFS 스냅샷 정리와
    /// 쓰기 성능이 눈에 띄게 나빠진다.
    pub const CRITICAL_PERCENT: f64 = 90.0;

    pub fn from_used_percent(percent: f64) -> Self {
        if percent >= Self::CRITICAL_PERCENT {
            Self::Critical
        } else if percent >= Self::WARNING_PERCENT {
            Self::Warning
        } else {
            Self::Normal
        }
    }
}

/// 한 번 측정한 볼륨의 바이트 단위 용량.
///
/// `used` 는 `total - free`(루트 예약 블록 포함), `available` 은 일반 사용자가 실제로
/// 쓸 수 있는 양이다. APFS 에서는 둘의 합이 `total` 과 같다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    pub total: u64,
    pub used: u64,
    pub available: u64,
}

impl DiskUsage {
    pub fn new(total: u64, used: u64, available: u64) -> Self {
        Self {
            total,
            used: used.min(total),
            available: available.min(total),
        }
    }

    pub fn used_fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.used as f64 / self.total as f64
    }

    pub fn used_percent(&self) -> f64 {
        self.used_fraction() * 100.0
    }

    pub fn severity(&self) -> Severity {
        Severity::from_used_percent(self.used_percent())
    }
}

/// 측정 시각이 붙은 사용량.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: DateTime<Local>,
    pub usage: DiskUsage,
}

/// 보관 해상도. 가까운 과거일수록 촘촘하게, 먼 과거일수록 성기게 남긴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Resolution {
    FiveMinutes,
    Hourly,
    Daily,
}

impl Resolution {
    pub const ALL: [Resolution; 3] = [
        Resolution::FiveMinutes,
        Resolution::Hourly,
        Resolution::Daily,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Resolution::FiveMinutes => "5m",
            Resolution::Hourly => "1h",
            Resolution::Daily => "1d",
        }
    }

    /// 명목 버킷 길이(초). 하루는 서머타임 전환일에 23·25시간일 수 있지만 격자와
    /// 공백 판정에만 쓰므로 명목값으로 충분하다.
    pub fn seconds(self) -> i64 {
        match self {
            Resolution::FiveMinutes => 300,
            Resolution::Hourly => 3_600,
            Resolution::Daily => 86_400,
        }
    }

    /// 이 해상도의 버킷을 얼마나 오래 남기는지.
    ///
    /// 화면이 보는 기간(24시간·7일·30일)보다 넉넉하게 잡는다. 그래야 가장 오래된
    /// 구간도 차트에 끊김 없이 들어가고, 더 성긴 해상도를 만들 때 소스가 온전히 남는다.
    /// 하루 버킷은 1년 보기까지 감당하도록 400일을 둔다 — 행 수백 개라 비용이 없다.
    pub fn retention(self) -> TimeDelta {
        match self {
            Resolution::FiveMinutes => TimeDelta::hours(48),
            Resolution::Hourly => TimeDelta::days(14),
            Resolution::Daily => TimeDelta::days(400),
        }
    }

    /// 이 해상도를 만드는 더 촘촘한 해상도.
    pub fn source(self) -> Option<Resolution> {
        match self {
            Resolution::FiveMinutes => None,
            Resolution::Hourly => Some(Resolution::FiveMinutes),
            Resolution::Daily => Some(Resolution::Hourly),
        }
    }

    /// `at` 이 속한 버킷의 시작(epoch 초).
    ///
    /// 5분·1시간 버킷은 UTC 기준으로 정렬하고, 하루 버킷은 로컬 자정에 맞춘다 —
    /// "어제 하루" 가 화면의 하루와 같아야 하기 때문이다.
    pub fn bucket_start(self, at: DateTime<Local>) -> i64 {
        match self {
            Resolution::Daily => local_midnight(at.date_naive())
                .map(|midnight| midnight.timestamp())
                .unwrap_or_else(|| at.timestamp().div_euclid(86_400) * 86_400),
            _ => at.timestamp().div_euclid(self.seconds()) * self.seconds(),
        }
    }

    /// epoch 초로 받은 시각이 속한 버킷의 시작.
    pub fn bucket_start_of(self, epoch: i64) -> i64 {
        match self {
            Resolution::Daily => Local
                .timestamp_opt(epoch, 0)
                .single()
                .map(|at| self.bucket_start(at))
                .unwrap_or_else(|| epoch.div_euclid(86_400) * 86_400),
            _ => epoch.div_euclid(self.seconds()) * self.seconds(),
        }
    }

    /// `epoch` 가 속한 버킷의 다음 버킷 시작.
    pub fn next_bucket_start(self, epoch: i64) -> i64 {
        let start = self.bucket_start_of(epoch);
        match self {
            Resolution::Daily => Local
                .timestamp_opt(start, 0)
                .single()
                .and_then(|at| at.date_naive().succ_opt())
                .and_then(local_midnight)
                .map(|midnight| midnight.timestamp())
                .unwrap_or(start + 86_400),
            _ => start + self.seconds(),
        }
    }

    /// `epoch` 이후(또는 같은) 첫 버킷 경계.
    ///
    /// 소스 버킷이 이 시각부터 온전히 남아 있을 때 재집계를 시작할 지점이다.
    /// 앞쪽이 잘린 버킷을 다시 세면 평균이 틀어지므로 거기서는 멈춘다.
    pub fn first_boundary_at_or_after(self, epoch: i64) -> i64 {
        let start = self.bucket_start_of(epoch);
        if start == epoch {
            start
        } else {
            self.next_bucket_start(epoch)
        }
    }
}

/// 로컬 시간대의 자정. 서머타임 전환으로 자정이 없는 날은 그 다음 유효 시각이다.
pub fn local_midnight(date: NaiveDate) -> Option<DateTime<Local>> {
    let naive = date.and_hms_opt(0, 0, 0)?;
    Local.from_local_datetime(&naive).earliest()
}

/// 한 버킷의 집계. `used_sum / samples` 가 평균이다.
///
/// 평균뿐 아니라 최소·최대·마지막을 함께 두어 성긴 해상도에서도 하루 안의 출렁임이
/// 사라지지 않게 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bucket {
    pub start: i64,
    pub total: u64,
    pub used_min: u64,
    pub used_max: u64,
    pub used_sum: u64,
    pub used_last: u64,
    pub available_last: u64,
    pub samples: u32,
}

impl Bucket {
    pub fn from_usage(start: i64, usage: DiskUsage) -> Self {
        Self {
            start,
            total: usage.total,
            used_min: usage.used,
            used_max: usage.used,
            used_sum: usage.used,
            used_last: usage.used,
            available_last: usage.available,
            samples: 1,
        }
    }

    pub fn used_avg(&self) -> u64 {
        if self.samples == 0 {
            return self.used_last;
        }
        self.used_sum / u64::from(self.samples)
    }

    fn percent(&self, used: u64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        used as f64 / self.total as f64 * 100.0
    }

    pub fn percent_avg(&self) -> f64 {
        self.percent(self.used_avg())
    }

    pub fn percent_min(&self) -> f64 {
        self.percent(self.used_min)
    }

    pub fn percent_max(&self) -> f64 {
        self.percent(self.used_max)
    }

    pub fn percent_last(&self) -> f64 {
        self.percent(self.used_last)
    }

    /// 뒤에 온 표본이나 버킷을 합친다. 호출 순서가 시간 순서이므로 마지막 값은
    /// `later` 것이 남는다.
    pub fn absorb(&mut self, later: &Bucket) {
        self.used_min = self.used_min.min(later.used_min);
        self.used_max = self.used_max.max(later.used_max);
        self.used_sum = self.used_sum.saturating_add(later.used_sum);
        self.samples = self.samples.saturating_add(later.samples);
        self.used_last = later.used_last;
        self.available_last = later.available_last;
        self.total = later.total;
    }
}

/// 촘촘한 버킷을 더 성긴 해상도로 합친다. 입력 순서와 무관하게 시작 시각으로 정렬한다.
pub fn rollup(resolution: Resolution, source: &[Bucket]) -> Vec<Bucket> {
    let mut sorted: Vec<&Bucket> = source.iter().collect();
    sorted.sort_by_key(|bucket| bucket.start);

    let mut out: Vec<Bucket> = Vec::new();
    for bucket in sorted {
        let start = resolution.bucket_start_of(bucket.start);
        match out.last_mut() {
            Some(last) if last.start == start => last.absorb(bucket),
            _ => out.push(Bucket { start, ..*bucket }),
        }
    }
    out
}

/// 화면에서 고르는 기간. 기간마다 한 해상도를 쓴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Range {
    Day,
    Week,
    Month,
    Year,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Day, Range::Week, Range::Month, Range::Year];
    pub const DEFAULT: Range = Range::Day;

    pub fn name(self) -> &'static str {
        match self {
            Range::Day => "24h",
            Range::Week => "7d",
            Range::Month => "30d",
            Range::Year => "1y",
        }
    }

    pub fn parse(text: &str) -> Option<Range> {
        match text.trim().to_ascii_lowercase().as_str() {
            "24h" | "1d" | "day" | "today" => Some(Range::Day),
            "7d" | "1w" | "week" => Some(Range::Week),
            "30d" | "1m" | "month" => Some(Range::Month),
            "1y" | "365d" | "year" => Some(Range::Year),
            _ => None,
        }
    }

    pub fn duration(self) -> TimeDelta {
        match self {
            Range::Day => TimeDelta::hours(24),
            Range::Week => TimeDelta::days(7),
            Range::Month => TimeDelta::days(30),
            Range::Year => TimeDelta::days(365),
        }
    }

    pub fn resolution(self) -> Resolution {
        match self {
            Range::Day => Resolution::FiveMinutes,
            Range::Week => Resolution::Hourly,
            Range::Month | Range::Year => Resolution::Daily,
        }
    }

    pub fn started_at(self, now: DateTime<Local>) -> DateTime<Local> {
        now - self.duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(used: u64) -> DiskUsage {
        DiskUsage::new(1_000, used, 1_000 - used)
    }

    fn at(epoch: i64) -> DateTime<Local> {
        Local.timestamp_opt(epoch, 0).single().unwrap()
    }

    #[test]
    fn severity_thresholds_are_inclusive() {
        assert_eq!(Severity::from_used_percent(79.9), Severity::Normal);
        assert_eq!(Severity::from_used_percent(80.0), Severity::Warning);
        assert_eq!(Severity::from_used_percent(90.0), Severity::Critical);
    }

    #[test]
    fn usage_clamps_impossible_values() {
        let value = DiskUsage::new(100, 150, 200);
        assert_eq!(value.used, 100);
        assert_eq!(value.available, 100);
        assert_eq!(DiskUsage::new(0, 0, 0).used_percent(), 0.0);
    }

    #[test]
    fn five_minute_and_hourly_buckets_align_to_utc_grid() {
        let five = Resolution::FiveMinutes;
        assert_eq!(five.bucket_start(at(1_000_000_299)), 1_000_000_200);
        assert_eq!(five.next_bucket_start(1_000_000_299), 1_000_000_500);
        assert_eq!(
            Resolution::Hourly.bucket_start_of(1_000_003_599),
            1_000_000_800
        );
    }

    #[test]
    fn daily_buckets_start_at_local_midnight() {
        let noon = Local
            .with_ymd_and_hms(2026, 10, 6, 12, 30, 0)
            .single()
            .unwrap();
        let start = Resolution::Daily.bucket_start(noon);
        let midnight = Local
            .with_ymd_and_hms(2026, 10, 6, 0, 0, 0)
            .single()
            .unwrap();
        assert_eq!(start, midnight.timestamp());
        let next = Resolution::Daily.next_bucket_start(noon.timestamp());
        assert_eq!(
            next,
            Local
                .with_ymd_and_hms(2026, 10, 7, 0, 0, 0)
                .single()
                .unwrap()
                .timestamp()
        );
    }

    #[test]
    fn first_boundary_keeps_aligned_starts_and_skips_partial_ones() {
        let hourly = Resolution::Hourly;
        assert_eq!(hourly.first_boundary_at_or_after(7_200), 7_200);
        assert_eq!(hourly.first_boundary_at_or_after(7_500), 10_800);
    }

    #[test]
    fn rollup_merges_buckets_of_the_same_hour_in_time_order() {
        let source = vec![
            Bucket::from_usage(3_900, usage(300)),
            Bucket::from_usage(3_600, usage(100)),
            Bucket::from_usage(7_200, usage(500)),
        ];
        let rolled = rollup(Resolution::Hourly, &source);
        assert_eq!(rolled.len(), 2);
        let first = rolled[0];
        assert_eq!(first.start, 3_600);
        assert_eq!(first.samples, 2);
        assert_eq!(first.used_min, 100);
        assert_eq!(first.used_max, 300);
        assert_eq!(first.used_last, 300, "뒤에 온 표본이 마지막 값이어야 함");
        assert_eq!(first.used_avg(), 200);
        assert_eq!(rolled[1].start, 7_200);
    }

    #[test]
    fn absorb_keeps_the_later_total_and_available() {
        let mut bucket = Bucket::from_usage(0, DiskUsage::new(1_000, 500, 500));
        bucket.absorb(&Bucket::from_usage(0, DiskUsage::new(2_000, 700, 1_300)));
        assert_eq!(bucket.total, 2_000);
        assert_eq!(bucket.available_last, 1_300);
        assert_eq!(bucket.used_sum, 1_200);
    }

    #[test]
    fn range_names_parse_with_aliases() {
        assert_eq!(Range::parse("24h"), Some(Range::Day));
        assert_eq!(Range::parse(" 1W "), Some(Range::Week));
        assert_eq!(Range::parse("month"), Some(Range::Month));
        assert_eq!(Range::parse("1y"), Some(Range::Year));
        assert_eq!(Range::parse("2h"), None);
        for range in Range::ALL {
            assert_eq!(Range::parse(range.name()), Some(range));
        }
    }

    #[test]
    fn each_range_uses_a_single_resolution_whose_retention_covers_it() {
        for range in Range::ALL {
            let resolution = range.resolution();
            assert!(
                resolution.retention() >= range.duration(),
                "{} 보기의 {} 보존 기간이 짧음",
                range.name(),
                resolution.name()
            );
        }
    }
}
