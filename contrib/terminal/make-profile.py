#!/usr/bin/env python3
"""Generate a Terminal.app profile file (`seekr.terminal`) for the launcher window.

The profile is a copy of your current default profile (font, colours) with:
- seekr run directly in place of the login shell, so ~/.zshrc is never sourced
  and the window shows seekr almost immediately (the binary's absolute path is
  baked in: re-run the installer if it moves),
- the window closing as soon as seekr exits,
- a 110x28 default size,
- a fixed "seekr" window title.

Usage: make-profile.py OUTPUT.terminal /path/to/seekr [ARG...]
Then `open OUTPUT.terminal` once to import it. Terminal names the imported
profile after the file, and every import adds a new profile rather than
replacing one of the same name, so delete the old profile first.
"""
import os
import plistlib
import subprocess
import sys

if len(sys.argv) < 3:
    sys.exit(__doc__)
OUT, SEEKR, ARGS = sys.argv[1], os.path.abspath(sys.argv[2]), sys.argv[3:]
if not os.access(SEEKR, os.X_OK):
    sys.exit(f"{SEEKR}: not an executable")
COMMAND = " ".join([SEEKR] + ARGS)

raw = subprocess.run(
    ["defaults", "export", "com.apple.Terminal", "-"], check=True, capture_output=True
).stdout
prefs = plistlib.loads(raw)
profiles = prefs.get("Window Settings", {})
base_name = prefs.get("Default Window Settings") or "Basic"
base = dict(profiles.get(base_name) or profiles.get("Basic") or {})

base.update(
    {
        "name": "seekr",
        "type": "Window Settings",
        "ProfileCurrentVersion": base.get("ProfileCurrentVersion", 2.07),
        # Startup command. RunCommandAsShell=True is Terminal's "Run inside shell"
        # box *unchecked*: the command is exec'd via `login -pfq` in place of the
        # shell, so no rc files run. (False would type it into a login zsh.)
        "CommandString": COMMAND,
        "RunCommandAsShell": True,
        "shellExitAction": 0,  # 0 = close the window when the command exits
        "columnCount": 110,
        "rowCount": 28,
        "ShowActiveProcessInTitle": False,
        "ShowActiveProcessArgumentsInTitle": False,
        "ShowCommandKeyInTitle": False,
        "ShowDimensionsInTitle": False,
        "ShowRepresentedURLInTitle": False,
        "ShowShellCommandInTitle": False,
        "ShowTTYNameInTitle": False,
        "ShowWindowSettingsNameInTitle": True,
    }
)

with open(OUT, "wb") as f:
    plistlib.dump(base, f)
print(f"wrote {OUT} (based on profile {base_name!r}, runs `{COMMAND}`)")
