# seekr

```text
███████╗███████╗███████╗██╗  ██╗██████╗
██╔════╝██╔════╝██╔════╝██║ ██╔╝██╔══██╗
███████╗█████╗  █████╗  █████╔╝ ██████╔╝
╚════██║██╔══╝  ██╔══╝  ██╔═██╗ ██╔══██╗
███████║███████╗███████╗██║  ██╗██║  ██║
╚══════╝╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝
```

Fast local search for macOS, in the terminal or behind ⌘Space. Type a few
letters to find an app, System Settings pane, document, folder or file and
open it with one key. Type `2+2` for an answer, a word for its definition,
`⇧⏎` to search the web for whatever you typed, or press `⇥` on a browser or
Spotify to search inside the app.

seekr keeps its own index and never relies on Spotlight's, which can break
silently: when `mdutil` reports an unknown indexing state for the home volume,
Spotlight cannot see installed apps or downloaded files and offers the App
Store instead. `seekr doctor` tells you whether your Mac is affected.

Every key is listed in [KEYBOARD_SHORTCUTS.md](KEYBOARD_SHORTCUTS.md).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/Aarjav0210/seekr/main/install.sh | sh
```

This puts the latest release, one binary for Apple silicon and Intel Macs
(macOS 11 or newer), in `~/.local/bin` and adds that folder to your PATH in
`~/.zshrc`. Then it offers to set up [the launcher](#the-launcher): it
installs kitty and Hammerspoon with Homebrew (and Homebrew itself if it's
missing) and turns off Spotlight's ⌘Space shortcut. macOS asks you to open
Hammerspoon, to let your terminal control it, and to allow it under
Privacy & Security › Accessibility. The first index can make macOS ask whether
your Desktop, Documents and Downloads may be read; allow it, or seekr won't find
files there.

Run the same command again to upgrade. Options go after `sh -s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/Aarjav0210/seekr/main/install.sh | sh -s -- --no-launcher
```

- `--no-launcher` installs just the `seekr` command; `--yes` sets up the
  launcher without asking.
- `--hotkey ctrl+space` uses another key and gives ⌘Space back to Spotlight;
  later runs keep it.
- `--opacity 1` makes the window solid black instead of translucent (any
  number from 0 to 1; later runs keep it).
- `--version v1.2.1` installs that release instead of the latest.
- `--uninstall` removes seekr, the launcher, the index and the settings, takes
  `~/.local/bin` back off your PATH and gives ⌘Space back. kitty and
  Hammerspoon stay installed.

From source, with Rust 1.85 or newer:

```sh
cargo install --path crates/seekr      # the binary, into ~/.cargo/bin
contrib/install-launcher.sh            # optional: the ⌘Space launcher
```

## Features

### Finder

`seekr` opens an interactive finder in the terminal; `seekr app: whatsapp`
starts it with a query. Matching is fuzzy and ignores case, and covers apps'
display names too (`code` finds Visual Studio Code). Each row shows the kind,
the name with the matched letters underlined, and the folder it is in. Windows
90 columns or wider add a details pane with the kind, path, size, last edit
and bundle id. With an empty query, recently opened items come first.

`⏎` opens the selection, `^r` reveals it in Finder, `^e` opens its folder,
`^y` copies its path, `^v` shows it in Quick Look and `^x` refreshes the
index. `⇧⏎` searches the web for the query instead (see [Search the
web](#search-the-web)). `esc` clears the query; a second `esc` quits.

What is searched:

- **Apps**, scanned live at every start from /Applications, ~/Applications,
  /System/Applications, their Utilities folders, CoreServices and Finder,
  wrapper folders such as `WhatsApp.localized` included (Spotlight misses
  those).
- **System Settings panes**, like Spotlight: `mouse` opens System Settings ›
  Mouse, `dark mode` finds Appearance, `dock` finds Desktop & Dock. The panes
  installed on the Mac are scanned live at every start (macOS 13 or later),
  named as the sidebar names them, with a few extra words each; `seekr
  settings` lists them. See [Settings panes](#settings-panes).
- **Places**: your home folder (also found as `home`), Desktop, Documents,
  Downloads, Pictures, Movies, Music, Public, Library, iCloud Drive, Trash,
  /Applications, /Applications/Utilities, ~/Applications and every root.
- **Files and folders** in the roots: by default whichever of ~/Desktop,
  ~/Documents, ~/Downloads, ~/Pictures, ~/Movies, ~/Music, ~/Developer,
  ~/Projects and ~/Code exist, 14 levels deep. Hidden files, files a git
  repository ignores, dependency and build folders (`node_modules`, `target`,
  `.venv`, `DerivedData`, …) and Python virtualenvs are left out. Bundles
  such as `.app`, `.pages`, `.key`, `.photoslibrary` and `.xcodeproj` are
  one item each.

Everything is an app, settings pane, document, folder, image, video, audio
file, archive, source file or other file. Source files are indexed but only
loaded when you ask for them with `code:`, which keeps everyday results to
things you open.

### Query language

| Query | Finds |
|-------|-------|
| `whatsapp` | Everything, fuzzy; apps rank first. |
| `app: whatsapp` | Apps only. |
| `settings: mouse` | System Settings panes only (also `prefs:`). |
| `doc: offer letter` | Documents: pdf, doc(x), ppt(x), xls(x), pages, numbers, key, md, txt, rtf, csv, epub, … |
| `dir: aeo` | Folders. |
| `img:` `vid:` `audio:` `zip:` `code:` `file:` | Images, video, audio, archives, source files, other files. |
| `is:app`, `kind:doc` | The same scopes, written as a filter. |
| `in:Downloads` | The path contains `Downloads` (also `path:`; `in:~/Desktop/Dev` works). |
| `ext:pdf` | The name ends in `.pdf` (also `type:`). |
| `safari: rust lifetimes` | Everything after the keyword goes to your browser; see [Search inside apps](#search-inside-apps). |

Scopes and filters combine in any order: `cv in:Downloads ext:pdf`. Scope
words also come plural or as synonyms: `apps:`, `folder:`, `images:`,
`photos:`, `movies:`, `music:` (audio), `src:`. `seekr kinds` lists the scopes.

### Search inside apps

Select a browser or Spotify (or type its keyword: `safari`, `firefox`,
`chrome`, `arc`, …, or `spotify`) and press `⇥`. The name turns into a chip,
`Firefox ›`, and what you type next is the search: `⏎` opens a Google search
in that browser, or Spotify's search, and seekr gets out of the way. `⏎` with
nothing typed just opens the app, `⌫` on an empty search goes back, and `^y`
copies the URL. Typing `firefox: rust lifetimes` works too. A search counts as
a use of the app for ranking.

Every browser installed in the application folders is offered, and the one
macOS opens links with comes first. Only installed apps count: on a Mac
without Arc, `arc` is an ordinary word and `⇥` cycles the scope to `app: arc`
as usual. `seekr searches` lists what was found, with the keywords.

Any app with a URL scheme, and any browser, can be added in
`~/.config/seekr/config.toml`. The list replaces the built-in one, so copy the
browser and Spotify entries from the commented template in that file:

```toml
[[searches]]
keyword = "yt"
name = "YouTube"
url = "https://www.youtube.com/results?search_query={query}"   # {query}: the terms, percent-encoded

[[searches]]
keyword = "obsidian"
url = "obsidian://search?query={query}"
app = "md.obsidian"      # the bundle id that opens it: osascript -e 'id of app "Obsidian"'
```

A keyword can't be a scope or filter word (`app`, `doc`, `in`, …). Apps
without a search URL (Slack, Notion, Notes, …) can't be added, and an entry
whose `app` is not installed is left out until it is. `seekr searches` lists
the searches and whether their apps are installed, and `seekr search-in
spotify daft punk` runs one from the shell.

### Search the web

Type anything, `what's the weather`, and press `⇧⏎`: a Google search for it
opens in your default browser and seekr gets out of the way. `^s` does the
same in terminals that can't tell `⇧⏎` from `⏎` (kitty, which hosts the
launcher, can). When a query matches nothing at all, plain `⏎` searches the web
too, once the index has finished loading, so nothing you meant to open is
pre-empted. In an app search, `⇧⏎` sends the terms to the web instead of the
app.

The browser is whichever one macOS opens links with; the URL template is the
one configured for that browser in `searches`, so switching it to DuckDuckGo
there switches this too. `seekr web what's the weather` does it from the
shell; `--print` shows the URL.

### Settings panes

Type the name of a System Settings pane, `mouse`, `displays`, `wallpaper`, and
`⏎` opens System Settings on it, the way Spotlight does; the launcher hides
and System Settings keeps the focus. The row says `System Settings` where a
file's row shows its folder, and the details pane shows the pane's extra
words and the URL that opens it: `^y` copies the URL, `^r` reveals System
Settings in Finder. A pane has no Quick Look.

Every pane in the sidebar is there, General's sub-panes included (Software
Update, Storage, Sharing, Time Machine, Date & Time, …), and each knows a few
other words it is usually looked for by: `dark mode` finds Appearance, `dock`
and `hot corners` Desktop & Dock, `wifi` Wi-Fi, `backup` Time Machine, `energy`
Battery. The list comes from the pane extensions installed on the Mac
(`/System/Library/ExtensionKit/Extensions` and System Settings' own
plug-ins), so a Mac without Touch ID or a pane a macOS update retired never
offers it; a pane seekr doesn't know by name is still listed, named after its
identifier. macOS also hides a few panes while their hardware is away (Mouse
without a mouse connected, Headphones without AirPods): those stay listed,
and opening one shows System Settings where it was. `seekr settings` prints
them all, and `seekr open
x-apple.systempreferences:com.apple.Mouse-Settings.extension` opens one from
the shell.

### Calculator

Type arithmetic and the answer is pinned above the results, selected. `⏎`
copies it (the launcher then hides and hands focus back), `^y` copies it and
stays, `↓` moves on to the files.

It understands `+ - * / ^ %` (also `×`, `÷`, `**` and `3 x 4`), parentheses
(`5(6+7)` and `(1+2)(3+4)` multiply, left to right like `*`), `1e3`,
`1,234,567`, `pi`, `tau`, `e`, `5!`, `50%`, `15% of 80`, `200 + 10%` (220),
`√16`, and `sqrt abs round floor ceil ln log log2 log10 exp sin cos tan asin
acos atan` (in radians). Answers keep 14 significant digits and use scientific
notation outside 1e-6 to 1e15.

Anything that parses as arithmetic gets an answer, dates (`2024-01-05` is
2018) and phone numbers included, with the matching files still listed below
it. Plain numbers, versions (`1.2.3`) and names like `IMG_1234` or `report-2`
just search; `1/0` has no answer.

### Definitions

Type a single English word and a definition card appears, like Spotlight's:
headword, pronunciation, senses by part of speech, and the origin when there
is room. It sits under the details pane (under the results in narrow
windows), or where the results would be when nothing matches. `^l` opens the
word in Dictionary.app; `⏎` still opens the selected result.

The entry comes from macOS's own dictionary (the Oxford Dictionary of English
by default), offline, looked up in the background 100 ms after you stop
typing. Only bare words are looked up (letters, with `-` or `'` inside), not
scoped queries, filters, app searches or file names. Inflections find their
entry (`notes` → note, `went` → go); names that aren't words (`spotify`) get
no card. macOS downloads its dictionaries the first time anything asks, so the
very first lookup on a fresh Mac can come back empty.

### The launcher

The installer (or `contrib/install-launcher.sh` in a checkout) binds ⌘Space,
through Hammerspoon, to a seekr
window without a title bar, centred on the screen and desktop you are working
on. The first press starts it in about a second. After that seekr stays
resident, and each press brings it back within a couple of frames, ready to
type, while the index refreshes in the background.

- `⏎` opens the selection and hides the window; the app you opened keeps the focus.
- `esc` clears the query, then hides the window and hands focus back to where you were.
- ⌘Space while seekr is in front hides it too, keeping the query: it comes
  back selected, so typing replaces it and any other key carries on with it.
- `^c` quits the resident seekr; the next ⌘Space starts a fresh one.

⌘Space belongs to Spotlight, and on macOS 15 also to Siri when held, so the
installer turns those two shortcuts off. Spotlight and Siri keep working;
System Settings › Keyboard › Keyboard Shortcuts › Spotlight and System
Settings › Siri turn the shortcuts back on. For another key, run the installer
with `--hotkey ctrl+space` (or `contrib/install-launcher.sh --hotkey
ctrl+space`): it gives ⌘Space back, and later runs keep the key you chose. When another app already holds
the key as a shared hotkey, Hammerspoon watches for it with an event tap
instead; the installer prints which of the two it uses.

The window is a kitty panel (it can appear on any desktop), started by
Hammerspoon with `seekr --resident` and no shell in between, so `~/.zshrc`
never slows it down. It is translucent black with the desktop blurred behind
it, like Spotlight; `--opacity 1` (installer or `contrib/install-launcher.sh`)
makes it solid. Hammerspoon shows and hides it with kitty's
`--toggle-visibility` and tells seekr through signals: SIGUSR1 when it shows
the window (seekr refreshes), SIGUSR2 when the hotkey hid it (seekr keeps the
query). seekr asks Hammerspoon to hide the window after `⏎` or `esc`. The
window is seekr's alone: kitty's own shortcuts (`⌘T`, `⌘N`, `⌘W`, …) are off
in it, so no key opens a shell there, and should the window ever outlive
seekr, the next ⌘Space replaces it with a fresh one. The Lua and the kitty
config live in `contrib/`.

## Command line

Every capability is also a subcommand, most with `--json` for scripts and
agents:

```sh
seekr                              # the finder (also `seekr tui`)
seekr app: whatsapp                # the finder, with a query
seekr --resident                   # the finder that hides instead of quitting (for the launcher)
seekr search doc: offer letter     # one-shot search, printed as a table
seekr search --json -n 5 cv        # … as JSON: [{item, score}]
seekr search --all main            # include source files without `code:`
seekr search --fresh invoice       # refresh the index first
seekr open ~/Downloads/x.pdf       # open, counting the use for ranking; --reveal shows it in Finder
seekr search-in spotify daft punk  # search inside an app; --print shows the URL instead
seekr web what's the weather       # search the web in the default browser; --print shows the URL
seekr searches                     # the apps ⇥ can search in, and whether they are installed
seekr settings                     # the System Settings panes the finder opens
seekr open x-apple.systempreferences:com.apple.Mouse-Settings.extension   # open a pane
seekr calc '(1+2)^10/7'            # 8435.5714285714; exits 1 when it isn't a calculation
seekr define serendipity           # the dictionary entry; exits 1 when there is none
seekr kinds                        # the query language's scopes and filters
seekr refresh                      # pick up new, changed and deleted files
seekr index [--full]               # rebuild the index; --full also asks LaunchServices for apps elsewhere
seekr doctor                       # Spotlight's health and the index's
```

## Freshness

Apps and settings panes are scanned live every time seekr starts. Everything
else comes from the index, which is refreshed in the background whenever the finder starts or the
launcher shows it (at most every 5 seconds), and on demand with `^x`, `seekr
refresh` or `seekr search --fresh`. A refresh re-walks the roots (about 1.5 s
for 150k items), writes only what changed and reports it: `refreshed in 1.6s ·
+1 added · −0 removed · 0 updated`. New files stream into an open finder as
they are found. Deleted ones leave the index at once, but an open finder keeps
listing them until seekr restarts.

## Ranking

Each result scores fuzzy match × kind × place or exact-app boost × depth
penalty × repository penalty, plus frecency:

- Apps outrank settings panes, which outrank documents, which outrank
  folders, media, archives, other files and source files, unless the query
  has a scope.
- A settings pane named exactly what you typed (`mouse`, `sound`, or one of
  its extra words: `dark mode`) comes before apps that merely start with it;
  an app of that exact name still wins, and so does an app that starts with
  the letters typed so far (`key`: Keynote, then Keyboard; `keyb`: Keyboard).
- Typing the start of a place's name (`downl`, `desk`, `icloud`, `home`)
  makes it the top hit, above apps that merely contain the word; an app named
  exactly what you typed still comes first (`music`: Music, then ~/Music).
- Deeply nested paths and anything inside a git checkout sink, unless you
  scope to `code:`.
- Every open through seekr is counted, and what you open often and recently
  floats up, by a capped amount.

## Configuration

`~/.config/seekr/config.toml` (or `$XDG_CONFIG_HOME/seekr/config.toml`) is
written on first run as a commented template of the defaults; uncomment a key
to change it.

| Key | Default | Meaning |
|-----|---------|---------|
| `roots` | The existing ones of ~/Desktop, ~/Documents, ~/Downloads, ~/Pictures, ~/Movies, ~/Music, ~/Developer, ~/Projects, ~/Code | Folders walked for files and folders. |
| `app_dirs` | /Applications, ~/Applications, /System/Applications, their Utilities, CoreServices, Finder | Where apps are looked for. |
| `skip_dirs` | `node_modules`, `.git`, `target`, `build`, `dist`, `.venv`, `Library`, … | Folder names never walked into. Setting it replaces the list. |
| `max_depth` | `14` | Levels below each root. |
| `respect_gitignore` | `true` | Leave out what `.gitignore` files inside git repositories ignore. |
| `refresh_on_launch` | `true` | Refresh in the background when the finder starts and when the launcher shows it. |
| `lazy_kinds` | `["code"]` | Kinds only loaded when a query asks for them. |
| `searches` | installed browsers, Spotify | The apps `⇥` searches in; see [Search inside apps](#search-inside-apps). |

The index and usage log live in `~/Library/Application Support/seekr/index.db`;
`SEEKR_DATA_DIR` moves them. `SEEKR_DEBUG=1` shows each result's score in the
details pane and in `seekr search`.

## Layout

```text
crates/seekr-core   library, no UI: items, query parser, providers (apps, settings
                    panes, places, files), SQLite index and usage log, nucleo matcher and ranking,
                    actions (open, reveal, copy, Quick Look), app searches,
                    calculator, definitions (macOS dictionary), doctor
crates/seekr        the binary: CLI (main.rs, cli.rs), finder (tui/: state and
                    keys, query line, drawing, definition card), background
                    loading, launcher glue (resident mode)
contrib/            the ⌘Space launcher: Hammerspoon config, kitty config, installer;
                    package.sh, which builds the release archive
install.sh          the curl installer: downloads a release, installs and uninstalls
```

The core crate has no UI and every command speaks JSON, so the same
operations can later be exposed to an AI agent as tools. `cargo test` runs the
unit tests; the dictionary test is skipped on a Mac without the dictionary.

To release, bump `version` in `crates/seekr/Cargo.toml`, then push a matching
tag: `git tag v0.2.0 && git push origin v0.2.0`.
`.github/workflows/release.yml` tests, builds the universal binary with
`contrib/package.sh` and publishes it as a GitHub release, which `install.sh`
picks up as the latest.

## Roadmap

- FSEvents watcher for live index updates.
- Document content search (text and PDF extraction into FTS5).
- More providers: browser bookmarks and history, contacts, clipboard, actions
  such as `> sleep`.
- MCP server exposing search, open, reveal and refresh as agent tools.
