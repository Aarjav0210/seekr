-- seekr launcher for Hammerspoon.
--
-- Ctrl+Space pops a small, centered Terminal window running `seekr`. The window
-- closes by itself when seekr exits, because the "seekr" Terminal profile is set
-- to close on shell exit and the command is `exec seekr`. Pressing the hotkey
-- while the window is already open just focuses it.
--
-- Install: contrib/install-launcher.sh (copies this file to ~/.hammerspoon and
-- adds `require("seekr")` to init.lua).

local M = {}

M.hotkey = { { "ctrl" }, "space" }
M.profile = "seekr"              -- Terminal.app profile name
M.command = "exec seekr"         -- exec: the shell is replaced, so exit closes the window
M.maxSize = { w = 1100, h = 600 } -- pixels; shrinks on small screens

M._windowId = nil -- CGWindowID of the launcher window, captured when it is created

local function findWindow()
  if M._windowId then
    local w = hs.window.get(M._windowId)
    if w then return w end
    M._windowId = nil
  end
  -- Fallback: match the title the launcher's tab ends up with.
  local term = hs.application.get("Terminal")
  if not term then return nil end
  for _, w in ipairs(term:allWindows()) do
    local t = w:title() or ""
    if t:sub(1, 5) == "seekr" or t:find("exec seekr", 1, true) then return w end
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

function M.show()
  local existing = findWindow()
  if existing then
    existing:focus()
    return
  end

  -- `launch` starts Terminal without its startup window if it is not running,
  -- so we never leave a stray empty shell behind.
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
  M._windowId = tonumber(result)

  -- Terminal creates the window asynchronously; wait for it, then place and focus it.
  local tries = 0
  local timer
  timer = hs.timer.doEvery(0.05, function()
    tries = tries + 1
    local w = findWindow()
    if w then
      place(w)
      w:focus()
      timer:stop()
    elseif tries > 60 then
      timer:stop()
    end
  end)
end

hs.hotkey.bind(M.hotkey[1], M.hotkey[2], M.show)

return M
