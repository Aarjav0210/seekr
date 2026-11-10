# Keyboard shortcuts

Every key seekr understands, by where you are. `^` is Control, `⌥` Option,
`⌘` Command, `⇥` Tab, `⏎` Return, `⌫` Delete and `⌦` forward delete (fn+⌫).

"Quit / hide": in a terminal the finder quits; in the ⌘Space launcher
(seekr.app) the window hides instead and seekr stays ready for the next press.

## The launcher

| Key | Action |
|-----|--------|
| `⌘Space` | Show seekr on the current desktop and screen. |
| `⌘Space` while seekr is up, or a click elsewhere | Hide it, keeping the query: it comes back selected (see [A selected query](#a-selected-query)). |
| `⌘W`, `⌘H`, `⌘Q` | Hide it the same way. |
| `⌘V` | Paste into the query (line breaks dropped). |
| `⌘A` | Select the query. |

The hotkey is set by the installer's `--hotkey <mods+key>`, for example
`--hotkey ctrl+space`.

## Results

| Key | Action |
|-----|--------|
| `⏎` | Open the selected item, then quit / hide. The app that opens keeps the focus. |
| `^r`, `^o` | Reveal it in Finder, then quit / hide. |
| `^e` | Open the folder it is in, then quit / hide. |
| `^y` | Copy its path. |
| `^v` | Show it in Quick Look. |
| `⇥` | On Arc or Spotify, or with `arc` / `spotify` typed: search inside the app (see [Searching inside an app](#searching-inside-an-app)). Otherwise cycle the scope: none → `app:` → `doc:` → `dir:` → none, keeping the terms and filters. |
| `^x` | Refresh the index now. |
| `^l` | With a definition card showing: open the word in Dictionary.app, then quit / hide. |
| `esc` | Clear the query. On an empty query: quit / hide (you are back in the app you came from). |
| `^c`, `^g` | Quit / hide. |

## Moving the selection

| Key | Action |
|-----|--------|
| `↓`, `^n`, `^j` | Next row; from the last row, back to the first. |
| `↑`, `^p`, `^k` | Previous row; from the first row, to the last. |
| `PageDown` (fn+↓), `^d` | Ten rows down, stopping at the last. |
| `PageUp` (fn+↑) | Ten rows up, stopping at the first. |
| `Home` (fn+←) | First row. |
| `End` (fn+→) | Last row. |
| Mouse wheel | One row per notch. |

## Editing the query

| Key | Action |
|-----|--------|
| `←`, `→` | Move the caret one character. |
| `⌥←`, `⌥→` (also `⌥b`, `⌥f`, `^←`, `^→`) | Move the caret one word. |
| `⌘←`, `⌘→` | Move the caret to the start / end. |
| `^a` | Move the caret to the start. |
| `⌫` | Delete the character before the caret. |
| `⌦` | Delete the character after the caret. |
| `⌥⌫`, `^w` | Delete the word before the caret. A scope like `doc:` counts as a word. |
| `⌘⌫`, `^u` | Delete everything before the caret. |

`^e` and `End` are taken by the results, so end-of-line is `⌘→`.

Terminal notes: `⌘←`, `⌘→` and `⌘⌫` work in the launcher; in a terminal use
`^a`, `⌥→` and `^u`. `⌥` chords need the terminal to send Option as Alt (the
launcher does; in Terminal.app turn on "Use Option as Meta key"). macOS keeps
`^←` and `^→` for switching desktops unless you turn those shortcuts off.

## A selected query

After `⌘Space` hid seekr with text in the query, the text comes back selected:

| Key | Action |
|-----|--------|
| A character | Replace the query with it. |
| `⌫` | Clear the query. |
| `←` | Keep the query; caret to the start. |
| Any other key | Keep the query and do what the key normally does. |

## The calculator's answer

When the query is arithmetic, its answer is the first row and starts out selected.

| Key | Action |
|-----|--------|
| `⏎` | Copy the answer. In the launcher, also hide. |
| `^y` | Copy the answer and stay. |
| `↓` | Move on to the results. |

## Searching inside an app

After `⇥` on Arc or Spotify the query shows the app as a chip, `Arc ›`, and
what you type is the search. The editing keys work on the search terms; the
caret can't enter the chip.

| Key | Action |
|-----|--------|
| `⏎` | Search in the app (Arc: a Google search; Spotify: its search), then quit / hide. With nothing typed, just open the app. |
| `^y` | Copy the search URL. |
| `⌫` with nothing typed | Leave the app search; the query goes back to the keyword. |
| `esc` | Clear the query, leaving the app search. |
| `⇥`, `↑`, `↓`, `PageUp`, `PageDown`, `Home`, `End`, `^r`, `^o`, `^e`, `^v`, `^n`, `^p`, `^j`, `^k`, `^d` | Nothing: there is no list here. |

Typing a keyword with a colon, like `arc: rust lifetimes`, starts an app search
too. Other apps can be added in `~/.config/seekr/config.toml` (see the
[README](README.md#search-inside-apps)).
