-- overlay-layout.lua — 오버레이 패널을 겹치지 않게 쌓는 레이아웃 관리자
--
-- 규칙
--   · 기준점은 주 화면 **우측 하단**. 보이는 패널을 아래에서 위로 쌓는다.
--   · 폭은 관리자가 정한다 (config.width, 기본 overlay-style 의 S.width). 배치할 때 모든 패널을 같은 폭으로 맞춘다.
--   · 패널 사이 간격과 열 사이 간격은 config.gap 하나다.
--   · 한 열이 화면 위를 넘으면 왼쪽으로 한 열 옮겨 다시 아래부터 쌓는다.
--   · 사용자가 드래그해 고정한 패널은 스택에서 빠져 장애물이 되고, 스택은 그 위로 비켜 간다.
--
-- 두 가지 다듬기
--   · arrange() hyper+shift+h — 정해진 order 로 다시 쌓는다. 고정과 계획을 모두 지운다.
--   · tidy()    hyper+t       — 지금 놓인 모양(어느 열, 아래부터 몇 번째)을 계획으로 읽어 두고 그 계획대로
--                               격자에 맞춘다. 가장 오른쪽 열의 맨 아래 패널이 우측 하단 기준점에 붙고, 나머지는
--                               공통 간격으로 위·왼쪽에 놓인다. 계획은 저장되어 재시작 뒤에도 유지되고, 패널 높이가
--                               바뀌면 계획 안에서 다시 흐른다. 고정은 풀린다.
--
-- 패널 모듈은 start() 에서 register() 하고, 그릴 때 slot(name, h) 로 자리를 묻고, 그린 뒤 schedule() 로
-- "높이나 표시 여부가 바뀌었을 수 있다" 고 알린다. 재배치는 캔버스 프레임만 옮기고 패널의 redraw 를
-- 부르지 않으므로 그리기 ↔ 재배치 재귀가 없다. 같은 틱에 여러 번 schedule() 해도 apply() 는 한 번만 돈다.
--
-- register 명세: { order = 정렬 순서(작을수록 기준점에 가까움), frame = fn → {x,y,w,h}|nil, visible = fn → bool,
--                place = fn(x, y, w) 캔버스 프레임만 바꿈, pinned = fn → bool 사용자가 고정했는지, unpin = fn 고정 해제 }
--
-- URL: hammerspoon://overlays-arrange · overlays-tidy

local S = require("overlay-style")
local log = hs.logger.new("overlay-layout", "info")

local M = {}
M.version = "2026-10-06.3"
M.config = {
  width = S.width,                                 -- 모든 패널의 폭
  gap = S.margin.gap,                              -- 패널 사이 · 열 사이 간격
  margin = { right = S.margin.right, bottom = S.margin.bottom or S.margin.top, top = S.margin.top },
  maxPasses = 32,                                  -- 장애물 회피 반복 상한. 화면이 온통 고정 패널이면 겹치더라도 멈춘다
}

local PLAN_KEY = "overlayLayout.plan"

local panels = {}          -- name → spec
local plan = {}            -- name → { column = 0부터 오른쪽에서 왼쪽으로, rank = 1부터 아래에서 위로 }. 비어 있으면 order 로 쌓는다
local pending = nil        -- 예약된 apply 타이머
local screenWatcher = nil

local REQUIRED = { "frame", "visible", "place", "pinned", "unpin" }

local function loadPlan()
  local saved = hs.settings.get(PLAN_KEY)
  plan = {}
  if type(saved) ~= "table" then return end
  for name, entry in pairs(saved) do
    if type(entry) == "table" and type(entry.column) == "number" and type(entry.rank) == "number" then
      plan[name] = { column = entry.column, rank = entry.rank }
    end
  end
end

local function savePlan()
  if next(plan) == nil then hs.settings.clear(PLAN_KEY) else hs.settings.set(PLAN_KEY, plan) end
end

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

function M.plan()
  local out = {}
  for name, entry in pairs(plan) do out[name] = { column = entry.column, rank = entry.rank } end
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

local function centerOn(f, screen)
  local cx, cy = f.x + f.w / 2, f.y + f.h / 2
  return cx >= screen.x and cx < screen.x + screen.w and cy >= screen.y and cy < screen.y + screen.h
end

-- 스택에 넣을 패널들을 열별 · 아래→위 순서로 나눈다.
-- 계획이 있으면 계획대로, 없으면 한 열에 order 순서. 계획에 없는 패널은 오른쪽 열 맨 위에 order 순서로 덧붙인다.
local function columnsOf(stack)
  local byColumn = {}
  for _, item in ipairs(stack) do
    local entry = plan[item.name]
    local column = entry and entry.column or 0
    item.rank = entry and entry.rank or (1e6 + (item.order or 99))
    byColumn[column] = byColumn[column] or {}
    table.insert(byColumn[column], item)
  end
  local indices = {}
  for index in pairs(byColumn) do indices[#indices + 1] = index end
  table.sort(indices)
  local columns = {}
  for _, index in ipairs(indices) do
    local list = byColumn[index]
    table.sort(list, function(p, q)
      if p.rank ~= q.rank then return p.rank < q.rank end
      return p.name < q.name
    end)
    columns[#columns + 1] = list
  end
  return columns
end

-- 현재 상태로 스택 패널들의 자리를 계산한다.
-- overrides[name] = { h = 높이 } 는 아직 그리지 않은 패널이 자기 높이를 미리 알려 줄 때 쓴다. 그 패널은 보이는 것으로 친다.
local function compute(overrides)
  overrides = overrides or {}
  local screen, a = anchor()
  local w, gap = M.config.width, M.config.gap
  local pitch = w + gap
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

  local positions = {}
  local x = a.x
  for _, column in ipairs(columnsOf(stack)) do
    local bottom = a.bottom
    for _, item in ipairs(column) do
      local y
      for _ = 1, M.config.maxPasses do
        y = bottom - item.h
        -- 이 열 위쪽에 안 들어가면 왼쪽 열 맨 아래로. 열 맨 아래에서조차 안 들어가는 긴 패널은 위 여백에 맞춘다.
        if y < a.top and bottom < a.bottom and x - pitch >= screen.x then
          x = x - pitch
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
    x = x - pitch   -- 다음 열
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

local function unpinAll()
  for name, spec in pairs(panels) do
    local ok, err = pcall(spec.unpin)
    if not ok then log.w(name .. " 고정 해제 실패: " .. tostring(err)) end
  end
end

-- 정해진 order 로 다시 쌓는다. 고정과 계획을 모두 지운다.
function M.arrange()
  plan = {}
  savePlan()
  unpinAll()
  M.apply()
  hs.alert.show("오버레이 정리", 0.6)
  return M
end

-- 지금 놓인 모양을 계획으로 읽어 격자에 맞춘다.
--
-- 가장 오른쪽 패널의 x 를 0번 열로 보고, 왼쪽으로 (폭 + 간격) 만큼 떨어질 때마다 열 번호가 하나씩 는다.
-- 열 안에서는 아래 변이 낮은 순서가 rank 1 이다. 그래서 가장 오른쪽 열의 맨 아래 패널이 우측 하단
-- 기준점에 붙고, 나머지는 공통 간격으로 위와 왼쪽에 놓인다. 주 화면 밖에 둔 패널은 건드리지 않는다.
function M.tidy()
  local screen = anchor()
  local pitch = M.config.width + M.config.gap
  local items = {}
  for name, spec in pairs(panels) do
    if spec.visible() then
      local f = spec.frame()
      if f and centerOn(f, screen) then items[#items + 1] = { name = name, spec = spec, f = f } end
    end
  end
  if #items == 0 then
    hs.alert.show("정돈할 패널이 없습니다", 0.8)
    return M
  end

  local rightmost = -math.huge
  for _, item in ipairs(items) do rightmost = math.max(rightmost, item.f.x) end
  local byColumn = {}
  for _, item in ipairs(items) do
    local column = math.max(0, math.floor((rightmost - item.f.x) / pitch + 0.5))
    byColumn[column] = byColumn[column] or {}
    table.insert(byColumn[column], item)
  end
  local indices = {}
  for index in pairs(byColumn) do indices[#indices + 1] = index end
  table.sort(indices)

  plan = {}
  for position, index in ipairs(indices) do        -- 빈 열은 없애고 0, 1, 2… 로 촘촘히
    local list = byColumn[index]
    table.sort(list, function(p, q) return (p.f.y + p.f.h) > (q.f.y + q.f.h) end)
    for rank, item in ipairs(list) do
      plan[item.name] = { column = position - 1, rank = rank }
    end
  end
  savePlan()
  for _, item in ipairs(items) do
    local ok, err = pcall(item.spec.unpin)
    if not ok then log.w(item.name .. " 고정 해제 실패: " .. tostring(err)) end
  end
  M.apply()
  hs.alert.show("오버레이 정돈", 0.6)
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
  loadPlan()
  hs.urlevent.bind("overlays-arrange", function() M.arrange() end)
  hs.urlevent.bind("overlays-tidy", function() M.tidy() end)
  local ok, hyper = pcall(require, "hyper")
  if ok and hyper and hyper.hyperMode then
    hyper.bindShiftKey("h", M.arrange)
    hyper.bindKey("t", M.tidy)
  else
    log.w("hyper 모듈 없음: hammerspoon://overlays-arrange · overlays-tidy URL 만 동작")
  end
  if not screenWatcher then
    screenWatcher = hs.screen.watcher.new(function() M.schedule() end)
    screenWatcher:start()
  end
  M.schedule()
  local planned = 0
  for _ in pairs(plan) do planned = planned + 1 end
  log.i(string.format("overlay-layout %s 시작 · 패널: %s · 계획 %d개", M.version, table.concat(M.names(), ", "), planned))
  return M
end

return M
