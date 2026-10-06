# Hammerspoon 패널

`hammerspoon/disk-meter.lua` 는 `diskmeter web --port 9998` 의 `/api/dashboard` JSON 을 읽어
Agent Meter 패널과 같은 오버레이에 볼륨별 소진율과 이력 차트를 그립니다. 같은
`overlay-style.lua`(폭 · 헤더 · 색 · 드래그)와 `hyper.lua` 를 쓰므로 두 패널은 나란히 붙고
같은 방식으로 움직입니다.

## 설치

```bash
cp hammerspoon/disk-meter.lua ~/.hammerspoon/
```

`~/.hammerspoon/init.lua` 끝에:

```lua
-- diskmeter 디스크 사용량 패널 (hyper+0 토글, hyper+shift+d 새로고침). 데이터: http://localhost:9998/api/dashboard
require("disk-meter").start()
```

Hammerspoon 설정을 다시 읽으면(`hyper+r`) 패널이 뜹니다. 기본 위치는 Agent Meter 아래 →
Agent Cockpit 아래 → Agent Shortcuts 아래 → 주 화면 우상단 순으로 비어 있는 자리이며, 헤더를
끌어 옮기면 그 위치를 기억합니다.

## 조작

| 동작 | 방법 |
|---|---|
| 표시 / 숨김 | `hyper+d`, `hammerspoon://diskmeter-toggle` |
| 즉시 새로고침 | `hyper+shift+d`, `hammerspoon://diskmeter-refresh` |
| 기간 전환 | 헤더의 `24h ▸` 클릭 (24h → 7d → 30d → 1y), `hammerspoon://diskmeter-range?range=7d` |
| 위치 | 헤더 드래그, `hammerspoon://diskmeter-move?x=100&y=200` (파라미터 없으면 기본 위치) |

고른 기간은 `hs.settings` 에 남아 재시작해도 유지됩니다.

`hyper+0` 은 Agent Shortcuts 의 오버레이 토글이 이미 쓰고 있어 피했습니다. 새 키를 고를 때는
`require("hyper").hyperMode.keys` 의 `idx` 목록을 보면 모달에 등록된 키를 전부 알 수 있습니다 —
`hyper.bindKey` 를 거치지 않고 `hyperMode:bind` 로 직접 거는 모듈도 있기 때문입니다.

## 모든 패널 한 번에 숨기기

`hammerspoon/overlay-all.lua` 를 `~/.hammerspoon/` 에 두고 패널 모듈들을 시작한 **뒤에**
`require("overlay-all").start()` 를 부릅니다.

| 동작 | 방법 |
|---|---|
| 모두 숨김 ↔ 되살림 | `hyper+h`, `hammerspoon://overlays-toggle` |
| 모두 숨김 | `hammerspoon://overlays-hide` |
| 모두 표시 | `hammerspoon://overlays-show` |

숨길 때 보이던 패널만 기억했다가 되살릴 때 그 패널만 다시 띄우므로, 따로 숨겨 둔 패널은 그대로
숨겨져 있습니다. 각 패널은 `isVisible` · `hide`/`hideOverlay` · `show`/`showOverlay` 를 제공해야 하며,
`package.loaded` 에 없는 모듈은 건너뜁니다.

## 겹치지 않게 쌓기

`hammerspoon/overlay-layout.lua` 를 `~/.hammerspoon/` 에 두고 패널 모듈들을 시작한 뒤
`require("overlay-layout").start()` 를 부릅니다. 각 패널은 `start()` 에서 자신을 등록하고, 그릴 때
`slot(name, h)` 로 자리를 묻고, 그린 뒤 `schedule()` 로 높이나 표시 여부가 바뀌었음을 알립니다.

- 보이는 패널을 `order` 순서(Shortcuts → Cockpit → Agent Meter → Disk Meter)로 주 화면 우상단부터
  세로로 쌓습니다. Cockpit 세션이 늘어 높이가 바뀌면 아래 패널이 따라 내려갑니다.
- 한 열이 화면 아래를 넘으면 왼쪽으로 한 열 옮겨 이어 쌓습니다.
- 헤더를 드래그해 놓은 패널은 **고정**됩니다. 고정 패널은 스택에서 빠져 장애물이 되고, 스택은 그 아래로
  비켜 갑니다. `hyper+shift+h` 또는 `hammerspoon://overlays-arrange` 는 모든 고정을 풀고 스택으로 되돌립니다.
- 재배치는 캔버스 좌표만 옮기고 패널의 `redraw` 를 부르지 않습니다. 같은 틱의 여러 `schedule()` 은
  한 번의 `apply()` 로 합쳐집니다. 모니터 구성이 바뀌어도 다시 배치합니다.
- 관리자가 없으면 각 패널은 예전처럼 위 패널의 `frame()` 아래를 스스로 찾습니다. 그래서
  `disk-meter.lua` 만 설치해도 동작합니다.

등록 명세는 파일 머리말에 있습니다. `overlaps()` 는 지금 보이는 패널 중 겹치는 쌍을 돌려주므로
검증에 씁니다.

## 서버가 없을 때

`diskmeter web --port 9998` 에 접속할 수 없으면 명령을 보여 주고, 클릭하면 복사하거나
Terminal 새 창에서 바로 실행합니다. 20초마다 다시 접속을 시도합니다.

## 그리는 것

pane 마다 경로와 `83% used`(심각도 색), 게이지, 차트, `408.3 GB of 494.4 GB · 86.1 GB free`,
기간과 변화량(`24h  +0.4%p`)을 그립니다. 차트는 서버가 계산한 좌표를 그대로 씁니다.

- `y_ticks` → 가로 격자와 왼쪽 눈금
- `markers` → 세로 점선. `midnight` · `week` · `month` 는 진하게, `hour` · `day` 는 연하게. 라벨은 아래에
- `points` → 면과 선. 표본 간격이 버킷 길이의 3배를 넘으면 끊고, 홀로 남은 점은 작은 원으로
- 버킷 안 최소~최대(`percent_min` · `percent_max`, `samples > 1`)가 있으면 연한 띠

`hs.canvas` 에는 SVG path 요소가 없으므로 `line_path` 대신 `points` 를 `segments` 로 그립니다.
색은 웹 UI 의 `:root` 팔레트와 같아 두 화면의 색이 같은 뜻을 갖습니다.

## 설정

`require("disk-meter").start({ ... })` 에 넘겨 바꿀 수 있습니다.

| 키 | 기본 | 의미 |
|---|---|---|
| `url` | `http://localhost:9998/api/dashboard` | 데이터 주소 (`?range=` 는 패널이 붙입니다) |
| `startCommand` | `diskmeter web --port 9998` | 오프라인일 때 보여 주고 실행할 명령 |
| `range` | `24h` | 시작 기간 |
| `pollSec` | 60 | `next_refresh_at` 을 읽지 못했을 때의 주기 |
| `retrySec` | 20 | 접속 실패 시 재시도 주기 |
| `showOnStart` | `true` | 시작할 때 보일지 |
| `offsetY` | 980 | 다른 패널이 없을 때 주 화면 우상단에서 내려올 거리 |
| `chartHeight` | 56 | 차트 높이 (px) |

## 디버깅

`M.snapshot(path)` 는 화면 전체가 아니라 패널 캔버스만 PNG 로 저장합니다.

```lua
require("disk-meter").snapshot(os.getenv("HOME") .. "/Desktop/disk-meter.png")
```
