#!/bin/sh
# Set up the Ctrl+Space launcher: Hammerspoon config + a "seekr" Terminal profile
# that runs `seekr --resident` directly (no login shell, so no ~/.zshrc delay).
# seekr then stays alive between uses and the hotkey only reveals its window.
# Safe to re-run; re-run it after seekr moves, since the profile stores its path.
# Requires Homebrew for the Hammerspoon install step.
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
SEEKR_CMD="$SEEKR_BIN --resident"

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

# "missing", "ok" (runs this binary directly) or "stale" (older layout or another path).
profile_state() {
  defaults export com.apple.Terminal - 2>/dev/null | python3 -c '
import plistlib, sys
p = plistlib.loads(sys.stdin.buffer.read()).get("Window Settings", {}).get("seekr")
if p is None:
    print("missing")
elif p.get("CommandString") == sys.argv[1] and p.get("RunCommandAsShell") is True:
    print("ok")
else:
    print("stale")' "$SEEKR_CMD"
}

# Terminal only imports profiles from .terminal files, and an import never replaces
# a profile of the same name (it adds "seekr 1"), so a stale one is deleted first.
remove_profile() {
  if pgrep -x Terminal >/dev/null; then
    osascript -e 'tell application "Terminal" to delete settings set "seekr"' >/dev/null
    sleep 1
  else
    TMPP=$(mktemp)
    defaults export com.apple.Terminal - | python3 -c '
import plistlib, sys
p = plistlib.loads(sys.stdin.buffer.read())
p.get("Window Settings", {}).pop("seekr", None)
sys.stdout.buffer.write(plistlib.dumps(p))' > "$TMPP"
    defaults import com.apple.Terminal "$TMPP"
    rm -f "$TMPP"
  fi
}

STATE=$(profile_state)
case "$STATE" in
  ok) echo "Terminal profile 'seekr' up to date ($SEEKR_CMD)" ;;
  *)
    if [ "$STATE" = stale ]; then
      echo "replacing Terminal profile 'seekr'…"
      remove_profile
    fi
    TMP=$(mktemp -d)
    python3 "$HERE/terminal/make-profile.py" "$TMP/seekr.terminal" "$SEEKR_BIN" --resident
    # Importing via `open` registers the profile with Terminal even while it is
    # running. It also opens a window with the profile, i.e. starts a seekr; end
    # that one so its window closes again.
    # Terminal starts the command through login(1), which names the process "-seekr".
    BEFORE=" $(pgrep -x -- 'seekr|-seekr' 2>/dev/null | tr '\n' ' ') "
    open "$TMP/seekr.terminal"
    i=0
    while [ $i -lt 40 ]; do
      sleep 0.1
      i=$((i + 1))
      NEW=""
      for pid in $(pgrep -x -- 'seekr|-seekr' 2>/dev/null); do
        case "$BEFORE" in *" $pid "*) ;; *) NEW="$NEW $pid" ;; esac
      done
      if [ -n "$NEW" ]; then
        sleep 0.3
        kill $NEW 2>/dev/null || true
        sleep 0.3
        rm -f "$HOME/Library/Application Support/seekr/resident.json"
        break
      fi
    done
    rm -rf "$TMP"
    ;;
esac
# Close any idle window still on the seekr profile (e.g. from an older install).
if pgrep -x Terminal >/dev/null; then
  osascript >/dev/null 2>&1 <<'AS' || true
tell application "Terminal"
  repeat with w in (every window)
    try
      if (name of current settings of selected tab of w) is "seekr" and (busy of selected tab of w) is false then close w
    end try
  end repeat
end tell
AS
fi

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
  profile:  Terminal.app › Settings › Profiles › seekr  (runs $SEEKR_CMD)

First time only: macOS will ask you to allow Hammerspoon under
System Settings › Privacy & Security › Accessibility (needed for global hotkeys
and for opening the Terminal profile), and to allow Hammerspoon to control
Terminal (Automation) the first time you press the hotkey.
MSG
