use crate::item::Kind;
use serde::{Deserialize, Serialize};

/// A parsed search query.
///
/// Grammar (whitespace separated tokens, order-insensitive except the leading scope):
/// - `<scope>:` or `<scope>:<word>` — restrict to a [`Kind`]; e.g. `app:`, `doc:`, `dir:`.
/// - `in:<substr>` — path must contain this (e.g. `in:Downloads`, `in:~/Desktop/Dev`).
/// - `ext:<ext>` — name must end with `.<ext>`.
/// - everything else — fuzzy terms.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub raw: String,
    pub scope: Option<Kind>,
    pub terms: String,
    pub in_dir: Option<String>,
    pub ext: Option<String>,
}

impl Query {
    pub fn parse(raw: &str) -> Query {
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

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.scope.is_none() && self.in_dir.is_none() && self.ext.is_none()
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
}
