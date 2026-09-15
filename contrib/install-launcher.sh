#!/bin/sh
# Set up the Ctrl+Space launcher: kitty (a panel window, so it is on every
# desktop) + Hammerspoon config. kitty runs `seekr --resident` directly (no login
# shell, so no ~/.zshrc delay); seekr then stays alive between uses and the
# hotkey only reveals its window.
# Safe to re-run. Requires Homebrew for the kitty/Hammerspoon install steps.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)

SEEKR_BIN=$(command -v seekr 2>/dev/null || true)
if [ -z "$SEEKR_BIN" ] && [ -x "$HOME/.cargo/bin/seekr" ]; then
  SEEKR_BIN="$HOME/.cargo/bin/seekr"
fi
if [ -z "$SEEKR_BIN" ]; then
  echo "seekr binary not found; install it first:  cargo install --path crates/seekr" >&2
  exit 1
fi

if [ ! -d /Applications/kitty.app ] && [ ! -d "$HOME/Applications/kitty.app" ]; then
  echo "installing kitty…"
  brew install --cask kitty
fi
# The window is a `kitten panel`, which needs kitty 0.42 or newer.
KITTEN=""
for k in /Applications/kitty.app/Contents/MacOS/kitten "$HOME/Applications/kitty.app/Contents/MacOS/kitten"; do
  if [ -x "$k" ]; then KITTEN=$k; break; fi
done
if [ -z "$KITTEN" ] || ! "$KITTEN" panel --help >/dev/null 2>&1; then
  echo "kitty 0.42 or newer is needed (kitten panel); upgrade with:  brew upgrade --cask kitty" >&2
  exit 1
fi
if [ ! -d /Applications/Hammerspoon.app ]; then
  echo "installing Hammerspoon…"
  brew install --cask hammerspoon
fi

mkdir -p "$HOME/.config/seekr"
cp "$HERE/kitty/seekr.conf" "$HOME/.config/seekr/kitty.conf"

mkdir -p "$HOME/.hammerspoon"
cp "$HERE/hammerspoon/seekr.lua" "$HOME/.hammerspoon/seekr.lua"
INIT="$HOME/.hammerspoon/init.lua"
touch "$INIT"
for line in 'hs.autoLaunch(true)' 'hs.allowAppleScript(true)' 'require("hs.ipc")' 'require("seekr")'; do
  grep -qF "$line" "$INIT" || printf '%s\n' "$line" >> "$INIT"
done

# Retire the Terminal.app-based launcher from earlier versions: end any resident
# seekr it started (its window closes with it) and drop the profile it created.
pkill -TERM -x -- '-seekr' 2>/dev/null || true
if pgrep -x Terminal >/dev/null; then
  defaults export com.apple.Terminal - 2>/dev/null | python3 -c '
import plistlib, sys
p = plistlib.loads(sys.stdin.buffer.read()).get("Window Settings", {}).get("seekr", {})
sys.exit(0 if "seekr" in str(p.get("CommandString", "")) else 1)' \
    && osascript -e 'tell application "Terminal" to delete settings set "seekr"' >/dev/null 2>&1 \
    && echo "removed the old Terminal profile 'seekr'" || true
fi
# End a resident seekr from a previous install too, so the next Ctrl+Space
# starts the freshly installed binary (its kitty window closes with it). seekr
# itself first, by the pid it advertises: ending kitty before seekr leaves
# seekr spinning on a dead pty. The pkill catches one without a state file.
STATE="$HOME/Library/Application Support/seekr/resident.json"
if [ -f "$STATE" ]; then
  RESIDENT=$(sed -n 's/.*"pid": *\([0-9][0-9]*\).*/\1/p' "$STATE")
  [ -n "$RESIDENT" ] && kill -TERM "$RESIDENT" 2>/dev/null && sleep 1 || true
fi
pkill -TERM -f -- 'seekr --resident' 2>/dev/null || true
rm -f "$STATE"

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
  hotkey:    ctrl+space (again while seekr is in front: hide, query kept)
  config:    ~/.hammerspoon/seekr.lua, ~/.config/seekr/kitty.conf
  runs:      $SEEKR_BIN --resident (inside a kitty panel)

First time only: macOS will ask you to allow Hammerspoon under
System Settings › Privacy & Security › Accessibility (global hotkeys, window placement).
MSG
