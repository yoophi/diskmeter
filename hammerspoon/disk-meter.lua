-- disk-meter.lua — diskmeter 디스크 사용량 패널
--
-- diskmeter(~/project/disk-usage-monitor) 가 `diskmeter web --port 9998` 로 띄우는 대시보드의
-- /api/dashboard JSON 을 주기적으로 받아, Agent Meter 와 같은 스타일의 오버레이에 볼륨별 소진율과
-- 24h / 7d / 30d / 1y 이력 차트를 그린다. 서버에 접속할 수 없으면 명령 복사·터미널 실행 버튼을 보여 준다.
--
-- 데이터 형식(diskmeter src/adapters/presentation/web.rs):
--   { timezone, generated_at, next_refresh_at, refreshing, range, ranges[], panes[] }
--   pane  { path, display, origin, sampled_at, error, used_percent, label, level(normal|warning|critical),
--           used, total, available, detail, delta, chart }
--   chart { from, to, resolution(5m|1h|1d), y_min, y_max, y_ticks[{y,label}], markers[{x,kind,label}],
--           points[{x, y, at, percent, percent_min, percent_max, used, total, available, samples}],
--           line_path, area_path, band_path }
--   x 는 기간 시작~지금을 0~1000 으로, y 는 y_max 가 0·y_min 이 100 인 viewBox 좌표다.
--   hs.canvas 에는 path 요소가 없어 points 를 segments(면·선)로 그린다. 표본 간격이 벌어진 자리는 끊는다.
--
-- 단축키: hyper+0 표시/숨김, hyper+shift+d 즉시 새로고침, 헤더의 기간 글자 클릭으로 24h→7d→30d→1y 순환
-- URL:   hammerspoon://diskmeter-toggle · diskmeter-refresh · diskmeter-range?range=7d · diskmeter-move?x=&y=

local S = require("overlay-style")
local log = hs.logger.new("disk-meter", "info")

local M = {}
M.version = "2026-10-06.1"

M.config = {
  url = "http://localhost:9998/api/dashboard",
  startCommand = "diskmeter web --port 9998",
  range = "24h",           -- 24h · 7d · 30d · 1y
  pollSec = 60,            -- next_refresh_at 을 읽지 못했을 때의 기본 주기
  retrySec = 20,           -- 접속 실패 시 재시도 주기
  showOnStart = true,
  offsetY = 980,           -- 기본 위치: Agent Meter 아래 → Cockpit 아래 → Shortcuts 아래 → 주 화면 우상단 offsetY
  chartHeight = 56,
}
local config = M.config
local RANGES = { "24h", "7d", "30d", "1y" }
local RESOLUTION_SEC = { ["5m"] = 300, ["1h"] = 3600, ["1d"] = 86400 }

-- diskmeter 웹 UI 팔레트 (web.html :root) — 두 화면의 색이 같은 뜻을 갖도록 그대로 쓴다
local C = {
  normal   = { hex = "#9290d2" },
  warning  = { hex = "#ae9559" },
  critical = { hex = "#bd6974" },
  accent   = { hex = "#79999a" },
  name     = { hex = "#cbd1de" },
  areaFill = { red = 121 / 255, green = 153 / 255, blue = 154 / 255, alpha = 0.55 },
  bandFill = { red = 121 / 255, green = 153 / 255, blue = 154 / 255, alpha = 0.26 },
  grid     = { white = 1, alpha = 0.10 },
  strong   = { white = 1, alpha = 0.30 },
  faint    = { white = 1, alpha = 0.16 },
  track    = { white = 1, alpha = 0.10 },
  chartBg  = { white = 1, alpha = 0.03 },
  cmdBg    = { white = 1, alpha = 0.08 },
}
local LEVEL = { normal = C.normal, warning = C.warning, critical = C.critical }

-- 레이아웃 (폭은 S.width, 좌우 여백 S.col.key)
local L = {
  x = S.col.key, w = S.width - 2 * S.col.key,
  paneHead = 24, errLine = 16, gauge = 10, axisW = 34, xLabels = 14, detail = 18, paneGap = 10,
}

local POS_KEY = "diskMeter.overlayPosition"
local RANGE_KEY = "diskMeter.range"
local state = { data = nil, offline = nil, canvas = nil, pos = nil, timer = nil, userHidden = false, stopDrag = nil }

-- ---------------------------------------------------------------- 유틸
local function txt(t, x, y, w, h, color, size, opts)
  local e = { type = "text", text = t or "", frame = { x = x, y = y, w = w, h = h },
    textColor = color, textSize = size, textLineBreak = "truncateTail" }
  if opts then for k, v in pairs(opts) do e[k] = v end end
  return e
end

local function hhmm(iso) return iso and iso:match("T(%d%d:%d%d)") or "?" end

local function clamp01(v) return math.max(0, math.min(1, tonumber(v) or 0)) end

-- ISO 8601 을 로컬 시각으로 해석. 서버(같은 Mac)와 오프셋이 같다는 전제이며, 대기 시간 계산에만 쓰고 클램프한다.
local function isoToTime(iso)
  if type(iso) ~= "string" then return nil end
  local Y, m, d, H, Mi, Sec = iso:match("(%d+)%-(%d+)%-(%d+)T(%d+):(%d+):(%d+)")
  if not Y then return nil end
  return os.time({ year = tonumber(Y), month = tonumber(m), day = tonumber(d), hour = tonumber(H), min = tonumber(Mi), sec = tonumber(Sec) })
end

local function fmtBytes(n)
  n = tonumber(n) or 0
  local units, u = { "B", "kB", "MB", "GB", "TB", "PB" }, 1
  while n >= 1000 and u < #units do n = n / 1000; u = u + 1 end
  if u == 1 then return string.format("%d B", n) end
  return string.format("%.1f %s", n, units[u])
end

-- 표본 간격이 버킷 길이의 3배를 넘으면 기록이 끊긴 것이다 (웹 projection 과 같은 규칙).
local function runsOf(points, resSec)
  local runs, cur, lastAt = {}, nil, nil
  for _, p in ipairs(points or {}) do
    if not cur or (lastAt and p.at - lastAt > resSec * 3) then cur = {}; runs[#runs + 1] = cur end
    cur[#cur + 1] = p
    lastAt = p.at
  end
  return runs
end

-- x 간격이 step(1000 단위) 미만인 점을 건너뛴다. 처음·끝 2점은 유지해 면의 바닥선이 보존된다.
local function thin(pts, step)
  local out, lastX = {}, -math.huge
  for i, p in ipairs(pts) do
    if i <= 2 or i >= #pts - 1 or p.x - lastX >= step then
      out[#out + 1] = p
      lastX = p.x
    end
  end
  return out
end

local function chartElements(chart, rect)
  local els = {}
  chart = chart or {}
  local function px(x) return rect.x + (tonumber(x) or 0) / 1000 * rect.w end
  local function py(y) return rect.y + (tonumber(y) or 0) / 100 * rect.h end
  local yMin, yMax = tonumber(chart.y_min) or 0, tonumber(chart.y_max) or 100
  local function pyPct(pct)
    local span = math.max(yMax - yMin, 1e-9)
    return py(100 - clamp01((pct - yMin) / span) * 100)
  end

  for _, t in ipairs(chart.y_ticks or {}) do                               -- 가로 격자 + 왼쪽 눈금
    els[#els + 1] = { type = "segments", action = "stroke", strokeColor = C.grid, strokeWidth = 1, strokeDashPattern = { 2, 6 },
      coordinates = { { x = rect.x, y = py(t.y) }, { x = rect.x + rect.w, y = py(t.y) } } }
    els[#els + 1] = txt(t.label, rect.x - L.axisW, py(t.y) - 7, L.axisW - 4, 14, S.colors.muted, 9, { textAlignment = "right" })
  end
  for _, mk in ipairs(chart.markers or {}) do                              -- 시간 경계
    local strong = mk.kind == "midnight" or mk.kind == "week" or mk.kind == "month"
    els[#els + 1] = { type = "segments", action = "stroke", strokeWidth = 1, strokeDashPattern = { 3, 4 },
      strokeColor = strong and C.strong or C.faint,
      coordinates = { { x = px(mk.x), y = rect.y }, { x = px(mk.x), y = rect.y + rect.h } } }
    if mk.label then
      els[#els + 1] = txt(mk.label, px(mk.x) - 30, rect.y + rect.h + 1, 60, 12, S.colors.muted, 9, { textAlignment = "center" })
    end
  end

  local step = 1000 / (rect.w * 1.5)
  local resSec = RESOLUTION_SEC[chart.resolution] or 300
  for _, run in ipairs(runsOf(chart.points, resSec)) do
    local pts = thin(run, step)
    if #pts == 1 then
      els[#els + 1] = { type = "circle", action = "fill", fillColor = C.accent, radius = 1.6,
        center = { x = px(pts[1].x), y = py(pts[1].y) } }
    else
      local hasBand = false
      for _, p in ipairs(pts) do
        if (p.samples or 0) > 1 and (p.percent_max or 0) > (p.percent_min or 0) then hasBand = true; break end
      end
      if hasBand then                                                       -- 버킷 안 최소~최대 띠
        local coords = {}
        for _, p in ipairs(pts) do coords[#coords + 1] = { x = px(p.x), y = pyPct(p.percent_max) } end
        for i = #pts, 1, -1 do coords[#coords + 1] = { x = px(pts[i].x), y = pyPct(pts[i].percent_min) } end
        els[#els + 1] = { type = "segments", action = "fill", fillColor = C.bandFill, closed = true, coordinates = coords }
      end
      local area = { { x = px(pts[1].x), y = rect.y + rect.h } }
      local line = {}
      for _, p in ipairs(pts) do
        area[#area + 1] = { x = px(p.x), y = py(p.y) }
        line[#line + 1] = { x = px(p.x), y = py(p.y) }
      end
      area[#area + 1] = { x = px(pts[#pts].x), y = rect.y + rect.h }
      els[#els + 1] = { type = "segments", action = "fill", fillColor = C.areaFill, closed = true, coordinates = area }
      els[#els + 1] = { type = "segments", action = "stroke", strokeColor = C.accent, strokeWidth = 1.2, closed = false, coordinates = line }
    end
  end
  return els
end

-- ---------------------------------------------------------------- 본문 구성
local function build(data)
  local els, y = {}, S.headerHeight + 6
  for _, pane in ipairs(data.panes or {}) do
    local lvl = LEVEL[pane.level] or C.normal
    els[#els + 1] = txt(pane.display or pane.path, L.x, y + 3, L.w - 130, 18, S.colors.title, 13, { textFont = S.fonts.title })
    els[#els + 1] = txt(pane.label or "", L.x + L.w - 130, y + 3, 130, 18, lvl, 13, { textFont = S.fonts.key, textAlignment = "right" })
    y = y + L.paneHead
    if pane.error and pane.error ~= "" then
      els[#els + 1] = txt(pane.error, L.x + 8, y, L.w - 8, 14, C.critical, 11)
      y = y + L.errLine
    end
    if type(pane.used_percent) == "number" then
      -- 게이지: 소진율을 레벨 색으로
      local gx, gw = L.x + 8, L.w - 8
      els[#els + 1] = { type = "rectangle", action = "fill", fillColor = C.track, frame = { x = gx, y = y, w = gw, h = 4 } }
      els[#els + 1] = { type = "rectangle", action = "fill", fillColor = lvl, frame = { x = gx, y = y, w = gw * clamp01(pane.used_percent / 100), h = 4 } }
      y = y + L.gauge

      -- 차트: 왼쪽에 세로축 눈금 자리를 비운다
      local rect = { x = L.x + 8 + L.axisW, y = y, w = L.w - 8 - L.axisW, h = config.chartHeight }
      els[#els + 1] = { type = "rectangle", action = "fill", fillColor = C.chartBg, frame = rect }
      if pane.chart and pane.chart.points and #pane.chart.points > 0 then
        for _, e in ipairs(chartElements(pane.chart, rect)) do els[#els + 1] = e end
      else
        els[#els + 1] = txt("no " .. (data.range or config.range) .. " history yet", rect.x, rect.y + rect.h / 2 - 8, rect.w, 16, S.colors.muted, 11, { textAlignment = "center" })
      end
      y = y + rect.h + L.xLabels

      -- 상세: 사용량 왼쪽, 기간·변화량·표본 시각 오른쪽
      els[#els + 1] = txt(pane.detail or "", gx, y, 300, 14, S.colors.muted, 11)
      local right = (data.range or config.range) .. (pane.delta and ("  " .. pane.delta) or "")
      els[#els + 1] = txt(right, gx + 300, y, gw - 300, 14, C.accent, 11, { textAlignment = "right" })
      y = y + L.detail
    end
    y = y + L.paneGap
  end
  if #(data.panes or {}) == 0 then
    els[#els + 1] = txt("감시하는 볼륨이 없습니다", L.x, y, L.w, 18, S.colors.muted, 12)
    y = y + 22
  end
  return els, y + S.bottomPadding
end

local function buildOffline(reason)
  local els, y = {}, S.headerHeight + 8
  local host = (config.url:gsub("^https?://", ""):gsub("/.*$", ""))
  els[#els + 1] = txt("diskmeter 에 연결할 수 없습니다  (" .. host .. ")", L.x, y, L.w, 18, C.critical, 13)
  y = y + 26
  els[#els + 1] = txt("터미널에서 실행:", L.x, y + 1, 104, 16, S.colors.muted, 12)
  els[#els + 1] = { id = "startCmd", type = "rectangle", action = "fill", fillColor = C.cmdBg,
    roundedRectRadii = { xRadius = 6, yRadius = 6 }, frame = { x = L.x + 104, y = y - 4, w = L.w - 104, h = 26 }, trackMouseDown = true }
  els[#els + 1] = txt(config.startCommand, L.x + 114, y, L.w - 124, 18, S.colors.text, 13,
    { id = "startCmdText", textFont = S.fonts.mono, trackMouseDown = true })
  y = y + 30
  els[#els + 1] = txt(string.format("클릭하면 명령을 복사합니다 · %s · %d초마다 재시도", reason or "", config.retrySec),
    L.x, y, L.w, 14, S.colors.muted, 11)
  y = y + 24
  els[#els + 1] = { id = "startService", type = "rectangle", action = "fill", fillColor = C.cmdBg,
    roundedRectRadii = { xRadius = 6, yRadius = 6 }, frame = { x = L.x, y = y, w = L.w, h = 30 }, trackMouseDown = true }
  els[#els + 1] = txt("▶ 터미널에서 시작", L.x, y + 5, L.w, 20, C.accent, 13,
    { id = "startServiceText", textAlignment = "center", trackMouseDown = true })
  return els, y + 30 + S.bottomPadding
end

-- ---------------------------------------------------------------- 오버레이
local function frameOf(moduleName)
  local ok, mod = pcall(require, moduleName)
  local f = ok and type(mod) == "table" and mod.frame and mod.frame()
  if f and S.onAnyScreen(f.x, f.y, S.width, S.headerHeight) then return f end
  return nil
end

local function defaultPosition()
  for _, name in ipairs({ "agent-meter", "agent-cockpit" }) do
    local f = frameOf(name)
    if f then return { x = f.x, y = f.y + f.h + S.margin.gap } end
  end
  local sf = hs.settings.get("agentShortcuts.overlayFrame")
  if type(sf) == "table" and sf.x and sf.y and sf.h and S.onAnyScreen(sf.x, sf.y, S.width, S.headerHeight) then
    return { x = sf.x, y = sf.y + sf.h + S.margin.gap }
  end
  return S.defaultTopRight(config.offsetY)
end

local function onClick(elementId)
  if elementId == "rangeText" then
    M.cycleRange()
  elseif elementId == "startCmd" or elementId == "startCmdText" then
    hs.pasteboard.setContents(config.startCommand)
    hs.alert.show("복사됨: " .. config.startCommand, 1.5)
  elseif state.offline and (elementId == "startService" or elementId == "startServiceText") then
    -- AppleScript 문자열로 인코딩한다. 셸 명령은 Terminal의 새 창에서 그대로 실행한다.
    local command = config.startCommand:gsub("\\", "\\\\"):gsub('"', '\\"')
    local ok, _, err = hs.osascript.applescript('tell application "Terminal"\n'
      .. 'do script "' .. command .. '"\nactivate\nend tell')
    if ok then
      hs.alert.show("터미널에서 diskmeter 시작 명령을 실행했습니다", 2)
    else
      log.e("터미널 실행 실패: " .. hs.inspect(err))
      hs.alert.show("터미널 실행 실패 · Hammerspoon 콘솔을 확인하세요", 3)
    end
  end
end

local function redraw()
  local els, h
  if state.offline then
    els, h = buildOffline(state.offline)
  elseif state.data then
    els, h = build(state.data)
  else
    els, h = { txt("diskmeter 불러오는 중…", L.x, S.headerHeight + 8, L.w, 18, S.colors.muted, 12) }, S.height(1)
  end

  -- 사용자가 드래그해 둔 위치가 있으면 그대로, 없으면 매번 기본 위치를 다시 계산한다.
  -- 위 패널들은 내용에 따라 높이가 바뀌므로 한 번만 계산하면 겹친다.
  local pos
  if state.pos and S.onAnyScreen(state.pos.x, state.pos.y, S.width, S.headerHeight) then
    pos = state.pos
  else
    pos = S.clampToScreenAt(defaultPosition(), S.width, h)
  end
  local frame = { x = pos.x, y = pos.y, w = S.width, h = h }

  if not state.canvas then
    state.canvas = S.newCanvas(frame)
    state.stopDrag = S.attachDrag(state.canvas, function(p)
      state.pos = p
      hs.settings.set(POS_KEY, p)
    end, onClick)
  else
    state.canvas:frame(frame)
  end

  local hint
  if state.offline then
    hint = "hyper+0 숨김 · ⇧D 새로고침 · 오프라인"
  elseif state.data then
    local d = state.data
    hint = string.format("hyper+0 숨김 · ⇧D 새로고침 · %s 갱신%s", hhmm(d.generated_at),
      d.refreshing and " · 표본 중" or (d.next_refresh_at and (" · 다음 " .. hhmm(d.next_refresh_at)) or ""))
  else
    hint = "hyper+0 숨김 · ⇧D 새로고침 · 불러오는 중"
  end
  local all = S.baseElements("Disk Meter", hint)
  -- 제목 옆 기간 글자. 클릭하면 24h → 7d → 30d → 1y 로 바뀐다.
  all[#all + 1] = txt(config.range .. " ▸", 110, 11, 60, 18, C.accent, 12, { id = "rangeText", trackMouseDown = true })
  for _, e in ipairs(els) do all[#all + 1] = e end
  all[#all + 1] = S.dragHandleElement()
  state.canvas:replaceElements(table.unpack(all))
  if not state.userHidden then state.canvas:show() end
end

-- ---------------------------------------------------------------- 데이터
local function schedule(sec)
  if state.timer then state.timer:stop() end
  state.timer = hs.timer.doAfter(sec, function() M.refresh() end)
end

function M.refresh()
  local url = config.url .. "?range=" .. config.range
  hs.http.asyncGet(url, nil, function(status, body)
    if status ~= 200 or type(body) ~= "string" then
      state.offline = "HTTP " .. tostring(status)
      state.data = nil
      redraw()
      schedule(config.retrySec)
      return
    end
    local ok, data = pcall(hs.json.decode, body)
    if not ok or type(data) ~= "table" or type(data.panes) ~= "table" then
      state.offline = "응답 해석 실패"
      redraw()
      schedule(config.retrySec)
      return
    end
    state.data, state.offline = data, nil
    redraw()
    local wait = config.pollSec
    local n = isoToTime(data.next_refresh_at)
    if n then wait = math.max(5, math.min(600, n - os.time() + 2)) end
    if data.refreshing then wait = 4 end
    schedule(wait)
  end)
  return M
end

-- ---------------------------------------------------------------- 공개 API
function M.setRange(range)
  if not hs.fnutils.contains(RANGES, range) then
    hs.alert.show("diskmeter: 알 수 없는 기간 " .. tostring(range), 1.5)
    return M
  end
  config.range = range
  hs.settings.set(RANGE_KEY, range)
  M.refresh()
  return M
end

function M.cycleRange()
  local i = hs.fnutils.indexOf(RANGES, config.range) or 1
  return M.setRange(RANGES[i % #RANGES + 1])
end

function M.toggle()
  if not state.canvas then redraw() end
  if state.canvas:isShowing() then
    state.userHidden = true
    state.canvas:hide(0.12)
  else
    state.userHidden = false
    state.canvas:show(0.12)
  end
  return M
end

function M.frame() return state.canvas and state.canvas:frame() or nil end

function M.move(x, y)
  if x and y then
    state.pos = { x = tonumber(x), y = tonumber(y) }
    hs.settings.set(POS_KEY, state.pos)
  else
    state.pos = nil
    hs.settings.clear(POS_KEY)
  end
  redraw()
  return M
end

-- 디버깅용: 캔버스를 PNG 로 저장한다. 화면 전체를 캡처하지 않고 패널만 그린다.
function M.snapshot(path)
  if not state.canvas then return false end
  local image = state.canvas:imageFromCanvas()
  if not image then return false end
  return image:saveToFile(path) and true or false
end

function M.stop()
  if state.timer then state.timer:stop(); state.timer = nil end
  if state.stopDrag then state.stopDrag(); state.stopDrag = nil end
  if state.canvas then state.canvas:delete(); state.canvas = nil end
  return M
end

function M.start(overrides)
  M.stop()
  for k, v in pairs(overrides or {}) do config[k] = v end
  local saved = hs.settings.get(POS_KEY)
  if type(saved) == "table" and type(saved.x) == "number" and type(saved.y) == "number" then state.pos = saved end
  local savedRange = hs.settings.get(RANGE_KEY)
  if hs.fnutils.contains(RANGES, savedRange) then config.range = savedRange end
  state.userHidden = not config.showOnStart

  hs.urlevent.bind("diskmeter-toggle", function() M.toggle() end)
  hs.urlevent.bind("diskmeter-refresh", function() M.refresh() end)
  hs.urlevent.bind("diskmeter-range", function(_, p) p = p or {}; if p.range then M.setRange(p.range) else M.cycleRange() end end)
  hs.urlevent.bind("diskmeter-move", function(_, p) p = p or {}; M.move(p.x, p.y) end)

  local ok, hyper = pcall(require, "hyper")
  if ok and hyper and hyper.hyperMode then
    hyper.bindKey("0", M.toggle)
    hyper.bindShiftKey("d", M.refresh)
  else
    log.w("hyper 모듈 없음: hammerspoon://diskmeter-* URL 만 동작")
  end

  redraw()
  M.refresh()
  log.i("disk-meter " .. M.version .. " 시작")
  return M
end

return M
