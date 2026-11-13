-- seekr launcher for Hammerspoon.
--
-- Cmd+Space (M.hotkey) shows a small, centered, undecorated kitty window
-- running `seekr --resident`. The first press starts it (a few hundred milliseconds);
-- after that seekr stays alive and Enter/Esc only *hide* the window (seekr asks
-- us via `hs -c "package.loaded.seekr.hide()"`), so every later press just
-- reveals it: the UI is back within a couple of frames and typing works
-- immediately. On reveal we send seekr SIGUSR1, which it takes as "refresh the
-- index in the background". Ctrl+C inside seekr quits it for real, and the
-- window goes with it.
--
-- The hotkey while seekr has focus hides it again, whatever the query says.
-- seekr is told with SIGUSR2 so it keeps the text and shows it selected on the
-- next reveal: typing replaces it, an arrow key keeps it.
--
-- Cmd+Space is Spotlight's shortcut, and while Spotlight holds it the key never
-- reaches Hammerspoon: contrib/install-launcher.sh turns that shortcut off
-- (System Settings › Keyboard › Keyboard Shortcuts › Spotlight turns it back
-- on) and installs another key with --hotkey.
--
-- The window is a kitty *panel* (`kitten panel`, kitty 0.42 or newer), started
-- as a process of its own just for seekr (config in contrib/kitty/seekr.conf),
-- so showing and hiding never touch other terminal windows. A panel is on
-- every desktop (Space) at once, so it comes back on whichever desktop is in
-- front; a regular window stays on the desktop it was created on and macOS
-- switches there when it gets focus (hs.spaces.moveWindowToSpace reports
-- success but does nothing on macOS 15). Showing and hiding go through kitty
-- itself (`kitten panel --single-instance --toggle-visibility`), not through
-- app hide/unhide: macOS puts an all-desktops window that was hidden and shown
-- again back on the desktop it was created on only (kitty issue 8740), and
-- just the process owning the window can re-attach it to the current desktop,
-- which kitty's own show does. kitty's hide also hands activation back to the
-- app that was in front last, so after Esc focus returns where it came from
-- and after Enter the app that was just opened keeps it. Which *screen* the
-- window appears on is our job: the screen of the window that had focus when
-- the hotkey was pressed, or the mouse's screen if none had.
-- Terminal.app was used before kitty; it cannot drop its title bar and hiding
-- one of its windows needed AppleScript (see git history).
--
-- The panel is seekr's alone: kitty's own shortcuts are turned off in its
-- config, since a ⌘T there opens a shell in a tab next to seekr, and a kitty
-- with a second tenant stays up after seekr quits, showing a terminal on
-- every press. Should a panel outlive its seekr anyway, the hotkey ends it and
-- starts a fresh one (see endIfAbandoned).
--
-- Install: contrib/install-launcher.sh (copies this file to ~/.hammerspoon and
-- adds `require("seekr")` to init.lua).

local M = {}

local HOME = os.getenv("HOME")

-- Modifiers and key joined with "+", in hs.hotkey's names: "cmd+space",
-- "ctrl+space", "cmd+shift+space". contrib/install-launcher.sh --hotkey
-- rewrites this line in the installed copy.
M.hotkey = "cmd+space"
-- The seekr binary the window runs. contrib/install-launcher.sh rewrites this
-- line in the installed copy with the one it found; when it is empty, the
-- usual install locations are tried (see M.seekr).
M.seekrPath = ""
M.maxSize = { w = 1100, h = 600 } -- text area in pixels; shrinks on small screens
M.config = HOME .. "/.config/seekr/kitty.conf"
-- Written by `seekr --resident` as {"pid": N, "park": "USR2"}; deleted when it
-- exits. "park" names the signal that tells seekr "the launcher hid you"; older
-- binaries omit it and are simply hidden without being told.
M.stateFile = HOME .. "/Library/Application Support/seekr/resident.json"
-- Written by us when the panel starts, as {"kitty": <pid>, "args": [...]}, and
-- deleted when it exits: a reloaded Hammerspoon finds the panel process again
-- even while its window is hidden, and the options it was started with, which
-- every toggle must repeat (see toggle).
M.launcherFile = HOME .. "/Library/Application Support/seekr/launcher.json"
M.debug = false -- when true, decisions and timings are appended to /tmp/seekr-hs.log

-- kitty runs one panel process per instance group; the toggle addresses it by name.
local INSTANCE_GROUP = "seekr"
-- Seconds a new panel gets to start seekr before a missing resident.json
-- means anything (kitty alone takes about one to open its window).
local STARTUP_GRACE = 10

local function firstExisting(paths)
  for _, p in ipairs(paths) do
    if p ~= "" and hs.fs.attributes(p) then return p end
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
  M.seekrPath,
  HOME .. "/.cargo/bin/seekr",
  HOME .. "/.local/bin/seekr", -- install.sh
  "/opt/homebrew/bin/seekr",
  "/usr/local/bin/seekr",
})

-- Panels run under kitty's background-only quick-access bundle. The plain kitty
-- bundle covers a resident started by an older version of this file.
local BUNDLES = { "net.kovidgoyal.kitty-quick-access", "net.kovidgoyal.kitty" }

M._task = nil       -- hs.task of the kitty process we started
M._panelArgs = nil  -- kitten's arguments for that process; toggles repeat them
M._launching = nil  -- hs.timer while a window is being created or shown; guards against double launches
M._launchedAt = nil -- when the panel was started, for STARTUP_GRACE
M._ending = nil     -- hs.task while a panel without seekr is being checked and ended (see endIfAbandoned)
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

-- "cmd+shift+space" -> { "cmd", "shift" }, "space". Modifier synonyms
-- (command, control, option) are folded to the names hs.eventtap reports.
local MODS = { command = "cmd", control = "ctrl", option = "alt" }
local function parseHotkey(spec)
  local parts = {}
  for part in spec:gmatch("[^+]+") do
    local p = part:match("^%s*(.-)%s*$"):lower()
    parts[#parts + 1] = MODS[p] or p
  end
  local key = table.remove(parts)
  return parts, key
end

local function readLauncherState()
  local f = io.open(M.launcherFile)
  if not f then return nil end
  local s = f:read("*a") or ""
  f:close()
  local ok, state = pcall(hs.json.decode, s)
  return ok and type(state) == "table" and state or nil
end

local function writeLauncherState(pid, args)
  local f = io.open(M.launcherFile, "w")
  if not f then return end
  f:write(hs.json.encode({ kitty = pid, args = args }))
  f:close()
end

-- The kitty process that hosts seekr, as an hs.application, or nil. Found by
-- pid, never by window: while the panel is hidden it has no window to find.
local function app()
  local pid
  if M._task and M._task:isRunning() then
    pid = M._task:pid()
  else
    -- Hammerspoon was reloaded while seekr kept running.
    local state = readLauncherState()
    pid = state and tonumber(state.kitty)
  end
  if not pid then return nil end
  local a = hs.application.applicationForPID(pid)
  if a and hs.fnutils.contains(BUNDLES, a:bundleID() or "") then return a end
  return nil
end

-- Is the panel process we started still up? Not the same as app() ~= nil: for
-- minutes after a wake from hibernation LaunchServices can lose track of
-- running processes, and the lookup fails for a panel that is alive.
local function panelRunning()
  return M._task ~= nil and M._task:isRunning()
end

-- The panel process's pid, or nil when there is none: the one we started, or
-- after a Hammerspoon reload the one launcher.json names, if that is a kitty.
local function panelPid()
  if panelRunning() then return M._task:pid() end
  local a = app()
  return a and a:pid()
end

-- Is `w` the launcher's window? Decided by process, not title: a Finder or
-- VS Code window showing the seekr repo is titled "seekr" as well, and matching
-- on the title made the hotkey hide the (already hidden) launcher instead of
-- showing it.
local function isLauncherWindow(w)
  if not w then return false end
  local a = app()
  return a ~= nil and w:pid() == a:pid()
end

-- The panel's window while it is on screen; nil while kitty has it hidden
-- (accessibility does not list a window that is ordered out).
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

-- kitten's arguments for the panel: the instance group toggles address, how
-- the window is placed and sized (margins are measured from the top left of
-- the screen's usable area; the size is fixed from then on), and the config
-- it reads. kitty compares the options a toggle brings with the panel's and
-- reconfigures it when they differ, so toggles repeat the launch list exactly.
local function panelArgs()
  local s = targetScreen():frame()
  local size = textSize(s)
  local at = topLeft(s, size)
  return {
    "panel",
    "--single-instance",
    "--instance-group=" .. INSTANCE_GROUP,
    "--edge=none",
    "--margin-left=" .. math.floor(at.x - s.x),
    "--margin-top=" .. math.floor(at.y - s.y),
    "--columns=" .. size.w .. "px",
    "--lines=" .. size.h .. "px",
    "--layer=top",
    "--focus-policy=on-demand",
    "--config=" .. M.config,
  }
end

-- The list the running panel was started with: kept in memory, read back
-- from launcher.json after a Hammerspoon reload, recomputed as a last resort.
local function launchArgs()
  if not M._panelArgs then
    local state = readLauncherState()
    M._panelArgs = (state and type(state.args) == "table" and #state.args > 0) and state.args or panelArgs()
  end
  return M._panelArgs
end

-- Ask the running panel to show its window if hidden, or hide it if shown
-- (kitty only offers the flip; see reveal and hide for keeping the two in
-- step). The command line is the launch one plus the flag: should no panel be
-- running after all, this starts a proper one instead of a panel with a shell.
local function toggle()
  local args = {}
  for _, v in ipairs(launchArgs()) do args[#args + 1] = v end
  args[#args + 1] = "--toggle-visibility"
  args[#args + 1] = M.seekr
  args[#args + 1] = "--resident"
  hs.task.new(M.kitten, nil, args):start()
end

-- kitty creates and shows its window asynchronously: poll `pick` until it
-- returns one, then place and focus it. `settle` re-places it once more a
-- moment later, for a window kitty places itself after creating it.
local function awaitWindow(pick, settle)
  local tries = 0
  M._launching = hs.timer.doEvery(0.01, function()
    tries = tries + 1
    local w = pick()
    if w or tries > 300 then
      M._launching:stop()
      M._launching = nil
      log("awaitWindow: %s after %d tries", w and "adopting" or "gave up", tries)
      if w then
        adopt(w)
        if settle then hs.timer.doAfter(0.4, function() place(w) end) end
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
  -- asked for the spot adopt() would pick; place() then runs once more after
  -- kitty is done, which matters when that is not the screen the user is on.
  -- The window level (regular, not the desktop-widget level panels default
  -- to) is set in the kitty config.
  M._panelArgs = panelArgs()
  local args = {}
  for _, v in ipairs(M._panelArgs) do args[#args + 1] = v end
  args[#args + 1] = M.seekr
  args[#args + 1] = "--resident"
  local task
  task = hs.task.new(M.kitten, function()
    -- A panel that was ended and replaced (see endIfAbandoned) may report in
    -- after its successor started; the state is the successor's by then.
    if M._task ~= task then return end
    M._task = nil
    os.remove(M.launcherFile)
    -- A panel of this instance group was already running: kitty handed our
    -- command line to it and this process quit at once. That panel is not
    -- ours to find, so say so instead of adding a window to it per press.
    if M._launching then
      hs.alert.show("seekr: another launcher panel is already running; quit it (Ctrl+C in it) or log out")
    end
  end, args)
  M._task = task
  M._task:start()
  M._launchedAt = hs.timer.secondsSinceEpoch()
  writeLauncherState(M._task:pid(), M._panelArgs)
  log("launch: kitten pid %s", tostring(M._task:pid()))
  awaitWindow(function()
    local a = app()
    return a and windowOf(a)
  end, true)
end

-- Bring the window back. Returns false if there is no panel process.
local function reveal()
  local a = app()
  if not a and not panelRunning() then return false end
  wakeResident()
  if not a then
    -- Up, but out of sight of LaunchServices (see panelRunning). launch() would
    -- hand kitty a second panel and lose track of this one for good, so flip
    -- it blind: when the hotkey is pressed it is nearly always hidden.
    toggle()
    return true
  end
  local w = windowOf(a)
  if w then
    -- Still on screen (the user clicked elsewhere): only the focus is missing.
    adopt(w)
    return true
  end
  toggle()
  awaitWindow(function() return windowOf(a) end, false)
  return true
end

-- The pid of a panel that may have lost its seekr, or nil: it is up and past
-- starting, yet no resident advertises itself. seekr deletes resident.json
-- when it quits, and kitty closes the window with it, unless kitty has
-- something else open there (a shell in a tab of its own, say): then the
-- panel lives on showing only that, and a toggle would bring up a terminal.
local function suspectPanel()
  if residentState() then return nil end
  if M._launchedAt and hs.timer.secondsSinceEpoch() - M._launchedAt < STARTUP_GRACE then return nil end
  return panelPid()
end

-- For such a panel: make sure, and if seekr is indeed gone, end the panel and
-- wait until it is. $1 is its pid, $2 the instance group. Sure means the
-- process list agrees: the pid is the launcher's panel and none of its
-- children is `seekr --resident` (a seekr whose resident.json went missing
-- stays). Exits 0 once the panel is gone, 3 when it is not ours to end, 4 if
-- it would not go.
local END_ABANDONED_PANEL = [[
case "$(/bin/ps -ww -o command= -p "$1" 2>/dev/null) " in
  *" --instance-group=$2 "* | *" --instance-group $2 "*) ;;
  *) exit 3 ;;
esac
/usr/bin/pgrep -P "$1" -f -- ' --resident$' >/dev/null
[ $? -eq 1 ] || exit 3
kill -TERM "$1" 2>/dev/null
n=0
while kill -0 "$1" 2>/dev/null; do
  n=$((n + 1))
  if [ "$n" -eq 20 ]; then kill -KILL "$1" 2>/dev/null; fi
  if [ "$n" -ge 50 ]; then exit 4; fi
  /bin/sleep 0.1
done
exit 0
]]

-- Run that check on panel `pid` and call `done(ended)`: true when the panel
-- had no seekr and is gone now (two panels of one instance group would be
-- one kitty process, so the next may only start then), false when it stays.
local function endIfAbandoned(pid, done)
  log("show: panel %d advertises no seekr, checking", pid)
  local task = hs.task.new("/bin/sh", function(code)
    M._ending = nil
    log("show: panel %d %s (check exited %s)", pid, code == 0 and "had no seekr, ended" or "stays", tostring(code))
    done(code == 0)
  end, { "-c", END_ABANDONED_PANEL, "sh", tostring(pid), INSTANCE_GROUP })
  if task and task:start() then
    M._ending = task
  else
    done(false)
  end
end

-- Called by seekr (via the `hs` CLI) after Enter/Esc: hide the window, keep the
-- process. kitty hands activation to the app that was in front last, which
-- after Esc is the one the user came from and after Enter the one `open` just
-- brought forward. Focus goes back to the exact window that was in front
-- before seekr unless restoreFocus is false: seekr passes that after opening
-- something, so the app just opened is not pushed behind that window a moment
-- later. No argument (Esc, the hotkey toggle, older binaries) means restore.
function M.hide(restoreFocus)
  if restoreFocus == nil then restoreFocus = true end
  local a = app()
  log("hide: app=%s prev=%s restore=%s", a and a:pid() or "nil",
    M._prevWindow and M._prevWindow:title() or "nil", tostring(restoreFocus))
  if not a then
    -- Out of sight of LaunchServices (see panelRunning); seekr only asks from
    -- a window that is showing, so a blind flip hides it.
    if panelRunning() then toggle() end
    return
  end
  -- Already hidden (a second Esc, say)? A toggle would bring it back.
  if windowOf(a) then toggle() end
  local prev = M._prevWindow
  M._prevWindow = nil
  -- pcall: prev's process may be gone.
  local ok, visible = pcall(function() return prev and prev:isVisible() end)
  if restoreFocus and ok and visible then prev:focus() end
end

-- The hotkey: reveal seekr, or hide it again when it already has focus.
function M.show()
  if M._launching or M._ending then return end -- a window is already on its way
  local focused = hs.window.focusedWindow()
  local onLauncher = isLauncherWindow(focused)
  local function hideAgain()
    log("show: seekr focused, hiding")
    M.hide()
    parkResident()
  end
  local suspect = suspectPanel()
  if onLauncher and not suspect then return hideAgain() end
  M._startedAt = hs.timer.secondsSinceEpoch()
  if focused and not onLauncher then M._prevWindow = focused end
  -- Where the user is: the focused window's screen, else the mouse's.
  M._screen = (focused and focused:screen()) or hs.mouse.getCurrentScreen()
  if not suspect then
    if not reveal() then launch() end
    return
  end
  endIfAbandoned(suspect, function(ended)
    if ended then
      launch() -- whatever that panel was showing, in front or not, the key asked for seekr
    elseif onLauncher then
      hideAgain()
    elseif not reveal() then
      launch()
    end
  end)
end

-- hs.hotkey registers the key with the system exclusively, which fails
-- (RegisterEventHotKey -9878 in the console) when any other app already
-- holds the same combination as a shared hotkey, even one that never
-- visibly reacts to it. Then watch for the key with an event tap instead: it
-- sees the press before that app and swallows it, at the cost of Hammerspoon
-- glancing at every key down.
local function bindHotkey()
  local mods, key = parseHotkey(M.hotkey)
  M._hotkey = hs.hotkey.bind(mods, key, M.show)
  if M._hotkey then return end
  local keycode = hs.keycodes.map[key]
  if not keycode then
    hs.alert.show("seekr: unknown key in hotkey " .. M.hotkey)
    return
  end
  local want = {}
  for _, m in ipairs(mods) do want[m] = true end
  local autorepeat = hs.eventtap.event.properties.keyboardEventAutorepeat
  M._tap = hs.eventtap.new({ hs.eventtap.event.types.keyDown }, function(e)
    if e:getKeyCode() ~= keycode or e:getProperty(autorepeat) ~= 0 then return false end
    local flags = e:getFlags()
    for _, m in ipairs({ "cmd", "ctrl", "alt", "shift" }) do
      if (flags[m] or false) ~= (want[m] or false) then return false end
    end
    M.show()
    return true
  end)
  M._tap:start()
  print(string.format("seekr: %s is held by another app as a shared hotkey; watching for it with an event tap instead", M.hotkey))
end
bindHotkey()

return M
