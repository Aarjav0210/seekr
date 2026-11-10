#!/bin/sh
# Install seekr on macOS:
#
#   curl -fsSL https://raw.githubusercontent.com/Aarjav0210/seekr/main/install.sh | sh
#
# Downloads the latest release and installs seekr.app into ~/Applications. The
# app is both the Cmd+Space launcher (it runs in the background, started at
# login, and needs no other app) and the `seekr` command, which is linked into
# ~/.local/bin and that folder put on PATH. Running this again upgrades. The
# options are in usage() below.
#
# The release archive is built by contrib/package.sh (.github/workflows/release.yml
# on every v* tag). This script is read from main while the archive comes from
# the latest release, so it only relies on the archive's layout. Archives from
# before seekr.app (1.2.1 and older) install their command, without a launcher.
#
# Everything runs inside main(), so `curl | sh` has read the whole script before
# anything starts: a command reading stdin would otherwise eat the rest of it.
set -eu

REPO=Aarjav0210/seekr
ASSET=seekr-macos-universal.tar.gz
INSTALL_CMD="curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s --"
# Ends the line this script adds to the shell's startup file; --uninstall
# removes the lines that end with it.
PATH_MARK="# added by the seekr installer"
# seekr.app's bundle identifier: its defaults domain and its launchd label.
APP_ID=io.github.aarjav0210.seekr
DATA="$HOME/Library/Application Support/seekr"
AGENT="$HOME/Library/LaunchAgents/$APP_ID.plist"
LOG="$HOME/Library/Logs/seekr.log"

usage() {
  cat <<USAGE
Install or upgrade seekr: $INSTALL_CMD [options]

  --no-launcher   just the seekr command, without the Cmd+Space launcher
  --yes           set up the launcher without asking
  --hotkey KEY    the launcher's key instead of Cmd+Space, e.g. ctrl+space
                  (implies --yes; later runs keep it)
  --version TAG   install the release TAG, e.g. v1.2.1, instead of the latest
  --uninstall     remove seekr, the launcher, the index and the settings, and
                  give Cmd+Space back to Spotlight

SEEKR_APP_DIR puts seekr.app somewhere other than ~/Applications; SEEKR_BIN_DIR
links the command from somewhere other than ~/.local/bin; SEEKR_ARCHIVE
installs from a local $ASSET instead of downloading it.
USAGE
}

say() { printf '%s\n' "$*"; }
die() { printf 'seekr install: %s\n' "$*" >&2; exit 1; }

# With `curl | sh` stdin is the script, so questions go to the terminal itself.
# The subshell matters: a failed redirection on `:` ends a POSIX shell (dash).
has_tty() { (: </dev/tty) 2>/dev/null; }
ask() { # <question>; succeeds on yes, the default
  printf '%s [Y/n] ' "$1" >/dev/tty
  read -r answer </dev/tty || answer=n
  case "$answer" in '' | y | Y | yes | Yes | YES) return 0 ;; *) return 1 ;; esac
}

fetch() { # <url> <file>
  curl -fsSL --proto '=https' --retry 3 -o "$2" "$1"
}

shell_rc() { # the startup file of the user's shell, or nothing
  case "$(basename "${SHELL:-/bin/zsh}")" in
    zsh) say "${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash) say "$HOME/.bash_profile" ;;
  esac
}

add_to_path() {
  case ":$PATH:" in *":$BIN_DIR:"*) return ;; esac
  rc=$(shell_rc)
  if [ -z "$rc" ]; then
    say "add $BIN_DIR to PATH to run seekr from a terminal"
    return
  fi
  if [ -f "$rc" ] && grep -qF "$PATH_MARK" "$rc"; then return; fi
  if [ "$BIN_DIR" = "$HOME/.local/bin" ]; then dir='$HOME/.local/bin'; else dir=$BIN_DIR; fi
  printf '\nexport PATH="%s:$PATH" %s\n' "$dir" "$PATH_MARK" >>"$rc"
  say "added $BIN_DIR to PATH in $rc"
  NEW_TERMINAL=1
}

remove_from_path() {
  for rc in "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.bash_profile"; do
    if [ -f "$rc" ] && grep -qF "$PATH_MARK" "$rc"; then
      # Written back in place, so a symlinked dotfile stays a symlink.
      kept=$(grep -vF "$PATH_MARK" "$rc" || true)
      printf '%s\n' "$kept" >"$rc"
      say "removed $BIN_DIR from PATH in $rc"
    fi
  done
}

# Cmd+Space belongs to Spotlight until its shortcut ("Show Spotlight search",
# symbolic hotkey 64) is turned off; the launcher's hotkey never fires while it
# is on. Turning it off is recorded so that a later run with another hotkey
# gives the key back, while a shortcut the user had already turned off is left
# alone. `activateSettings -u` makes the running session pick the change up;
# without it the change waits for a logout.
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
take_cmd_space() {
  mkdir -p "$DATA"
  if spotlight_shortcut_is_on; then
    spotlight_shortcut off
    touch "$SPOT_MARK"
    say "Spotlight's Cmd+Space shortcut turned off (System Settings › Keyboard › Keyboard Shortcuts › Spotlight turns it back on)."
  fi
  if siri_hold_cmd_space_is_on; then
    siri_shortcut off
    touch "$SIRI_MARK"
    say "Siri's hold-Cmd+Space shortcut turned off (System Settings › Siri › Keyboard shortcut turns it back on)."
  fi
}
# Give back what an earlier run with Cmd+Space turned off.
give_back_shortcuts() {
  if [ -f "$SPOT_MARK" ]; then
    spotlight_shortcut on
    rm -f "$SPOT_MARK"
    say "Cmd+Space given back to Spotlight."
  fi
  if [ -f "$SIRI_MARK" ]; then
    siri_shortcut on
    rm -f "$SIRI_MARK"
    say "Hold-Cmd+Space given back to Siri."
  fi
}

# The launcher's key: what --hotkey saved, else what the launcher of 1.2.1 and
# older used (its Lua held it), else Cmd+Space.
current_hotkey() {
  defaults read "$APP_ID" hotkey 2>/dev/null ||
    sed -n 's/^M\.hotkey = "\([^"]*\)"$/\1/p' "$HOME/.hammerspoon/seekr.lua" 2>/dev/null | grep . ||
    say cmd+space
}

# The launcher is seekr.app run by launchd: at login, and again whenever it
# crashes (a clean quit stays quit). What it prints goes to ~/Library/Logs/seekr.log.
write_agent() {
  mkdir -p "$(dirname "$AGENT")" "$(dirname "$LOG")"
  cat >"$AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$APP_ID</string>
	<key>ProgramArguments</key>
	<array>
		<string>$APP/Contents/MacOS/seekr</string>
		<string>app</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>LimitLoadToSessionType</key>
	<string>Aqua</string>
	<key>StandardOutPath</key>
	<string>$LOG</string>
	<key>StandardErrorPath</key>
	<string>$LOG</string>
</dict>
</plist>
PLIST
  plutil -lint -s "$AGENT" || die "wrote a broken $AGENT"
}

start_launcher() {
  launchctl bootstrap "gui/$(id -u)" "$AGENT" 2>/dev/null ||
    { sleep 1; launchctl bootstrap "gui/$(id -u)" "$AGENT"; } ||
    die "launchd would not start $AGENT"
}

# The running launcher ends before its app is replaced or removed: launchd's
# copy, and one opened by hand (Finder, `open`), which launchd does not know.
stop_launcher() {
  launchctl bootout "gui/$(id -u)/$APP_ID" 2>/dev/null || true
  pkill -x -f -- '[^ ]*/seekr\.app/Contents/MacOS/seekr( app.*)?' 2>/dev/null || true
}

# seekr 1.2.1 and older ran the launcher in a kitty panel that Hammerspoon
# started; seekr.app replaces both. Undo what their contrib/install-launcher.sh
# set up. kitty and Hammerspoon themselves stay installed.
retire_old_launcher() {
  lua="$HOME/.hammerspoon/seekr.lua"
  init="$HOME/.hammerspoon/init.lua"
  [ -f "$lua" ] || [ -f "$DATA/resident.json" ] || return 0
  # The old resident seekr first, by the pid it advertised. Once its kitty is
  # gone a resident from before 1.2.1 spins and ignores SIGTERM, hence SIGKILL.
  if [ -f "$DATA/resident.json" ]; then
    pid=$(sed -n 's/.*"pid": *\([0-9][0-9]*\).*/\1/p' "$DATA/resident.json")
    [ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null && sleep 1 || true
  fi
  pkill -TERM -x -f -- '[^ ]*/seekr --resident' 2>/dev/null && sleep 1 || true
  pkill -KILL -x -f -- '[^ ]*/seekr --resident' 2>/dev/null || true
  rm -f "$lua" "${XDG_CONFIG_HOME:-$HOME/.config}/seekr/kitty.conf" "$DATA/resident.json" "$DATA/launcher.json"
  if [ -f "$init" ] && grep -qxF 'require("seekr")' "$init"; then
    # Written back in place, so a symlinked init.lua stays a symlink.
    kept=$(grep -vxF 'require("seekr")' "$init" || true)
    printf '%s\n' "$kept" >"$init"
  fi
  # Hammerspoon watches for the old hotkey until it reloads. With nothing of
  # its own left to run (only the lines the old installer added) it is quit;
  # otherwise restarted without seekr.
  if pgrep -x Hammerspoon >/dev/null; then
    osascript -e 'tell application "Hammerspoon" to quit' >/dev/null 2>&1 || true
    sleep 1
    if [ -f "$init" ] && grep -vxE '[[:space:]]*|hs\.autoLaunch\(true\)|hs\.allowAppleScript\(true\)|require\("hs\.ipc"\)' "$init" >/dev/null; then
      open /Applications/Hammerspoon.app 2>/dev/null || true
      say "restarted Hammerspoon without the old seekr launcher"
    else
      say "quit Hammerspoon, which only ran the old seekr launcher"
    fi
  fi
  say "the old kitty + Hammerspoon launcher is gone; if nothing else uses them: brew uninstall --cask kitty hammerspoon"
}

uninstall() {
  stop_launcher
  rm -f "$AGENT"
  retire_old_launcher
  give_back_shortcuts
  rm -rf "$APP"
  if [ -L "$BIN_DIR/seekr" ] || [ -f "$BIN_DIR/seekr" ]; then rm -f "$BIN_DIR/seekr"; fi
  remove_from_path
  defaults delete "$APP_ID" >/dev/null 2>&1 || true
  rm -rf "$DATA" "${XDG_CONFIG_HOME:-$HOME/.config}/seekr"
  say "seekr removed: $APP, $BIN_DIR/seekr, its index (~/Library/Application Support/seekr) and settings (~/.config/seekr)."
  if other=$(command -v seekr 2>/dev/null); then
    say "another seekr is still on PATH: $other"
  fi
}

main() {
  LAUNCHER=ask
  HOTKEY=""
  VERSION=""
  UNINSTALL=""
  NEW_TERMINAL=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --no-launcher) LAUNCHER=no; shift ;;
      -y | --yes) LAUNCHER=yes; shift ;;
      --hotkey)
        [ $# -ge 2 ] || die "--hotkey needs a value, e.g. --hotkey ctrl+space"
        HOTKEY=$2; LAUNCHER=yes; shift 2 ;;
      --hotkey=*) HOTKEY=${1#--hotkey=}; LAUNCHER=yes; shift ;;
      --version)
        [ $# -ge 2 ] || die "--version needs a release tag, e.g. --version v1.2.1"
        VERSION=$2; shift 2 ;;
      --version=*) VERSION=${1#--version=}; shift ;;
      --uninstall) UNINSTALL=1; shift ;;
      -h | --help) usage; exit 0 ;;
      *) die "unknown option: $1 (see --help)" ;;
    esac
  done

  [ "$(uname -s)" = Darwin ] || die "seekr runs on macOS only"
  BIN_DIR=${SEEKR_BIN_DIR:-$HOME/.local/bin}
  APP_DIR=${SEEKR_APP_DIR:-$HOME/Applications}
  for d in "$BIN_DIR" "$APP_DIR"; do
    case "$d" in /*) ;; *) die "SEEKR_BIN_DIR and SEEKR_APP_DIR must be absolute paths" ;; esac
  done
  APP="$APP_DIR/seekr.app"

  if [ -n "$UNINSTALL" ]; then
    uninstall
    return
  fi

  TMP=$(mktemp -d "${TMPDIR:-/tmp}/seekr-install.XXXXXX")
  trap 'rm -rf "$TMP"' EXIT
  if [ -n "${SEEKR_ARCHIVE:-}" ]; then
    cp "$SEEKR_ARCHIVE" "$TMP/$ASSET"
  else
    if [ -n "$VERSION" ]; then
      URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
    else
      URL="https://github.com/$REPO/releases/latest/download/$ASSET"
    fi
    say "downloading $URL"
    fetch "$URL" "$TMP/$ASSET" ||
      die "could not download $URL (is ${VERSION:-a release} published on https://github.com/$REPO/releases?)"
    fetch "$URL.sha256" "$TMP/$ASSET.sha256" || die "could not download $URL.sha256"
    (cd "$TMP" && shasum -a 256 -c "$ASSET.sha256" >/dev/null) ||
      die "$ASSET does not match its checksum; run this again"
  fi
  tar -xzf "$TMP/$ASSET" -C "$TMP" || die "could not unpack $ASSET"
  SRC="$TMP/seekr"

  mkdir -p "$BIN_DIR"
  if [ -d "$SRC/seekr.app" ]; then
    # The running launcher ends first: its app is about to be replaced. The
    # new copy goes next to the old one and is renamed over it. xattr -cr drops
    # a quarantine flag in case the archive came by browser or AirDrop.
    stop_launcher
    mkdir -p "$APP_DIR"
    rm -rf "$APP_DIR/.seekr.app.new"
    ditto "$SRC/seekr.app" "$APP_DIR/.seekr.app.new"
    xattr -cr "$APP_DIR/.seekr.app.new" 2>/dev/null || true
    rm -rf "$APP"
    mv "$APP_DIR/.seekr.app.new" "$APP"
    BIN="$APP/Contents/MacOS/seekr"
    # The command is the app's own binary; this also replaces the copy that
    # 1.2.1 and older put here.
    ln -sfn "$BIN" "$BIN_DIR/seekr"
  elif [ -f "$SRC/seekr" ]; then
    # A release from before seekr.app: its command only.
    cp "$SRC/seekr" "$BIN_DIR/.seekr.new"
    chmod 755 "$BIN_DIR/.seekr.new"
    xattr -c "$BIN_DIR/.seekr.new" 2>/dev/null || true
    mv -f "$BIN_DIR/.seekr.new" "$BIN_DIR/seekr"
    BIN="$BIN_DIR/seekr"
    LAUNCHER=none
  else
    die "$ASSET is not a seekr release archive"
  fi
  INSTALLED=$("$BIN" --version </dev/null) || die "the installed binary does not run: $BIN"
  if [ "$LAUNCHER" = none ]; then
    say "installed $INSTALLED to $BIN_DIR/seekr (this release predates the seekr.app launcher)"
  else
    say "installed $INSTALLED: $APP, and the seekr command in $BIN_DIR"
  fi
  add_to_path
  other=$(command -v seekr 2>/dev/null || true)
  if [ -n "$other" ] && [ "$other" != "$BIN_DIR/seekr" ]; then
    say "note: $other comes before it on PATH"
  fi

  # An existing launcher is updated without asking, keeping its hotkey.
  if [ "$LAUNCHER" = ask ] && { [ -f "$AGENT" ] || [ -f "$HOME/.hammerspoon/seekr.lua" ]; }; then
    LAUNCHER=yes
  fi
  if [ "$LAUNCHER" = ask ]; then
    if has_tty; then
      cat >/dev/tty <<MSG

seekr can also open from any app with Cmd+Space, like Spotlight. Setting that up:
  - runs seekr.app in the background from now on (it starts at login and
    stays out of the Dock; no other apps needed)
  - turns off Spotlight's own Cmd+Space shortcut; Spotlight keeps working, and
    --hotkey ctrl+space or --uninstall gives the key back
MSG
      if ask "Set up the Cmd+Space launcher?"; then LAUNCHER=yes; else LAUNCHER=no; fi
    else
      LAUNCHER=no
      say "skipped the Cmd+Space launcher (no terminal to ask on); add it with: $INSTALL_CMD --yes"
    fi
  fi

  if [ "$LAUNCHER" = yes ]; then
    KEY=${HOTKEY:-$(current_hotkey)}
    KEY=$(printf '%s' "$KEY" | tr 'A-Z' 'a-z')
    "$BIN" app --hotkey "$KEY" --check || die "seekr cannot use '$KEY' as a hotkey; try e.g. --hotkey ctrl+space"
    if [ "$KEY" = cmd+space ]; then
      defaults delete "$APP_ID" hotkey >/dev/null 2>&1 || true
      take_cmd_space
    else
      defaults write "$APP_ID" hotkey -string "$KEY"
      give_back_shortcuts
    fi
    retire_old_launcher
  elif [ "$LAUNCHER" = no ] && [ -f "$AGENT" ]; then
    # --no-launcher on a Mac that had it: off for good, Cmd+Space given back.
    rm -f "$AGENT"
    give_back_shortcuts
    say "removed the launcher; seekr.app stays in $APP_DIR for the command"
  fi

  # Index now, while the terminal is in front: the first walk of Desktop,
  # Documents and Downloads is when macOS asks whether they may be read. The
  # lookup makes macOS fetch its dictionary, which a fresh Mac lacks until
  # something asks.
  say "indexing…"
  "$BIN" refresh </dev/null || say "indexing failed; seekr retries when it starts"
  "$BIN" define seekr </dev/null >/dev/null 2>&1 || true

  if [ "$LAUNCHER" = yes ]; then
    write_agent
    start_launcher
  fi

  say ""
  say "seekr is installed."
  if [ -n "$NEW_TERMINAL" ]; then
    say "  try it:    seekr   (in a new terminal window; this one has the old PATH)"
  else
    say "  try it:    seekr"
  fi
  if [ "$LAUNCHER" = yes ]; then
    say "  launcher:  press $KEY from any app (seekr.app runs in the background; its log: ~/Library/Logs/seekr.log)"
    say "             macOS may ask once whether seekr may read your Desktop, Documents and Downloads"
  elif [ "$LAUNCHER" = no ]; then
    say "  launcher:  not set up; add it with: $INSTALL_CMD --yes"
  fi
  say "  upgrade:   run the same curl command again"
  say "  uninstall: $INSTALL_CMD --uninstall"
}

main "$@"
