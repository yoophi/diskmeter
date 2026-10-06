//! `statfs(2)` 로 마운트된 파일시스템의 용량을 읽는 아웃바운드 어댑터.
//!
//! `statvfs` 가 아니라 `statfs` 를 쓰는 이유: macOS 의 `statvfs` 는 블록 수가 32비트라
//! 16 TiB 를 넘는 볼륨에서 넘친다. `statfs` 는 macOS·Linux 모두 64비트 블록 수를 준다.
//! `df` 가 보여 주는 값과 같은 계산이다.

use std::ffi::CString;

use crate::application::{ProbeError, UsageProbe};
use crate::domain::disk::DiskUsage;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StatfsProbe;

impl UsageProbe for StatfsProbe {
    fn measure(&self, path: &str) -> Result<DiskUsage, ProbeError> {
        let c_path = CString::new(path)
            .map_err(|_| ProbeError::Io(format!("{path}: the path contains a NUL byte")))?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: `c_path` 는 NUL 로 끝나는 유효한 C 문자열이고 `stat` 은 `statfs` 크기의
        // 쓰기 가능한 버퍼다. 결과는 호출이 0 을 돌려준 뒤에만 읽는다.
        let code = unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if code != 0 {
            let error = std::io::Error::last_os_error();
            return Err(match error.kind() {
                std::io::ErrorKind::NotFound => ProbeError::Missing(path.to_string()),
                _ => ProbeError::Io(format!("{path}: {error}")),
            });
        }
        // SAFETY: `statfs` 가 성공했으므로 커널이 구조체 전체를 채웠다.
        let stat = unsafe { stat.assume_init() };

        let block = widen(stat.f_bsize);
        let total = widen(stat.f_blocks).saturating_mul(block);
        let free = widen(stat.f_bfree).saturating_mul(block);
        let available = widen(stat.f_bavail).saturating_mul(block);
        Ok(DiskUsage::new(total, total.saturating_sub(free), available))
    }
}

/// 플랫폼마다 필드 폭이 다르므로(macOS `u32`/`u64`, Linux `c_long`) 캐스트 대신 변환한다.
fn widen<T: TryInto<u64>>(value: T) -> u64 {
    value.try_into().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_volume_reports_a_plausible_size() {
        let usage = StatfsProbe.measure("/").unwrap();
        assert!(usage.total > 0);
        assert!(usage.used <= usage.total);
        assert!(usage.available <= usage.total);
    }

    #[test]
    fn a_missing_path_is_reported_as_missing() {
        let error = StatfsProbe
            .measure("/definitely/not/mounted/diskmeter")
            .unwrap_err();
        assert!(matches!(error, ProbeError::Missing(_)), "{error}");
    }
}
