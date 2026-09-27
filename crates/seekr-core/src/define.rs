//! Dictionary definitions from macOS's built-in dictionary: the same offline
//! source (DictionaryServices, the Oxford Dictionary of English by default)
//! that Spotlight and Look Up use.
//!
//! [`define`] looks a single word up, checks that the entry really is for that
//! word (or an inflection of it: `notes` → `note`, `went` → `go`), parses the
//! plain-text entry into a [`Definition`] and caches the answer, misses
//! included. On other platforms every lookup is a miss.
//!
//! macOS downloads the dictionaries on first use; until they are there,
//! lookups miss and [`available`] is false (misses are then not cached).
use crate::app_search::percent_encode;
use crate::query::Query;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A dictionary entry, parsed from the dictionary's plain text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    /// What was looked up, as given.
    pub word: String,
    /// The entry's headword; differs from `word` for inflections (`notes` → `note`).
    pub headword: String,
    /// IPA, as the dictionary writes it (`ˌsɛrənˈdɪpɪti`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronunciation: Option<String>,
    /// Variants and labels before the first part of speech: `(also okay) informal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Senses grouped by part of speech, in dictionary order.
    pub parts: Vec<PartOfSpeech>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The dictionary's plain-text entry, unparsed.
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartOfSpeech {
    /// `noun`, `verb`, …; `None` for entries without one (places, product names).
    pub name: Option<String>,
    /// Grammar and inflections: `[mass noun]`, `(runs, running; past ran)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub senses: Vec<Sense>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sense {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// The `•` senses under a numbered one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsenses: Vec<Sense>,
}

impl Definition {
    /// `dict://<headword>`: opens this entry in Dictionary.app.
    pub fn url(&self) -> String {
        dictionary_url(&self.headword)
    }
}

/// `dict://<word>`, the URL Dictionary.app opens at that word.
pub fn dictionary_url(word: &str) -> String {
    format!("dict://{}", percent_encode(word.trim()))
}

/// Show `word` in Dictionary.app.
pub fn open_in_dictionary(word: &str) -> anyhow::Result<()> {
    crate::actions::open_url(&dictionary_url(word), None)
}

/// Whether `s` is worth a lookup: one word of letters (hyphens and apostrophes
/// allowed inside), two letters or more. Paths, numbers, file names and
/// multi-word queries are not.
pub fn is_word(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 || chars.len() > 45 {
        return false;
    }
    let first = chars[0];
    let last = chars[chars.len() - 1];
    first.is_alphabetic()
        && last.is_alphabetic()
        && chars
            .iter()
            .all(|c| c.is_alphabetic() || matches!(c, '-' | '\'' | '’'))
        && !s.contains("--")
}

/// The word a finder query asks to define, if any: a bare single word, with no
/// scope (`app:`), filters (`in:`, `ext:`) or app search (`arc:`).
pub fn word_for_query(q: &Query) -> Option<&str> {
    if q.scope.is_some() || q.in_dir.is_some() || q.ext.is_some() || q.app_search.is_some() {
        return None;
    }
    let w = q.terms.trim();
    is_word(w).then_some(w)
}

const CACHE_CAP: usize = 512;

fn cache() -> &'static Mutex<HashMap<String, Option<Definition>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Definition>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(word: &str) -> String {
    word.trim().to_lowercase()
}

/// A cached answer for `word`: `Some(None)` is a remembered miss, `None` means
/// it has not been looked up yet.
pub fn cached(word: &str) -> Option<Option<Definition>> {
    let key = cache_key(word);
    let mut hit = cache().lock().ok()?.get(&key).cloned()?;
    if let Some(d) = hit.as_mut() {
        d.word = word.trim().to_string();
    }
    Some(hit)
}

/// Look `word` up in the system dictionary (cached). `None` when it is not a
/// word ([`is_word`]), has no entry, or the entry is for something else.
///
/// Takes ~1–10 ms once the dictionary is loaded, but the first lookup in a
/// process loads it (~100–250 ms): call this off the UI thread.
pub fn define(word: &str) -> Option<Definition> {
    let word = word.trim();
    if !is_word(word) {
        return None;
    }
    if let Some(hit) = cached(word) {
        return hit;
    }
    let found = lookup(word);
    // Before macOS has downloaded the dictionaries everything misses; don't
    // remember those misses, the next attempt may well succeed.
    if found.is_some() || available() {
        if let Ok(mut c) = cache().lock() {
            if c.len() >= CACHE_CAP {
                c.clear();
            }
            c.insert(cache_key(word), found.clone());
        }
    }
    found
}

/// Load the dictionary now so the first real lookup is fast.
pub fn warm_up() {
    let _ = available();
}

/// Whether the system dictionary answers at all (a common word resolves).
pub fn available() -> bool {
    sys::whole_term("the")
}

/// One uncached lookup.
fn lookup(word: &str) -> Option<Definition> {
    // The dictionary finds terms inside longer strings and prefixes
    // (`the cat` → `the`); only an entry for the whole query counts.
    if !sys::whole_term(word) {
        return None;
    }
    let raw = sys::copy_definition(word)?;
    let def = parse(word, &raw)?;
    is_entry_for(word, &def).then_some(def)
}

/// Whether `def` is the entry for `word`: its headword, a regular inflection
/// of it, or a form the entry itself lists (`went` in `go`, `mice` in `mouse`).
pub fn is_entry_for(word: &str, def: &Definition) -> bool {
    let w = word.trim().to_lowercase();
    let h = def.headword.to_lowercase();
    w == h || is_regular_inflection(&w, &h) || contains_word(&def.raw.to_lowercase(), &w)
}

fn is_regular_inflection(w: &str, head: &str) -> bool {
    let Some(rest) = w.strip_prefix(head) else {
        // happy → happier, study → studies, make → making
        if let Some(stem) = head.strip_suffix('y') {
            if let Some(r) = w.strip_prefix(stem) {
                return matches!(r, "ies" | "ied" | "ier" | "iest");
            }
        }
        if let Some(stem) = head.strip_suffix('e') {
            if let Some(r) = w.strip_prefix(stem) {
                return matches!(r, "ing" | "ed" | "er" | "est");
            }
        }
        return false;
    };
    matches!(rest, "s" | "es" | "ed" | "d" | "ing" | "er" | "est" | "'s" | "’s")
        // stop → stopped, run → running
        || rest.chars().next().is_some_and(|c| {
            head.ends_with(c) && matches!(&rest[c.len_utf8()..], "ed" | "ing" | "er" | "est")
        })
}

/// `needle` appears in `hay` as a whole word (letters, `-` and `'` join words).
fn contains_word(hay: &str, needle: &str) -> bool {
    let joins = |c: char| c.is_alphanumeric() || matches!(c, '-' | '\'' | '’');
    hay.match_indices(needle).any(|(i, m)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + m.len()..].chars().next();
        !before.is_some_and(joins) && !after.is_some_and(joins)
    })
}

// ---------------------------------------------------------------------------
// Parsing the plain-text entry.
//
// DictionaryServices returns one line per entry, e.g.
//   `arc | ɑːk | noun 1 a part of a curve…: the arc of a circle. • a shape…
//    verb (arcs, arcing | ˈɑːkɪŋ |, arced) [no object] 1 … ORIGIN late Middle English…`
// headword (with a homograph number: `cat 1`), `|pronunciation|`, then parts
// of speech each followed by numbered senses (`1`, `2`, …) with `•` subsenses;
// `definition: example | example`; upper-case section words (ORIGIN, PHRASES,
// DERIVATIVES, USAGE) at the end. Apple's own dictionary uses a heading line
// and paragraphs instead. Anything unrecognised ends up as one plain sense.

/// Part-of-speech labels, longest first so `plural noun` wins over `noun`.
const PARTS_OF_SPEECH: &[&str] = &[
    "possessive determiner",
    "possessive pronoun",
    "infinitive particle",
    "cardinal number",
    "ordinal number",
    "combining form",
    "auxiliary verb",
    "phrasal verb",
    "plural noun",
    "proper noun",
    "modal verb",
    "predeterminer",
    "abbreviation",
    "interjection",
    "conjunction",
    "preposition",
    "exclamation",
    "contraction",
    "determiner",
    "adjective",
    "pronoun",
    "article",
    "adverb",
    "symbol",
    "prefix",
    "suffix",
    "number",
    "noun",
    "verb",
];

/// Section words that end the senses.
const SECTIONS: &[&str] = &["PHRASAL VERBS", "PHRASES", "DERIVATIVES", "USAGE", "ORIGIN"];

/// Parse a dictionary entry (the text `DCSCopyTextDefinition` returns) for `word`.
/// `None` only for empty text; unrecognised layouts become a single sense.
pub fn parse(word: &str, raw: &str) -> Option<Definition> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let mut def = Definition {
        word: word.trim().to_string(),
        headword: String::new(),
        pronunciation: None,
        note: None,
        parts: Vec::new(),
        origin: None,
        raw: raw.to_string(),
    };
    let first_line = text.lines().next().unwrap_or("");
    if text.contains('\n') && !first_line.contains('|') {
        // Apple Dictionary: a heading line, then paragraphs.
        def.headword = first_line.trim().to_string();
        let senses: Vec<Sense> = text
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(plain_sense)
            .collect();
        def.parts.push(PartOfSpeech {
            name: None,
            note: None,
            senses,
        });
        return Some(finish(def, text));
    }

    let flat = collapse_ws(text);
    let (main, origin) = split_sections(&flat);
    def.origin = origin;

    // Header: `headword [n] | pron |` (sometimes `headword pos [note] | pron |`).
    let (head, pron, body) = match header_split(main) {
        Some((h, p, b)) => (h, Some(p), b),
        None => {
            let (h, b) = main.split_once(' ').unwrap_or((main, ""));
            (h, None, b)
        }
    };
    def.pronunciation = pron.map(str::to_string).filter(|p| !p.is_empty());
    let mut head_tokens: Vec<&str> = head.split_whitespace().collect();
    if head_tokens.len() > 1 && head_tokens.last().is_some_and(|t| t.chars().all(|c| c.is_ascii_digit())) {
        head_tokens.pop();
    }
    // `discord noun [mass noun] | ˈdɪskɔːd | 1 …`: the part of speech came first.
    let pos_at = (1..head_tokens.len()).find(|&i| pos_at_start(&head_tokens[i..].join(" ")).is_some());
    let body = match pos_at {
        Some(i) => {
            let moved = head_tokens.split_off(i).join(" ");
            collapse_ws(&format!("{moved} {body}"))
        }
        None => body.to_string(),
    };
    def.headword = head_tokens.join(" ");
    if def.headword.is_empty() {
        def.headword = def.word.clone();
    }

    let (note, parts) = parse_body(body.trim());
    def.note = note;
    def.parts = parts;
    Some(finish(def, text))
}

/// Never hand out an entry without senses: fall back to the text itself.
fn finish(mut def: Definition, text: &str) -> Definition {
    def.parts.retain(|p| !p.senses.is_empty());
    if def.parts.is_empty() {
        def.parts.push(PartOfSpeech {
            name: None,
            note: None,
            senses: vec![plain_sense(&collapse_ws(text))],
        });
    }
    def
}

fn plain_sense(s: &str) -> Sense {
    Sense {
        text: s.trim().to_string(),
        examples: Vec::new(),
        subsenses: Vec::new(),
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bracket depth before each byte of `s`: `(`/`[` open, `)`/`]` close.
fn depths(s: &str) -> Vec<u32> {
    let mut out = Vec::with_capacity(s.len());
    let mut d: u32 = 0;
    for (_, c) in s.char_indices() {
        for _ in 0..c.len_utf8() {
            out.push(d);
        }
        match c {
            '(' | '[' => d += 1,
            ')' | ']' => d = d.saturating_sub(1),
            _ => {}
        }
    }
    out
}

/// Cut the trailing sections (ORIGIN, PHRASES, …); return the senses part and the origin.
fn split_sections(s: &str) -> (&str, Option<String>) {
    let d = depths(s);
    let mut marks: Vec<(usize, &str)> = Vec::new();
    for sec in SECTIONS {
        let pat = format!(" {sec} ");
        let mut from = 0;
        while let Some(i) = s[from..].find(&pat) {
            let at = from + i;
            if d[at] == 0 {
                marks.push((at, sec));
            }
            from = at + pat.len();
        }
        if s.ends_with(&format!(" {sec}")) {
            marks.push((s.len() - sec.len() - 1, sec));
        }
    }
    marks.sort();
    let Some(&(first, _)) = marks.first() else {
        return (s, None);
    };
    let origin = marks.iter().enumerate().find(|(_, m)| m.1 == "ORIGIN").map(|(i, &(at, sec))| {
        let start = (at + sec.len() + 2).min(s.len());
        let end = marks.get(i + 1).map(|m| m.0).unwrap_or(s.len()).max(start);
        s[start..end].trim().to_string()
    });
    (&s[..first], origin.filter(|o| !o.is_empty()))
}

/// `head | pron | body`, when the first `|` pair is close to the start.
fn header_split(s: &str) -> Option<(&str, &str, &str)> {
    let i = s.find('|')?;
    if s[..i].chars().count() > 80 {
        return None;
    }
    let after = &s[i + 1..];
    let j = after.find('|')?;
    if after[..j].chars().count() > 80 || after[..j].contains(':') {
        return None;
    }
    Some((s[..i].trim(), after[..j].trim(), &after[j + 1..]))
}

/// The part-of-speech label `s` starts with, as a whole word.
fn pos_at_start(s: &str) -> Option<&'static str> {
    PARTS_OF_SPEECH.iter().copied().find(|p| {
        s.starts_with(p) && s[p.len()..].chars().next().is_none_or(|c| c == ' ')
    })
}

/// Byte offsets (and labels) where a new part of speech starts in `body`.
fn pos_boundaries(body: &str) -> Vec<(usize, &'static str)> {
    let d = depths(body);
    let mut out = Vec::new();
    let mut prev_end: Option<char> = None; // last non-space char before this word
    let mut at_word_start = true;
    for (i, c) in body.char_indices() {
        if at_word_start && c != ' ' && d[i] == 0 {
            if let Some(p) = pos_at_start(&body[i..]) {
                let ok = if out.is_empty() {
                    // Leading labels only: `(also okay) informal exclamation …`.
                    let lead = &body[..i];
                    lead.split_whitespace().count() <= 8 && !lead.contains(": ") && !lead.contains(". ")
                        || prev_end.is_some_and(ends_sentence)
                } else {
                    prev_end.is_some_and(ends_sentence)
                };
                if ok {
                    out.push((i, p));
                }
            }
        }
        at_word_start = c == ' ';
        if c != ' ' {
            prev_end = Some(c);
        }
    }
    out
}

fn ends_sentence(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '’' | '”' | '"')
}

fn parse_body(body: &str) -> (Option<String>, Vec<PartOfSpeech>) {
    let bounds = pos_boundaries(body);
    let mut note = None;
    let mut parts = Vec::new();
    let lead_end = bounds.first().map(|b| b.0).unwrap_or(body.len());
    let lead = body[..lead_end].trim();
    if !lead.is_empty() {
        if bounds.is_empty() || lead.contains(": ") || lead.contains(". ") || lead.split_whitespace().count() > 8 {
            // No part of speech (places, names): the text is the sense.
            parts.push(parse_part(None, lead));
        } else {
            note = Some(strip_prons(lead));
        }
    }
    for (k, &(at, name)) in bounds.iter().enumerate() {
        let end = bounds.get(k + 1).map(|b| b.0).unwrap_or(body.len());
        parts.push(parse_part(Some(name), &body[at + name.len()..end]));
    }
    (note.filter(|n| !n.is_empty()), parts)
}

/// One part of speech: its grammar note, then numbered senses (or one sense).
fn parse_part(name: Option<&str>, s: &str) -> PartOfSpeech {
    let s = s.trim();
    let (note, rest) = if name.is_some() { take_groups(s) } else { ("", s) };
    let note = strip_prons(note);
    let senses = split_numbered(rest)
        .into_iter()
        .map(parse_sense)
        .filter(|s| !s.text.is_empty() || !s.examples.is_empty())
        .collect();
    PartOfSpeech {
        name: name.map(str::to_string),
        note: (!note.is_empty()).then_some(note),
        senses,
    }
}

/// Leading `(…)`, `[…]` and `|…|` groups: `(plural mice | mʌɪs |) [mass noun] 1 …`.
fn take_groups(s: &str) -> (&str, &str) {
    let mut end = 0;
    loop {
        let rest = &s[end..];
        let trimmed = rest.trim_start();
        let skip = rest.len() - trimmed.len();
        let close = match trimmed.chars().next() {
            Some('(') | Some('[') => {
                let d = depths(trimmed);
                // The group closes where the depth first drops back to 0.
                trimmed
                    .char_indices()
                    .skip(1)
                    .find(|&(i, _)| d[i] == 0)
                    .map(|(i, _)| i)
            }
            Some('|') => trimmed[1..].find('|').map(|j| j + 2),
            _ => None,
        };
        match close {
            Some(n) if n > 0 => end += skip + n,
            _ => break,
        }
    }
    (s[..end].trim(), s[end..].trim())
}

/// Drop inline pronunciations (`ran | ran |` → `ran`) from grammar notes.
fn strip_prons(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        if c == '|' {
            inside = !inside;
            continue;
        }
        if !inside {
            out.push(c);
        }
    }
    collapse_ws(&out).replace(" ,", ",").replace(" ;", ";").replace(" )", ")")
}

/// `1 … 2 … 3 …` → the numbered chunks; text without a leading `1` is one chunk.
fn split_numbered(s: &str) -> Vec<&str> {
    let Some(rest) = s.strip_prefix("1 ") else {
        return vec![s];
    };
    let base = s.len() - rest.len();
    let d = depths(s);
    let mut chunks = Vec::new();
    let mut start = base;
    let mut next = 2u32;
    let mut tok = format!("{next} ");
    let mut i = base;
    while i < s.len() {
        if s[i..].starts_with(&tok) && d[i] == 0 && s[..i].ends_with(' ') {
            chunks.push(&s[start..i]);
            start = i + tok.len();
            i = start;
            next += 1;
            tok = format!("{next} ");
            continue;
        }
        i += s[i..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    chunks.push(&s[start..]);
    chunks
}

/// `definition: example | example • subsense: example • …`
fn parse_sense(s: &str) -> Sense {
    let mut pieces = split_top(s, '•').into_iter().map(parse_piece);
    let mut sense = pieces.next().unwrap_or_else(|| plain_sense(""));
    sense.subsenses = pieces.filter(|p| !p.text.is_empty()).collect();
    sense
}

fn parse_piece(s: &str) -> Sense {
    let s = s.trim();
    let d = depths(s);
    // The first `: ` starts the examples, unless a sentence ended before it
    // (`… a long tail. Family Muridae: many genera …` is all definition).
    let colon = s
        .match_indices(": ")
        .map(|(i, _)| i)
        .find(|&i| d[i] == 0)
        .filter(|&i| !s[..i].contains(". "));
    let (text, examples) = match colon {
        Some(i) => (&s[..i], &s[i + 2..]),
        None => (s, ""),
    };
    let examples = split_top(examples, '|')
        .into_iter()
        .map(trim_example)
        .filter(|e| !e.is_empty())
        .collect();
    Sense {
        text: trim_period(text.trim()).to_string(),
        examples,
        subsenses: Vec::new(),
    }
}

fn trim_period(s: &str) -> &str {
    if s.ends_with("etc.") {
        return s;
    }
    s.strip_suffix('.').unwrap_or(s).trim_end()
}

fn trim_example(s: &str) -> String {
    let s = s.trim();
    // `humour me, OK?.` / `LOL!.`: the full stop only ends the sense.
    let s = s.strip_suffix('.').filter(|t| t.ends_with(['?', '!', '.', '’'])).unwrap_or(s);
    trim_period(s).to_string()
}

/// Split at `sep` outside brackets.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let d = depths(s);
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == sep && d[i] == 0 {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

// ---------------------------------------------------------------------------
// DictionaryServices.

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::c_void;
    use std::ptr;

    type CFIndex = isize;
    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CFRange {
        location: CFIndex,
        length: CFIndex,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(
            alloc: *const c_void,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: u32,
            is_external_representation: u8,
        ) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> CFIndex;
        fn CFStringGetBytes(
            s: CFStringRef,
            range: CFRange,
            encoding: u32,
            loss_byte: u8,
            is_external_representation: u8,
            buffer: *mut u8,
            max_buf_len: CFIndex,
            used_buf_len: *mut CFIndex,
        ) -> CFIndex;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn DCSGetTermRangeInString(
            dictionary: *const c_void,
            text: CFStringRef,
            offset: CFIndex,
        ) -> CFRange;
        fn DCSCopyTextDefinition(
            dictionary: *const c_void,
            text: CFStringRef,
            range: CFRange,
        ) -> CFStringRef;
    }

    /// An owned CFString (from a Create or Copy call), released on drop.
    struct CfString(CFStringRef);

    impl Drop for CfString {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }

    impl CfString {
        fn new(s: &str) -> Option<CfString> {
            let p = unsafe {
                CFStringCreateWithBytes(ptr::null(), s.as_ptr(), s.len() as CFIndex, UTF8, 0)
            };
            (!p.is_null()).then_some(CfString(p))
        }

        /// Length in UTF-16 units, the unit DictionaryServices ranges use.
        fn len(&self) -> CFIndex {
            unsafe { CFStringGetLength(self.0) }
        }

        fn to_string(&self) -> String {
            let range = CFRange {
                location: 0,
                length: self.len(),
            };
            let mut used: CFIndex = 0;
            unsafe {
                CFStringGetBytes(self.0, range, UTF8, 0, 0, ptr::null_mut(), 0, &mut used);
            }
            let mut buf = vec![0u8; used.max(0) as usize];
            unsafe {
                CFStringGetBytes(
                    self.0,
                    range,
                    UTF8,
                    0,
                    0,
                    buf.as_mut_ptr(),
                    buf.len() as CFIndex,
                    &mut used,
                );
            }
            buf.truncate(used.max(0) as usize);
            String::from_utf8_lossy(&buf).into_owned()
        }
    }

    /// Whether the dictionary sees all of `text` as one term.
    pub fn whole_term(text: &str) -> bool {
        let Some(s) = CfString::new(text) else {
            return false;
        };
        let r = unsafe { DCSGetTermRangeInString(ptr::null(), s.0, 0) };
        r.location == 0 && r.length == s.len()
    }

    /// The plain-text entry for all of `text`, if there is one.
    pub fn copy_definition(text: &str) -> Option<String> {
        let s = CfString::new(text)?;
        let range = CFRange {
            location: 0,
            length: s.len(),
        };
        let d = unsafe { DCSCopyTextDefinition(ptr::null(), s.0, range) };
        if d.is_null() {
            return None;
        }
        Some(CfString(d).to_string())
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    pub fn whole_term(_text: &str) -> bool {
        false
    }

    pub fn copy_definition(_text: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from DCSCopyTextDefinition (Oxford Dictionary of English, macOS 15).
    const SERENDIPITY: &str = "serendipity | ˌsɛrənˈdɪpɪti | noun [mass noun] the occurrence and development of events by chance in a happy or beneficial way: a fortunate stroke of serendipity | [count noun] : a series of small serendipities. ORIGIN 1754: coined by Horace Walpole, suggested by The Three Princes of Serendip, the title of a fairy tale in which the heroes ‘were always making discoveries, by accidents and sagacity, of things they were not in quest of’.";
    const EPHEMERAL: &str = "ephemeral | ɪˈfɛm(ə)rəl, ɛˈfɛm(ə)rəl | adjective lasting for a very short time: fashions are ephemeral: new ones regularly drive out the old | works of more than ephemeral interest. • (chiefly of plants) having a very short life cycle: chickweed is an ephemeral weed, producing several generations in one season. noun an ephemeral plant: ephemerals avoid the periods of drought as seeds. DERIVATIVES ephemerality | ɪfɛm(ə)ˈralɪti, ɪfiːm(ə)ˈralɪti | noun ephemerally adverb ephemeralness noun ORIGIN late 16th century: from Greek ephēmeros (see ephemera) + -al.";
    const ARC: &str = "arc | ɑːk | noun 1 a part of a curve, especially a part of the circumference of a circle: the point where a tangential line touches the arc of a circle. • a shape or structure resembling an arc: the huge arc of the sky. • a curving trajectory: he swung his torch in a wide arc. • [as modifier] Mathematics indicating the inverse of a trigonometrical function. [from the former method of defining trigonometrical functions by arcs] 2 (also electric arc) a luminous electrical discharge between two electrodes or other points: modern welding generators are designed so that there is a high voltage for striking the arc. 3 (in a novel, play, or film) the development or resolution of the narrative or principal theme: his transformation provides the emotional arc of the story. verb (arcs, arcing | ˈɑːkɪŋ |, arced | ɑːkt |) [no object] 1 [with adverbial of direction] move with a curving trajectory: the ball arced across the room. 2 form an electric arc: the outage was caused by a bad switch that arced | I have no idea whether they touched the power lines or arced. ORIGIN late Middle English (denoting the path of a celestial object, especially the sun, from horizon to horizon): via Old French from Latin arcus ‘bow, curve’.";
    const DISCORD: &str = "discord noun [mass noun] | ˈdɪskɔːd | 1 disagreement between people: a prosperous family who showed no signs of discord. • lack of agreement or harmony between things: the discord between indigenous and Western cultures. 2 Music lack of harmony between notes sounding together: the music faded in discord. • [count noun] a chord which (in conventional harmonic terms) is regarded as unpleasing or requiring resolution by another. • [count noun] any interval except unison, an octave, a perfect fifth or fourth, a major or minor third and sixth, or their octaves. • [count noun] a single note dissonant with another. verb [no object] | dɪˈskɔːd | archaic (of people) disagree: we discorded commonly on two points. • (of things) be different: the party's views were apt to discord with those of the leading members of the government. ORIGIN Middle English: from Old French descord (noun), descorder (verb), from Latin discordare, from discors ‘discordant’, from dis- (expressing negation, reversal) + cor, cord- ‘heart’.";
    const KITTY: &str = "kitty 1 | ˈkɪti | noun (plural kitties) 1 a fund of money for communal use, made up of contributions from a group of people. • a pool of money in some gambling card games. 2 (in bowls) the jack. ORIGIN early 19th century (denoting a jail): of unknown origin.";
    const OK: &str = "OK 1 | ˌəʊˈkeɪ | (also okay) informal exclamation used to express agreement or acceptance: OK, I'll pass on your message | OK, OK, I give in. • used to introduce an utterance: ‘OK, let's go’. • used at the end of a statement to invite agreement, approval, or confirmation: humour me, OK?. adjective satisfactory but not especially good: the flight was OK. • in a satisfactory physical or mental state: are you okay, Ben?. • permissible; allowable: it's not OK to say that to a teacher. adverb in a satisfactory manner or to a satisfactory extent: the computer continues to work OK. noun [in singular] an authorization or approval: the officer gave me the OK. verb (OKs, OKing, OK'd or OKed) [with object] give approval to: despite objections, the committee ok'd the construction. ORIGIN mid 19th century (originally US): probably an abbreviation of orl korrect, humorous form of all correct, popularized as a slogan during President Van Buren's re-election campaign of 1840 in the US; his nickname Old Kinderhook (derived from his birthplace) provided the initials.";
    const PARIS: &str = "Paris 1 | ˈparɪs | the capital of France, on the River Seine; population 2,203,817 (2006). Paris was held by the Romans, who called it Lutetia, and the Franks, and was established as the capital in 987 under Hugh Capet. It was organized into three parts, the Île de la Cité (an island in the Seine), the Right Bank, and the Left Bank, during the reign of Philippe-Auguste (1180–1223). The city's neoclassical architecture dates from the modernization of the Napoleonic era; this continued under Napoleon III, when the bridges and boulevards of the modern city were built ORIGIN named after the Parisii, a Gallic people who settled on the Île de la Cité.";
    const MOUSE: &str = "mouse | maʊs | noun (plural mice | mʌɪs |) 1 a small rodent that typically has a pointed snout, relatively large ears and eyes, and a long tail. Family Muridae: many genera and numerous species. Also, some species in the families Heteromyidae, Zapodidae, and Muscardinidae • (in general use) any small mammal similar to a mouse, such as a shrew or vole. • a shy, timid, and quiet person: Jane may be a bit of a mouse, but she is very nosy. • [mass noun] a dull light brown colour reminiscent of a mouse's fur: her flaxen hair dulled to mouse. 2 (plural mice or plural mouses) a small handheld device which is moved across a mat or flat surface to move the cursor on a computer screen: copy the file with a click of the mouse | [as modifier] : the right mouse button. 3 informal a lump or bruise on or near the eye: she touched the mouse under her eye. verb [no object] | maʊs | 1 (of a cat or owl) hunt for or catch mice: female cats are usually much better at mousing than males. • [with adverbial] prowl about as if searching: he was mousing among the books of the old library. 2 [with adverbial of direction] use a mouse to move or position a cursor on a computer screen: simply mouse over any item on the list. DERIVATIVES mouse-like adjective USAGE Is the plural of mouse in the computing sense mice or mouses? People often feel that this sense needs its own distinctive plural, but in fact the ordinary plural mice is commoner, and the first recorded use of the term in the plural (1984) is mice ORIGIN Old English mūs, (plural) mȳs, of Germanic origin; related to Dutch muis and German Maus, from an Indo-European root shared by Latin and Greek mus.";
    const CAT_HEAD: &str = "cat 1 | kat | noun 1 a small domesticated carnivorous mammal with soft fur, a short snout, and retractable claws. It is widely kept as a pet or for catching mice. The cat family also includes the ocelot, serval, margay, lynx, and the big cats • a wild animal of the cat family. See also big cat: a marbled cat. • historical short for cat-o'-nine-tails • short for catboat 2 informal, mainly North American English (especially among jazz enthusiasts) a man: this West Coast cat had managed him since the early 80s | the cat went crazy on the horn. 3 historical a short tapered stick used in the game of tipcat. verb (cats, catting, catted) [with object] Nautical raise (an anchor) from the surface of the water to the cathead: I kept her off the wind and sailing free until I had the anchor catted. PHRASES all cats are grey in the dark proverb the qualities that distinguish people from one another are obscured in some circumstances. ORIGIN Old English catt, catte, of Germanic origin.";
    const WAS: &str = "was | wɒz, wəz | verb first and third person singular past of be ";
    const XCODE: &str = "Xcode\nA set of software development tools from Apple used to create apps for Apple platforms including Mac, iPhone, iPad, and Apple\u{a0}Watch. Xcode includes the Xcode IDE, Swift, and Objective-C compilers, the Instruments analysis tool, OS simulators, the latest SDKs, and hundreds of powerful features. Xcode is available in the Mac App Store.\n";

    fn names(d: &Definition) -> Vec<Option<&str>> {
        d.parts.iter().map(|p| p.name.as_deref()).collect()
    }

    #[test]
    fn parses_a_single_sense_entry() {
        let d = parse("serendipity", SERENDIPITY).unwrap();
        assert_eq!(d.headword, "serendipity");
        assert_eq!(d.pronunciation.as_deref(), Some("ˌsɛrənˈdɪpɪti"));
        assert_eq!(names(&d), [Some("noun")]);
        let noun = &d.parts[0];
        assert_eq!(noun.note.as_deref(), Some("[mass noun]"));
        assert_eq!(noun.senses.len(), 1);
        let s = &noun.senses[0];
        assert_eq!(
            s.text,
            "the occurrence and development of events by chance in a happy or beneficial way"
        );
        assert_eq!(
            s.examples,
            ["a fortunate stroke of serendipity", "[count noun] : a series of small serendipities"]
        );
        assert!(d.origin.as_deref().unwrap().starts_with("1754: coined by Horace Walpole"));
        assert_eq!(d.raw, SERENDIPITY);
    }

    #[test]
    fn parses_several_parts_of_speech_and_subsenses() {
        let d = parse("ephemeral", EPHEMERAL).unwrap();
        assert_eq!(d.pronunciation.as_deref(), Some("ɪˈfɛm(ə)rəl, ɛˈfɛm(ə)rəl"));
        assert_eq!(names(&d), [Some("adjective"), Some("noun")]);
        let adj = &d.parts[0].senses[0];
        assert_eq!(adj.text, "lasting for a very short time");
        assert_eq!(
            adj.examples,
            [
                "fashions are ephemeral: new ones regularly drive out the old",
                "works of more than ephemeral interest"
            ]
        );
        assert_eq!(adj.subsenses.len(), 1);
        assert_eq!(adj.subsenses[0].text, "(chiefly of plants) having a very short life cycle");
        assert_eq!(d.parts[1].senses[0].text, "an ephemeral plant");
        // DERIVATIVES is cut off, ORIGIN kept.
        assert!(!format!("{:?}", d.parts).contains("ephemerality"));
        assert!(d.origin.as_deref().unwrap().starts_with("late 16th century"));
    }

    #[test]
    fn numbered_senses_and_inline_pronunciations() {
        let d = parse("arc", ARC).unwrap();
        assert_eq!(names(&d), [Some("noun"), Some("verb")]);
        let noun = &d.parts[0];
        assert_eq!(noun.senses.len(), 3);
        assert_eq!(noun.senses[0].subsenses.len(), 3);
        assert!(noun.senses[1].text.starts_with("(also electric arc) a luminous"));
        let verb = &d.parts[1];
        assert_eq!(verb.note.as_deref(), Some("(arcs, arcing, arced) [no object]"));
        assert_eq!(verb.senses.len(), 2);
        assert_eq!(verb.senses[1].text, "form an electric arc");
        assert_eq!(verb.senses[1].examples.len(), 2);
    }

    #[test]
    fn part_of_speech_before_the_pronunciation() {
        let d = parse("discord", DISCORD).unwrap();
        assert_eq!(d.headword, "discord");
        assert_eq!(d.pronunciation.as_deref(), Some("ˈdɪskɔːd"));
        assert_eq!(names(&d), [Some("noun"), Some("verb")]);
        assert_eq!(d.parts[0].note.as_deref(), Some("[mass noun]"));
        assert_eq!(d.parts[0].senses.len(), 2);
        assert_eq!(d.parts[0].senses[1].subsenses.len(), 3);
        assert_eq!(d.parts[1].note.as_deref(), Some("[no object]"));
        assert_eq!(d.parts[1].senses[0].text, "archaic (of people) disagree");
    }

    #[test]
    fn homograph_numbers_and_leading_labels() {
        let d = parse("kitty", KITTY).unwrap();
        assert_eq!(d.headword, "kitty");
        assert_eq!(d.parts[0].note.as_deref(), Some("(plural kitties)"));
        assert_eq!(d.parts[0].senses.len(), 2);
        assert_eq!(d.parts[0].senses[1].text, "(in bowls) the jack");

        let d = parse("ok", OK).unwrap();
        assert_eq!(d.headword, "OK");
        assert_eq!(d.note.as_deref(), Some("(also okay) informal"));
        assert_eq!(
            names(&d),
            [Some("exclamation"), Some("adjective"), Some("adverb"), Some("noun"), Some("verb")]
        );
        assert_eq!(d.parts[0].senses[0].subsenses[1].examples, ["humour me, OK?"]);
    }

    #[test]
    fn sense_numbers_without_a_full_stop_before_them() {
        let d = parse("cat", CAT_HEAD).unwrap();
        assert_eq!(d.headword, "cat");
        assert_eq!(names(&d), [Some("noun"), Some("verb")]);
        let noun = &d.parts[0];
        assert_eq!(noun.senses.len(), 3);
        // A colon after a full stop is not the start of the examples.
        assert!(noun.senses[0].examples.is_empty());
        assert_eq!(noun.senses[0].subsenses.len(), 3);
        assert!(noun.senses[1].text.starts_with("informal, mainly North American English"));
        assert_eq!(noun.senses[1].examples.len(), 2);
    }

    #[test]
    fn entries_without_a_part_of_speech() {
        let d = parse("paris", PARIS).unwrap();
        assert_eq!(d.headword, "Paris");
        assert_eq!(names(&d), [None]);
        assert!(d.parts[0].senses[0].text.starts_with("the capital of France"));
        assert!(d.origin.as_deref().unwrap().starts_with("named after the Parisii"));

        let d = parse("xcode", XCODE).unwrap();
        assert_eq!(d.headword, "Xcode");
        assert_eq!(d.pronunciation, None);
        assert_eq!(names(&d), [None]);
        assert!(d.parts[0].senses[0].text.starts_with("A set of software development tools"));
    }

    #[test]
    fn sections_and_verbs_with_their_own_pronunciation() {
        let d = parse("mice", MOUSE).unwrap();
        assert_eq!(d.headword, "mouse");
        assert_eq!(names(&d), [Some("noun"), Some("verb")]);
        assert_eq!(d.parts[0].note.as_deref(), Some("(plural mice)"));
        assert_eq!(d.parts[0].senses.len(), 3);
        assert_eq!(d.parts[1].senses.len(), 2);
        assert!(d.origin.as_deref().unwrap().starts_with("Old English mūs"));
        assert!(is_entry_for("mice", &d));
        let d = parse("was", WAS).unwrap();
        assert_eq!(d.parts[0].senses[0].text, "first and third person singular past of be");
    }

    #[test]
    fn unrecognised_text_falls_back_to_one_sense() {
        let d = parse("foo", "foo something odd without separators").unwrap();
        assert_eq!(d.headword, "foo");
        assert_eq!(d.parts.len(), 1);
        assert!(!d.parts[0].senses.is_empty());
        assert!(parse("foo", "  ").is_none());
    }

    #[test]
    fn entry_must_be_for_the_query() {
        let mouse = parse("mouse", MOUSE).unwrap();
        assert!(is_entry_for("mouse", &mouse));
        assert!(is_entry_for("Mouse", &mouse));
        assert!(is_entry_for("mice", &mouse));
        assert!(is_entry_for("mouses", &mouse));
        assert!(!is_entry_for("spotify", &mouse));
        assert!(!is_entry_for("mous", &mouse));
        assert!(is_regular_inflection("notes", "note"));
        assert!(is_regular_inflection("running", "run"));
        assert!(is_regular_inflection("studies", "study"));
        assert!(is_regular_inflection("making", "make"));
        assert!(!is_regular_inflection("notion", "note"));
    }

    #[test]
    fn only_single_words_are_looked_up() {
        for w in ["serendipity", "notes", "well-being", "don't", "OK", "naïve"] {
            assert!(is_word(w), "{w}");
        }
        for w in ["a", "", "two words", "file.pdf", "~/x", "abc1", "-ab", "ab-", "a--b", "x_y"] {
            assert!(!is_word(w), "{w}");
        }
        let q = |s: &str| Query::parse_with(s, &crate::AppSearch::builtin());
        assert_eq!(word_for_query(&q("serendipity")), Some("serendipity"));
        assert_eq!(word_for_query(&q("  notes ")), Some("notes"));
        assert_eq!(word_for_query(&q("app: notes")), None);
        assert_eq!(word_for_query(&q("notes in:Downloads")), None);
        assert_eq!(word_for_query(&q("arc: lifetimes")), None);
        assert_eq!(word_for_query(&q("offer letter")), None);
    }

    #[test]
    fn dictionary_urls() {
        assert_eq!(dictionary_url("serendipity"), "dict://serendipity");
        assert_eq!(dictionary_url("don't"), "dict://don%27t");
    }

    /// Talks to the real dictionary; skipped when it is not installed.
    #[cfg(target_os = "macos")]
    #[test]
    fn system_dictionary_lookups() {
        if !available() {
            eprintln!("system dictionary not available; skipping");
            return;
        }
        let d = define("serendipity").expect("serendipity");
        assert_eq!(d.headword, "serendipity");
        assert_eq!(define("notes").expect("notes").headword, "note");
        assert!(define("xyzzq").is_none());
        assert!(define("spotify").is_none());
        assert!(define("the cat").is_none());
        // Cached, misses included.
        assert_eq!(cached("xyzzq"), Some(None));
        assert_eq!(cached("Serendipity").unwrap().unwrap().word, "Serendipity");
    }
}
