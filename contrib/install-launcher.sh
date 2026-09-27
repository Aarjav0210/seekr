#!/bin/sh
# Set up the Cmd+Space launcher: kitty (a panel window, so it is on every
# desktop) + Hammerspoon config. kitty runs `seekr --resident` directly (no login
# shell, so no ~/.zshrc delay); seekr then stays alive between uses and the
# hotkey only reveals its window.
#
#   contrib/install-launcher.sh [--hotkey cmd+space]
#
# --hotkey takes modifiers and key joined with "+" (hs.hotkey names: cmd, ctrl,
# alt, shift; keys as in hs.keycodes.map). Cmd+Space is Spotlight's shortcut
# and, on macOS 15, Siri's (hold), so with that hotkey the script turns both
# shortcuts off (nothing else about Spotlight or Siri changes) and gives them
# back when a later run picks another key. Re-running keeps the hotkey last
# installed unless --hotkey says otherwise.
# Safe to re-run. Requires Homebrew for the kitty/Hammerspoon install steps.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
LUA="$HOME/.hammerspoon/seekr.lua"
DEFAULT_HOTKEY=cmd+space

HOTKEY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --hotkey)
      [ $# -ge 2 ] || { echo "--hotkey needs a value, e.g. --hotkey ctrl+space" >&2; exit 2; }
      HOTKEY=$2; shift 2 ;;
    --hotkey=*) HOTKEY=${1#--hotkey=}; shift ;;
    -h|--help) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done
# Without --hotkey, keep what an earlier run installed.
if [ -z "$HOTKEY" ] && [ -f "$LUA" ]; then
  HOTKEY=$(sed -n 's/^M\.hotkey = "\([^"]*\)"$/\1/p' "$LUA")
fi
HOTKEY=${HOTKEY:-$DEFAULT_HOTKEY}
HOTKEY=$(printf '%s' "$HOTKEY" | tr 'A-Z' 'a-z')
case "$HOTKEY" in
  ''|*[!a-z0-9+]*) echo "hotkey: letters, digits and '+' only, e.g. cmd+space (got '$HOTKEY')" >&2; exit 2 ;;
esac

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
# The window is a `kitten panel` shown and hidden with `--toggle-visibility`,
# which needs kitty 0.42 or newer.
KITTEN=""
for k in /Applications/kitty.app/Contents/MacOS/kitten "$HOME/Applications/kitty.app/Contents/MacOS/kitten"; do
  if [ -x "$k" ]; then KITTEN=$k; break; fi
done
if [ -z "$KITTEN" ] || ! "$KITTEN" panel --help 2>/dev/null | grep -q -- '--toggle-visibility'; then
  echo "kitty 0.42 or newer is needed (kitten panel --toggle-visibility); upgrade with:  brew upgrade --cask kitty" >&2
  exit 1
fi
if [ ! -d /Applications/Hammerspoon.app ]; then
  echo "installing Hammerspoon…"
  brew install --cask hammerspoon
fi

mkdir -p "$HOME/.config/seekr"
cp "$HERE/kitty/seekr.conf" "$HOME/.config/seekr/kitty.conf"

mkdir -p "$HOME/.hammerspoon"
cp "$HERE/hammerspoon/seekr.lua" "$LUA"
# The hotkey is one line of the Lua; put the chosen one into the installed copy.
sed -i '' "s/^M\.hotkey = .*/M.hotkey = \"$HOTKEY\"/" "$LUA"
grep -q "^M\.hotkey = \"$HOTKEY\"\$" "$LUA"
INIT="$HOME/.hammerspoon/init.lua"
touch "$INIT"
for line in 'hs.autoLaunch(true)' 'hs.allowAppleScript(true)' 'require("hs.ipc")' 'require("seekr")'; do
  grep -qF "$line" "$INIT" || printf '%s\n' "$line" >> "$INIT"
done

# Cmd+Space belongs to Spotlight until its shortcut ("Show Spotlight search",
# symbolic hotkey 64) is turned off; Hammerspoon's binding never fires while it
# is on. Turning it off is recorded so that a later run with another hotkey
# gives the key back, while a shortcut the user had already turned off is left
# alone. `activateSettings -u` makes the running session pick the change up;
# without it the change waits for a logout.
DATA="$HOME/Library/Application Support/seekr"
SPOT_MARK="$DATA/spotlight-cmd-space-off"
spotlight_shortcut() { # on|off
  if [ "$1" = on ]; then en=1; else en=0; fi
  defaults write com.apple.symbolichotkeys AppleSymbolicHotKeys -dict-add 64 \
    "<dict><key>enabled</key><integer>$en</integer><key>value</key><dict><key>parameters</key><array><integer>65535</integer><integer>49</integer><integer>1048576</integer></array><key>type</key><string>standard</string></dict></dict>"
  /System/Library/PrivateFrameworks/SystemAdministration.framework/Resources/activateSettings -u >/dev/null 2>&1 || true
}
spotlight_shortcut_is_on() {
  v=$(defaults export com.apple.symbolichotkeys - 2>/dev/null \
      | plutil -extract 'AppleSymbolicHotKeys.64.enabled' raw -o - - 2>/dev/null || echo 1)
  case "$v" in 0|false) return 1 ;; *) return 0 ;; esac
}
mkdir -p "$DATA"
if [ "$HOTKEY" = cmd+space ]; then
  if spotlight_shortcut_is_on; then
    spotlight_shortcut off
    touch "$SPOT_MARK"
    echo "Spotlight's Cmd+Space shortcut turned off (System Settings › Keyboard › Keyboard Shortcuts › Spotlight turns it back on)."
  fi
elif [ -f "$SPOT_MARK" ]; then
  spotlight_shortcut on
  rm -f "$SPOT_MARK"
  echo "Cmd+Space given back to Spotlight."
fi
# Siri's keyboard shortcut defaults to "hold Cmd+Space" on macOS 15: a second
# claim on the key, handled the same way. Only the enabled flag is changed, so
# System Settings › Siri › Keyboard shortcut shows the old choice once it is on
# again; Siri reads the flag at its next start (a logout does it for sure).
SIRI_MARK="$DATA/siri-cmd-space-off"
siri_pref() { # <plutil key path> of com.apple.Siri KeyboardShortcutPreSAE, or ""
  defaults export com.apple.Siri - 2>/dev/null \
    | plutil -extract "KeyboardShortcutPreSAE.$1" raw -o - - 2>/dev/null || true
}
siri_hold_cmd_space_is_on() {
  case "$(siri_pref enabled)" in 1|true) ;; *) return 1 ;; esac
  m=$(siri_pref value.parameters.2)
  [ "$(siri_pref value.parameters.1)" = 49 ] && [ -n "$m" ] && [ $((m & 1048576)) -ne 0 ]
}
siri_shortcut() { # on|off
  if [ "$1" = on ]; then en=1; else en=0; fi
  defaults write com.apple.Siri KeyboardShortcutPreSAE -dict-add enabled "<integer>$en</integer>"
}
if [ "$HOTKEY" = cmd+space ]; then
  if siri_hold_cmd_space_is_on; then
    siri_shortcut off
    touch "$SIRI_MARK"
    echo "Siri's hold-Cmd+Space shortcut turned off (System Settings › Siri › Keyboard shortcut turns it back on)."
  fi
elif [ -f "$SIRI_MARK" ]; then
  siri_shortcut on
  rm -f "$SIRI_MARK"
  echo "Hold-Cmd+Space given back to Siri."
fi

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
# End a resident seekr from a previous install too, so the next hotkey press
# starts the freshly installed binary (its kitty window closes with it). seekr
# itself first, by the pid it advertises: ending kitty before seekr leaves
# seekr spinning on a dead pty. The pkill catches one without a state file; it
# must match seekr's whole command line only, since kitty's ends the same way.
STATE="$DATA/resident.json"
if [ -f "$STATE" ]; then
  RESIDENT=$(sed -n 's/.*"pid": *\([0-9][0-9]*\).*/\1/p' "$STATE")
  [ -n "$RESIDENT" ] && kill -TERM "$RESIDENT" 2>/dev/null && sleep 1 || true
fi
pkill -TERM -x -f -- '[^ ]*/seekr --resident' 2>/dev/null || true
rm -f "$STATE" "$DATA/launcher.json"

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
# How the key is watched: "hotkey" (registered with the system), "tap" (another
# app holds the key as a shared hotkey, so an event tap watches for it; see the
# Lua) or "none" (Accessibility not granted yet, or an unknown key name).
HOW=$(osascript -e 'tell application "Hammerspoon" to execute lua code "local M = package.loaded.seekr; return M and ((M._hotkey and \"hotkey\") or (M._tap and \"tap\")) or \"none\""' 2>/dev/null || echo "unknown")

cat <<MSG

seekr launcher installed.
  hotkey:    $HOTKEY (again while seekr is in front: hide, query kept); watched as: $HOW
  change it: contrib/install-launcher.sh --hotkey ctrl+space
  config:    ~/.hammerspoon/seekr.lua, ~/.config/seekr/kitty.conf
  runs:      $SEEKR_BIN --resident (inside a kitty panel)

First time only: macOS will ask you to allow Hammerspoon under
System Settings › Privacy & Security › Accessibility (global hotkeys, window placement).
MSG
