//! 설치된 바이너리가 어떤 코드인지 버전으로 식별되게 한다.
//!
//! 릴리즈 태그를 그대로 체크아웃한 깨끗한 트리는 CalVer 버전만 쓰고,
//! 그 밖의 빌드는 개발 버전이므로 커밋 해시를 붙인다.

use std::process::Command;

fn main() {
    // git 상태나 소스가 바뀌면 버전 문자열도 다시 만들어야 한다.
    for path in [".git/HEAD", ".git/refs", "src", "Cargo.toml"] {
        println!("cargo:rerun-if-changed={path}");
    }

    let package = std::env::var("CARGO_PKG_VERSION").expect("cargo가 패키지 버전을 준다");
    let version = match development_suffix(&package) {
        Some(suffix) => format!("{package}-{suffix}"),
        None => package,
    };
    println!("cargo:rustc-env=DISKMETER_VERSION={version}");
}

/// 개발 빌드면 버전에 붙일 접미사를, 릴리즈 빌드면 `None`을 돌려준다.
///
/// git이 없거나 저장소가 아니면(배포 tarball) 접미사 없이 릴리즈로 본다.
fn development_suffix(package: &str) -> Option<String> {
    let hash = git(&["rev-parse", "--short", "HEAD"])?;
    let dirty = !git(&["status", "--porcelain"])?.is_empty();
    let tagged = git(&["describe", "--exact-match", "--tags", "HEAD"]);

    if !dirty && tagged.as_deref() == Some(package) {
        return None;
    }
    Some(if dirty { format!("{hash}-dirty") } else { hash })
}

fn git(arguments: &[&str]) -> Option<String> {
    let result = Command::new("git").args(arguments).output().ok()?;
    if !result.status.success() {
        return None;
    }
    let text = String::from_utf8(result.stdout).ok()?.trim().to_string();
    Some(text)
}
