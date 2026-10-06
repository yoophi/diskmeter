-- overlay-layout.lua — 오버레이 패널을 겹치지 않게 쌓는 레이아웃 관리자
--
-- 규칙
--   · 기준점은 주 화면 **우측 하단**. 보이는 패널을 order 순서로 아래에서 위로 쌓는다 (order 1 이 맨 아래).
--   · 폭은 관리자가 정한다 (config.width, 기본 overlay-style 의 S.width). 배치할 때 모든 패널을 같은 폭으로 맞춘다.
--   · 패널 사이 간격은 config.gap 하나다. 열 사이 간격도 같다.
--   · 한 열이 화면 위를 넘으면 왼쪽으로 한 열 옮겨 다시 아래부터 쌓는다.
--   · 사용자가 드래그해 고정한 패널은 스택에서 빠져 장애물이 되고, 스택은 그 위로 비켜 간다.
--
-- 패널 모듈은 start() 에서 register() 하고, 그릴 때 slot(name, h) 로 자리를 묻고, 그린 뒤 schedule() 로
-- "높이나 표시 여부가 바뀌었을 수 있다" 고 알린다. 재배치는 캔버스 프레임만 옮기고 패널의 redraw 를
-- 부르지 않으므로 그리기 ↔ 재배치 재귀가 없다. 같은 틱에 여러 번 schedule() 해도 apply() 는 한 번만 돈다.
--
-- register 명세: { order = 정렬 순서(작을수록 기준점에 가까움), frame = fn → {x,y,w,h}|nil, visible = fn → bool,
--                place = fn(x, y, w) 캔버스 프레임만 바꿈, pinned = fn → bool 사용자가 고정했는지, unpin = fn 고정 해제 }
--
-- 단축키: hyper+shift+h 정리 — 모든 고정을 풀고 스택으로 되돌린다
-- URL:   hammerspoon://overlays-arrange

local S = require("overlay-style")
local log = hs.logger.new("overlay-layout", "info")

local M = {}
M.version = "2026-10-06.2"
M.config = {
  width = S.width,                                 -- 모든 패널의 폭
  gap = S.margin.gap,                              -- 패널 사이 · 열 사이 간격
  margin = { right = S.margin.right, bottom = S.margin.bottom or S.margin.top, top = S.margin.top },
  maxPasses = 32,                                  -- 장애물 회피 반복 상한. 화면이 온통 고정 패널이면 겹치더라도 멈춘다
}

local panels = {}          -- name → spec
local pending = nil        -- 예약된 apply 타이머
local screenWatcher = nil

local REQUIRED = { "frame", "visible", "place", "pinned", "unpin" }

function M.register(name, spec)
  for _, fn in ipairs(REQUIRED) do
    if type(spec[fn]) ~= "function" then
      log.e(name .. ": spec." .. fn .. " 가 함수가 아니라 등록하지 않음")
      return M
    end
  end
  panels[name] = spec
  M.schedule()
  return M
end

function M.unregister(name)
  panels[name] = nil
  M.schedule()
  return M
end

function M.names()
  local out = {}
  for name in pairs(panels) do out[#out + 1] = name end
  table.sort(out)
  return out
end

-- 주 화면과 기준점(우측 하단). anchor.x 는 패널 왼쪽 변, anchor.bottom 은 패널 아래 변이 놓일 자리.
local function anchor()
  local screen = hs.screen.primaryScreen():frame()
  local c = M.config
  return screen, {
    x = screen.x + screen.w - c.width - c.margin.right,
    bottom = screen.y + screen.h - c.margin.bottom,
    top = screen.y + c.margin.top,
  }
end

local function intersects(a, b)
  return a.x < b.x + b.w and a.x + a.w > b.x and a.y < b.y + b.h and a.y + a.h > b.y
end

-- 현재 상태로 스택 패널들의 자리를 계산한다.
-- overrides[name] = { h = 높이 } 는 아직 그리지 않은 패널이 자기 높이를 미리 알려 줄 때 쓴다. 그 패널은 보이는 것으로 친다.
local function compute(overrides)
  overrides = overrides or {}
  local screen, a = anchor()
  local w, gap = M.config.width, M.config.gap
  local obstacles, stack = {}, {}

  for name, spec in pairs(panels) do
    local override = overrides[name]
    if override ~= nil or spec.visible() then
      local f = spec.frame()
      local h = (override and override.h) or (f and f.h)
      if h then
        if spec.pinned() then
          if f then obstacles[#obstacles + 1] = { x = f.x, y = f.y, w = f.w, h = f.h } end
        else
          stack[#stack + 1] = { name = name, order = spec.order or 99, h = h }
        end
      end
    end
  end
  table.sort(stack, function(p, q)
    if p.order ~= q.order then return p.order < q.order end
    return p.name < q.name
  end)

  local positions = {}
  local x, bottom = a.x, a.bottom
  for _, item in ipairs(stack) do
    local y
    for _ = 1, M.config.maxPasses do
      y = bottom - item.h
      -- 이 열 위쪽에 안 들어가면 왼쪽 열 맨 아래로. 열 맨 아래에서조차 안 들어가는 긴 패널은 위 여백에 맞춘다.
      if y < a.top and bottom < a.bottom and x - (w + gap) >= screen.x then
        x = x - (w + gap)
        bottom = a.bottom
        y = bottom - item.h
      end
      if y < a.top then y = a.top end
      local rect = { x = x, y = y, w = w, h = item.h }
      local hit = nil
      for _, obstacle in ipairs(obstacles) do
        if intersects(rect, obstacle) then hit = obstacle; break end
      end
      if not hit then break end
      bottom = hit.y - gap   -- 장애물 위로 올라간다
    end
    positions[item.name] = { x = x, y = y }
    bottom = y - gap
  end
  return positions
end

-- 그리기 전에 "내 높이가 h 라면 어디에 놓이는가". 고정된 패널이거나 모르는 이름이면 기준점.
function M.slot(name, h)
  local positions = compute({ [name] = { h = h } })
  if positions[name] then return positions[name] end
  local _, a = anchor()
  return { x = a.x, y = math.max(a.top, a.bottom - h) }
end

local function apply()
  local positions = compute()
  local w = M.config.width
  for name, pos in pairs(positions) do
    local spec = panels[name]
    local f = spec.frame()
    if f and (math.abs(f.x - pos.x) > 0.5 or math.abs(f.y - pos.y) > 0.5 or math.abs(f.w - w) > 0.5) then
      spec.place(pos.x, pos.y, w)
    end
  end
end

-- 다음 틱에 한 번 재배치한다. 패널이 그린 직후, 표시/숨김 직후, 드래그를 놓은 직후 부른다.
function M.schedule()
  if pending then return M end
  pending = hs.timer.doAfter(0.05, function()
    pending = nil
    local ok, err = pcall(apply)
    if not ok then log.e("재배치 실패: " .. tostring(err)) end
  end)
  return M
end

function M.apply()
  local ok, err = pcall(apply)
  if not ok then log.e("재배치 실패: " .. tostring(err)) end
  return M
end

-- 모든 고정을 풀고 스택으로 되돌린다.
function M.arrange()
  for name, spec in pairs(panels) do
    local ok, err = pcall(spec.unpin)
    if not ok then log.w(name .. " 고정 해제 실패: " .. tostring(err)) end
  end
  M.apply()
  hs.alert.show("오버레이 정리", 0.6)
  return M
end

-- 현재 보이는 패널 중 서로 겹치는 쌍. 검증과 디버깅용.
function M.overlaps()
  local frames = {}
  for name, spec in pairs(panels) do
    if spec.visible() then
      local f = spec.frame()
      if f then frames[#frames + 1] = { name = name, x = f.x, y = f.y, w = f.w, h = f.h } end
    end
  end
  table.sort(frames, function(p, q) return p.name < q.name end)
  local out = {}
  for i = 1, #frames do
    for j = i + 1, #frames do
      if intersects(frames[i], frames[j]) then out[#out + 1] = frames[i].name .. "×" .. frames[j].name end
    end
  end
  return out
end

function M.start()
  hs.urlevent.bind("overlays-arrange", function() M.arrange() end)
  local ok, hyper = pcall(require, "hyper")
  if ok and hyper and hyper.hyperMode then
    hyper.bindShiftKey("h", M.arrange)
  else
    log.w("hyper 모듈 없음: hammerspoon://overlays-arrange URL 만 동작")
  end
  if not screenWatcher then
    screenWatcher = hs.screen.watcher.new(function() M.schedule() end)
    screenWatcher:start()
  end
  M.schedule()
  log.i("overlay-layout " .. M.version .. " 시작 · 패널: " .. table.concat(M.names(), ", "))
  return M
end

return M
