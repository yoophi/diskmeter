//! 상주 모드 TUI. 표본은 세션이 담당하고 이 모듈은 상태를 읽어 그린다.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph, Sparkline};
use ratatui::{DefaultTerminal, Frame};

use crate::application::{LiveSession, SessionState, WatchPane};
use crate::domain::disk::{Range, Severity};

use super::chart;
use super::model::{self, Bar};

const TICK: Duration = Duration::from_secs(1);
const INDENT: u16 = 3;
const RIGHT_MARGIN: u16 = 2;
const PANE_GAP: u16 = 1;
/// 차트 높이 후보. 터미널이 낮으면 차례로 줄이고, 그래도 안 들어가면 차트를 뺀다.
const CHART_ROWS: [u16; 4] = [4, 3, 2, 0];
/// 제목·게이지·상세·각주 네 줄은 차트와 무관하게 필요하다.
const FIXED_ROWS: u16 = 4;

/// 한 프레임을 그리는 데 필요한 전부. 화면은 세션 상태를 읽기만 한다.
struct Screen<'a> {
    prog: &'a str,
    timezone: &'a str,
    /// 함께 뜬 웹 대시보드 주소. 없으면 배너를 그리지 않는다.
    web: Option<&'a str>,
    range: Range,
    state: &'a SessionState,
}

/// 표본은 세션이 담당하므로 화면은 상태를 읽고 요청만 보낸다.
pub(crate) fn run(
    prog: &str,
    timezone: String,
    session: Arc<LiveSession>,
    web: Option<String>,
    range: Range,
) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(
        &mut terminal,
        prog,
        &timezone,
        web.as_deref(),
        range,
        &session,
    );
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    prog: &str,
    timezone: &str,
    web: Option<&str>,
    initial: Range,
    session: &LiveSession,
) -> Result<()> {
    let mut range = initial;
    loop {
        {
            let state = session.read();
            let screen = Screen {
                prog,
                timezone,
                web,
                range,
                state: &state,
            };
            terminal.draw(|frame| draw(frame, &screen))?;
        }

        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Char('r') => session.request(),
                KeyCode::Tab => range = next_range(range),
                code => {
                    if let Some(selected) = range_for_key(code) {
                        range = selected;
                    }
                }
            }
        }
    }
}

fn range_for_key(code: KeyCode) -> Option<Range> {
    match code {
        KeyCode::Char('1') => Some(Range::Day),
        KeyCode::Char('2') => Some(Range::Week),
        KeyCode::Char('3') => Some(Range::Month),
        KeyCode::Char('4') => Some(Range::Year),
        _ => None,
    }
}

fn next_range(range: Range) -> Range {
    let index = Range::ALL
        .iter()
        .position(|candidate| *candidate == range)
        .unwrap_or(0);
    Range::ALL[(index + 1) % Range::ALL.len()]
}

fn header_height(screen: &Screen) -> u16 {
    if screen.web.is_some() { 3 } else { 2 }
}

fn draw(frame: &mut Frame, screen: &Screen) {
    let full = frame.area();
    let canvas = Rect {
        width: full.width.saturating_sub(RIGHT_MARGIN),
        ..full
    };
    let footer_height = if screen.state.watch.any_failed() {
        2
    } else {
        1
    };
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(header_height(screen)),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .areas(canvas);

    draw_header(frame, header, screen);
    draw_panes(frame, body, screen);
    draw_footer(frame, footer, screen);
}

fn draw_header(frame: &mut Frame, area: Rect, screen: &Screen) {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {}", screen.prog),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", screen.timezone),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if let Some(address) = screen.web {
        lines.push(Line::from(vec![
            Span::styled(" web server running", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("  {address}"),
                Style::default()
                    .fg(Color::Indexed(109))
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// 구획은 세로로 쌓는다. 높이가 모자라면 차트부터 줄인다.
fn draw_panes(frame: &mut Frame, area: Rect, screen: &Screen) {
    let panes = screen.state.watch.panes();
    let Ok(count) = u16::try_from(panes.len()) else {
        return;
    };
    if count == 0 {
        return;
    }
    let chart_rows = CHART_ROWS
        .into_iter()
        .find(|rows| count * (FIXED_ROWS + rows) + (count - 1) * PANE_GAP <= area.height)
        .unwrap_or(0);
    let pane_height = FIXED_ROWS + chart_rows;
    let mut y = area.y;
    for pane in panes {
        if y + pane_height > area.bottom() {
            break;
        }
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: pane_height,
        };
        draw_pane(frame, rect, pane, chart_rows, screen.range);
        y += pane_height + PANE_GAP;
    }
}

fn draw_pane(frame: &mut Frame, area: Rect, pane: &WatchPane, chart_rows: u16, range: Range) {
    let now = Local::now();
    let [title, gauge_row, chart_area, detail, footnote] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(chart_rows),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", pane.path),
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        title,
    );

    let Some(sample) = &pane.sample else {
        let message = match &pane.error {
            Some(error) => Span::styled(
                error.as_str(),
                Style::default().fg(color_for(Severity::Critical)),
            ),
            None => Span::styled(
                "waiting for the first sample",
                Style::default().fg(Color::DarkGray),
            ),
        };
        frame.render_widget(
            Paragraph::new(Line::from(message)),
            indent(gauge_row, INDENT),
        );
        return;
    };

    let meter = model::project(sample);
    frame.render_widget(gauge(&meter.usage), indent(gauge_row, INDENT));

    let series = pane.series(range);
    if chart_rows > 0 {
        let plot = indent(chart_area, INDENT);
        let cells = chart::sparkline_cells(series, range, now, plot.width as usize);
        frame.render_widget(
            Sparkline::default()
                .data(cells.iter().copied())
                .max(100)
                .style(Style::default().fg(Color::Indexed(109)))
                .absent_value_style(Style::default().fg(Color::DarkGray))
                .absent_value_symbol("·"),
            plot,
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            meter.detail.as_str(),
            Style::default().fg(Color::DarkGray),
        ))),
        indent(detail, INDENT),
    );

    let mut note = if series.is_empty() {
        format!("{} · no history yet", range.name())
    } else {
        format!("{} · {}", range.name(), chart::domain_label(series))
    };
    if let Some(delta) = model::delta(series) {
        note.push_str(&format!(" · {delta}"));
    }
    note.push_str(&format!("  ·  {}", model::origin_text(sample.at, now)));
    let mut spans = vec![Span::styled(note, Style::default().fg(Color::DarkGray))];
    if let Some(error) = &pane.error {
        spans.push(Span::styled(
            format!("  ·  {error}"),
            Style::default().fg(color_for(Severity::Critical)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), indent(footnote, INDENT));
}

fn gauge(bar: &Bar) -> Gauge<'_> {
    Gauge::default()
        .gauge_style(Style::default().fg(color_for(bar.level)))
        .ratio(bar.fill_clamped())
        .label(bar.label.as_str())
}

fn draw_footer(frame: &mut Frame, area: Rect, screen: &Screen) {
    let now = Local::now();
    let mut parts = Vec::new();
    if let Some(at) = screen.state.watch.latest_sample_at() {
        parts.push(model::origin_text(at, now));
    }
    if screen.state.refreshing {
        parts.push("sampling".to_string());
    } else if let Some(seconds) = screen.state.seconds_until_refresh(now) {
        parts.push(format!("next in {seconds}s"));
    }
    parts.push(format!("range {} [1-4]", screen.range.name()));
    parts.push("[r] sample now  [q] quit".to_string());

    let mut lines = vec![Line::from(Span::styled(
        format!(" {}", parts.join("  ·  ")),
        Style::default().fg(Color::DarkGray),
    ))];
    let errors: Vec<String> = screen
        .state
        .watch
        .panes()
        .iter()
        .filter_map(|pane| {
            pane.error
                .as_ref()
                .map(|error| format!("{}: {error}", pane.path))
        })
        .collect();
    if !errors.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" sampling failed: {}", errors.join(" · ")),
            Style::default().fg(color_for(Severity::Critical)),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn indent(area: Rect, by: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(by).min(area.right()),
        y: area.y,
        width: area.width.saturating_sub(by),
        height: area.height,
    }
}

fn color_for(severity: Severity) -> Color {
    match severity {
        Severity::Normal => Color::Indexed(147),
        Severity::Warning => Color::Indexed(179),
        Severity::Critical => Color::Indexed(203),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::application::testing::application;
    use crate::domain::disk::DiskUsage;

    fn screen_text(width: u16, height: u16, session: &LiveSession) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = session.read();
        let screen = Screen {
            prog: "diskmeter",
            timezone: "Asia/Seoul",
            web: Some("http://127.0.0.1:9998"),
            range: Range::Day,
            state: &state,
        };
        terminal.draw(|frame| draw(frame, &screen)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|row| {
                (0..width)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_screen_shows_gauge_chart_and_keys() {
        let (application, _) = application(DiskUsage::new(1_000, 820, 180));
        let session = LiveSession::new(application, vec!["/".into()]);
        session.refresh_blocking().unwrap();
        let text = screen_text(100, 14, &session);
        assert!(text.contains("web server running"));
        assert!(text.contains("82% used"));
        assert!(text.contains("24h · "));
        assert!(text.contains("[r] sample now"));
    }

    #[test]
    fn a_short_terminal_drops_the_chart_before_the_text() {
        let (application, _) = application(DiskUsage::new(1_000, 820, 180));
        let session = LiveSession::new(application, vec!["/".into(), "/Volumes/Gone".into()]);
        session.refresh_blocking().unwrap();
        let text = screen_text(80, 12, &session);
        assert!(text.contains("/Volumes/Gone"));
        assert!(text.contains("not mounted"));
        assert!(text.contains("sampling failed"));
    }

    #[test]
    fn number_keys_and_tab_select_ranges() {
        assert_eq!(range_for_key(KeyCode::Char('3')), Some(Range::Month));
        assert_eq!(range_for_key(KeyCode::Char('x')), None);
        assert_eq!(next_range(Range::Year), Range::Day);
    }
}
