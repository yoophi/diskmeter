# 웹 대시보드

## 실행

```bash
diskmeter web
diskmeter web --port 9998
diskmeter web --host 0.0.0.0 --port 9998
diskmeter web -n 60 -r 7d
```

기본값은 `127.0.0.1:0` 이며 운영체제가 고른 ephemeral port 를 씁니다. Hammerspoon 패널과
launchd 등록은 고정 포트 `9998` 을 전제로 합니다. `0.0.0.0` 은 모든 IPv4 인터페이스에서
요청을 받습니다. 인증과 TLS 가 없으므로 신뢰할 수 없는 네트워크에 직접 노출하지 마세요.

## 두 화면, 한 세션

터미널이 화면으로 쓸 수 있으면 `web` 은 서버를 배경으로 돌리고 `-w` 와 같은 TUI 를 띄웁니다.
접속 주소는 헤더 둘째 줄에 남습니다.

```text
 diskmeter  Asia/Seoul
 web server running  http://127.0.0.1:9998
```

`q` 로 나가면 서버도 graceful shutdown 후 함께 내려갑니다. 출력이 터미널이 아니면(파이프,
`nohup`, launchd) TUI 없이 주소만 출력하고 서버만 남습니다.

```text
diskmeter web: http://127.0.0.1:9998
Press Ctrl-C to stop.
```

## 엔드포인트

### `GET /`

대시보드. HTML · CSS · JavaScript 는 실행 파일에 포함되어 있고 외부 CDN 을 쓰지 않습니다.
`?range=7d` 로 열면 그 기간으로 시작하고, 고른 기간은 `localStorage` 에 남습니다.

### `GET /api/dashboard?range=24h`

화면이 그리는 전부. `range` 는 `24h` · `7d` · `30d` · `1y`(별칭 `1d` · `1w` · `1m` · `365d`),
모르는 값은 `400`.

```json
{
  "timezone": "Asia/Seoul",
  "generated_at": "2026-10-06T13:34:09+09:00",
  "next_refresh_at": "2026-10-06T13:35:00+09:00",
  "refreshing": false,
  "range": "24h",
  "ranges": ["24h", "7d", "30d", "1y"],
  "panes": [
    {
      "path": "/", "display": "/",
      "origin": "sampled 13:34 (just now)", "sampled_at": "2026-10-06T13:34:09+09:00",
      "error": null,
      "used_percent": 82.6, "label": "83% used", "level": "warning",
      "used": 408332611584, "total": 494384795648, "available": 86052184064,
      "detail": "408.3 GB of 494.4 GB · 86.1 GB free",
      "delta": "+0.4%p",
      "chart": {
        "from": 1791174849, "to": 1791261249, "resolution": "5m",
        "y_min": 79.0, "y_max": 85.0,
        "y_ticks": [{ "y": 83.3, "label": "80%" }, { "y": 50.0, "label": "82%" }, { "y": 16.7, "label": "84%" }],
        "markers": [{ "x": 104.2, "kind": "hour", "label": "18:00" }, { "x": 312.5, "kind": "midnight", "label": "Oct 6" }],
        "points": [{ "x": 0.0, "y": 61.2, "at": 1791174900, "percent": 81.3, "percent_min": 81.3, "percent_max": 81.3,
                     "used": 402000000000, "total": 494384795648, "available": 92384795648, "samples": 1 }],
        "line_path": "M0.0 61.2 L3.5 61.0 …", "area_path": "M0.0 100 L0.0 61.2 … Z", "band_path": ""
      }
    }
  ]
}
```

- `x` 는 기간 시작(`from`)~지금(`to`)을 0~1000 으로, `y` 는 `y_max` 가 0 · `y_min` 이 100 인
  viewBox 좌표입니다. 세로축은 기간 안의 실제 변화 폭에 맞추되 최소 6%p 를 유지하고 0~100 안에
  둡니다.
- `markers.kind` 는 `hour` · `midnight`(24h), `midnight`(7d), `day` · `week`(30d), `month`(1y).
  `label` 이 있는 것만 축 아래에 글자가 붙습니다.
- `band_path` 는 버킷 안 최소~최대 범위. 모든 버킷이 표본 하나(24h 보기)면 비어 있습니다.
- 기록이 끊긴 자리에서는 `line_path` 가 새 `M` 으로 시작하고 `area_path` 가 따로 닫힙니다.
  홀로 남은 표본은 길이 0 subpath 라 `stroke-linecap: round` 가 점으로 그립니다.
- 표본이 없는 경로는 `used_percent` 등이 `null` 이고 `label` 이 `waiting for the first sample` 입니다.

### `GET /api/history?path=/&range=7d`

선택한 경로 · 기간의 버킷 행. `path` 를 생략하면 첫 번째 감시 경로, 모르는 경로는 `404`.
`diskmeter history -f json` 과 같은 형식입니다.

```json
{ "path": "/", "range": "7d", "resolution": "1h",
  "rows": [{ "start": 1791259200, "at": "2026-10-06T13:00:00+09:00", "used_percent": 82.4,
             "used_avg": 407199009517, "used_min": 406625379469, "used_max": 408327016448,
             "used_last": 408327016448, "total": 494384795648, "available": 86057779200, "samples": 8 }] }
```

### `POST /api/refresh`

지금 표본을 떠서 기록하고 `204` 를 돌려줍니다. 측정 · 기록은 blocking 이라 `spawn_blocking`
에서 실행합니다.

## 갱신 흐름

1. 전용 스레드가 `LiveSession::run_refresh_loop` 를 돌립니다 — 즉시 한 번 측정한 뒤 주기의
   배수 시각마다 반복합니다 (기본 300초, 최소 10초).
2. 측정 결과는 `WatchState` 에, 네 기간의 이력은 저장소에서 다시 읽어 함께 둡니다.
3. 브라우저는 `GET /api/dashboard` 를 5초마다 읽고, TUI 는 매초 같은 상태를 다시 그립니다.
4. 화면의 `Sample now` 와 TUI 의 `r` 은 다음 주기를 기다리지 않고 지금 표본을 뜹니다.

## 화면

- 볼륨마다 경로 · 표본 시각 · 큰 소진율 숫자 · 심각도 글자 · 변화량 · 게이지 · 용량 문구
- area chart: 왼쪽에 세로축 눈금, 아래에 시간 라벨, 성긴 해상도에서는 min~max 띠
- hover: 가장 가까운 점에 세로선과 점을 놓고 시각 · 소진율 · 용량 · (표본이 여럿이면) 최소~최대를 보여 줍니다
- `Data table`: 같은 점들을 표로. 색을 못 보거나 수치가 필요할 때
- 측정 오류는 pane 아래에 붉게, 직전 표본은 그대로 유지
- 좁은 화면(760px 미만)에서는 pane 을 한 열로 쌓습니다
