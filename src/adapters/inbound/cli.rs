use anyhow::{Result, bail};
use clap::Parser;

use crate::domain::disk::Range;

#[derive(Debug, Parser)]
#[command(version = crate::VERSION, long_about = None)]
pub struct Cli {
    /// Stay resident and keep sampling full screen
    #[arg(short = 'w', long)]
    pub watch: bool,

    /// Sampling interval in seconds. Implies watch mode
    #[arg(short = 'n', long, value_name = "SECS")]
    pub interval: Option<u64>,

    /// Print JSON, for statuslines and scripts
    #[arg(short = 'j', long, conflicts_with_all = ["watch", "interval"])]
    pub json: bool,

    /// History range to show: 24h, 7d, 30d or 1y
    #[arg(short = 'r', long, value_name = "RANGE", default_value = "24h")]
    pub range: String,

    /// Disable colour
    #[arg(long)]
    pub no_color: bool,
}

/// 5분 버킷 하나에 표본 하나. 디스크는 그보다 자주 재도 얻는 것이 없다.
pub const DEFAULT_INTERVAL: u64 = 300;
/// 로컬 `statfs` 는 공짜에 가깝지만, 이보다 짧으면 화면 갱신 외에 의미가 없다.
pub const MIN_INTERVAL: u64 = 10;

impl Cli {
    /// `--watch` 또는 `--interval` 중 하나라도 있으면 상주 모드.
    pub fn is_watch(&self) -> bool {
        self.watch || self.interval.is_some()
    }

    /// 사용자가 준 값을 하한선으로 잘라낸 실제 주기.
    pub fn interval_secs(&self) -> u64 {
        self.interval.unwrap_or(DEFAULT_INTERVAL).max(MIN_INTERVAL)
    }

    /// 요청값이 하한선에 걸렸으면 알려주기 위해.
    pub fn interval_was_clamped(&self) -> bool {
        matches!(self.interval, Some(v) if v < MIN_INTERVAL)
    }

    pub fn range(&self) -> Result<Range> {
        parse_range(&self.range)
    }
}

/// `24h`, `7d`, `30d`, `1y` 와 그 별칭을 받는다.
pub fn parse_range(text: &str) -> Result<Range> {
    match Range::parse(text) {
        Some(range) => Ok(range),
        None => bail!(
            "unknown range: {text}. use one of {}",
            Range::ALL
                .iter()
                .map(|range| range.name())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
