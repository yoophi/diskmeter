# CLAUDE.md

프로젝트 규칙은 [AGENTS.md](AGENTS.md)가 정본입니다. 이 파일은 놓치기 쉬운 것만 반복합니다.

## 버전

**개발 버전을 설치할 때는 version 을 `YYYY.M.#-{short commit hash}` 형태로 표시합니다.**

- `build.rs` 가 빌드 시점에 git 에서 만들어 넣습니다. `Cargo.toml` 의 `version` 에
  해시를 직접 적지 않습니다.
- 커밋하지 않은 변경이 있으면 `-dirty` 가 붙습니다 — `2026.10.1-2879224-dirty`.
- 릴리즈 태그를 그대로 체크아웃한 깨끗한 트리에서만 CalVer 버전만 나옵니다 — `2026.10.1`.

## 이력 보관 규칙

- 5분 버킷 48시간, 1시간 버킷 14일, 하루 버킷 400일. 화면의 24h / 7d / 30d / 1y 보기가
  각각 한 해상도를 씁니다. 근거는 [docs/history-retention.md](docs/history-retention.md).
- 재집계는 **소스 보존 기한 이후 첫 경계부터만** 합니다. 기한이 걸친 버킷은 소스가 잘렸을 수
  있으므로 건드리지 않습니다. 이 규칙을 바꾸면 `adapters/outbound/history.rs` 의
  `pruning_drops_only_the_fine_rows_and_freezes_their_rollups` 테스트가 알려 줍니다.

## 화면 문구

- 사용자에게 보이는 문구는 영문, 주석과 문서는 한국어입니다 (agentmeter 와 같은 관례).
- 웹·TUI·plain·JSON 이 같은 `presentation::model` 투영을 씁니다. 한 곳에만 문구를 추가하지 않습니다.
