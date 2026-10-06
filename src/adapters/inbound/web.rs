//! 로컬 웹 대시보드 인바운드 어댑터.
//!
//! 표본 상태는 `LiveSession`이 소유한다. 이 어댑터는 그 상태를 HTTP로 노출하고
//! 표본 요청을 세션에 전달하기만 한다.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Local;
use serde::Deserialize;
use tokio::sync::Notify;

use crate::adapters::presentation::{history, web as presentation};
use crate::application::LiveSession;
use crate::domain::disk::Range;

/// 런타임 종료를 기다리는 상한. 표본이 진행 중이어도 프롬프트를 오래 붙잡지 않는다.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(200);

pub(crate) struct Options {
    pub session: Arc<LiveSession>,
    pub timezone: String,
    pub port: Option<u16>,
    pub host: IpAddr,
    /// TUI와 함께 뜨면 터미널이 화면에 쓰이므로 진단 출력을 억제한다.
    pub quiet: bool,
}

#[derive(Clone)]
struct WebState {
    session: Arc<LiveSession>,
    timezone: Arc<String>,
    quiet: bool,
}

impl WebState {
    fn report(&self, error: &dyn std::fmt::Display) {
        if !self.quiet {
            eprintln!("diskmeter web: {error}");
        }
    }
}

/// 백그라운드에서 도는 대시보드 서버. 주소는 바인딩 직후 확정된다.
pub(crate) struct Server {
    address: SocketAddr,
    shutdown: Arc<Notify>,
    thread: JoinHandle<Result<()>>,
}

impl Server {
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    /// 종료를 요청하고 서버 스레드가 정리될 때까지 기다린다.
    pub(crate) fn shutdown(self) -> Result<()> {
        self.shutdown.notify_one();
        self.join()
    }

    /// Ctrl-C 처럼 서버가 스스로 끝낼 때까지 기다린다.
    pub(crate) fn wait(self) -> Result<()> {
        self.join()
    }

    fn join(self) -> Result<()> {
        match self.thread.join() {
            Ok(result) => result,
            Err(_) => bail!("the web server thread exited abnormally"),
        }
    }
}

/// 서버를 백그라운드 스레드에서 띄우고 확정된 주소를 돌려준다.
///
/// 바인딩 실패는 이 함수의 오류로 나온다 — 화면을 띄우기 전에 알아야 한다.
pub(crate) fn spawn(options: Options) -> Result<Server> {
    let shutdown = Arc::new(Notify::new());
    let signal = Arc::clone(&shutdown);
    let host = options.host;
    let port = options.port.unwrap_or(0);
    let state = WebState {
        session: options.session,
        timezone: Arc::new(options.timezone),
        quiet: options.quiet,
    };
    let (ready_tx, ready_rx) = mpsc::channel::<Result<SocketAddr>>();

    let thread = thread::Builder::new()
        .name("diskmeter-web".into())
        .spawn(move || serve_blocking(state, host, port, signal, ready_tx))
        .context("could not spawn the web server thread")?;

    match ready_rx.recv() {
        Ok(Ok(address)) => Ok(Server {
            address,
            shutdown,
            thread,
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            bail!("the web server never reported its address")
        }
    }
}

fn serve_blocking(
    state: WebState,
    host: IpAddr,
    port: u16,
    shutdown: Arc<Notify>,
    ready: mpsc::Sender<Result<SocketAddr>>,
) -> Result<()> {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return report_startup(
                &ready,
                Err(anyhow::Error::new(error).context("could not build the web server runtime")),
            );
        }
    };

    let bound = runtime.block_on(bind(host, port));
    let (listener, address) = match bound {
        Ok(pair) => pair,
        Err(error) => return report_startup(&ready, Err(error)),
    };
    if ready.send(Ok(address)).is_err() {
        // 호출자가 사라졌으면 서버를 띄울 이유가 없다.
        return Ok(());
    }

    let served = runtime.block_on(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(shutdown_signal(shutdown))
            .await
            .context("the web server failed")
    });
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    served
}

/// 시작 실패는 호출자에게만 전하고 스레드 자체는 조용히 끝낸다.
fn report_startup(
    ready: &mpsc::Sender<Result<SocketAddr>>,
    result: Result<SocketAddr>,
) -> Result<()> {
    let _ = ready.send(result);
    Ok(())
}

async fn bind(host: IpAddr, port: u16) -> Result<(tokio::net::TcpListener, SocketAddr)> {
    let listener = tokio::net::TcpListener::bind((host, port))
        .await
        .with_context(|| {
            if port == 0 {
                format!("could not open an ephemeral port on {host}")
            } else {
                format!("could not open the port {host}:{port}")
            }
        })?;
    let address = listener
        .local_addr()
        .context("could not read the assigned port")?;
    Ok((listener, address))
}

fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/dashboard", get(dashboard))
        .route("/api/history", get(history_api))
        .route("/api/refresh", post(refresh))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Html(presentation::INDEX),
    )
}

#[derive(Debug, Default, Deserialize)]
struct DashboardQuery {
    range: Option<String>,
}

/// 잘못된 기간은 400 으로 돌려준다. 응답 본문은 호출자가 만든다.
fn parse_range(text: Option<&str>) -> Result<Range, (StatusCode, &'static str)> {
    match text {
        None => Ok(Range::DEFAULT),
        Some(text) => Range::parse(text).ok_or((
            StatusCode::BAD_REQUEST,
            "unknown range, use one of 24h, 7d, 30d, 1y",
        )),
    }
}

async fn dashboard(State(state): State<WebState>, Query(query): Query<DashboardQuery>) -> Response {
    let range = match parse_range(query.range.as_deref()) {
        Ok(range) => range,
        Err(rejection) => return rejection.into_response(),
    };
    let payload = {
        let current = state.session.read();
        presentation::project(
            &current.watch,
            &state.timezone,
            Local::now(),
            current.next_refresh_at,
            current.refreshing,
            range,
        )
    };
    Json(payload).into_response()
}

#[derive(Debug, Default, Deserialize)]
struct HistoryQuery {
    path: Option<String>,
    range: Option<String>,
}

/// 세션이 들고 있는 이력을 그대로 내보낸다. 표본 주기마다 새로 읽으므로 디스크를 다시 열지 않는다.
async fn history_api(State(state): State<WebState>, Query(query): Query<HistoryQuery>) -> Response {
    let range = match parse_range(query.range.as_deref()) {
        Ok(range) => range,
        Err(rejection) => return rejection.into_response(),
    };
    let payload = {
        let current = state.session.read();
        let pane = match &query.path {
            Some(path) => current.watch.pane(path),
            None => current.watch.panes().first(),
        };
        let Some(pane) = pane else {
            return (
                StatusCode::NOT_FOUND,
                "unknown path, pass one of the watched mount points",
            )
                .into_response();
        };
        history::series(&pane.path, range, pane.series(range))
    };
    Json(payload).into_response()
}

async fn refresh(State(state): State<WebState>) -> impl IntoResponse {
    let session = Arc::clone(&state.session);
    // 기록은 blocking I/O 라 런타임 워커를 막지 않도록 따로 돌린다.
    match tokio::task::spawn_blocking(move || session.refresh_blocking()).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT,
        Ok(Err(error)) => {
            state.report(&format!("{error:#}"));
            StatusCode::INTERNAL_SERVER_ERROR
        }
        Err(error) => {
            state.report(&format!("the sampling task was interrupted: {error}"));
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn shutdown_signal(shutdown: Arc<Notify>) {
    tokio::select! {
        _ = shutdown.notified() => {}
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                eprintln!("diskmeter web: could not wait for the shutdown signal: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::application::testing::application;
    use crate::domain::disk::DiskUsage;

    fn session() -> Arc<LiveSession> {
        let (application, _) = application(DiskUsage::new(1_000, 820, 180));
        Arc::new(LiveSession::new(application, vec!["/".into()]))
    }

    fn options(session: Arc<LiveSession>) -> Options {
        Options {
            session,
            timezone: "Asia/Seoul".into(),
            port: None,
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            quiet: true,
        }
    }

    #[test]
    fn spawn_reports_the_bound_address_before_the_screen_starts() {
        let server = spawn(options(session())).unwrap();
        let address = server.address();
        assert_eq!(address.ip().to_string(), "127.0.0.1");
        assert_ne!(address.port(), 0);
        server.shutdown().unwrap();
    }

    #[test]
    fn a_taken_port_fails_before_the_screen_starts() {
        let held = spawn(options(session())).unwrap();
        let mut taken = options(session());
        taken.port = Some(held.address().port());
        let error = match spawn(taken) {
            Ok(_) => panic!("이미 쓰는 포트에는 바인딩되지 않아야 함"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("could not open the port"),
            "{error:#}"
        );
        held.shutdown().unwrap();
    }

    #[test]
    fn the_screen_and_the_server_read_one_session() {
        let session = session();
        let server = spawn(options(Arc::clone(&session))).unwrap();
        session.refresh_blocking().unwrap();
        assert!(session.read().watch.panes()[0].sample.is_some());
        server.shutdown().unwrap();
    }

    #[test]
    fn unknown_ranges_are_rejected_and_the_default_is_a_day() {
        assert_eq!(parse_range(None).unwrap(), Range::Day);
        assert_eq!(parse_range(Some("30d")).unwrap(), Range::Month);
        assert!(parse_range(Some("2h")).is_err());
    }

    #[test]
    fn dashboard_asset_is_self_contained() {
        assert!(presentation::INDEX.contains("<svg"));
        assert!(presentation::INDEX.contains("/api/dashboard?range="));
        assert!(presentation::INDEX.contains("/api/refresh"));
        assert!(presentation::INDEX.contains("viewBox=\"0 0 1000 100\""));
        assert!(
            presentation::INDEX.contains("class=\"tooltip\""),
            "hover tooltip 이 있어야 함"
        );
        assert!(
            presentation::INDEX.contains("<details class=\"table\">"),
            "표 보기가 있어야 함"
        );
        assert!(presentation::INDEX.contains("stroke-linecap: round"));
        assert!(presentation::INDEX.contains("setInterval(load, POLL_MS)"));
        assert!(
            !presentation::INDEX.contains("https://"),
            "CDN 을 쓰지 않는다"
        );
    }
}
