//! `watch`, 파이프, 일회성 실행용 stdout 렌더러.
//!
//! ratatui 는 alternate screen + raw mode 를 쓰기 때문에 `watch` 아래에서는
//! 동작하지 않는다. stdout 이 TTY 가 아니면 항상 이쪽으로 온다.

use chrono::{DateTime, Local};

use super::model::{self, Bar};
use super::{BOLD, DIM, MUTED, RESET, SPARK, VolumeView, ansi_for, chart};
use crate::domain::disk::{Range, Severity};

const GAUGE_MIN: usize = 20;
const GAUGE_MAX: usize = 48;
/// 게이지 우측 라벨(`100% used`)이 들어갈 자리 + 오른쪽 여백.
const SUFFIX_ROOM: usize = 14 + RIGHT_MARGIN;
const RIGHT_MARGIN: usize = 2;

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub(crate) fn render(
    views: &[VolumeView<'_>],
    range: Range,
    color: bool,
    width: usize,
    now: DateTime<Local>,
) -> String {
    let gauge_width = width
        .saturating_sub(SUFFIX_ROOM)
        .clamp(GAUGE_MIN, GAUGE_MAX);
    views
        .iter()
        .map(|view| render_one(view, range, color, gauge_width, now))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_one(
    view: &VolumeView<'_>,
    range: Range,
    color: bool,
    gauge_width: usize,
    now: DateTime<Local>,
) -> String {
    let (b, mu, r) = if color {
        (BOLD, MUTED, RESET)
    } else {
        ("", "", "")
    };
    let mut s = format!("{b} {}{r}\n", view.path);
    match view.result {
        Ok(sample) => {
            let meter = model::project(sample);
            s.push_str(&render_bar(&meter.usage, color, gauge_width));
            s.push_str(&format!("  {mu}{}{r}\n", meter.detail));
            s.push_str(&render_history(view, range, color, gauge_width, now));
            // 값이 언제 기준인지 항상 밝힌다
            s.push_str(&format!(
                "  {mu}{}{r}\n",
                model::origin_text(sample.at, now)
            ));
        }
        Err(error) => {
            let a = if color {
                ansi_for(Severity::Critical)
            } else {
                ""
            };
            s.push_str(&format!("  {a}{error}{r}\n"));
        }
    }
    s
}

fn render_history(
    view: &VolumeView<'_>,
    range: Range,
    color: bool,
    gauge_width: usize,
    now: DateTime<Local>,
) -> String {
    let (mu, sp, r) = if color {
        (MUTED, SPARK, RESET)
    } else {
        ("", "", "")
    };
    if view.history.is_empty() {
        return format!("  {mu}{} · no history yet{r}\n", range.name());
    }
    let cells = chart::sparkline_cells(view.history, range, now, gauge_width);
    let spark: String = cells
        .iter()
        .map(|cell| match cell {
            Some(value) => {
                BARS[((*value as usize) * (BARS.len() - 1))
                    .div_ceil(100)
                    .min(BARS.len() - 1)]
            }
            None => '·',
        })
        .collect();
    let mut note = format!("{} · {}", range.name(), chart::domain_label(view.history));
    if let Some(delta) = model::delta(view.history) {
        note.push_str(&format!(" · {delta}"));
    }
    format!("  {sp}{spark}{r}  {mu}{note}{r}\n")
}

fn render_bar(bar: &Bar, color: bool, gauge_width: usize) -> String {
    let filled = (bar.fill_clamped() * gauge_width as f64).round() as usize;
    let (d, mu, r) = if color {
        (DIM, MUTED, RESET)
    } else {
        ("", "", "")
    };
    let a = if color { ansi_for(bar.level) } else { "" };
    // 색이 꺼진 환경에서는 색만으로 채움/빈칸을 구분할 수 없으므로 글자를 바꾼다
    let empty_ch = if color { "█" } else { "░" };

    let mut s = String::from("  ");
    s.push_str(a);
    s.push_str(&"█".repeat(filled));
    s.push_str(r);
    s.push_str(d);
    s.push_str(&empty_ch.repeat(gauge_width - filled));
    s.push_str(r);
    s.push_str(&format!("  {mu}{}{r}\n", bar.label));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{ProbeError, VolumeResult};
    use crate::domain::disk::{Bucket, DiskUsage, Sample};

    fn volume(path: &str, used: u64) -> VolumeResult {
        VolumeResult {
            path: path.into(),
            result: Ok(Sample {
                at: Local::now(),
                usage: DiskUsage::new(1_000, used, 1_000 - used),
            }),
        }
    }

    #[test]
    fn plain_output_shows_gauge_detail_sparkline_and_origin() {
        let now = Local::now();
        let ok = volume("/", 820);
        let start = Range::Day.started_at(now).timestamp();
        let history = [
            Bucket::from_usage(start + 300, DiskUsage::new(1_000, 810, 190)),
            Bucket::from_usage(now.timestamp(), DiskUsage::new(1_000, 820, 180)),
        ];
        let text = render(
            &[VolumeView::new(&ok, &history)],
            Range::Day,
            false,
            80,
            now,
        );
        assert!(text.starts_with(" /\n"));
        assert!(text.contains("82% used"));
        assert!(text.contains("820 B of 1.0 kB · 180 B free"));
        assert!(text.contains("24h · "));
        assert!(text.contains("+1.0%p"));
        assert!(text.contains("sampled "));
        assert!(text.contains('░'), "색이 없으면 빈칸은 다른 글자여야 함");
    }

    #[test]
    fn a_failed_volume_prints_its_error_in_place() {
        let gone = VolumeResult {
            path: "/Volumes/Gone".into(),
            result: Err(ProbeError::Missing("/Volumes/Gone".into())),
        };
        let text = render(
            &[VolumeView::new(&gone, &[])],
            Range::Day,
            false,
            80,
            Local::now(),
        );
        assert!(text.contains("/Volumes/Gone is not mounted"));
        assert!(!text.contains("used"));
    }

    #[test]
    fn missing_history_is_said_not_drawn() {
        let ok = volume("/", 100);
        let text = render(
            &[VolumeView::new(&ok, &[])],
            Range::Week,
            false,
            80,
            Local::now(),
        );
        assert!(text.contains("7d · no history yet"));
    }
}
