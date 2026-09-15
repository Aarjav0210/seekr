use crate::app_search::{self, AppSearch};
use crate::item::Kind;
use serde::{Deserialize, Serialize};

/// A parsed search query.
///
/// Grammar (whitespace separated tokens, order-insensitive except the leading scope):
/// - `<scope>:` or `<scope>:<word>` — restrict to a [`Kind`]; e.g. `app:`, `doc:`, `dir:`.
/// - `in:<substr>` — path must contain this (e.g. `in:Downloads`, `in:~/Desktop/Dev`).
/// - `ext:<ext>` — name must end with `.<ext>`.
/// - everything else — fuzzy terms.
///
/// One exception: when the first token is `<keyword>:` for one of the app
/// searches passed to [`Query::parse_with`] (`arc:`, `spotify:`), the rest of
/// the query is taken verbatim as that app's search terms and nothing above applies.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub raw: String,
    pub scope: Option<Kind>,
    pub terms: String,
    pub in_dir: Option<String>,
    pub ext: Option<String>,
    /// `arc: rust lifetimes`: hand `terms` to this app instead of the index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_search: Option<AppSearch>,
}

impl Query {
    pub fn parse(raw: &str) -> Query {
        Self::parse_with(raw, &[])
    }

    /// [`parse`](Self::parse), plus app searches: a leading `<keyword>:` naming
    /// one of `searches` makes the rest of the query, verbatim, its terms.
    pub fn parse_with(raw: &str, searches: &[AppSearch]) -> Query {
        if let Some(q) = Self::parse_app_search(raw, searches) {
            return q;
        }
        let mut q = Query {
            raw: raw.to_string(),
            ..Default::default()
        };
        let mut terms: Vec<&str> = Vec::new();
        for tok in raw.split_whitespace() {
            if let Some((prefix, rest)) = tok.split_once(':') {
                let pl = prefix.to_ascii_lowercase();
                match pl.as_str() {
                    "in" | "path" => {
                        if !rest.is_empty() {
                            q.in_dir = Some(normalize_in(rest));
                        }
                        continue;
                    }
                    "ext" | "type" => {
                        if !rest.is_empty() {
                            q.ext = Some(rest.trim_start_matches('.').to_ascii_lowercase());
                        }
                        continue;
                    }
                    "kind" | "is" => {
                        if let Some(k) = Kind::from_scope(rest) {
                            q.scope = Some(k);
                            continue;
                        }
                    }
                    _ => {
                        if let Some(k) = Kind::from_scope(&pl) {
                            q.scope = Some(k);
                            if !rest.is_empty() {
                                terms.push(rest);
                            }
                            continue;
                        }
                    }
                }
            }
            terms.push(tok);
        }
        q.terms = terms.join(" ");
        q
    }

    /// `arc: rust lifetimes` → search Arc for "rust lifetimes". Only the first
    /// token counts, and the scope/filter words always win over keywords.
    fn parse_app_search(raw: &str, searches: &[AppSearch]) -> Option<Query> {
        let start = raw.trim_start();
        let end = start.find(char::is_whitespace).unwrap_or(start.len());
        let (first, rest) = start.split_at(end);
        let (prefix, inline) = first.split_once(':')?;
        if is_reserved_prefix(prefix) {
            return None;
        }
        let search = app_search::by_keyword(searches, prefix)?;
        // `arc:foo bar` keeps its space; `arc: foo` drops only the separator, so
        // what the user is typing (trailing space included) survives as-is.
        let rest = if inline.is_empty() {
            rest.strip_prefix(char::is_whitespace).unwrap_or(rest)
        } else {
            rest
        };
        Some(Query {
            raw: raw.to_string(),
            terms: format!("{inline}{rest}"),
            app_search: Some(search.clone()),
            ..Default::default()
        })
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.scope.is_none()
            && self.in_dir.is_none()
            && self.ext.is_none()
            && self.app_search.is_none()
    }

    /// Human readable description of active filters, for status lines.
    pub fn describe_filters(&self) -> String {
        let mut parts = Vec::new();
        if let Some(k) = self.scope {
            parts.push(format!("kind={}", k.as_str()));
        }
        if let Some(d) = &self.in_dir {
            parts.push(format!("in={d}"));
        }
        if let Some(e) = &self.ext {
            parts.push(format!("ext={e}"));
        }
        parts.join(" ")
    }
}

/// Prefixes the query language owns (`app:`, `in:`, `ext:` …); an app search
/// keyword may not shadow them.
pub(crate) fn is_reserved_prefix(prefix: &str) -> bool {
    let pl = prefix.to_ascii_lowercase();
    matches!(pl.as_str(), "in" | "path" | "ext" | "type" | "kind" | "is")
        || Kind::from_scope(&pl).is_some()
}

/// `in:~/Downloads` → `Downloads/`-style substring that appears in displayed paths.
fn normalize_in(s: &str) -> String {
    let s = s.trim_matches('"');
    let s = s.strip_prefix("~/").unwrap_or(s);
    s.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scope_with_space() {
        let q = Query::parse("app: WhatsApp");
        assert_eq!(q.scope, Some(Kind::App));
        assert_eq!(q.terms, "WhatsApp");
    }

    #[test]
    fn parses_scope_without_space_and_multiword() {
        let q = Query::parse("doc:Offer Letter 26");
        assert_eq!(q.scope, Some(Kind::Doc));
        assert_eq!(q.terms, "Offer Letter 26");
    }

    #[test]
    fn parses_filters() {
        let q = Query::parse("offer in:~/Downloads ext:.PDF");
        assert_eq!(q.scope, None);
        assert_eq!(q.terms, "offer");
        assert_eq!(q.in_dir.as_deref(), Some("Downloads"));
        assert_eq!(q.ext.as_deref(), Some("pdf"));
    }

    #[test]
    fn unknown_prefix_is_a_term() {
        let q = Query::parse("http://example.com");
        assert_eq!(q.terms, "http://example.com");
        assert!(q.scope.is_none());
    }

    fn keyword(q: &Query) -> Option<&str> {
        q.app_search.as_ref().map(|s| s.keyword.as_str())
    }

    #[test]
    fn leading_keyword_hands_the_rest_to_the_app_verbatim() {
        let q = Query::parse_with("arc: how to in:Downloads ext:pdf ", &AppSearch::builtin());
        assert_eq!(keyword(&q), Some("arc"));
        assert_eq!(q.terms, "how to in:Downloads ext:pdf ");
        assert!(q.scope.is_none() && q.in_dir.is_none() && q.ext.is_none());
        assert!(!q.is_empty());
    }

    #[test]
    fn keyword_is_case_insensitive_and_may_touch_the_terms() {
        let all = AppSearch::builtin();
        let q = Query::parse_with("Spotify:daft punk", &all);
        assert_eq!(keyword(&q), Some("spotify"));
        assert_eq!(q.terms, "daft punk");
        let q = Query::parse_with("spotify:", &all);
        assert_eq!(keyword(&q), Some("spotify"));
        assert_eq!(q.terms, "");
        assert_eq!(Query::parse_with("spotify: ", &all).terms, "");
    }

    #[test]
    fn keyword_only_counts_at_the_start() {
        let q = Query::parse_with("notes arc: foo", &AppSearch::builtin());
        assert!(q.app_search.is_none());
        assert_eq!(q.terms, "notes arc: foo");
        // Without a search list (one-shot CLI) the prefix is just a term, as before.
        assert!(Query::parse("arc: foo").app_search.is_none());
    }

    #[test]
    fn scope_and_filter_words_win_over_keywords() {
        let mut all = AppSearch::builtin();
        for k in ["doc", "in"] {
            all.push(AppSearch {
                keyword: k.into(),
                name: None,
                url: "x{query}".into(),
                app: None,
            });
        }
        let q = Query::parse_with("doc: offer", &all);
        assert!(q.app_search.is_none());
        assert_eq!(q.scope, Some(Kind::Doc));
        let q = Query::parse_with("in:Downloads cv", &all);
        assert!(q.app_search.is_none());
        assert_eq!(q.in_dir.as_deref(), Some("Downloads"));
    }
}
