# seekr

Fast local search for macOS, built because Spotlight stopped finding installed apps.

`seekr` keeps its own index of your applications, documents and folders, matches
them with a fuzzy engine (nucleo, the matcher behind the Helix editor) and opens
the result with one keypress. It never depends on Spotlight's index.

```
seekr                       # interactive finder
seekr app: whatsapp         # finder, pre-filled
seekr search doc: offer letter -n 5
seekr search --json app: chrome
seekr search --fresh offer      # refresh first, then search
seekr open ~/Downloads/x.pdf
seekr refresh               # pick up new / changed / deleted files
seekr index [--full]        # rebuild from scratch
seekr doctor
```

## Query language

| Syntax            | Meaning                                              |
|-------------------|------------------------------------------------------|
| `whatsapp`        | fuzzy match across everything; apps are boosted      |
| `app: whatsapp`   | only applications (`apps:`, `application:` also work)|
| `doc: offer letter` | documents: pdf, docx, pages, md, txt, csv, …       |
| `dir: aeo`        | folders (`folder:` also works)                       |
| `img:` `vid:` `audio:` `zip:` `code:` `file:` | other kinds             |
| `in:Downloads`    | path must contain this                               |
| `ext:pdf`         | name must end with `.pdf`                            |

Filters combine: `cv in:Downloads ext:pdf`.

## Keys (TUI)

| Key        | Action                     |
|------------|----------------------------|
| `⏎`        | open, then quit            |
| `^r` / `^o`| reveal in Finder           |
| `^e`       | open enclosing folder      |
| `^y`       | copy path                  |
| `^v`       | Quick Look                 |
| `⇥`        | cycle scope none→app→doc→dir |
| `^x`       | refresh the index          |
| `esc`      | clear the query; when empty, quit (Spotlight-style) |
| `^u`       | clear the query            |
| `^w` / `⌥⌫`| delete the last word       |
| `^c`       | quit                       |

## Ctrl+Space launcher (Hammerspoon + Terminal.app)

```
contrib/install-launcher.sh
```

Installs Hammerspoon if needed, drops `contrib/hammerspoon/seekr.lua` into
`~/.hammerspoon`, and creates a "seekr" Terminal profile that closes its window
when the shell exits. Ctrl+Space then pops a centered Terminal window running
seekr; Enter or Esc makes it disappear again. Pressing the hotkey while the
window is open just focuses it.

First run only: allow Hammerspoon under System Settings › Privacy & Security ›
Accessibility (global hotkeys), and allow it to control Terminal when macOS
asks (Automation).

The popup appears immediately but seekr itself shows up after your login shell
has initialised (about 1.4 s on a shell with conda/nvm; seekr starts in 7 ms).
Trimming `.zshrc`, or using a terminal with a native hotkey window (iTerm2,
Ghostty), removes that delay.

## Keeping the index fresh

Apps are rescanned live every time seekr starts. Files and folders come from
the index, which is refreshed:

- **on every launch** of the TUI, in the background (`refresh_on_launch`, default
  on). The walk takes about 1.5 s and new items stream into the results as they
  are found, so a file you just downloaded shows up a second or two after opening
  seekr.
- **on demand** with `^x` in the TUI (also suggested whenever a search has no
  results), or `seekr refresh` / `seekr search --fresh` from the shell.

A refresh diffs the walk against the cached index and writes only what changed,
then reports it: `refreshed in 1.6s · +1 added · −0 removed · 0 updated`.
Deleted files disappear from the index immediately but stay visible in an
already-open TUI until the next launch. Live FSEvents-based updates are the
planned next step.

## How it works

```
crates/
  seekr-core/   library, no UI. The layer an AI agent would call.
    item        Item + Kind taxonomy
    query       query language parser
    providers/  apps (folder scan incl. *.localized wrappers, LaunchServices dump)
                files (parallel gitignore-aware walk, package bundles as one item,
                       virtualenvs skipped, repo contents flagged)
    index       SQLite: items + usage (frecency) + meta
    search      nucleo multi-column matcher + re-ranking
    actions     open / reveal / copy / quick-look
    doctor      Spotlight + index diagnostics
  seekr/        binary: clap CLI + ratatui TUI
```

Ranking = fuzzy score × kind boost × depth penalty × repo penalty + frecency.
Everything you open through seekr is remembered in `usage`, so frequently used
items float up over time.

Index lives in `~/Library/Application Support/seekr/index.db`.
Config lives in `~/.config/seekr/config.toml` (roots, app dirs, skip dirs, lazy kinds).

Source files (`code` kind) are indexed but only loaded into the matcher when you
scope with `code:`; this keeps the default view to apps, docs, folders and media.

## Install

```
cargo install --path crates/seekr
```

This puts `seekr` in `~/.cargo/bin`, which rustup adds to your PATH. Re-run it
after pulling changes. For development, `cargo build --release` and run
`target/release/seekr`.

## Roadmap

- Live updates via FSEvents instead of periodic re-walks.
- More providers: browser bookmarks/history, contacts, system settings panes,
  clipboard history, calculator, "actions" (`> sleep`, `> lock`).
- Content search for documents (PDF/text extraction into an FTS5 table).
- MCP server exposing `search`, `open`, `reveal`, `index` as tools for an agent.
- Global hotkey launcher (Hammerspoon/skhd recipe, or a tiny Swift shim).
