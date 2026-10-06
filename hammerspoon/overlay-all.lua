-- overlay-all.lua — 모든 오버레이 패널을 한 번에 숨기고 되살리기
--
-- Agent Shortcuts · Agent Cockpit · Agent Meter · Disk Meter 패널의 표시/숨김 함수를 한데 묶는다.
-- 숨길 때 보이던 패널만 기억했다가 되살릴 때 그 패널만 다시 띄운다 — 사용자가 따로 숨겨 둔 패널은
-- 건드리지 않는다. 불러오지 않은 모듈은 건너뛰므로 일부 패널만 쓰는 설정에서도 동작한다.
-- init.lua 에서 패널 모듈들을 start() 한 뒤에 이 모듈을 start() 한다.
--
-- 단축키: hyper+h 모두 숨김 ↔ 되살림
-- URL:   hammerspoon://overlays-toggle · overlays-hide · overlays-show (show 는 기억과 무관하게 전부)

local log = hs.logger.new("overlay-all", "info")

local M = {}
M.version = "2026-10-06.1"

-- 모듈 이름과 그 모듈이 제공하는 (표시 여부 · 숨김 · 표시) 함수 이름. 모듈마다 이름이 달라 여기서 맞춘다.
M.panels = {
  { name = "agent-shortcuts", visible = "isVisible", hide = "hideOverlay", show = "showOverlay" },
  { name = "agent-cockpit",   visible = "isVisible", hide = "hideOverlay", show = "showOverlay" },
  { name = "agent-meter",     visible = "isVisible", hide = "hide",        show = "show" },
  { name = "disk-meter",      visible = "isVisible", hide = "hide",        show = "show" },
}

local remembered = nil   -- 마지막 hideAll 때 보이던 패널 이름 목록

-- require 가 아니라 package.loaded 를 본다. 사용자가 시작하지 않은 패널을 여기서 불러오지 않기 위해서다.
local function loaded(panel)
  local m = package.loaded[panel.name]
  if type(m) ~= "table" then return nil end
  for _, fn in ipairs({ panel.visible, panel.hide, panel.show }) do
    if type(m[fn]) ~= "function" then
      log.w(panel.name .. " 에 " .. fn .. " 가 없어 건너뜀")
      return nil
    end
  end
  return m
end

function M.visiblePanels()
  local out = {}
  for _, panel in ipairs(M.panels) do
    local m = loaded(panel)
    if m and m[panel.visible]() then out[#out + 1] = panel.name end
  end
  return out
end

function M.hideAll()
  local visible = M.visiblePanels()
  if #visible > 0 then remembered = visible end
  for _, panel in ipairs(M.panels) do
    local m = loaded(panel)
    if m then m[panel.hide]() end
  end
  hs.alert.show("오버레이 모두 숨김 · hyper+h 로 되살림", 0.9)
  return M
end

-- 기억과 무관하게 전부 띄운다.
function M.showAll()
  for _, panel in ipairs(M.panels) do
    local m = loaded(panel)
    if m then m[panel.show]() end
  end
  remembered = nil
  hs.alert.show("오버레이 모두 표시", 0.6)
  return M
end

-- 마지막에 숨긴 패널만 되살린다. 기억이 없으면 전부.
function M.restore()
  if not remembered then return M.showAll() end
  for _, panel in ipairs(M.panels) do
    local m = loaded(panel)
    if m and hs.fnutils.contains(remembered, panel.name) then m[panel.show]() end
  end
  remembered = nil
  hs.alert.show("오버레이 되살림", 0.6)
  return M
end

function M.toggleAll()
  if #M.visiblePanels() > 0 then M.hideAll() else M.restore() end
  return M
end

function M.start()
  hs.urlevent.bind("overlays-toggle", function() M.toggleAll() end)
  hs.urlevent.bind("overlays-hide", function() M.hideAll() end)
  hs.urlevent.bind("overlays-show", function() M.showAll() end)

  local ok, hyper = pcall(require, "hyper")
  if ok and hyper and hyper.hyperMode then
    hyper.bindKey("h", M.toggleAll)
  else
    log.w("hyper 모듈 없음: hammerspoon://overlays-* URL 만 동작")
  end
  log.i("overlay-all " .. M.version .. " 시작")
  return M
end

return M
