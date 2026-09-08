-- seekr launcher for Hammerspoon.
--
-- Ctrl+Space shows a small, centered Terminal window running `seekr --resident`.
-- The first press creates it; seekr then stays alive, and Enter/Esc only *hide*
-- the window (seekr asks us via `hs -c "package.loaded.seekr.hide()"`), so every
-- later press just reveals it again: the UI is back in well under 100 ms and
-- typing works immediately. On reveal we send seekr SIGUSR1, which it takes
-- as "refresh the index in the background". Ctrl+C inside seekr quits for real,
-- and the window closes with it.
--
-- Terminal.app specifics, all found by experiment:
--   * The "seekr" profile (contrib/terminal/make-profile.py) runs the binary in
--     place of a login shell, so ~/.zshrc is never sourced. AppleScript's
--     `do script` always goes through a shell, so the profile is opened via its
--     "Shell › New Window › seekr" menu item through the accessibility API.
--   * A single window can be hidden with AppleScript `set visible to false`;
--     setting `visible` back does nothing, but `set frontmost to true` reveals
--     it again. Both take ~40 ms. Hiding the whole app would also hide the
--     user's other Terminal windows, so we never do that.
--   * Windows cannot be moved fully off screen (macOS clamps them), and
--     minimize/unminimize animates for ~200 ms, so neither is used for parking.
--
-- Install: contrib/install-launcher.sh (copies this file to ~/.hammerspoon and
-- adds `require("seekr")` to init.lua).

local M = {}

M.hotkey = { { "ctrl" }, "space" }
M.profile = "seekr"               -- Terminal.app profile name
M.command = "exec seekr"          -- fallback only: typed into a shell when the profile is missing
M.maxSize = { w = 1100, h = 600 } -- pixels; shrinks on small screens

-- Written by `seekr --resident` as {"pid": N}; deleted when it exits.
M.stateFile = os.getenv("HOME") .. "/Library/Application Support/seekr/resident.json"

M.debug = false -- when true, decisions are appended to /tmp/seekr-hs.log

local function log(fmt, ...)
  if not M.debug then return end
  local f = io.open("/tmp/seekr-hs.log", "a")
  if not f then return end
  f:write(os.date("%H:%M:%S "), string.format(fmt, ...), "\n")
  f:close()
end

M._windowId = nil   -- CGWindowID of the launcher window, captured when it is created
M._launching = nil  -- hs.timer while a window is being created; guards against double launches
M._startedAt = nil  -- when the current show began; M.lastLatency holds hotkey → window placed, in seconds
M._prevWindow = nil -- window that had focus before seekr was shown; focus returns there on hide

-- The profile shows only its name in the title bar; Terminal renders that as
-- "seekr" or "Terminal — seekr". The shell fallback shows the typed command.
local function isLauncherTitle(t)
  t = t or ""
  return t == "seekr" or t:match("^seekr%s") ~= nil or t:match("— seekr$") ~= nil
    or t:find("exec seekr", 1, true) ~= nil
end

-- Look a window up among Terminal's only. hs.window.get(id) walks every running
-- application's windows through the accessibility API and can take seconds when
-- some app is slow to answer; Terminal alone answers in a few milliseconds.
local function terminalWindow(id)
  local term = hs.application.get("Terminal")
  if not term then return nil end
  for _, w in ipairs(term:allWindows()) do
    if w:id() == id then return w end
  end
  return nil
end

local function findWindow()
  if M._windowId then
    local w = terminalWindow(M._windowId)
    if w then return w end
    M._windowId = nil
  end
  -- Fallback: match the title the launcher's tab ends up with.
  local term = hs.application.get("Terminal")
  if not term then return nil end
  for _, w in ipairs(term:allWindows()) do
    if isLauncherTitle(w:title()) then return w end
  end
  return nil
end

local function place(w)
  local s = hs.screen.mainScreen():frame()
  local width = math.min(M.maxSize.w, math.floor(s.w * 0.72))
  local height = math.min(M.maxSize.h, math.floor(s.h * 0.55))
  w:setFrame({
    x = s.x + math.floor((s.w - width) / 2),
    y = s.y + math.floor(s.h * 0.16),
    w = width,
    h = height,
  })
end

local function adopt(w)
  M._windowId = w:id()
  local t0 = hs.timer.secondsSinceEpoch()
  place(w)
  local t1 = hs.timer.secondsSinceEpoch()
  w:focus()
  log("adopt: place %.0f ms, focus %.0f ms", (t1 - t0) * 1000, (hs.timer.secondsSinceEpoch() - t1) * 1000)
  if M._startedAt then
    M.lastLatency = hs.timer.secondsSinceEpoch() - M._startedAt
    M._startedAt = nil
  end
end


-- Terminal creates windows asynchronously: poll `pick` until it returns one.
local function awaitWindow(pick)
  local tries = 0
  M._launching = hs.timer.doEvery(0.03, function()
    tries = tries + 1
    local w = pick(tries)
    if w or tries > 100 then
      M._launching:stop()
      M._launching = nil
      log("awaitWindow: %s after %d tries", w and ("adopting " .. w:id()) or "gave up", tries)
      if w then adopt(w) end
    end
  end)
end

local function residentPid()
  local f = io.open(M.stateFile)
  if not f then return nil end
  local s = f:read("*a") or ""
  f:close()
  return tonumber(s:match('"pid"%s*:%s*(%d+)'))
end

-- Tell the resident seekr to wake up (refresh in the background). Fire-and-forget
-- without a shell: hs.execute would cost tens of milliseconds on the hot path.
local function wakeResident()
  local pid = residentPid()
  if pid then hs.task.new("/bin/kill", nil, { "-USR1", tostring(pid) }):start() end
end

-- Bring a parked window back. Hidden windows are invisible to the accessibility
-- API (so hs.window cannot find them) but Terminal's own AppleScript window list
-- still has them: one round trip checks the remembered id, falls back to the
-- launcher title, and un-hides via `frontmost`. Returns the window id, or nil.
local function reveal()
  local script = string.format([[
    tell application "Terminal"
      if %s then
        set frontmost of window id %d to true
        return %d
      end if
      set ws to (windows whose name is "Terminal — seekr" or name is "seekr")
      if (count of ws) > 0 then
        set frontmost of item 1 of ws to true
        return id of item 1 of ws
      end if
      return 0
    end tell
  ]], M._windowId and string.format("exists window id %d", M._windowId) or "false",
     M._windowId or 0, M._windowId or 0)
  local t0 = hs.timer.secondsSinceEpoch()
  local ok, id, err = hs.osascript.applescript(script)
  local t1 = hs.timer.secondsSinceEpoch()
  log("reveal: remembered=%s ok=%s result=%s err=%s (applescript %.0f ms)", tostring(M._windowId), tostring(ok), tostring(id), ok and "" or hs.inspect(err), (t1 - t0) * 1000)
  id = ok and tonumber(id) or 0
  if id == 0 then return nil end
  local term = hs.application.get("Terminal")
  if term and term:isHidden() then term:unhide() end
  M._windowId = id
  local w = terminalWindow(id)
  log("reveal: lookup %s in %.0f ms", w and "found" or "not yet", (hs.timer.secondsSinceEpoch() - t1) * 1000)
  if w then
    adopt(w)
  else
    awaitWindow(function() return terminalWindow(id) end)
  end
  return id
end

-- Called by seekr (via the `hs` CLI) after Enter/Esc: hide the window, keep the process.
function M.hide()
  local w = findWindow()
  log("hide: window=%s prev=%s", w and w:id() or "nil", M._prevWindow and M._prevWindow:title() or "nil")
  if not w then return end
  local term = w:application()
  hs.osascript.applescript(string.format(
    'tell application "Terminal" to set visible of window id %d to false', w:id()))
  local prev = M._prevWindow
  M._prevWindow = nil
  if prev and prev:id() ~= w:id() and pcall(function() return prev:isVisible() end) then
    prev:focus()
  elseif term and #term:visibleWindows() == 0 then
    -- Nothing to hand focus back to and no other Terminal windows: step aside entirely.
    term:hide()
  end
end

-- Preferred path: "Shell › New Window › <profile>" is the only scriptable entry
-- point that runs a profile's own startup command (here: the seekr binary).
local function openProfile(term)
  local before = {}
  for _, w in ipairs(term:allWindows()) do before[w:id()] = true end
  if not term:selectMenuItem({ "Shell", "New Window", M.profile }) then return false end
  awaitWindow(function(tries)
    local t = hs.application.get("Terminal")
    if not t then return nil end
    local fresh
    for _, w in ipairs(t:allWindows()) do
      if not before[w:id()] then
        if isLauncherTitle(w:title()) then return w end
        fresh = fresh or w
      end
    end
    -- The title can lag the window by a few frames; eventually accept any new window.
    if tries > 15 then return fresh end
    return nil
  end)
  return true
end

-- Fallback: type the command into a fresh login shell. Works without the
-- profile's startup command, but pays for ~/.zshrc on every launch.
local function openViaShell()
  local script = string.format([[
    tell application "Terminal"
      if not running then launch
      set t to do script "%s"
      set current settings of t to settings set "%s"
      set custom title of t to "seekr"
      return id of front window
    end tell
  ]], M.command, M.profile)
  local ok, result, err = hs.osascript.applescript(script)
  if not ok then
    local msg = type(err) == "table" and (err.NSLocalizedDescription or err.OSAScriptErrorMessageKey) or tostring(err)
    hs.alert.show("seekr: " .. tostring(msg))
    return
  end
  local id = tonumber(result)
  awaitWindow(function()
    local w = id and terminalWindow(id)
    return w or findWindow()
  end)
end

local function launch(attempt)
  local term = hs.application.get("Terminal")
  if term then
    if openProfile(term) then return end
    -- Menus present but no "seekr" entry: the profile is missing. Menus absent:
    -- Terminal is still starting up, so try again shortly.
    if term:findMenuItem({ "Shell", "New Window" }) then
      openViaShell()
      return
    end
  elseif attempt == 1 then
    -- `launch` (unlike `activate`) starts Terminal without its startup window,
    -- so we never leave a stray empty shell behind.
    hs.osascript.applescript('tell application "Terminal" to launch')
  end
  if attempt < 60 then
    M._launching = hs.timer.doAfter(0.05, function() launch(attempt + 1) end)
  else
    openViaShell()
  end
end

function M.show()
  if M._launching then return end -- a window is already on its way
  M._startedAt = hs.timer.secondsSinceEpoch()
  local focused = hs.window.focusedWindow()
  if focused and focused:id() ~= M._windowId then M._prevWindow = focused end
  log("show: focusedWindow took %.0f ms", (hs.timer.secondsSinceEpoch() - M._startedAt) * 1000)
  if hs.application.get("Terminal") then
    -- Wake the resident first so it is redrawing while the window comes forward.
    -- (If its window is gone, so is the process: Terminal closes one with the other.)
    local t0 = hs.timer.secondsSinceEpoch()
    wakeResident()
    log("show: wake took %.0f ms", (hs.timer.secondsSinceEpoch() - t0) * 1000)
    if reveal() then return end
  end
  launch(1)
end

hs.hotkey.bind(M.hotkey[1], M.hotkey[2], M.show)

return M
