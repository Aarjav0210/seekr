//! Glue for the Ctrl+Space launcher in contrib/ (Hammerspoon + Terminal.app).
//!
//! In resident mode one seekr stays alive in a parked Terminal window, so the
//! hotkey only has to reveal it: the UI is on screen in a couple of frames and
//! typing works immediately. Hiding is Hammerspoon's job (it owns window
//! placement and restores focus to the previous app); seekr just asks for it.
//! Hammerspoon in turn sends SIGUSR1 when it reveals the window, which seekr
//! treats as "wake up and refresh in the background", and SIGUSR2 when the
//! hotkey hid the window behind seekr's back (Ctrl+Space while seekr had focus),
//! so seekr can stop drawing and keep the query for the next reveal.
use std::path::PathBuf;
use std::process::{Command, Stdio};

use seekr_core::Config;

/// Where the resident process advertises itself: `{"pid": N, "park": "USR2"}`.
/// `park` names the signal the launcher may send after hiding the window; a
/// launcher script that predates it just reads `pid`.
pub fn state_path() -> PathBuf {
    Config::data_dir().join("resident.json")
}

pub fn write_state() {
    let _ = std::fs::write(
        state_path(),
        format!("{{\"pid\": {}, \"park\": \"USR2\"}}\n", std::process::id()),
    );
}

pub fn clear_state() {
    let _ = std::fs::remove_file(state_path());
}

const LUA_HIDE: &str = "package.loaded.seekr.hide()";

/// Ask Hammerspoon to park the launcher window. The `hs` CLI answers in a few
/// milliseconds; AppleScript is the slower fallback. Returns false when neither
/// route works (no Hammerspoon), in which case the caller should quit instead.
pub fn hide_window() -> bool {
    for hs in ["/opt/homebrew/bin/hs", "/usr/local/bin/hs", "hs"] {
        if run(hs, &["-c", LUA_HIDE]) {
            return true;
        }
    }
    let script = format!("tell application \"Hammerspoon\" to execute lua code \"{LUA_HIDE}\"");
    run("osascript", &["-e", &script])
}

fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
