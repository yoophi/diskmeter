//! diskmeter 실행 진입점. 내부 헥사곤은 바이너리 API로 노출하지 않는다.

mod adapters;
mod application;
mod bootstrap;
mod domain;

/// 화면에 보이는 버전. 개발 빌드는 `build.rs`가 커밋 해시를 붙인다.
pub(crate) const VERSION: &str = env!("DISKMETER_VERSION");

/// 통합 CLI를 실행한다.
pub fn run_diskmeter() -> std::process::ExitCode {
    adapters::inbound::runner::main_diskmeter()
}

fn local_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "local".to_string())
}
