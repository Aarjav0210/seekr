use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Coarse category of an item. Doubles as the search scope vocabulary
/// (`app:`, `doc:`, `dir:` ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    App,
    Doc,
    Folder,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    File,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::App,
        Kind::Doc,
        Kind::Folder,
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Archive,
        Kind::Code,
        Kind::File,
    ];

    /// Canonical short name; also what is stored in the index and matched by scope prefixes.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Doc => "doc",
            Kind::Folder => "folder",
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Archive => "archive",
            Kind::Code => "code",
            Kind::File => "file",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::App => "Application",
            Kind::Doc => "Document",
            Kind::Folder => "Folder",
            Kind::Image => "Image",
            Kind::Video => "Video",
            Kind::Audio => "Audio",
            Kind::Archive => "Archive",
            Kind::Code => "Source file",
            Kind::File => "File",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Kind::App => "",
            Kind::Doc => "",
            Kind::Folder => "",
            Kind::Image => "",
            Kind::Video => "",
            Kind::Audio => "",
            Kind::Archive => "",
            Kind::Code => "",
            Kind::File => "",
        }
    }

    /// Parse a user-facing scope word (without the trailing colon). Accepts aliases.
    pub fn from_scope(word: &str) -> Option<Kind> {
        Some(match word.to_ascii_lowercase().as_str() {
            "app" | "apps" | "application" | "applications" => Kind::App,
            "doc" | "docs" | "document" | "documents" => Kind::Doc,
            "dir" | "dirs" | "folder" | "folders" | "directory" => Kind::Folder,
            "img" | "image" | "images" | "pic" | "pics" | "photo" | "photos" => Kind::Image,
            "vid" | "video" | "videos" | "movie" | "movies" => Kind::Video,
            "audio" | "music" | "song" | "songs" | "sound" => Kind::Audio,
            "zip" | "archive" | "archives" => Kind::Archive,
            "code" | "src" | "source" => Kind::Code,
            "file" | "files" => Kind::File,
            _ => return None,
        })
    }

    pub fn from_db(s: &str) -> Kind {
        Kind::from_scope(s).unwrap_or(Kind::File)
    }

    /// Multiplier applied to the fuzzy score when no scope is given, so that
    /// e.g. an app called "Notes" outranks a stray `notes.txt` in a repo.
    pub fn base_boost(self) -> f32 {
        match self {
            Kind::App => 1.6,
            Kind::Doc => 1.2,
            Kind::Folder => 1.15,
            Kind::Image | Kind::Video | Kind::Audio => 1.0,
            Kind::Archive => 0.9,
            Kind::File => 0.85,
            Kind::Code => 0.7,
        }
    }

    /// Classify a file by extension (lowercase, without dot).
    pub fn from_ext(ext: &str) -> Kind {
        match ext {
            "app" => Kind::App,
            "pdf" | "doc" | "docx" | "ppt" | "pptx" | "xls" | "xlsx" | "pages" | "numbers" | "key"
            | "txt" | "md" | "markdown" | "rtf" | "rtfd" | "csv" | "tsv" | "epub" | "odt" | "ods"
            | "odp" | "tex" | "bib" | "org" => Kind::Doc,
            "png" | "jpg" | "jpeg" | "gif" | "heic" | "heif" | "svg" | "webp" | "tiff" | "tif"
            | "bmp" | "ico" | "icns" | "raw" | "cr2" | "nef" | "arw" | "dng" | "psd" | "ai"
            | "sketch" | "fig" | "avif" => Kind::Image,
            "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "flv" | "wmv" | "mpg" | "mpeg" => {
                Kind::Video
            }
            "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "aiff" | "aif" | "wma" | "opus"
            | "caf" => Kind::Audio,
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "dmg" | "pkg"
            | "iso" | "jar" | "whl" => Kind::Archive,
            "rs" | "go" | "py" | "js" | "mjs" | "cjs" | "ts" | "jsx" | "tsx" | "java" | "kt"
            | "kts" | "swift" | "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "m" | "mm"
            | "cs" | "rb" | "php" | "sh" | "zsh" | "bash" | "fish" | "lua" | "sql" | "toml"
            | "yaml" | "yml" | "json" | "jsonc" | "xml" | "html" | "htm" | "css" | "scss"
            | "sass" | "less" | "vue" | "svelte" | "astro" | "dart" | "scala" | "clj" | "cljs"
            | "ex" | "exs" | "erl" | "hs" | "ml" | "mli" | "r" | "jl" | "ipynb" | "proto"
            | "graphql" | "gql" | "tf" | "hcl" | "nix" | "zig" | "v" | "vhd" | "asm" | "s"
            | "cmake" | "gradle" | "lock" | "env" | "ini" | "cfg" | "conf" | "plist"
            | "entitlements" | "xcconfig" | "pbxproj" | "storyboard" | "xib" | "strings" => {
                Kind::Code
            }
            _ => Kind::File,
        }
    }

    /// Classify by filename (handles extension-less well-known code files).
    pub fn from_filename(name: &str) -> Kind {
        match name {
            "Makefile" | "Dockerfile" | "Justfile" | "Rakefile" | "Gemfile" | "Procfile"
            | "CMakeLists.txt" | "Podfile" | "Brewfile" | "Vagrantfile" | "LICENSE" => {
                return Kind::Code;
            }
            _ => {}
        }
        match Path::new(name).extension().and_then(|e| e.to_str()) {
            Some(ext) => Kind::from_ext(&ext.to_ascii_lowercase()),
            None => Kind::File,
        }
    }
}

/// Directory extensions that macOS treats as a single opaque document/package.
/// We emit them as one item and never descend into them.
pub const PACKAGE_EXTS: &[&str] = &[
    "app",
    "pages",
    "numbers",
    "key",
    "rtfd",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "imovielibrary",
    "fcpbundle",
    "logicx",
    "band",
    "xcodeproj",
    "xcworkspace",
    "playground",
    "framework",
    "bundle",
    "appex",
    "prefpane",
    "qlgenerator",
    "sparsebundle",
    "scptd",
    "workflow",
    "download",
];

/// One findable thing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// Display name. For apps this is the bundle's file stem ("Visual Studio Code").
    pub name: String,
    pub path: PathBuf,
    /// Alternate name that should also match (e.g. CFBundleName "Code" for VS Code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub size: u64,
    /// Modification time, unix seconds.
    #[serde(default)]
    pub modified: i64,
    /// Bit flags, see `FLAG_*`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// The item lives inside a git repository (source tree noise for most searches).
pub const FLAG_IN_REPO: u32 = 1 << 0;

impl Item {
    pub fn new(kind: Kind, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Item {
            kind,
            name: name.into(),
            path: path.into(),
            alt: None,
            bundle_id: None,
            size: 0,
            modified: 0,
            flags: 0,
        }
    }

    pub fn in_repo(&self) -> bool {
        self.flags & FLAG_IN_REPO != 0
    }

    /// Text the fuzzy matcher sees for the primary column: name plus alt name.
    pub fn search_text(&self) -> String {
        match &self.alt {
            Some(alt) if !alt.eq_ignore_ascii_case(&self.name) => format!("{} {}", self.name, alt),
            _ => self.name.clone(),
        }
    }

    /// Path with the home directory collapsed to `~`.
    pub fn path_display(&self) -> String {
        tilde(&self.path)
    }

    /// Parent directory with `~` collapsed.
    pub fn parent_display(&self) -> String {
        self.path.parent().map(tilde).unwrap_or_default()
    }

    /// Number of path components; used as a mild relevance penalty for deep files.
    pub fn depth(&self) -> usize {
        self.path.components().count()
    }
}

/// Collapse the home directory prefix to `~`.
pub fn tilde(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            let rest = rest.to_string_lossy();
            return if rest.is_empty() {
                "~".to_string()
            } else {
                format!("~/{rest}")
            };
        }
    }
    p.to_string_lossy().into_owned()
}

/// Expand a leading `~` to the home directory.
pub fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            let rest = rest.trim_start_matches('/');
            return if rest.is_empty() { home } else { home.join(rest) };
        }
    }
    PathBuf::from(s)
}
