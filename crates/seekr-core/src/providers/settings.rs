//! System Settings panes, so that `mouse` opens System Settings › Mouse the
//! way it does in Spotlight.
//!
//! Since macOS 13, System Settings is built from extensions: one `.appex`
//! bundle per pane, registered for the `com.apple.Settings.extension.ui`
//! extension point, which `open x-apple.systempreferences:<bundle id>` shows.
//! The bundles live in two folders; whatever is installed there is a pane,
//! so a Mac without Touch ID or a pane macOS has retired never lists it. A
//! pane's name comes from [`PANES`] (the sidebar's names), because the
//! bundles' own display names are often internal (`PowerPreferences`,
//! `WiFiSettings`), with extra words Spotlight would find it by.
use crate::item::{Item, Kind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Where macOS keeps the pane extensions.
const EXTENSION_DIRS: &[&str] = &[
    "/System/Library/ExtensionKit/Extensions",
    "/System/Applications/System Settings.app/Contents/PlugIns",
];
const EXTENSION_POINT: &str = "com.apple.Settings.extension.ui";
/// The URL scheme that opens a pane; a pane item's path is such a URL.
pub const URL_PREFIX: &str = "x-apple.systempreferences:";
pub const SYSTEM_SETTINGS_APP: &str = "/System/Applications/System Settings.app";
/// What a pane's row shows where a file's row shows its folder.
pub const LOCATION: &str = "System Settings";

/// Bundle identifier, sidebar name (macOS 15, English) and other words that
/// should find the pane, for every pane we know. Unknown panes still appear,
/// named after their identifier.
const PANES: &[(&str, &str, &str)] = &[
    ("com.apple.systempreferences.AppleIDSettings", "Apple Account", "Apple ID, iCloud"),
    ("com.apple.Family-Settings.extension", "Family", "Family Sharing"),
    ("com.apple.wifi-settings-extension", "Wi-Fi", "wifi, wireless"),
    ("com.apple.BluetoothSettings", "Bluetooth", ""),
    ("com.apple.Network-Settings.extension", "Network", "ethernet, firewall, DNS"),
    ("com.apple.NetworkExtensionSettingsUI.NESettingsUIExtension", "VPN", ""),
    ("com.apple.Classroom-Settings.extension", "Classroom", ""),
    ("com.apple.Notifications-Settings.extension", "Notifications", ""),
    ("com.apple.Sound-Settings.extension", "Sound", "audio, volume, output, input"),
    ("com.apple.Focus-Settings.extension", "Focus", "do not disturb"),
    ("com.apple.Screen-Time-Settings.extension", "Screen Time", ""),
    ("com.apple.systempreferences.GeneralSettings", "General", ""),
    ("com.apple.Appearance-Settings.extension", "Appearance", "dark mode, accent colour, theme"),
    ("com.apple.Accessibility-Settings.extension", "Accessibility", "zoom, VoiceOver"),
    ("com.apple.ControlCenter-Settings.extension", "Control Center", "menu bar"),
    ("com.apple.Siri-Settings.extension", "Apple Intelligence & Siri", "Siri"),
    ("com.apple.Spotlight-Settings.extension", "Spotlight", ""),
    ("com.apple.settings.PrivacySecurity.extension", "Privacy & Security", "permissions, FileVault"),
    ("com.apple.Desktop-Settings.extension", "Desktop & Dock", "Dock, Mission Control, Stage Manager, hot corners"),
    ("com.apple.Displays-Settings.extension", "Displays", "monitor, resolution, brightness, Night Shift"),
    ("com.apple.Wallpaper-Settings.extension", "Wallpaper", "desktop picture, background"),
    ("com.apple.ScreenSaver-Settings.extension", "Screen Saver", ""),
    ("com.apple.Battery-Settings.extension", "Battery", "energy, power, low power mode"),
    ("com.apple.Lock-Screen-Settings.extension", "Lock Screen", ""),
    ("com.apple.Touch-ID-Settings.extension", "Touch ID & Password", "login password, fingerprint"),
    ("com.apple.Users-Groups-Settings.extension", "Users & Groups", "accounts"),
    ("com.apple.Passwords-Settings.extension", "Passwords", ""),
    ("com.apple.Internet-Accounts-Settings.extension", "Internet Accounts", "mail accounts, Google, Exchange"),
    ("com.apple.Game-Center-Settings.extension", "Game Center", ""),
    ("com.apple.WalletSettingsExtension", "Wallet & Apple Pay", ""),
    ("com.apple.HeadphoneSettings", "Headphones", "AirPods"),
    ("com.apple.Keyboard-Settings.extension", "Keyboard", "shortcuts, input sources, text replacements"),
    ("com.apple.Mouse-Settings.extension", "Mouse", "tracking speed, scrolling"),
    ("com.apple.Trackpad-Settings.extension", "Trackpad", "gestures"),
    ("com.apple.Game-Controller-Settings.extension", "Game Controllers", ""),
    ("com.apple.CD-DVD-Settings.extension", "CDs & DVDs", ""),
    ("com.apple.Print-Scan-Settings.extension", "Printers & Scanners", "printer, scanner"),
    // Under General.
    ("com.apple.SystemProfiler.AboutExtension", "About", "about this Mac"),
    ("com.apple.Software-Update-Settings.extension", "Software Update", "macOS update"),
    ("com.apple.settings.Storage", "Storage", "disk space"),
    ("com.apple.AirDrop-Handoff-Settings.extension", "AirDrop & Handoff", ""),
    ("com.apple.LoginItems-Settings.extension", "Login Items & Extensions", "startup items"),
    ("com.apple.Sharing-Settings.extension", "Sharing", "screen sharing, file sharing, remote login"),
    ("com.apple.Time-Machine-Settings.extension", "Time Machine", "backup"),
    ("com.apple.Transfer-Reset-Settings.extension", "Transfer or Reset", "erase, Migration Assistant"),
    ("com.apple.Startup-Disk-Settings.extension", "Startup Disk", ""),
    ("com.apple.Date-Time-Settings.extension", "Date & Time", "time zone, clock"),
    ("com.apple.Localization-Settings.extension", "Language & Region", ""),
    ("com.apple.Coverage-Settings.extension", "AppleCare & Warranty", "coverage"),
    ("com.apple.Profiles-Settings.extension", "Device Management", "profiles, MDM"),
    ("com.apple.ClassKit-Settings.extension", "Class Progress", ""),
];

/// Registered for the extension point, but not a pane anyone opens: the
/// "finish setting up" banner at the top of the sidebar.
const HIDDEN: &[&str] = &["com.apple.FollowUpSettings.FollowUpSettingsExtension"];

/// Every pane installed on this Mac. Reads ~50 small plists; a few milliseconds.
pub fn scan() -> Vec<Item> {
    scan_in(EXTENSION_DIRS.iter().map(Path::new))
}

fn scan_in<'a>(dirs: impl IntoIterator<Item = &'a Path>) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut bundles: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "appex"))
            .collect();
        bundles.sort();
        for bundle in bundles {
            let Some(id) = pane_id(&bundle) else { continue };
            if seen.insert(id.clone()) {
                out.extend(item(&id));
            }
        }
    }
    out
}

/// The bundle identifier of the pane this extension provides, if it is one.
fn pane_id(appex: &Path) -> Option<String> {
    let plist = plist::Value::from_file(appex.join("Contents").join("Info.plist")).ok()?;
    let dict = plist.as_dictionary()?;
    // SwiftUI panes use the ExtensionKit keys, older ones the NSExtension keys.
    let point = ["EXAppExtensionAttributes", "NSExtension"]
        .iter()
        .zip(["EXExtensionPointIdentifier", "NSExtensionPointIdentifier"])
        .find_map(|(attrs, key)| dict.get(attrs)?.as_dictionary()?.get(key)?.as_string());
    if point != Some(EXTENSION_POINT) {
        return None;
    }
    dict.get("CFBundleIdentifier")?.as_string().map(str::to_string)
}

/// The URL that opens the pane with this bundle identifier.
pub fn url(id: &str) -> String {
    format!("{URL_PREFIX}{id}")
}

/// The pane with this bundle identifier, or `None` for one that is [`HIDDEN`].
pub fn item(id: &str) -> Option<Item> {
    if HIDDEN.contains(&id) {
        return None;
    }
    let (name, words) = PANES
        .iter()
        .find(|(known, _, _)| *known == id)
        .map_or_else(|| (derive_name(id), ""), |&(_, n, w)| (n.to_string(), w));
    let mut it = Item::new(Kind::Setting, name, url(id));
    it.bundle_id = Some(id.to_string());
    if !words.is_empty() {
        it.alt = Some(words.to_string());
    }
    Some(it)
}

/// A readable name for a pane that isn't in [`PANES`]:
/// `com.apple.Foo-Bar-Settings.extension` → `Foo Bar`.
fn derive_name(id: &str) -> String {
    let last = id
        .rsplit('.')
        .find(|s| !s.eq_ignore_ascii_case("extension"))
        .unwrap_or(id);
    let words: Vec<&str> = last
        .split('-')
        .filter(|w| !w.is_empty() && !matches!(w.to_ascii_lowercase().as_str(), "settings" | "extension"))
        .collect();
    let name = words.join(" ");
    let name = name.strip_suffix("Settings").unwrap_or(&name).trim().to_string();
    if name.is_empty() { last.to_string() } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir;

    fn write_appex(dir: &Path, name: &str, id: &str, point: Option<(&str, &str, &str)>) {
        let contents = dir.join(format!("{name}.appex")).join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let mut dict = plist::Dictionary::new();
        dict.insert("CFBundleIdentifier".into(), id.into());
        if let Some((attrs, key, value)) = point {
            let mut inner = plist::Dictionary::new();
            inner.insert(key.into(), value.into());
            dict.insert(attrs.into(), plist::Value::Dictionary(inner));
        }
        plist::Value::Dictionary(dict)
            .to_file_xml(contents.join("Info.plist"))
            .unwrap();
    }

    #[test]
    fn installed_panes_become_items_named_like_the_sidebar() {
        let tmp = temp_dir("settings");
        let (a, b) = (tmp.join("Extensions"), tmp.join("PlugIns"));
        let ui = ("EXAppExtensionAttributes", "EXExtensionPointIdentifier", EXTENSION_POINT);
        let legacy = ("NSExtension", "NSExtensionPointIdentifier", EXTENSION_POINT);
        write_appex(&a, "MouseExtension", "com.apple.Mouse-Settings.extension", Some(ui));
        write_appex(&a, "PowerPreferences", "com.apple.Battery-Settings.extension", Some(legacy));
        write_appex(&a, "Widget", "com.apple.SomeWidget", Some(("EXAppExtensionAttributes", "EXExtensionPointIdentifier", "com.apple.widgetkit-extension")));
        write_appex(&a, "NoPoint", "com.apple.NoPoint", None);
        write_appex(&a, "FollowUp", "com.apple.FollowUpSettings.FollowUpSettingsExtension", Some(legacy));
        write_appex(&a, "New", "com.apple.Holographic-Display-Settings.extension", Some(ui));
        write_appex(&b, "GeneralSettings", "com.apple.systempreferences.GeneralSettings", Some(ui));
        // The same pane in both folders counts once.
        write_appex(&b, "MouseAgain", "com.apple.Mouse-Settings.extension", Some(ui));

        let items = scan_in([a.as_path(), b.as_path(), tmp.join("missing").as_path()]);
        // Folder by folder, bundles in name order; the widget, the plain
        // bundle and the follow-up banner are not panes.
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Mouse", "Holographic Display", "Battery", "General"]);
        for it in &items {
            assert_eq!(it.kind, Kind::Setting);
            assert!(it.path.to_string_lossy().starts_with(URL_PREFIX), "{it:?}");
            assert_eq!(it.parent_display(), LOCATION);
        }
        let mouse = items.iter().find(|i| i.name == "Mouse").unwrap();
        assert_eq!(mouse.path, Path::new("x-apple.systempreferences:com.apple.Mouse-Settings.extension"));
        assert_eq!(mouse.bundle_id.as_deref(), Some("com.apple.Mouse-Settings.extension"));
        assert_eq!(mouse.alt.as_deref(), Some("tracking speed, scrolling"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn unknown_panes_are_named_after_their_identifier() {
        assert_eq!(derive_name("com.apple.Foo-Bar-Settings.extension"), "Foo Bar");
        assert_eq!(derive_name("com.apple.systempreferences.FooSettings"), "Foo");
        assert_eq!(derive_name("com.apple.settings.Storage"), "Storage");
        assert_eq!(derive_name("com.apple.FooSettingsExtension"), "FooSettingsExtension");
    }

    #[test]
    fn every_known_pane_is_listed_once() {
        let mut seen = HashSet::new();
        for (id, name, _) in PANES {
            assert!(seen.insert(*id), "{id} twice");
            assert!(!name.is_empty(), "{id} has no name");
            assert!(item(id).is_some(), "{id} is hidden");
        }
    }
}
