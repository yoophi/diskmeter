//! 측정이 실패했을 때 인바운드 어댑터에 전달하는 오류.

#[derive(Debug)]
pub enum ProbeError {
    /// 경로가 없거나 마운트되어 있지 않다. 외장 디스크를 뽑으면 흔히 생기므로
    /// 상주 모드에서는 종료시키지 않고 화면에 띄운다.
    Missing(String),
    Io(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Missing(path) => write!(f, "{path} is not mounted or does not exist"),
            ProbeError::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ProbeError {}
