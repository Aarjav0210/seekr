# seekr

```text
███████╗███████╗███████╗██╗  ██╗██████╗
██╔════╝██╔════╝██╔════╝██║ ██╔╝██╔══██╗
███████╗█████╗  █████╗  █████╔╝ ██████╔╝
╚════██║██╔══╝  ██╔══╝  ██╔═██╗ ██╔══██╗
███████║███████╗███████╗██║  ██╗██║  ██║
╚══════╝╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝
```

Fast local search for macOS: apps, documents, folders and media, found by fuzzy
matching and opened with one keypress.

Built because Spotlight's index can silently break. On a machine where
`mdutil` reports an unknown state for the home volume, Spotlight cannot see
installed apps or downloaded files and sends you to the App Store instead.
seekr keeps its own index and never touches Spotlight's. `seekr doctor` tells
you whether Spotlight is broken on your machine.

## Install

```sh
cargo install --path crates/seekr      # binary → ~/.cargo/bin/seekr
contrib/install-launcher.sh            # optional: Cmd+Space popup
```

The launcher script installs kitty and Hammerspoon, drops
`contrib/hammerspoon/seekr.lua` into `~/.hammerspoon` and
`contrib/kitty/seekr.conf` into `~/.config/seekr/kitty.conf`. Cmd+Space opens
an undecorated kitty window (no title bar) running `seekr --resident` directly,
with no login shell in between, so `~/.zshrc` is never sourced. The window is a
kitty panel (kitty 0.42 or newer), which is on every desktop at once: it pops up
on whatever desktop you are on, on the screen you are working on, instead of
switching you back to the desktop it was first opened on. kitty itself shows
and hides the window (its `--toggle-visibility`), because a window that macOS
hid and showed again lands back on the desktop it was created on and only its
owner can re-attach it to the current one. That kitty is
kept out of Cmd+Tab and the Dock; the hotkey is the only way to it. The first press
creates the window in a few hundred milliseconds. After that seekr stays alive:
Enter and Esc hide the window instead of quitting, and the next press
reveals it again within a couple of frames, ready to type, while the index
refreshes in the background. After Enter the app that was just opened keeps
the focus; after Esc, focus goes back to the window you came from. Cmd+Space while seekr is in front hides it again,
whatever the query says; the query is kept and comes back selected, so typing
replaces it and an arrow key (or Enter, Tab, any `^` chord) keeps it. Ctrl+C
inside seekr quits it for real. The
resident process holds the index in memory (tens of MB) and idles otherwise.
macOS will ask once to allow Hammerspoon under Accessibility (global hotkeys,
window placement).

Cmd+Space is Spotlight's shortcut and, on macOS 15, Siri's (hold), so the
installer turns both shortcuts off (Spotlight and Siri themselves are
untouched; System Settings › Keyboard › Keyboard Shortcuts › Spotlight and
System Settings › Siri › Keyboard shortcut turn them back on). For another
key run `contrib/install-launcher.sh --hotkey ctrl+space`: the script then
gives Cmd+Space back to Spotlight and Siri, and later runs keep the key you
chose. If some other app already holds the key as a shared global hotkey,
Hammerspoon cannot register it and the Lua watches for the key with an event
tap instead; the installer prints which of the two is in use.

## Use

```sh
seekr                              # interactive finder
seekr --resident                   # finder that hides instead of quitting (launcher)
seekr app: whatsapp                # finder, pre-filled
seekr search doc: offer letter     # one-shot, prints a table
seekr search --json -n 5 cv        # JSON for scripts and agents
seekr search --fresh invoice       # refresh the index, then search
seekr open ~/Downloads/x.pdf       # open (records usage for ranking)
seekr search-in spotify daft punk  # hand a query to an app (⇥ in the finder)
seekr searches                     # the apps ⇥ can search in
seekr define serendipity [--json]  # the macOS dictionary's entry (exit 1: none)
seekr refresh                      # pick up new, changed and deleted files
seekr index [--full]               # rebuild; --full also asks LaunchServices
seekr doctor                       # Spotlight health + index stats
```

### Query language

| Syntax                | Meaning                                                   |
|-----------------------|-----------------------------------------------------------|
| `whatsapp`            | fuzzy match across everything; apps rank first            |
| `app: whatsapp`       | applications only                                         |
| `doc: offer letter`   | documents: pdf, docx, pages, md, txt, csv, …              |
| `dir: aeo`            | folders                                                   |
| `img:` `vid:` `audio:` `zip:` `code:` `file:` | other kinds                       |
| `in:Downloads`        | path must contain this                                    |
| `ext:pdf`             | name must end with `.pdf`                                 |
| `arc: rust lifetimes` | hand the rest, verbatim, to Arc; `spotify:` likewise (`⇥` types this for you) |

Filters combine: `cv in:Downloads ext:pdf`. Scope words accept plurals and
synonyms (`apps:`, `folder:`, `image:`). Source files (`code:`) are indexed but
only loaded when you ask for them, which keeps the default view to the things
you actually launch.

### Keys

| Key          | Action                                        |
|--------------|-----------------------------------------------|
| `⏎`          | open, then quit (launcher: hide)              |
| `^r`         | reveal in Finder                              |
| `^e`         | open enclosing folder                         |
| `^y`         | copy path                                     |
| `^v`         | Quick Look                                    |
| `⇥`          | on Arc or Spotify: search inside the app; otherwise cycle scope: none → app → doc → dir |
| `^x`         | refresh the index                             |
| `^l`         | open the definition card's word in Dictionary.app |
| `esc`        | clear the query; on an empty query, quit (launcher: hide) |
| `^u` / `^w`  | clear the query / delete the last word        |
| `^c`         | quit, also in the launcher                    |
| `⌘space`     | launcher only (the hotkey): hide, keeping the query selected for next time |

### Searching inside apps

Select Arc or Spotify (or type `arc` / `spotify`) and press `⇥`. The keyword
turns into a chip and everything you type next is the search: `⏎` opens
`https://www.google.com/search?q=…` in Arc, or `spotify:search:…` in Spotify,
and hides the window as usual. `⏎` with nothing typed just opens the app, `⌫`
on an empty search goes back to the results, `^y` copies the URL. A search
counts as a use of the app for ranking.

Under the hood this is `open -b <bundle id> <url>`, so any app that registers
a URL scheme (or a browser, which takes `https`) can be added in
`~/.config/seekr/config.toml`:

```toml
[[searches]]
keyword = "yt"
name = "YouTube"
url = "https://www.youtube.com/results?search_query={query}"   # {query} is percent-encoded

[[searches]]
keyword = "obsidian"
url = "obsidian://search?query={query}"
app = "md.obsidian"      # bundle id: osascript -e 'id of app "Obsidian"'
```

Setting `searches` replaces the built-in list, so copy the Arc and Spotify
entries from the commented template in the config file. Keywords cannot be
scope words (`app`, `doc`, `in`, …). Apps without a search URL (Slack, Notion,
Notes, …) cannot be added this way. `seekr searches` lists the configured
searches and whether their apps are installed; `seekr search-in spotify daft
punk` runs one from the shell, and `--print --json` shows what it would open.

### Definitions

Type a single English word and a definition card appears, like Spotlight's:
headword, pronunciation, senses by part of speech, and the origin when there
is room. It sits under the details pane (under the results in narrow
windows), or where the results would be when nothing in the index matches.
`^l` opens the word in Dictionary.app; `⏎` still opens the selected result.

The entry comes from macOS's own dictionary (DictionaryServices, the Oxford
Dictionary of English by default), offline, looked up on a background thread
about 100 ms after you stop typing. Only bare words are looked up (letters,
inner `-` or `'`, two or more): not scoped queries (`app: notes`), filters,
app searches or file names. Inflections resolve to their entry (`notes` →
note, `went` → go); names that are not words (`spotify`) get no card. macOS
downloads its dictionaries the first time anything asks for one, so the very
first lookup on a fresh machine can come back empty. `seekr define <word>
--json` prints the parsed entry (and the raw text) for scripts and agents.

## Freshness

Apps are rescanned live on every start. Files come from the index, which is
refreshed in the background on every launch and every time the launcher reveals
a resident seekr (about 1.5 s for 150k items), and on demand with `^x`,
`seekr refresh`, or `seekr search --fresh`. A refresh
diffs the walk against the cache and writes only the delta, then reports it:
`refreshed in 1.6s · +1 added · −0 removed · 0 updated`. New files stream into
an open finder as they are found; deleted ones leave the index immediately but
stay visible in that session until the next launch.

## Ranking

fuzzy score × kind boost × depth penalty × repo penalty + frecency.

Apps outrank files of the same name. Deeply nested paths and anything inside a
git checkout are pushed down unless you scope to `code:`. Every open through
seekr is logged, so things you use often float up. Python virtualenvs,
`node_modules`, build output and similar noise are never indexed.

## Layout

```
crates/seekr-core   library, no UI: items, query parser, providers (apps, files),
                    SQLite index + usage log, nucleo matcher + re-ranking,
                    actions (open/reveal/copy/quick-look), app searches (⇥),
                    definitions (macOS dictionary), doctor
crates/seekr        binary: clap CLI + ratatui TUI; launcher glue (resident mode)
contrib/            Cmd+Space launcher (Hammerspoon config, kitty config)
```

Index: `~/Library/Application Support/seekr/index.db`.
Config: `~/.config/seekr/config.toml`, written as a commented template on first
run; uncomment a key to override roots, app dirs, skip dirs or lazy kinds.

The core crate is deliberately UI-free and every command speaks JSON, so the
same operations can later be exposed to an AI agent as tools.

## Roadmap

- FSEvents watcher for live index updates.
- Document content search (text/PDF extraction into FTS5).
- More providers: browser bookmarks and history, contacts, settings panes,
  clipboard, calculator, actions such as `> sleep`.
- MCP server exposing search / open / reveal / refresh as agent tools.
