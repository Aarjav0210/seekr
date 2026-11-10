//! Searches handed to another application ("Tab to search").
//!
//! Select an app (or type its keyword), press Tab, type the terms, Enter: seekr
//! builds a URL the app understands and hands it over with `open(1)`, e.g.
//! `spotify:search:daft%20punk`, or a Google search opened in the browser.
//! Only apps that register a URL scheme (or browsers, which take `https`) can
//! be searched this way. The default browser (see [`crate::browser`]) and
//! Spotify are built in; `searches` in `~/.config/seekr/config.toml` replaces
//! the list.
use crate::browser::{self, Browser};
use crate::providers::apps;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Stands in for the percent-encoded terms in [`AppSearch::url`].
pub const QUERY_PLACEHOLDER: &str = "{query}";

/// One app that a query can be handed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSearch {
    /// What you type before Tab (`arc`), and the prefix of the query while
    /// searching (`arc: rust lifetimes`). Matched case-insensitively; must not
    /// be a scope word like `app` or `doc`.
    pub keyword: String,
    /// Shown in the UI ("Arc"). Defaults to the keyword.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// URL template; `{query}` is replaced by the percent-encoded terms.
    pub url: String,
    /// Bundle identifier of the app that opens the URL (`open -b`). Also ties
    /// the search to that app's result row, so Tab on the row enters it, and
    /// lets Enter with nothing typed simply open the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

impl AppSearch {
    /// The searches seekr ships with: a Google search in every installed
    /// browser (the default one first) and Spotify. Computed once per process
    /// from the standard application folders.
    pub fn builtin() -> Vec<AppSearch> {
        static BUILTIN: OnceLock<Vec<AppSearch>> = OnceLock::new();
        BUILTIN
            .get_or_init(|| {
                let apps = apps::walk_app_dirs(&crate::config::default_app_dir_paths());
                let mut out = AppSearch::web_searches(&browser::installed_browsers(&apps));
                let spotify = AppSearch::spotify();
                if !out.iter().any(|s| s.keyword.eq_ignore_ascii_case(&spotify.keyword)) {
                    out.push(spotify);
                }
                out
            })
            .clone()
    }

    /// A Google search opened in `browser`; its keyword is the browser's name
    /// ("Google Chrome" → `chrome`), so Tab works on its row or on the word.
    pub fn web(browser: &Browser) -> AppSearch {
        AppSearch {
            keyword: browser::keyword_for(&browser.name),
            name: Some(browser.name.clone()),
            url: "https://www.google.com/search?q={query}".into(),
            app: Some(browser.bundle_id.clone()),
        }
    }

    /// [`AppSearch::web`] for each browser, keywords kept unique: when two
    /// share one ("Firefox" and "Firefox Developer Edition" are both
    /// `firefox`) the later takes its whole name, `firefoxdeveloperedition`.
    pub fn web_searches(browsers: &[Browser]) -> Vec<AppSearch> {
        let mut out: Vec<AppSearch> = Vec::new();
        for b in browsers {
            let mut s = AppSearch::web(b);
            let taken = |k: &str| out.iter().any(|o| o.keyword.eq_ignore_ascii_case(k));
            if taken(&s.keyword) {
                s.keyword = browser::slug(&b.name);
            }
            if taken(&s.keyword) || s.validate().is_err() {
                continue;
            }
            out.push(s);
        }
        out
    }

    pub fn spotify() -> AppSearch {
        AppSearch {
            keyword: "spotify".into(),
            name: Some("Spotify".into()),
            url: "spotify:search:{query}".into(),
            app: Some("com.spotify.client".into()),
        }
    }

    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.keyword)
    }

    /// The URL that searches for `terms` (trimmed, percent-encoded).
    pub fn url_for(&self, terms: &str) -> String {
        self.url
            .replace(QUERY_PLACEHOLDER, &percent_encode(terms.trim()))
    }

    /// Arguments for `open(1)`: the app (when set) and the URL for `terms`.
    /// With nothing to search for only the app is named, so `open` just
    /// launches or raises it. Empty when there is neither.
    pub fn open_args(&self, terms: &str) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(b) = &self.app {
            args.push("-b".to_string());
            args.push(b.clone());
        }
        if !terms.trim().is_empty() {
            args.push(self.url_for(terms));
        }
        args
    }

    /// Reject keywords the query language could not tell apart from its own
    /// prefixes, and templates that would silently drop the terms.
    pub fn validate(&self) -> Result<()> {
        let k = &self.keyword;
        if k.is_empty() || k.contains(|c: char| c.is_whitespace() || c == ':') {
            bail!("app search keyword {k:?} must be a single word without ':'");
        }
        if crate::query::is_reserved_prefix(k) {
            bail!("app search keyword {k:?} is a scope word; pick another");
        }
        if !self.url.contains(QUERY_PLACEHOLDER) {
            bail!("app search {k:?}: url must contain {QUERY_PLACEHOLDER}");
        }
        Ok(())
    }
}

/// The search whose keyword is `word`, case-insensitively.
pub fn by_keyword<'a>(searches: &'a [AppSearch], word: &str) -> Option<&'a AppSearch> {
    searches.iter().find(|s| s.keyword.eq_ignore_ascii_case(word))
}

/// The search attached to the app with this bundle identifier.
pub fn for_app<'a>(searches: &'a [AppSearch], bundle_id: &str) -> Option<&'a AppSearch> {
    searches
        .iter()
        .find(|s| s.app.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(bundle_id)))
}

/// RFC 3986 percent-encoding: everything but unreserved ASCII is escaped, so
/// `daft punk` becomes `daft%20punk`, which both `https` and `spotify:` accept.
pub fn percent_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(keyword: &str, url: &str) -> AppSearch {
        AppSearch {
            keyword: keyword.into(),
            name: None,
            url: url.into(),
            app: None,
        }
    }

    #[test]
    fn encodes_spaces_and_reserved_characters() {
        assert_eq!(percent_encode("daft punk"), "daft%20punk");
        assert_eq!(percent_encode("a&b=c/d?e#f"), "a%26b%3Dc%2Fd%3Fe%23f");
        assert_eq!(percent_encode("caf\u{e9}"), "caf%C3%A9");
        assert_eq!(percent_encode("ok-_.~"), "ok-_.~");
    }

    fn arc() -> AppSearch {
        AppSearch::web(&Browser {
            bundle_id: "company.thebrowser.Browser".into(),
            name: "Arc".into(),
        })
    }

    #[test]
    fn url_for_trims_and_encodes() {
        let spotify = AppSearch::spotify();
        assert_eq!(spotify.url_for("  daft punk "), "spotify:search:daft%20punk");
    }

    fn browser(name: &str, id: &str) -> Browser {
        Browser {
            bundle_id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn the_web_search_is_named_after_the_browser() {
        let arc = arc();
        assert_eq!(arc.keyword, "arc");
        assert_eq!(arc.name(), "Arc");
        let chrome = AppSearch::web(&browser("Google Chrome", "com.google.Chrome"));
        assert_eq!(chrome.keyword, "chrome");
        assert_eq!(chrome.app.as_deref(), Some("com.google.Chrome"));
        chrome.validate().unwrap();
    }

    #[test]
    fn web_searches_keep_keywords_unique() {
        let all = AppSearch::web_searches(&[
            browser("Firefox", "org.mozilla.firefox"),
            browser("Firefox Developer Edition", "org.mozilla.firefoxdeveloperedition"),
            browser("Firefox", "org.example.firefox-clone"),
            browser("Google Chrome", "com.google.Chrome"),
        ]);
        let kws: Vec<&str> = all.iter().map(|s| s.keyword.as_str()).collect();
        assert_eq!(kws, ["firefox", "firefoxdeveloperedition", "chrome"]);
    }

    #[test]
    fn builtins_are_usable_on_this_mac() {
        let all = AppSearch::builtin();
        assert!(all.len() >= 2, "{all:?}");
        assert_eq!(all.last(), Some(&AppSearch::spotify()));
        let mut cfg = crate::config::Config::default();
        cfg.searches = all;
        cfg.validate().unwrap();
    }

    #[test]
    fn open_args_name_the_app_and_drop_the_url_without_terms() {
        let arc = arc();
        assert_eq!(
            arc.open_args("rust lifetimes"),
            vec![
                "-b",
                "company.thebrowser.Browser",
                "https://www.google.com/search?q=rust%20lifetimes"
            ]
        );
        assert_eq!(arc.open_args("  "), vec!["-b", "company.thebrowser.Browser"]);
        let yt = web("yt", "https://www.youtube.com/results?search_query={query}");
        assert_eq!(
            yt.open_args("cats"),
            vec!["https://www.youtube.com/results?search_query=cats"]
        );
        assert!(yt.open_args("").is_empty());
        assert_eq!(yt.name(), "yt");
    }

    #[test]
    fn lookups_ignore_case() {
        let all = vec![arc(), AppSearch::spotify()];
        assert_eq!(by_keyword(&all, "Spotify").map(|s| s.name()), Some("Spotify"));
        assert!(by_keyword(&all, "spot").is_none());
        assert_eq!(
            for_app(&all, "com.spotify.client").map(|s| s.keyword.as_str()),
            Some("spotify")
        );
        // LaunchServices and `Info.plist` may disagree on case.
        assert_eq!(for_app(&all, "company.thebrowser.browser").map(|s| s.name()), Some("Arc"));
        assert!(for_app(&all, "com.apple.Safari").is_none());
    }

    #[test]
    fn validation() {
        for s in [arc(), AppSearch::spotify()] {
            s.validate().unwrap();
        }
        assert!(web("doc", "x{query}").validate().is_err(), "scope word");
        assert!(web("in", "x{query}").validate().is_err(), "filter word");
        assert!(web("two words", "x{query}").validate().is_err());
        assert!(web("a:b", "x{query}").validate().is_err());
        assert!(web("", "x{query}").validate().is_err());
        assert!(web("yt", "https://example.com/").validate().is_err(), "no placeholder");
        web("yt", "https://example.com/?q={query}").validate().unwrap();
    }
}
