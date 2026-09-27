//! SQLite-backed persistent index and usage log.
use crate::config::Config;
use crate::item::{Item, Kind};
use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How often and how recently an item was opened through seekr.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub count: u32,
    pub last_used: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub total: usize,
    pub by_kind: Vec<(Kind, usize)>,
    pub last_indexed: Option<i64>,
    pub usage_entries: usize,
    pub db_path: PathBuf,
    pub db_bytes: u64,
}

pub struct Index {
    conn: Connection,
    path: PathBuf,
}

const SCHEMA_VERSION: &str = "2";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS items (
    path      TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,
    name      TEXT NOT NULL,
    alt       TEXT,
    bundle_id TEXT,
    size      INTEGER NOT NULL DEFAULT 0,
    modified  INTEGER NOT NULL DEFAULT 0,
    flags     INTEGER NOT NULL DEFAULT 0,
    source    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS items_kind ON items(kind);
CREATE INDEX IF NOT EXISTS items_source ON items(source);
CREATE TABLE IF NOT EXISTS usage (
    path      TEXT PRIMARY KEY,
    count     INTEGER NOT NULL DEFAULT 0,
    last_used INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening index at {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY;",
        )?;
        conn.execute_batch(SCHEMA)?;
        let idx = Index {
            conn,
            path: path.to_path_buf(),
        };
        idx.migrate()?;
        Ok(idx)
    }

    /// Items are a cache: on a schema change we simply drop and rebuild them.
    /// Usage history is precious and is always kept.
    fn migrate(&self) -> Result<()> {
        let current = self.get_meta("schema_version")?;
        if current.as_deref() != Some(SCHEMA_VERSION) {
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS items; DELETE FROM meta WHERE key = 'last_indexed';",
            )?;
            self.conn.execute_batch(SCHEMA)?;
            self.set_meta("schema_version", SCHEMA_VERSION)?;
        }
        Ok(())
    }

    pub fn open_default() -> Result<Index> {
        Self::open(&Config::db_path())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replace every item that came from `source` with `items`, atomically.
    pub fn replace_items(&mut self, source: &str, items: &[Item]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM items WHERE source = ?1", params![source])?;
        insert_items(&tx, source, items)?;
        tx.commit()?;
        Ok(())
    }

    /// Insert or update items without touching anything else.
    pub fn upsert_items(&mut self, source: &str, items: &[Item]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        insert_items(&tx, source, items)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_paths(&mut self, paths: &[PathBuf]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM items WHERE path = ?1")?;
            for p in paths {
                stmt.execute(params![p.to_string_lossy()])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// `path -> (modified, size, flags)` for every indexed item; the basis for incremental refresh.
    pub fn load_fingerprints(&self) -> Result<HashMap<PathBuf, (i64, u64, u32)>> {
        let mut stmt = self.conn.prepare("SELECT path, modified, size, flags FROM items")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(0)?),
                (
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, i64>(3)? as u32,
                ),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Load items. `kinds` of `None` loads everything; otherwise only those kinds.
    pub fn load_items(&self, kinds: Option<&[Kind]>) -> Result<Vec<Item>> {
        let mut sql = format!("SELECT {ITEM_COLUMNS} FROM items i");
        let names: Vec<&str> = kinds.unwrap_or_default().iter().map(|k| k.as_str()).collect();
        if kinds.is_some() {
            let marks = vec!["?"; names.len()].join(",");
            sql.push_str(&format!(" WHERE i.kind IN ({marks})"));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&names), item_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    pub fn load_usage(&self) -> Result<HashMap<PathBuf, Usage>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, count, last_used FROM usage")?;
        let rows = stmt.query_map([], |r| Ok((PathBuf::from(r.get::<_, String>(0)?), usage_at(r, 1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Items that have been used, most recent first (for the empty-query view).
    pub fn recent_items(&self, limit: usize) -> Result<Vec<(Item, Usage)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS}, u.count, u.last_used
             FROM usage u JOIN items i ON i.path = u.path
             ORDER BY u.last_used DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map(params![limit as i64], |r| Ok((item_from_row(r)?, usage_at(r, 8)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Count one more use of `path`, now; returns its updated usage.
    pub fn record_use(&self, path: &Path) -> Result<Usage> {
        let usage = self.conn.query_row(
            "INSERT INTO usage (path, count, last_used) VALUES (?1, 1, ?2)
             ON CONFLICT(path) DO UPDATE SET count = count + 1, last_used = excluded.last_used
             RETURNING count, last_used",
            params![path.to_string_lossy(), crate::now_secs()],
            |r| usage_at(r, 0),
        )?;
        Ok(usage)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn last_indexed(&self) -> Option<i64> {
        self.get_meta("last_indexed")
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
    }

    pub fn mark_indexed(&self) -> Result<()> {
        self.set_meta("last_indexed", &crate::now_secs().to_string())
    }

    pub fn stats(&self) -> Result<Stats> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) FROM items GROUP BY kind ORDER BY COUNT(*) DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok((Kind::from_db(&r.get::<_, String>(0)?), r.get::<_, i64>(1)? as usize))
        })?;
        let by_kind: Vec<(Kind, usize)> = rows.collect::<rusqlite::Result<_>>()?;
        let usage_entries: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM usage", [], |r| r.get(0))?;
        let db_bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        Ok(Stats {
            total: by_kind.iter().map(|(_, n)| n).sum(),
            by_kind,
            last_indexed: self.last_indexed(),
            usage_entries: usage_entries as usize,
            db_path: self.path.clone(),
            db_bytes,
        })
    }
}

/// What [`item_from_row`] reads, in order, from the items table aliased `i`.
const ITEM_COLUMNS: &str = "i.path, i.kind, i.name, i.alt, i.bundle_id, i.size, i.modified, i.flags";

fn item_from_row(r: &Row) -> rusqlite::Result<Item> {
    Ok(Item {
        path: PathBuf::from(r.get::<_, String>(0)?),
        kind: Kind::from_db(&r.get::<_, String>(1)?),
        name: r.get(2)?,
        alt: r.get(3)?,
        bundle_id: r.get(4)?,
        size: r.get::<_, i64>(5)? as u64,
        modified: r.get(6)?,
        flags: r.get::<_, i64>(7)? as u32,
    })
}

/// The `count, last_used` pair starting at column `i`.
fn usage_at(r: &Row, i: usize) -> rusqlite::Result<Usage> {
    Ok(Usage {
        count: r.get::<_, i64>(i)? as u32,
        last_used: r.get(i + 1)?,
    })
}

fn insert_items(tx: &Transaction, source: &str, items: &[Item]) -> Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT OR REPLACE INTO items (path, kind, name, alt, bundle_id, size, modified, flags, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    for it in items {
        stmt.execute(params![
            it.path.to_string_lossy(),
            it.kind.as_str(),
            it.name,
            it.alt,
            it.bundle_id,
            it.size as i64,
            it.modified,
            it.flags as i64,
            source,
        ])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir;

    fn item(kind: Kind, path: &str) -> Item {
        let mut it = Item::new(kind, path.rsplit('/').next().unwrap(), path);
        it.alt = Some("alt".into());
        it.bundle_id = Some("com.example".into());
        it.size = 42;
        it.modified = 1_700_000_000;
        it.flags = crate::item::FLAG_IN_REPO;
        it
    }

    #[test]
    fn items_round_trip_and_filter_by_kind() {
        let tmp = temp_dir("index-items");
        let mut idx = Index::open(&tmp.join("index.db")).unwrap();
        let a = item(Kind::App, "/Applications/A.app");
        let d = item(Kind::Doc, "/x/offer.pdf");
        idx.replace_items("walk", &[a.clone(), d.clone()]).unwrap();
        assert_eq!(idx.load_items(None).unwrap().len(), 2);
        assert_eq!(idx.load_items(Some(&[Kind::Doc])).unwrap(), [d.clone()]);
        assert_eq!(idx.load_items(Some(&[Kind::Doc, Kind::App])).unwrap().len(), 2);
        assert!(idx.load_items(Some(&[])).unwrap().is_empty());
        assert_eq!(idx.stats().unwrap().total, 2);

        // Upserts touch only their items; a replace drops the source's old ones.
        let mut d2 = d.clone();
        d2.size = 7;
        idx.upsert_items("walk", &[d2.clone()]).unwrap();
        assert_eq!(idx.load_items(Some(&[Kind::Doc])).unwrap(), [d2]);
        idx.replace_items("walk", &[a.clone()]).unwrap();
        assert_eq!(idx.load_items(None).unwrap(), [a]);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn uses_are_counted_and_listed_most_recent_first() {
        let tmp = temp_dir("index-usage");
        let mut idx = Index::open(&tmp.join("index.db")).unwrap();
        let a = item(Kind::Doc, "/x/a.pdf");
        let b = item(Kind::Doc, "/x/b.pdf");
        idx.replace_items("walk", &[a.clone(), b.clone()]).unwrap();
        assert_eq!(idx.record_use(&a.path).unwrap().count, 1);
        let u = idx.record_use(&a.path).unwrap();
        assert_eq!(u.count, 2);
        assert!(u.last_used > 0);
        idx.conn
            .execute("UPDATE usage SET last_used = 1 WHERE path = ?1", params![a.path.to_string_lossy()])
            .unwrap();
        idx.record_use(&b.path).unwrap();
        // A use of something no longer indexed is kept, but not listed.
        idx.record_use(Path::new("/gone.txt")).unwrap();

        let recent = idx.recent_items(10).unwrap();
        let paths: Vec<_> = recent.iter().map(|(i, _)| i.path.clone()).collect();
        assert_eq!(paths, [b.path.clone(), a.path.clone()]);
        assert_eq!(recent[1].0, a, "items come back whole");
        assert_eq!(recent[1].1.count, 2);
        assert_eq!(idx.load_usage().unwrap().len(), 3);
        assert_eq!(idx.recent_items(1).unwrap().len(), 1);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
