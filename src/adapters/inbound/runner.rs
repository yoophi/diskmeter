//! CLI 명령을 애플리케이션 사용 사례에 연결한다.

use std::io::{IsTerminal, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::{DateTime, Local};
use clap::{Args, Parser, Subcommand, ValueEnum};

use super::cli::{self, Cli, parse_range};
use super::web;
use crate::adapters::presentation::{self, VolumeView, history, plain};
use crate::application::{LiveSession, UsageApplication, VolumeResult};
use crate::bootstrap::{self, Runtime};
use crate::domain::disk::{Bucket, Range};
use crate::local_timezone;

const PROG: &str = "diskmeter";
const ABOUT: &str = "Watch disk usage and keep a 24h / 7d / 30d history you can chart";

#[derive(Debug, Parser)]
#[command(name = PROG, about = ABOUT, version = crate::VERSION, long_about = None)]
struct Root {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    view: Cli,

    /// Mount point to watch for this run, ignoring the configuration (repeatable)
    #[arg(long, global = true, value_name = "PATH")]
    path: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Configure which mount points to watch
    Config(ConfigArgs),
    /// Run the local dashboard server, with the terminal screen when attached
    Web(WebArgs),
    /// Take one sample, store it and exit — for launchd or cron
    Record,
    /// Print the stored history as a table, CSV or JSON
    History(HistoryArgs),
}

#[derive(Debug, Args)]
struct WebArgs {
    /// Sampling interval in seconds
    #[arg(short = 'n', long, value_name = "SECS", default_value_t = cli::DEFAULT_INTERVAL)]
    interval: u64,

    /// Bind a fixed port (an ephemeral port when omitted)
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,

    /// IP address the server binds to
    #[arg(long, value_name = "HOST", default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    host: IpAddr,

    /// History range the terminal screen starts with: 24h, 7d, 30d or 1y
    #[arg(short = 'r', long, value_name = "RANGE", default_value = "24h")]
    range: String,
}

impl WebArgs {
    fn interval_secs(&self) -> u64 {
        self.interval.max(cli::MIN_INTERVAL)
    }
}

#[derive(Debug, Args)]
struct HistoryArgs {
    /// Range to print: 24h, 7d, 30d or 1y
    #[arg(short = 'r', long, value_name = "RANGE", default_value = "24h")]
    range: String,

    /// Output format
    #[arg(short = 'f', long, value_enum, default_value_t = Format::Table)]
    format: Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Table,
    Csv,
    Json,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    action: ConfigAction,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Show the whole configuration
    List,
    /// Show one value
    Get { key: String },
    /// Store a value, as in `paths=/,/Volumes/Backup`
    Set { assignment: String },
}

pub(crate) fn main_diskmeter() -> ExitCode {
    let root = Root::parse();
    let runtime = match bootstrap::production() {
        Ok(runtime) => runtime,
        Err(error) => return report_error(error),
    };
    let outcome = match root.command {
        Some(Command::Config(arguments)) => {
            config_command(arguments.action, &runtime).map(|()| ExitCode::SUCCESS)
        }
        Some(Command::Web(arguments)) => selected_paths(&root.path, &runtime)
            .and_then(|paths| web_command(arguments, runtime, paths))
            .map(|()| ExitCode::SUCCESS),
        Some(Command::Record) => selected_paths(&root.path, &runtime)
            .and_then(|paths| record_command(&runtime.usage, &paths)),
        Some(Command::History(arguments)) => selected_paths(&root.path, &runtime)
            .and_then(|paths| history_command(arguments, &runtime.usage, &paths))
            .map(|()| ExitCode::SUCCESS),
        None => selected_paths(&root.path, &runtime)
            .and_then(|paths| run(&root.view, runtime.usage, paths)),
    };
    match outcome {
        Ok(code) => code,
        Err(error) => report_error(error),
    }
}

/// `--path` 가 있으면 설정 파일을 읽지 않고 이번 실행에만 그 경로를 쓴다.
fn selected_paths(explicit: &[String], runtime: &Runtime) -> Result<Vec<String>> {
    if explicit.is_empty() {
        return runtime.settings.load().map(|settings| settings.paths);
    }
    for path in explicit {
        if !path.starts_with('/') {
            bail!("path must be absolute: {path}");
        }
    }
    Ok(explicit.to_vec())
}

fn report_error(error: anyhow::Error) -> ExitCode {
    eprintln!("{PROG}: {error:#}");
    ExitCode::FAILURE
}

/// 웹 서버는 배경에서 돌리고 터미널에는 상주 화면을 띄운다.
///
/// 두 화면이 같은 `LiveSession`을 보므로 측정과 기록은 한 번만 일어난다.
fn web_command(arguments: WebArgs, runtime: Runtime, paths: Vec<String>) -> Result<()> {
    let range = parse_range(&arguments.range)?;
    if arguments.interval < cli::MIN_INTERVAL {
        eprintln!(
            "{PROG}: raised the interval to {}s (minimum {}s)",
            arguments.interval_secs(),
            cli::MIN_INTERVAL
        );
    }
    let timezone = local_timezone();
    let session = Arc::new(LiveSession::new(runtime.usage, paths));
    spawn_refresh_loop(&session, arguments.interval_secs());

    // 터미널이 화면에 쓰이면 서버는 stdout·stderr를 건드릴 수 없다.
    let with_screen = std::io::stdout().is_terminal();
    let server = web::spawn(web::Options {
        session: Arc::clone(&session),
        timezone: timezone.clone(),
        port: arguments.port,
        host: arguments.host,
        quiet: with_screen,
    })?;
    let address = format!("http://{}", server.address());

    if with_screen {
        presentation::tui::run(PROG, timezone, session, Some(address), range)?;
        server.shutdown()
    } else {
        println!("{PROG} web: {address}");
        println!("Press Ctrl-C to stop.");
        server.wait()
    }
}

/// 세션의 주기 표본을 전용 스레드에서 돌린다. 세션마다 한 번만 호출한다.
fn spawn_refresh_loop(session: &Arc<LiveSession>, interval_secs: u64) {
    let session = Arc::clone(session);
    let interval = Duration::from_secs(interval_secs);
    thread::spawn(move || session.run_refresh_loop(interval));
}

fn run(arguments: &Cli, application: UsageApplication, paths: Vec<String>) -> Result<ExitCode> {
    let range = arguments.range()?;
    let stdout_is_tty = std::io::stdout().is_terminal();
    let timezone = local_timezone();

    if arguments.json {
        let now = Local::now();
        let results = application.record(&paths, now)?;
        let histories = histories(&application, &results, range, now)?;
        println!(
            "{}",
            presentation::to_json(&views(&results, &histories), range, &timezone, now)?
        );
        return Ok(exit_for(&results));
    }

    if arguments.is_watch() {
        if stdout_is_tty {
            if arguments.interval_was_clamped() {
                eprintln!(
                    "{PROG}: raised the interval to {}s (minimum {}s)",
                    arguments.interval_secs(),
                    cli::MIN_INTERVAL
                );
            }
            let session = Arc::new(LiveSession::new(application, paths));
            spawn_refresh_loop(&session, arguments.interval_secs());
            presentation::tui::run(PROG, timezone, session, None, range)?;
            return Ok(ExitCode::SUCCESS);
        }
        eprintln!("{PROG}: output is not a terminal, printing once (pair this with watch)");
    }

    once(arguments, stdout_is_tty, &application, &paths, range)
}

/// 일회성 실행도 표본을 남긴다 — 측정은 곧 기록이고, 가끔 손으로 돌려도 이력이 쌓인다.
fn once(
    arguments: &Cli,
    is_tty: bool,
    application: &UsageApplication,
    paths: &[String],
    range: Range,
) -> Result<ExitCode> {
    let color = presentation::use_color(arguments.no_color, is_tty);
    let width = terminal_width();
    let now = Local::now();
    let results = application.record(paths, now)?;
    let histories = histories(application, &results, range, now)?;
    let text = plain::render(&views(&results, &histories), range, color, width, now);
    write!(std::io::stdout().lock(), "{text}")?;
    Ok(exit_for(&results))
}

fn histories(
    application: &UsageApplication,
    results: &[VolumeResult],
    range: Range,
    now: DateTime<Local>,
) -> Result<Vec<Vec<Bucket>>> {
    results
        .iter()
        .map(|volume| application.history(&volume.path, range, now))
        .collect()
}

fn views<'a>(results: &'a [VolumeResult], histories: &'a [Vec<Bucket>]) -> Vec<VolumeView<'a>> {
    results
        .iter()
        .zip(histories)
        .map(|(volume, history)| VolumeView::new(volume, history))
        .collect()
}

fn exit_for(results: &[VolumeResult]) -> ExitCode {
    if results.iter().all(|volume| volume.result.is_ok()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn record_command(application: &UsageApplication, paths: &[String]) -> Result<ExitCode> {
    let results = application.record(paths, Local::now())?;
    let mut out = std::io::stdout().lock();
    for volume in &results {
        match &volume.result {
            Ok(sample) => {
                let meter = presentation::model::project(sample);
                writeln!(
                    out,
                    "{}  {}  {}",
                    volume.path, meter.usage.label, meter.detail
                )?;
            }
            Err(error) => writeln!(out, "{}  {error}", volume.path)?,
        }
    }
    Ok(exit_for(&results))
}

fn history_command(
    arguments: HistoryArgs,
    application: &UsageApplication,
    paths: &[String],
) -> Result<()> {
    let range = parse_range(&arguments.range)?;
    let now = Local::now();
    let all = paths
        .iter()
        .map(|path| {
            application
                .history(path, range, now)
                .map(|buckets| history::series(path, range, &buckets))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut out = std::io::stdout().lock();
    match arguments.format {
        Format::Table => {
            for (index, series) in all.iter().enumerate() {
                if index > 0 {
                    writeln!(out)?;
                }
                write!(out, "{}", history::render_table(series))?;
            }
        }
        Format::Csv => write!(out, "{}", history::render_csv(&all))?,
        Format::Json => writeln!(out, "{}", serde_json::to_string_pretty(&all)?)?,
    }
    Ok(())
}

fn config_command(action: ConfigAction, runtime: &Runtime) -> Result<()> {
    match action {
        ConfigAction::List => {
            let current = runtime.settings.load()?;
            println!("paths = {}", current.paths.join(","));
            println!("# config file: {}", runtime.settings_path.display());
            println!("# history: {}", runtime.history_path.display());
            Ok(())
        }
        ConfigAction::Get { key } => {
            let current = runtime.settings.load()?;
            match key.as_str() {
                "paths" => {
                    println!("{}", current.paths.join(","));
                    Ok(())
                }
                other => bail!("unknown config key: {other}. available keys: paths"),
            }
        }
        ConfigAction::Set { assignment } => {
            let (key, value) = split_assignment(&assignment)?;
            let current = match key {
                "paths" => runtime.settings.replace_paths(split_list(value))?,
                other => bail!("unknown config key: {other}. available keys: paths"),
            };
            println!("paths = {}", current.paths.join(","));
            println!("saved: {}", runtime.settings_path.display());
            Ok(())
        }
    }
}

fn split_assignment(argument: &str) -> Result<(&str, &str)> {
    match argument.split_once('=') {
        Some((key, value)) => Ok((key.trim(), value.trim())),
        None => bail!("expected `key=value`, for example paths=/,/Volumes/Backup"),
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_syntax_belongs_to_the_inbound_adapter() {
        assert_eq!(
            split_assignment(" paths = /,/Volumes/Backup ").unwrap(),
            ("paths", "/,/Volumes/Backup")
        );
        assert_eq!(
            split_list("/, /Volumes/Backup,"),
            vec!["/", "/Volumes/Backup"]
        );
        assert!(split_assignment("paths").is_err());
    }

    #[test]
    fn path_override_combines_with_watch_json_and_subcommands() {
        let watch = Root::try_parse_from(["diskmeter", "--path", "/", "--watch"]).unwrap();
        assert_eq!(watch.path, vec!["/"]);
        assert!(watch.view.watch);

        let json = Root::try_parse_from(["diskmeter", "--json", "--range", "7d"]).unwrap();
        assert!(json.view.json);
        assert_eq!(json.view.range().unwrap(), Range::Week);

        let record =
            Root::try_parse_from(["diskmeter", "record", "--path", "/", "--path", "/tmp"]).unwrap();
        assert_eq!(record.path, vec!["/", "/tmp"]);
        assert!(matches!(record.command, Some(Command::Record)));
    }

    #[test]
    fn web_is_an_explicit_subcommand() {
        let root = Root::try_parse_from([
            "diskmeter",
            "web",
            "--interval",
            "60",
            "--port",
            "9998",
            "--host",
            "0.0.0.0",
            "-r",
            "30d",
        ])
        .unwrap();
        let Some(Command::Web(arguments)) = root.command else {
            panic!("web subcommand를 파싱해야 함");
        };
        assert_eq!(arguments.interval_secs(), 60);
        assert_eq!(arguments.port, Some(9998));
        assert_eq!(arguments.host, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(parse_range(&arguments.range).unwrap(), Range::Month);
    }

    #[test]
    fn web_interval_uses_the_floor_and_the_loopback_default() {
        let root = Root::try_parse_from(["diskmeter", "web", "-n", "1"]).unwrap();
        let Some(Command::Web(arguments)) = root.command else {
            panic!("web subcommand를 파싱해야 함");
        };
        assert_eq!(arguments.interval_secs(), cli::MIN_INTERVAL);
        assert_eq!(arguments.host, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn history_format_defaults_to_table() {
        let root = Root::try_parse_from(["diskmeter", "history", "-r", "1y", "-f", "csv"]).unwrap();
        let Some(Command::History(arguments)) = root.command else {
            panic!("history subcommand를 파싱해야 함");
        };
        assert_eq!(arguments.format, Format::Csv);
        let root = Root::try_parse_from(["diskmeter", "history"]).unwrap();
        let Some(Command::History(arguments)) = root.command else {
            panic!("history subcommand를 파싱해야 함");
        };
        assert_eq!(arguments.format, Format::Table);
        assert!(parse_range("2h").is_err());
    }

    #[test]
    fn version_extends_the_calver_package_version() {
        let package = env!("CARGO_PKG_VERSION");
        assert!(crate::VERSION.starts_with(package), "{}", crate::VERSION);
        match crate::VERSION.strip_prefix(package) {
            Some("") => {}
            Some(suffix) => assert!(
                suffix.starts_with('-') && suffix.len() > 1,
                "개발 버전 접미사는 `-`로 시작해야 함: {suffix}"
            ),
            None => unreachable!("접두사를 이미 확인했다"),
        }
    }

    #[test]
    fn version_flag_reports_the_build_version() {
        let rendered = Root::try_parse_from(["diskmeter", "--version"])
            .expect_err("clap은 --version을 오류로 돌려준다")
            .to_string();
        assert!(rendered.contains(crate::VERSION), "{rendered}");
    }
}
