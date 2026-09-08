#!/usr/bin/env python3
"""Generate a Terminal.app profile file (`seekr.terminal`) for the launcher window.

The profile is a copy of your current default profile (font, colours) with:
- the window closing as soon as the shell exits (so it vanishes when seekr quits),
- a 110x28 default size,
- a fixed "seekr" window title.

Usage: make-profile.py OUTPUT.terminal   (then `open OUTPUT.terminal` once to import it)
"""
import plistlib
import subprocess
import sys

OUT = sys.argv[1] if len(sys.argv) > 1 else "seekr.terminal"

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
        "shellExitAction": 0,  # 0 = close the window when the shell exits
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
print(f"wrote {OUT} (based on profile {base_name!r})")
