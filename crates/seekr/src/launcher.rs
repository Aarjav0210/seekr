//! Glue for the hotkey launcher (Cmd+Space) in contrib/ (Hammerspoon + kitty).
//!
//! In resident mode one seekr stays alive in a parked kitty window, so the
//! hotkey only has to reveal it: the UI is on screen in a couple of frames and
//! typing works immediately. Hiding is Hammerspoon's job (it owns window
//! placement and, after Esc, hands focus back to the previous app; after Enter
//! the app that was just opened keeps it); seekr just asks for it.
//! Hammerspoon in turn sends SIGUSR1 when it reveals the window, which seekr
//! treats as "wake up and refresh in the background", and SIGUSR2 when the
//! hotkey hid the window behind seekr's back (the hotkey while seekr had focus),
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

/// Ask Hammerspoon to park the launcher window. With `restore_focus` the window
/// that was in front before seekr gets the focus back (Esc); without it, the
/// app that was just opened keeps the focus `open` gave it (Enter), instead of
/// being pushed behind the previous app a moment later. The `hs` CLI answers
/// in a few milliseconds; AppleScript is the slower fallback. Returns false
/// when neither route works (no Hammerspoon), in which case the caller should
/// quit instead.
pub fn hide_window(restore_focus: bool) -> bool {
    // A launcher script that predates the argument ignores it and always
    // restores focus: the old behaviour, not a failure.
    let lua = if restore_focus {
        "package.loaded.seekr.hide()"
    } else {
        "package.loaded.seekr.hide(false)"
    };
    for hs in ["/opt/homebrew/bin/hs", "/usr/local/bin/hs", "hs"] {
        if run(hs, &["-c", lua]) {
            return true;
        }
    }
    let script = format!("tell application \"Hammerspoon\" to execute lua code \"{lua}\"");
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
