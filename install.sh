#!/bin/sh
# Install seekr on macOS:
#
#   curl -fsSL https://raw.githubusercontent.com/Aarjav0210/seekr/main/install.sh | sh
#
# Downloads the latest release (one binary for Apple silicon and Intel, and the
# launcher files) into ~/.local/bin, puts that folder on PATH, then offers to
# set up the Cmd+Space launcher with contrib/install-launcher.sh. Running it
# again upgrades. The options are in usage() below.
#
# The release archive is built by contrib/package.sh (.github/workflows/release.yml
# on every v* tag). This script is read from main while the archive comes from
# the latest release, so it only relies on the archive's layout and on
# install-launcher.sh's options and SEEKR_BIN / SEEKR_LAUNCHER_CMD.
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
HOMEBREW_INSTALLER=https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh

usage() {
  cat <<USAGE
Install or upgrade seekr: $INSTALL_CMD [options]

  --no-launcher   just the seekr command, without the Cmd+Space launcher
  --yes           set up the launcher without asking
  --hotkey KEY    the launcher's key instead of Cmd+Space, e.g. ctrl+space
                  (implies --yes; later runs keep it)
  --opacity N     the launcher window's opacity, 0 to 1: 0.85 is translucent
                  black, 1 is solid (implies --yes; later runs keep it)
  --version TAG   install the release TAG, e.g. v1.2.1, instead of the latest
  --uninstall     remove seekr, the launcher, the index and the settings, and
                  give Cmd+Space back to Spotlight

SEEKR_BIN_DIR installs somewhere other than ~/.local/bin; SEEKR_ARCHIVE installs
from a local $ASSET instead of downloading it.
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

# brew's path, or nothing. A fresh Homebrew is not on PATH until the shell
# profile change its installer prints has been made.
find_brew() {
  if command -v brew 2>/dev/null; then return; fi
  for b in /opt/homebrew/bin/brew /usr/local/bin/brew; do
    if [ -x "$b" ]; then say "$b"; return; fi
  done
}

# The two apps install-launcher.sh would otherwise install with Homebrew.
launcher_apps_present() {
  { [ -d /Applications/kitty.app ] || [ -d "$HOME/Applications/kitty.app" ]; } &&
    [ -d /Applications/Hammerspoon.app ]
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

uninstall() {
  sh "$SRC/contrib/install-launcher.sh" --uninstall </dev/null
  rm -f "$BIN_DIR/seekr"
  remove_from_path
  rm -rf "$HOME/Library/Application Support/seekr" "${XDG_CONFIG_HOME:-$HOME/.config}/seekr"
  say "seekr removed: $BIN_DIR/seekr, its index (~/Library/Application Support/seekr) and settings (~/.config/seekr)."
  if other=$(command -v seekr 2>/dev/null); then
    say "another seekr is still on PATH: $other"
  fi
}

main() {
  LAUNCHER=ask
  HOTKEY=""
  OPACITY=""
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
      --opacity)
        [ $# -ge 2 ] || die "--opacity needs a value between 0 and 1, e.g. --opacity 0.85"
        OPACITY=$2; LAUNCHER=yes; shift 2 ;;
      --opacity=*) OPACITY=${1#--opacity=}; LAUNCHER=yes; shift ;;
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
  case "$BIN_DIR" in /*) ;; *) die "SEEKR_BIN_DIR must be an absolute path" ;; esac

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
  [ -f "$SRC/seekr" ] && [ -f "$SRC/contrib/install-launcher.sh" ] || die "$ASSET is not a seekr release archive"

  if [ -n "$UNINSTALL" ]; then
    uninstall
    return
  fi

  # Copied next to the old binary and renamed over it: a running seekr keeps
  # its file. xattr -c drops a quarantine flag in case the archive came by
  # browser or AirDrop rather than curl.
  mkdir -p "$BIN_DIR"
  cp "$SRC/seekr" "$BIN_DIR/.seekr.new"
  chmod 755 "$BIN_DIR/.seekr.new"
  xattr -c "$BIN_DIR/.seekr.new" 2>/dev/null || true
  mv -f "$BIN_DIR/.seekr.new" "$BIN_DIR/seekr"
  INSTALLED=$("$BIN_DIR/seekr" --version </dev/null) || die "the installed binary does not run: $BIN_DIR/seekr"
  say "installed $INSTALLED to $BIN_DIR/seekr"
  add_to_path
  other=$(command -v seekr 2>/dev/null || true)
  if [ -n "$other" ] && [ "$other" != "$BIN_DIR/seekr" ]; then
    say "note: $other comes before it on PATH"
  fi

  # An existing launcher is updated without asking, keeping its hotkey and opacity.
  if [ "$LAUNCHER" = ask ] && [ -f "$HOME/.hammerspoon/seekr.lua" ]; then
    LAUNCHER=yes
  fi
  if [ "$LAUNCHER" = ask ]; then
    if has_tty; then
      cat >/dev/tty <<MSG

seekr can also open from any app with Cmd+Space, like Spotlight. Setting that up:
  - installs kitty (seekr's window) and Hammerspoon (the hotkey) with Homebrew
  - turns off Spotlight's own Cmd+Space shortcut; Spotlight keeps working, and
    --hotkey ctrl+space or --uninstall gives the key back
  - macOS then asks you to open Hammerspoon, to let this terminal control it,
    and to allow it under Privacy & Security > Accessibility
MSG
      if ask "Set up the Cmd+Space launcher?"; then LAUNCHER=yes; else LAUNCHER=no; fi
    else
      LAUNCHER=no
      say "skipped the Cmd+Space launcher (no terminal to ask on); add it with: $INSTALL_CMD --yes"
    fi
  fi

  if [ "$LAUNCHER" = yes ]; then
    if ! launcher_apps_present; then
      BREW=$(find_brew)
      if [ -z "$BREW" ]; then
        has_tty || die "the launcher needs Homebrew (https://brew.sh) for kitty and Hammerspoon; install it and run this again"
        say "kitty and Hammerspoon come from Homebrew, which is not installed."
        ask "Install Homebrew now? (its installer asks for your password)" ||
          die "the launcher needs Homebrew (https://brew.sh); install it, then run: $INSTALL_CMD --yes"
        /bin/bash -c "$(curl -fsSL "$HOMEBREW_INSTALLER")" </dev/tty || die "Homebrew's installer failed"
        BREW=$(find_brew)
        [ -n "$BREW" ] || die "Homebrew's installer finished, but brew was not found"
      fi
      PATH="$(dirname "$BREW"):$PATH"
      export PATH
    fi
    set --
    if [ -n "$HOTKEY" ]; then set -- "$@" --hotkey "$HOTKEY"; fi
    if [ -n "$OPACITY" ]; then set -- "$@" --opacity "$OPACITY"; fi
    if has_tty; then IN=/dev/tty; else IN=/dev/null; fi
    SEEKR_BIN="$BIN_DIR/seekr" SEEKR_LAUNCHER_CMD="$INSTALL_CMD" \
      sh "$SRC/contrib/install-launcher.sh" "$@" <"$IN"
  fi

  # Index now, while the terminal is in front: the first walk of Desktop,
  # Documents and Downloads is when macOS asks whether they may be read. The
  # lookup makes macOS fetch its dictionary, which a fresh Mac lacks until
  # something asks.
  say "indexing…"
  "$BIN_DIR/seekr" refresh </dev/null || say "indexing failed; seekr retries when it starts"
  "$BIN_DIR/seekr" define seekr </dev/null >/dev/null 2>&1 || true

  say ""
  say "seekr is installed."
  if [ -n "$NEW_TERMINAL" ]; then
    say "  try it:    seekr   (in a new terminal window; this one has the old PATH)"
  else
    say "  try it:    seekr"
  fi
  if [ "$LAUNCHER" = yes ]; then
    key=$(sed -n 's/^M\.hotkey = "\([^"]*\)"$/\1/p' "$HOME/.hammerspoon/seekr.lua" 2>/dev/null || true)
    say "  launcher:  press ${key:-the hotkey} from any app"
  else
    say "  launcher:  not set up; add it with: $INSTALL_CMD --yes"
  fi
  say "  upgrade:   run the same curl command again"
  say "  uninstall: $INSTALL_CMD --uninstall"
}

main "$@"
