use crate::app_search::{self, AppSearch};
use crate::item::{expand_tilde, Kind};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// User configuration, stored at `~/.config/seekr/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Directories to walk for files/folders. `~` is expanded.
    pub roots: Vec<String>,
    /// Directories (or `.app` paths) to scan for applications.
    pub app_dirs: Vec<String>,
    /// Directory names that are never descended into.
    pub skip_dirs: Vec<String>,
    /// Maximum depth below each root.
    pub max_depth: usize,
    /// Honour `.gitignore` files inside git repositories.
    pub respect_gitignore: bool,
    /// Re-walk the roots in the background every time the TUI starts.
    pub refresh_on_launch: bool,
    /// Kinds that are only loaded into the matcher when explicitly scoped
    /// (e.g. `code:`). Keeps the default view small and fast.
    pub lazy_kinds: Vec<String>,
    /// Apps that Tab hands the query to (`arc`, `spotify`). See [`AppSearch`];
    /// setting this replaces the built-in list.
    pub searches: Vec<AppSearch>,
}

impl Default for Config {
    fn default() -> Self {
        let candidates = [
            "~/Desktop",
            "~/Documents",
            "~/Downloads",
            "~/Pictures",
            "~/Movies",
            "~/Music",
            "~/Developer",
            "~/Projects",
            "~/Code",
        ];
        let roots = candidates
            .iter()
            .filter(|c| expand_tilde(c).is_dir())
            .map(|s| s.to_string())
            .collect();
        Config {
            roots,
            app_dirs: [
                "/Applications",
                "/Applications/Utilities",
                "~/Applications",
                "/System/Applications",
                "/System/Applications/Utilities",
                "/System/Library/CoreServices/Applications",
                "/System/Library/CoreServices/Finder.app",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            skip_dirs: [
                "node_modules",
                ".git",
                "target",
                "build",
                "dist",
                ".venv",
                "venv",
                "env",
                "__pycache__",
                ".cache",
                ".next",
                ".nuxt",
                ".turbo",
                ".parcel-cache",
                "Pods",
                "DerivedData",
                ".Trash",
                "Library",
                ".npm",
                ".cargo",
                ".rustup",
                ".gradle",
                ".idea",
                ".vscode",
                "vendor",
                "bower_components",
                ".terraform",
                ".mypy_cache",
                ".pytest_cache",
                ".tox",
                ".svn",
                ".hg",
                "site-packages",
                ".build",
                ".swiftpm",
                "xcuserdata",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            max_depth: 14,
            respect_gitignore: true,
            refresh_on_launch: true,
            lazy_kinds: vec!["code".to_string()],
            searches: AppSearch::builtin(),
        }
    }
}

impl Config {
    pub fn config_dir() -> PathBuf {
        if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
            if !x.is_empty() {
                return PathBuf::from(x).join("seekr");
            }
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("seekr")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// `~/Library/Application Support/seekr` on macOS. Overridable with `SEEKR_DATA_DIR`.
    pub fn data_dir() -> PathBuf {
        if let Ok(x) = std::env::var("SEEKR_DATA_DIR") {
            if !x.is_empty() {
                return PathBuf::from(x);
            }
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seekr")
    }

    pub fn db_path() -> PathBuf {
        Self::data_dir().join("index.db")
    }

    /// Load the config. Missing keys fall back to defaults, so the file only
    /// needs to contain what the user wants to change. On first run a fully
    /// commented template is written so defaults keep evolving with the code.
    pub fn load() -> Result<Config> {
        let path = Self::config_path();
        if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let cfg: Config =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            cfg.validate()
                .with_context(|| format!("in {}", path.display()))?;
            Ok(cfg)
        } else {
            let cfg = Config::default();
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).ok();
            }
            fs::write(&path, cfg.template()).ok();
            Ok(cfg)
        }
    }

    /// App searches must have usable keywords and templates; see [`AppSearch::validate`].
    pub fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for s in &self.searches {
            s.validate()?;
            if !seen.insert(s.keyword.to_ascii_lowercase()) {
                bail!("app search keyword {:?} is listed twice", s.keyword);
            }
        }
        Ok(())
    }

    /// A commented-out config showing every option with its current default.
    pub fn template(&self) -> String {
        fn arr(v: &[String]) -> String {
            let items: Vec<toml::Value> = v.iter().map(|s| toml::Value::String(s.clone())).collect();
            toml::Value::Array(items).to_string()
        }
        // One commented `[[searches]]` table per built-in search.
        let searches = self
            .searches
            .iter()
            .map(|s| {
                let body = toml::to_string(s).unwrap_or_default();
                let mut block = String::from("# [[searches]]\n");
                for line in body.lines() {
                    block.push_str("# ");
                    block.push_str(line);
                    block.push('\n');
                }
                block
            })
            .collect::<Vec<_>>()
            .join("#\n");
        format!(
            "# seekr configuration. Every key is optional; unset keys use the built-in\n\
             # defaults shown below. Uncomment a line to override it. `~` is expanded.\n\
             \n\
             # Folders walked for files and folders.\n\
             # roots = {roots}\n\
             \n\
             # Where applications are looked for (folders or .app paths).\n\
             # app_dirs = {app_dirs}\n\
             \n\
             # Directory names never descended into. Setting this replaces the whole list.\n\
             # skip_dirs = {skip}\n\
             \n\
             # Maximum depth below each root.\n\
             # max_depth = {depth}\n\
             \n\
             # Honour .gitignore files inside git repositories.\n\
             # respect_gitignore = {gi}\n\
             \n\
             # Re-walk the roots in the background every time the TUI starts so new\n\
             # downloads appear within a second or two. Ctrl-x refreshes on demand.\n\
             # refresh_on_launch = {rol}\n\
             \n\
             # Kinds only loaded into the matcher when explicitly scoped (e.g. `code:`).\n\
             # lazy_kinds = {lazy}\n\
             \n\
             # Apps that Tab hands the query to: select the app (or type its keyword),\n\
             # press Tab, type the terms, Enter. `{{query}}` is replaced by the\n\
             # percent-encoded terms; `app` is the bundle id that must open the URL\n\
             # (`osascript -e 'id of app \"Arc\"'` prints it). Only apps with a URL scheme,\n\
             # or browsers, can be searched this way. Setting this replaces the whole\n\
             # list, so keep the entries you still want.\n\
             {searches}",
            roots = arr(&self.roots),
            app_dirs = arr(&self.app_dirs),
            skip = arr(&self.skip_dirs),
            depth = self.max_depth,
            gi = self.respect_gitignore,
            rol = self.refresh_on_launch,
            lazy = arr(&self.lazy_kinds),
            searches = searches,
        )
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .map(|r| expand_tilde(r))
            .filter(|p| p.is_dir())
            .collect()
    }

    pub fn app_dir_paths(&self) -> Vec<PathBuf> {
        self.app_dirs
            .iter()
            .map(|r| expand_tilde(r))
            .filter(|p| p.exists())
            .collect()
    }

    pub fn is_lazy(&self, kind: Kind) -> bool {
        self.lazy_kinds.iter().any(|k| k == kind.as_str())
    }

    /// Kinds loaded into the matcher up front: all but the lazy ones.
    pub fn eager_kinds(&self) -> Vec<Kind> {
        Kind::ALL.into_iter().filter(|&k| !self.is_lazy(k)).collect()
    }

    /// The app search whose keyword is `word` (`arc`), case-insensitively.
    pub fn search_by_keyword(&self, word: &str) -> Option<&AppSearch> {
        app_search::by_keyword(&self.searches, word)
    }

    /// The app search attached to the app with this bundle identifier.
    pub fn search_for_app(&self, bundle_id: &str) -> Option<&AppSearch> {
        app_search::for_app(&self.searches, bundle_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_in_the_template_parse_back_to_the_builtins() {
        let toml_text: String = Config::default()
            .template()
            .lines()
            .skip_while(|l| !l.starts_with("# [[searches]]"))
            .filter_map(|l| l.strip_prefix("# "))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg: Config = toml::from_str(&toml_text).unwrap();
        assert_eq!(cfg.searches, AppSearch::builtin());
    }

    #[test]
    fn a_config_without_searches_keeps_the_builtins() {
        let cfg: Config = toml::from_str("max_depth = 3").unwrap();
        assert_eq!(cfg.max_depth, 3);
        assert_eq!(cfg.searches, AppSearch::builtin());
    }

    #[test]
    fn a_searches_list_replaces_the_builtins_and_is_validated() {
        let text = "[[searches]]\nkeyword = \"yt\"\nurl = \"https://youtube.com/results?search_query={query}\"\n";
        let cfg: Config = toml::from_str(text).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.searches.len(), 1);
        assert_eq!(cfg.searches[0].name(), "yt");
        assert!(cfg.searches[0].app.is_none());
        let dup = format!("{text}{text}");
        assert!(toml::from_str::<Config>(&dup).unwrap().validate().is_err());
        let scope = "[[searches]]\nkeyword = \"doc\"\nurl = \"x{query}\"\n";
        assert!(toml::from_str::<Config>(scope).unwrap().validate().is_err());
    }
}
