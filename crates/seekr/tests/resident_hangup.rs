//! A resident seekr must not outlive its terminal. When kitty goes away
//! (crash, Hammerspoon reload) the kernel hangs the pty up, and crossterm then
//! loops forever on a read that returns nothing, so without a way out seekr
//! burns a whole core until someone sends SIGKILL.
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The launcher's state dir and friends, removed at the end, and a seekr that
/// outlived the test ended, so a failure doesn't leave a core spinning.
struct Sandbox {
    root: PathBuf,
    resident: Option<String>,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(pid) = &self.resident {
            let _ = Command::new("/bin/kill").args(["-KILL", pid]).stderr(Stdio::null()).status();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sandbox() -> Sandbox {
    let root = std::env::temp_dir().join(format!("seekr-hangup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("config/seekr")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join("files")).unwrap();
    let roots = format!("roots = [\"{}\"]\n", root.join("files").display());
    std::fs::write(root.join("config/seekr/config.toml"), roots).unwrap();
    Sandbox { root, resident: None }
}

/// script(1) plays kitty: it owns the pty master that seekr runs on.
fn start_terminal(sandbox: &Sandbox) -> Child {
    Command::new("/usr/bin/script")
        .args(["-q", "/dev/null", env!("CARGO_BIN_EXE_seekr"), "--resident"])
        .env("HOME", &sandbox.root)
        .env("SEEKR_DATA_DIR", sandbox.root.join("data"))
        .env("XDG_CONFIG_HOME", sandbox.root.join("config"))
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start script(1)")
}

fn wait_for(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < within, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn running(pid: &str) -> bool {
    Command::new("/bin/kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_parked_resident_ends_when_its_terminal_dies() {
    let mut sandbox = sandbox();
    let state = sandbox.root.join("data/resident.json");
    let mut terminal = start_terminal(&sandbox);

    let pid = {
        let mut pid = None;
        wait_for("resident.json", Duration::from_secs(15), || {
            let text = std::fs::read_to_string(&state).unwrap_or_default();
            pid = text.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).map(str::to_string);
            pid.is_some()
        });
        pid.unwrap()
    };
    sandbox.resident = Some(pid.clone());

    // Parked is where a launcher spends its time: no drawing, so nothing but
    // the poll ever finds out that the terminal is gone.
    std::thread::sleep(Duration::from_millis(500));
    Command::new("/bin/kill").args(["-USR2", &pid]).status().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(running(&pid), "seekr should still be up before the terminal dies");

    terminal.kill().unwrap();
    terminal.wait().unwrap();

    wait_for("seekr to exit after the hangup", Duration::from_secs(5), || !running(&pid));
}
