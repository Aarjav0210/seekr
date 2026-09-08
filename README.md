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
contrib/install-launcher.sh            # optional: Ctrl+Space popup
```

The launcher script installs kitty and Hammerspoon, drops
`contrib/hammerspoon/seekr.lua` into `~/.hammerspoon` and
`contrib/kitty/seekr.conf` into `~/.config/seekr/kitty.conf`. Ctrl+Space opens
an undecorated kitty window (no title bar) running `seekr --resident` directly,
with no login shell in between, so `~/.zshrc` is never sourced. The first press
creates the window in a few hundred milliseconds. After that seekr stays alive:
Enter and Esc hide the window instead of quitting, and the next Ctrl+Space
reveals it again within a couple of frames, ready to type, while the index
refreshes in the background. Ctrl+C inside seekr quits it for real. The
resident process holds the index in memory (tens of MB) and idles otherwise.
macOS will ask once to allow Hammerspoon under Accessibility (global hotkeys,
window placement).

## Use

```sh
seekr                              # interactive finder
seekr --resident                   # finder that hides instead of quitting (launcher)
seekr app: whatsapp                # finder, pre-filled
seekr search doc: offer letter     # one-shot, prints a table
seekr search --json -n 5 cv        # JSON for scripts and agents
seekr search --fresh invoice       # refresh the index, then search
seekr open ~/Downloads/x.pdf       # open (records usage for ranking)
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
| `⇥`          | cycle scope: none → app → doc → dir           |
| `^x`         | refresh the index                             |
| `esc`        | clear the query; on an empty query, quit (launcher: hide) |
| `^u` / `^w`  | clear the query / delete the last word        |
| `^c`         | quit, also in the launcher                    |

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
                    actions (open/reveal/copy/quick-look), doctor
crates/seekr        binary: clap CLI + ratatui TUI; launcher glue (resident mode)
contrib/            Ctrl+Space launcher (Hammerspoon config, kitty config)
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
