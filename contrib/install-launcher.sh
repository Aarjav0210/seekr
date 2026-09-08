#!/bin/sh
# Set up the Ctrl+Space launcher: Hammerspoon config + a "seekr" Terminal profile.
# Safe to re-run. Requires Homebrew for the Hammerspoon install step.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)

if [ ! -d /Applications/Hammerspoon.app ]; then
  echo "installing Hammerspoon…"
  brew install --cask hammerspoon
fi

mkdir -p "$HOME/.hammerspoon"
cp "$HERE/hammerspoon/seekr.lua" "$HOME/.hammerspoon/seekr.lua"
INIT="$HOME/.hammerspoon/init.lua"
touch "$INIT"
for line in 'hs.autoLaunch(true)' 'hs.allowAppleScript(true)' 'require("hs.ipc")' 'require("seekr")'; do
  grep -qF "$line" "$INIT" || printf '%s\n' "$line" >> "$INIT"
done

has_profile() {
  defaults export com.apple.Terminal - 2>/dev/null | python3 -c \
    'import plistlib,sys; sys.exit(0 if "seekr" in plistlib.loads(sys.stdin.buffer.read()).get("Window Settings",{}) else 1)'
}
if has_profile; then
  echo "Terminal profile 'seekr' already present"
else
  TMP=$(mktemp -d)
  python3 "$HERE/terminal/make-profile.py" "$TMP/seekr.terminal"
  # Importing via `open` registers the profile with Terminal even while it is running.
  # It also opens a window using the profile; close that one again.
  open "$TMP/seekr.terminal"
  sleep 1
  rm -rf "$TMP"
fi
osascript >/dev/null 2>&1 <<'AS' || true
tell application "Terminal"
  repeat with w in (every window)
    try
      if (name of current settings of selected tab of w) is "seekr" and (busy of selected tab of w) is false then close w
    end try
  end repeat
end tell
AS

# (Re)start Hammerspoon so it picks up the config. The `hs` CLI is avoided on
# purpose: Gatekeeper kills it while the app is still quarantined.
if pgrep -x Hammerspoon >/dev/null; then
  osascript -e 'tell application "Hammerspoon" to quit' >/dev/null 2>&1 || true
  sleep 1
fi
open -a Hammerspoon
sleep 2
# Ask for Accessibility if it has not been granted yet (opens the system prompt).
osascript -e 'tell application "Hammerspoon" to execute lua code "hs.accessibilityState(true)"' >/dev/null 2>&1 || true

cat <<MSG

seekr launcher installed.
  hotkey:   ctrl+space
  config:   ~/.hammerspoon/seekr.lua
  profile:  Terminal.app › Settings › Profiles › seekr

First time only: macOS will ask you to allow Hammerspoon under
System Settings › Privacy & Security › Accessibility (needed for global hotkeys),
and to allow Hammerspoon to control Terminal (Automation) the first time you press the hotkey.
MSG
