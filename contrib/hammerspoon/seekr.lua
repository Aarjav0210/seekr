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
-- Ctrl+Space while seekr has focus hides it again, whatever the query says.
-- seekr is told with SIGUSR2 so it keeps the text and shows it selected on the
-- next reveal: typing replaces it, an arrow key keeps it.
--
-- The window is a kitty *panel* (`kitten panel`, kitty 0.42 or newer), started
-- as a process of its own just for seekr (config in contrib/kitty/seekr.conf),
-- so hiding and revealing is plain app hide/unhide on that one process:
-- instant, and it never touches other terminal windows. A panel, unlike a
-- regular kitty window, is on every desktop (Space) at once, so it comes back
-- on whichever desktop is in front. A regular window stays on the desktop it
-- was created on and macOS switches there when it gets focus; Hammerspoon's
-- hs.spaces.moveWindowToSpace reports success but does nothing on macOS 15.
-- Which *screen* it appears on is our job: the screen of the window that had
-- focus when the hotkey was pressed, or the mouse's screen if none had.
-- Terminal.app was used before kitty; it cannot drop its title bar and hiding
-- one of its windows needed AppleScript (see git history).
--
-- Install: contrib/install-launcher.sh (copies this file to ~/.hammerspoon and
-- adds `require("seekr")` to init.lua).

local M = {}

local HOME = os.getenv("HOME")

M.hotkey = { { "ctrl" }, "space" }
M.maxSize = { w = 1100, h = 600 } -- text area in pixels; shrinks on small screens
M.config = HOME .. "/.config/seekr/kitty.conf"
-- Written by `seekr --resident` as {"pid": N, "park": "USR2"}; deleted when it
-- exits. "park" names the signal that tells seekr "the launcher hid you"; older
-- binaries omit it and are simply hidden without being told.
M.stateFile = HOME .. "/Library/Application Support/seekr/resident.json"
M.debug = false -- when true, decisions and timings are appended to /tmp/seekr-hs.log

local function firstExisting(paths)
  for _, p in ipairs(paths) do
    if hs.fs.attributes(p) then return p end
  end
  return nil
end

-- kitty's `kitten` binary; `kitten panel` is what opens the window.
M.kitten = firstExisting({
  "/Applications/kitty.app/Contents/MacOS/kitten",
  HOME .. "/Applications/kitty.app/Contents/MacOS/kitten",
  "/opt/homebrew/bin/kitten",
  "/usr/local/bin/kitten",
})
M.seekr = firstExisting({
  HOME .. "/.cargo/bin/seekr",
  "/opt/homebrew/bin/seekr",
  "/usr/local/bin/seekr",
})

-- Panels run under kitty's background-only quick-access bundle. The plain kitty
-- bundle covers a resident started by an older version of this file.
local BUNDLES = { "net.kovidgoyal.kitty-quick-access", "net.kovidgoyal.kitty" }

M._task = nil       -- hs.task of the kitty process we started
M._launching = nil  -- hs.timer while a window is being created; guards against double launches
M._startedAt = nil  -- when the current show began; M.lastLatency holds hotkey → window placed, in seconds
M._prevWindow = nil -- window that had focus before seekr was shown; focus returns there on hide
M._screen = nil     -- screen the user was on when the hotkey was pressed; the window goes there

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
  for _, bundle in ipairs(BUNDLES) do
    for _, a in ipairs(hs.application.applicationsForBundleID(bundle)) do
      for _, w in ipairs(a:allWindows()) do
        if w:title() == "seekr" then return a end
      end
    end
  end
  return nil
end

local function windowOf(a)
  local w = a:mainWindow()
  if w then return w end
  return a:allWindows()[1]
end

-- The screen the window belongs on: where the user was at the hotkey (see
-- show), else the screen with keyboard focus.
local function targetScreen()
  return M._screen or hs.screen.mainScreen()
end

-- Size of the text area for a screen of frame `s`. A panel's size is fixed
-- when it is created (kitty ignores later resize requests), so this is only
-- used at launch; the window ends up this plus its padding.
local function textSize(s)
  return {
    w = math.min(M.maxSize.w, math.floor(s.w * 0.72)),
    h = math.min(M.maxSize.h, math.floor(s.h * 0.55)),
  }
end

-- Top-left corner for a window `size` wide on a screen of frame `s`: centred,
-- a little above the middle.
local function topLeft(s, size)
  return {
    x = s.x + math.floor((s.w - size.w) / 2),
    y = s.y + math.floor(s.h * 0.16),
  }
end

local function place(w)
  w:setTopLeft(topLeft(targetScreen():frame(), w:size()))
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

-- The resident seekr's pid and the name of its "parked" signal (nil for old binaries).
local function residentState()
  local f = io.open(M.stateFile)
  if not f then return nil, nil end
  local s = f:read("*a") or ""
  f:close()
  return tonumber(s:match('"pid"%s*:%s*(%d+)')), s:match('"park"%s*:%s*"(%w+)"')
end

-- Fire-and-forget signal to the resident seekr, without a shell: hs.execute
-- would cost tens of milliseconds on the hot path.
local function signalResident(pid, sig)
  hs.task.new("/bin/kill", nil, { "-" .. sig, tostring(pid) }):start()
end

-- Tell the resident seekr to wake up (refresh in the background).
local function wakeResident()
  local pid = residentState()
  if pid then signalResident(pid, "USR1") end
end

-- Tell the resident seekr the launcher just hid it, so it stops drawing and
-- keeps the query for the next reveal.
local function parkResident()
  local pid, park = residentState()
  if pid and park then signalResident(pid, park) end
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
      if w then
        adopt(w)
        hs.timer.doAfter(0.4, function() place(w) end) -- see launch()
      end
    end
  end)
end

local function launch()
  if not M.kitten or not M.seekr then
    hs.alert.show("seekr: kitten (kitty 0.42+) or seekr not found; run contrib/install-launcher.sh")
    return
  end
  -- The size is fixed here (see textSize). kitty places the window itself a
  -- moment after creating it, overriding anything done to it before, so it is
  -- asked for the spot adopt() would pick (margins are measured from the top
  -- left of the screen's usable area); place() then runs once more after kitty
  -- is done, which matters when that is not the screen the user is on. The
  -- window level (regular, not the desktop-widget level panels default to) is
  -- set in the kitty config.
  local s = targetScreen():frame()
  local size = textSize(s)
  local at = topLeft(s, size)
  M._task = hs.task.new(M.kitten, function() M._task = nil end, {
    "panel",
    "--edge=none",
    "--margin-left=" .. math.floor(at.x - s.x),
    "--margin-top=" .. math.floor(at.y - s.y),
    "--columns=" .. size.w .. "px",
    "--lines=" .. size.h .. "px",
    "--layer=top",
    "--focus-policy=on-demand",
    "--config=" .. M.config,
    M.seekr, "--resident",
  })
  M._task:start()
  log("launch: kitten pid %s", tostring(M._task:pid()))
  awaitWindow(function()
    local a = app()
    return a and windowOf(a)
  end)
end

-- Bring the hidden window back. Returns false if there is none.
local function reveal()
  local a = app()
  if not a then return false end
  wakeResident()
  a:unhide()
  local w = windowOf(a)
  if not w then return false end
  adopt(w)
  return true
end

-- Called by seekr (via the `hs` CLI) after Enter/Esc: hide the window, keep the
-- process. Focus goes back to the window that was in front before seekr unless
-- restoreFocus is false: seekr passes that after opening something, so the app
-- `open` just brought forward stays there instead of being pushed behind the
-- previous one a moment later. No argument (Esc, the Ctrl+Space toggle, older
-- binaries) means restore.
function M.hide(restoreFocus)
  if restoreFocus == nil then restoreFocus = true end
  local a = app()
  log("hide: app=%s prev=%s restore=%s", a and a:pid() or "nil",
    M._prevWindow and M._prevWindow:title() or "nil", tostring(restoreFocus))
  if not a then return end
  a:hide()
  local prev = M._prevWindow
  M._prevWindow = nil
  if restoreFocus and prev and pcall(function() return prev:isVisible() end) then prev:focus() end
end

-- Ctrl+Space: reveal seekr, or hide it again when it already has focus.
function M.show()
  if M._launching then return end -- a window is already on its way
  local focused = hs.window.focusedWindow()
  if focused and focused:title() == "seekr" then
    log("show: seekr focused, hiding")
    M.hide()
    parkResident()
    return
  end
  M._startedAt = hs.timer.secondsSinceEpoch()
  if focused then M._prevWindow = focused end
  -- Where the user is: the focused window's screen, else the mouse's.
  M._screen = (focused and focused:screen()) or hs.mouse.getCurrentScreen()
  if reveal() then return end
  launch()
end

hs.hotkey.bind(M.hotkey[1], M.hotkey[2], M.show)

return M
