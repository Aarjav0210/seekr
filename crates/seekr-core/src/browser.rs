//! The default web browser, as LaunchServices records it.
//!
//! macOS keeps the user's choice of browser as the handler for the `http`
//! and `https` URL schemes in
//! `~/Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist`.
//! Nothing there means Safari, the factory default. The built-in web searches
//! ([`crate::AppSearch::builtin`]) cover that browser and every other installed
//! one, so a Mac without Arc searches in Firefox, Chrome, Safari, … instead.
use crate::item::Item;

/// The browser that opens `https` URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    /// Bundle identifier, in the bundle's own case when it is known.
    pub bundle_id: String,
    /// The app's name as shown to the user ("Firefox", "Google Chrome").
    pub name: String,
}

/// The factory default.
const SAFARI: &str = "com.apple.Safari";

/// Browsers whose names are known without finding the bundle on disk.
/// LaunchServices stores identifiers lowercased, so these are matched
/// case-insensitively and the canonical spelling is kept.
const KNOWN: &[(&str, &str)] = &[
    ("com.apple.Safari", "Safari"),
    ("com.apple.SafariTechnologyPreview", "Safari Technology Preview"),
    ("org.mozilla.firefox", "Firefox"),
    ("org.mozilla.firefoxdeveloperedition", "Firefox Developer Edition"),
    ("org.mozilla.nightly", "Firefox Nightly"),
    ("com.google.Chrome", "Google Chrome"),
    ("com.google.Chrome.canary", "Google Chrome Canary"),
    ("org.chromium.Chromium", "Chromium"),
    ("company.thebrowser.Browser", "Arc"),
    ("company.thebrowser.dia", "Dia"),
    ("com.microsoft.edgemac", "Microsoft Edge"),
    ("com.brave.Browser", "Brave Browser"),
    ("com.vivaldi.Vivaldi", "Vivaldi"),
    ("com.operasoftware.Opera", "Opera"),
    ("com.operasoftware.OperaGX", "Opera GX"),
    ("app.zen-browser.zen", "Zen"),
    ("com.kagi.kagimacOS", "Orion"),
    ("com.duckduckgo.macos.browser", "DuckDuckGo"),
    ("org.torproject.torbrowser", "Tor Browser"),
    ("ai.perplexity.comet", "Comet"),
];

/// The user's default browser: the `https` handler LaunchServices records,
/// else Safari. `apps` (the installed applications) supplies the name of a
/// browser not in [`KNOWN`]; failing that it comes from the identifier.
pub fn default_browser(apps: &[Item]) -> Browser {
    let id = launch_services_handler().unwrap_or_else(|| SAFARI.to_string());
    describe(&id, apps)
}

/// Every browser that can be searched in: the default one first (it is what
/// LaunchServices opens links with, wherever it lives), then each installed
/// app from [`KNOWN`], by name. Typing a browser's keyword and pressing Tab
/// only means a search when that browser is actually on this Mac.
pub fn installed_browsers(apps: &[Item]) -> Vec<Browser> {
    let mut out = vec![default_browser(apps)];
    let mut others: Vec<Browser> = apps
        .iter()
        .filter_map(|a| a.bundle_id.as_deref())
        .filter(|id| KNOWN.iter().any(|(k, _)| k.eq_ignore_ascii_case(id)))
        .map(|id| describe(id, apps))
        .collect();
    others.sort_by(|a, b| a.name.cmp(&b.name));
    for b in others {
        if !out.iter().any(|o| o.bundle_id.eq_ignore_ascii_case(&b.bundle_id)) {
            out.push(b);
        }
    }
    out
}

/// Read the `https` (or `http`) handler from the LaunchServices preferences.
fn launch_services_handler() -> Option<String> {
    let path = dirs::home_dir()?
        .join("Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist");
    let v = plist::Value::from_file(path).ok()?;
    handler_in(&v)
}

/// The bundle id handling `https`, else `http`, in a parsed
/// `com.apple.launchservices.secure.plist`.
pub fn handler_in(prefs: &plist::Value) -> Option<String> {
    let handlers = prefs.as_dictionary()?.get("LSHandlers")?.as_array()?;
    let entries = handlers.iter().filter_map(|h| h.as_dictionary());
    for scheme in ["https", "http"] {
        let found = entries.clone().find_map(|d| {
            let s = d.get("LSHandlerURLScheme")?.as_string()?;
            if !s.eq_ignore_ascii_case(scheme) {
                return None;
            }
            d.get("LSHandlerRoleAll")
                .or_else(|| d.get("LSHandlerRoleViewer"))
                .and_then(|v| v.as_string())
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Name the browser with this bundle identifier.
fn describe(bundle_id: &str, apps: &[Item]) -> Browser {
    if let Some((id, name)) = KNOWN.iter().find(|(id, _)| id.eq_ignore_ascii_case(bundle_id)) {
        return Browser {
            bundle_id: id.to_string(),
            name: name.to_string(),
        };
    }
    let found = apps
        .iter()
        .find(|a| a.bundle_id.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(bundle_id)));
    match found {
        Some(app) => Browser {
            bundle_id: app.bundle_id.clone().unwrap_or_else(|| bundle_id.to_string()),
            name: app.name.clone(),
        },
        None => Browser {
            bundle_id: bundle_id.to_string(),
            name: name_from_id(bundle_id),
        },
    }
}

/// `com.example.webbrowser` → "Webbrowser": the last component, capitalised.
fn name_from_id(bundle_id: &str) -> String {
    let last = bundle_id.rsplit('.').next().unwrap_or(bundle_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => "Browser".to_string(),
    }
}

/// The whole name as one lowercase word: "Firefox Developer Edition" →
/// `firefoxdeveloperedition`. `browser` when nothing is left.
pub fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if s.is_empty() || crate::query::is_reserved_prefix(&s) {
        "browser".to_string()
    } else {
        s
    }
}

/// The keyword that enters a browser's search: the first word of its name
/// that says which browser it is, lowercased ("Google Chrome" → `chrome`,
/// "Brave Browser" → `brave`, "Firefox Developer Edition" → `firefox`).
/// `browser` when nothing usable is left.
pub fn keyword_for(name: &str) -> String {
    const FILLER: &[&str] = &["google", "microsoft", "the", "browser", "web"];
    let word = name
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        })
        .find(|w| !w.is_empty() && !FILLER.contains(&w.as_str()));
    match word {
        Some(w) if !crate::query::is_reserved_prefix(&w) => w,
        _ => "browser".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Dictionary, Value};

    fn handler(key: &str, val: &str, role: &str, id: &str) -> Value {
        let mut d = Dictionary::new();
        d.insert(key.into(), Value::String(val.into()));
        d.insert(role.into(), Value::String(id.into()));
        Value::Dictionary(d)
    }

    fn prefs(handlers: Vec<Value>) -> Value {
        let mut d = Dictionary::new();
        d.insert("LSHandlers".into(), Value::Array(handlers));
        Value::Dictionary(d)
    }

    #[test]
    fn https_handler_wins_then_http_then_nothing() {
        let p = prefs(vec![
            handler("LSHandlerContentType", "public.html", "LSHandlerRoleAll", "com.apple.safari"),
            handler("LSHandlerURLScheme", "http", "LSHandlerRoleAll", "com.google.chrome"),
            handler("LSHandlerURLScheme", "https", "LSHandlerRoleAll", "org.mozilla.firefox"),
        ]);
        assert_eq!(handler_in(&p).as_deref(), Some("org.mozilla.firefox"));
        let p = prefs(vec![handler("LSHandlerURLScheme", "http", "LSHandlerRoleViewer", "com.google.chrome")]);
        assert_eq!(handler_in(&p).as_deref(), Some("com.google.chrome"));
        assert!(handler_in(&prefs(vec![])).is_none());
        assert!(handler_in(&Value::String("junk".into())).is_none());
    }

    fn app(name: &str, id: &str) -> crate::item::Item {
        let mut a = crate::item::Item::new(crate::item::Kind::App, name.to_string(), format!("/Applications/{name}.app"));
        a.bundle_id = Some(id.to_string());
        a
    }

    #[test]
    fn known_browsers_get_their_canonical_id_and_name() {
        let b = describe("org.mozilla.firefox", &[]);
        assert_eq!(b.name, "Firefox");
        let b = describe("company.thebrowser.browser", &[]);
        assert_eq!(b.bundle_id, "company.thebrowser.Browser");
        assert_eq!(b.name, "Arc");
        let b = describe("com.example.NoSuchBrowser", &[]);
        assert_eq!(b.bundle_id, "com.example.NoSuchBrowser");
        assert_eq!(b.name, "NoSuchBrowser");
        let apps = [app("Mystery", "com.example.Mystery")];
        let b = describe("com.example.mystery", &apps);
        assert_eq!((b.bundle_id.as_str(), b.name.as_str()), ("com.example.Mystery", "Mystery"));
    }

    #[test]
    fn installed_browsers_are_the_default_then_the_known_ones_present() {
        let apps = [
            app("Spotify", "com.spotify.client"),
            app("Google Chrome", "com.google.Chrome"),
            app("Arc", "company.thebrowser.Browser"),
            app("Safari", "com.apple.Safari"),
        ];
        let found = installed_browsers(&apps);
        // Whatever this Mac's default is, it comes first and is listed once.
        let names: Vec<&str> = found.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names[0], default_browser(&apps).name);
        let mut rest = names[1..].to_vec();
        rest.sort();
        let mut expected = vec!["Arc", "Google Chrome", "Safari"];
        expected.retain(|n| *n != names[0]);
        assert_eq!(rest, expected);
        assert!(!names.contains(&"Spotify"));
        let ids: std::collections::HashSet<String> =
            found.iter().map(|b| b.bundle_id.to_ascii_lowercase()).collect();
        assert_eq!(ids.len(), found.len(), "no duplicates: {names:?}");
    }

    #[test]
    fn keywords_name_the_browser_not_the_vendor() {
        assert_eq!(keyword_for("Safari"), "safari");
        assert_eq!(keyword_for("Google Chrome"), "chrome");
        assert_eq!(keyword_for("Microsoft Edge"), "edge");
        assert_eq!(keyword_for("Brave Browser"), "brave");
        assert_eq!(keyword_for("Firefox Developer Edition"), "firefox");
        assert_eq!(keyword_for("Arc"), "arc");
        assert_eq!(keyword_for("Browser"), "browser");
        assert_eq!(keyword_for(""), "browser");
        assert_eq!(keyword_for("Doc"), "browser", "scope words are not keywords");
    }
}
