-- seekr launcher for Hammerspoon.
--
-- Ctrl+Space shows a small, centered, undecorated kitty window running
-- `seekr --resident`. The first press starts it (a few hundred milliseconds);
-- after that seekr stays alive and Enter/Esc only *hide* the window (seekr asks
-- us via `hs -c "package.loaded.seekr.hide()"`), so every later press just
-- reveals it: the UI is back within a couple of frames and typing works
-- immediately. On reveal we send seekr SIGUSR1, which it takes as "refresh the
-- index in the background". Ctrl+C inside seekr quits it for real, and the
-- window goes with it.
--
-- kitty is started as a process of its own just for seekr (config in
-- contrib/kitty/seekr.conf), so hiding and revealing is plain app hide/unhide
-- on that one process: instant, and it never touches other terminal windows.
-- Terminal.app was used before; it cannot drop its title bar and hiding one of
-- its windows needed AppleScript (see git history).
--
-- Install: contrib/install-launcher.sh (copies this file to ~/.hammerspoon and
-- adds `require("seekr")` to init.lua).

local M = {}

local HOME = os.getenv("HOME")

M.hotkey = { { "ctrl" }, "space" }
M.maxSize = { w = 1100, h = 600 } -- pixels; shrinks on small screens
M.config = HOME .. "/.config/seekr/kitty.conf"
-- Written by `seekr --resident` as {"pid": N}; deleted when it exits.
M.stateFile = HOME .. "/Library/Application Support/seekr/resident.json"
M.debug = false -- when true, decisions and timings are appended to /tmp/seekr-hs.log

local function firstExisting(paths)
  for _, p in ipairs(paths) do
    if hs.fs.attributes(p) then return p end
  end
  return nil
end

M.terminal = firstExisting({
  "/Applications/kitty.app/Contents/MacOS/kitty",
  HOME .. "/Applications/kitty.app/Contents/MacOS/kitty",
  "/opt/homebrew/bin/kitty",
  "/usr/local/bin/kitty",
})
M.seekr = firstExisting({
  HOME .. "/.cargo/bin/seekr",
  "/opt/homebrew/bin/seekr",
  "/usr/local/bin/seekr",
})

M._task = nil       -- hs.task of the kitty process we started
M._launching = nil  -- hs.timer while a window is being created; guards against double launches
M._startedAt = nil  -- when the current show began; M.lastLatency holds hotkey → window placed, in seconds
M._prevWindow = nil -- window that had focus before seekr was shown; focus returns there on hide

local function log(fmt, ...)
  if not M.debug then return end
  local f = io.open("/tmp/seekr-hs.log", "a")
  if not f then return end
  f:write(os.date("%H:%M:%S "), string.format(fmt, ...), "\n")
  f:close()
end

-- The kitty process that hosts seekr, as an hs.application, or nil.
local function app()
  if M._task and M._task:isRunning() then
    local a = hs.application.applicationForPID(M._task:pid())
    if a then return a end
  end
  -- Hammerspoon was reloaded while seekr kept running: find the window by title.
  for _, a in ipairs(hs.application.applicationsForBundleID("net.kovidgoyal.kitty")) do
    for _, w in ipairs(a:allWindows()) do
      if w:title() == "seekr" then return a end
    end
  end
  return nil
end

local function windowOf(a)
  local w = a:mainWindow()
  if w then return w end
  return a:allWindows()[1]
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
  }, 0)
end

local function adopt(w)
  local t0 = hs.timer.secondsSinceEpoch()
  place(w)
  w:focus()
  log("adopt: place+focus %.0f ms", (hs.timer.secondsSinceEpoch() - t0) * 1000)
  if M._startedAt then
    M.lastLatency = hs.timer.secondsSinceEpoch() - M._startedAt
    M._startedAt = nil
  end
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

-- kitty creates its window asynchronously: poll `pick` until it returns one.
local function awaitWindow(pick)
  local tries = 0
  M._launching = hs.timer.doEvery(0.03, function()
    tries = tries + 1
    local w = pick()
    if w or tries > 150 then
      M._launching:stop()
      M._launching = nil
      log("awaitWindow: %s after %d tries", w and "adopting" or "gave up", tries)
      if w then adopt(w) end
    end
  end)
end

local function launch()
  if not M.terminal or not M.seekr then
    hs.alert.show("seekr: kitty or seekr not found; run contrib/install-launcher.sh")
    return
  end
  M._task = hs.task.new(M.terminal, function() M._task = nil end, {
    "--config", M.config,
    "--title", "seekr",
    M.seekr, "--resident",
  })
  M._task:start()
  log("launch: kitty pid %s", tostring(M._task:pid()))
  awaitWindow(function()
    local a = app()
    return a and windowOf(a)
  end)
end

-- Bring the hidden window back on the current space. Returns false if there is none.
local function reveal()
  local a = app()
  if not a then return false end
  wakeResident()
  a:unhide()
  local w = windowOf(a)
  if not w then return false end
  pcall(function()
    local here = hs.spaces.focusedSpace()
    local there = hs.spaces.windowSpaces(w)
    if here and there and there[1] and there[1] ~= here then
      hs.spaces.moveWindowToSpace(w, here)
    end
  end)
  adopt(w)
  return true
end

-- Called by seekr (via the `hs` CLI) after Enter/Esc: hide the window, keep the process.
function M.hide()
  local a = app()
  log("hide: app=%s prev=%s", a and a:pid() or "nil", M._prevWindow and M._prevWindow:title() or "nil")
  if not a then return end
  a:hide()
  local prev = M._prevWindow
  M._prevWindow = nil
  if prev and pcall(function() return prev:isVisible() end) then prev:focus() end
end

function M.show()
  if M._launching then return end -- a window is already on its way
  M._startedAt = hs.timer.secondsSinceEpoch()
  local focused = hs.window.focusedWindow()
  if focused and focused:title() ~= "seekr" then M._prevWindow = focused end
  if reveal() then return end
  launch()
end

hs.hotkey.bind(M.hotkey[1], M.hotkey[2], M.show)

return M
