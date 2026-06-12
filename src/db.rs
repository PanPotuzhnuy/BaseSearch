//! SQLite storage: schema, batched inserts, FTS5 indexing, search, and filters.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::schema::{COLUMNS, RESULT_COLUMNS, SEARCH_COLUMNS};

/// Filter values; an empty string means the filter is not set.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Filters {
    pub year: String,
    pub product_code: String,
    pub sender: String,
    pub recipient: String,
    pub edrpou: String,
    pub trade_country: String,
    pub dispatch_country: String,
    pub origin_country: String,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        [
            &self.year,
            &self.product_code,
            &self.sender,
            &self.recipient,
            &self.edrpou,
            &self.trade_country,
            &self.dispatch_country,
            &self.origin_country,
        ]
        .iter()
        .all(|v| v.trim().is_empty())
    }

    pub fn clear(&mut self) {
        *self = Filters::default();
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub filters: Filters,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.filters.is_empty()
    }
}

/// One row prepared for insertion during import.
pub struct ImportRecord {
    pub hash: [u8; 16],
    pub year: Option<i64>,
    pub values: Vec<String>,
}

pub struct RecordCard {
    pub fields: Vec<(&'static str, String)>,
    pub source_file: String,
}

#[derive(Clone)]
pub struct ImportLogEntry {
    pub file_name: String,
    pub total_rows: u64,
    pub imported: u64,
    pub duplicates: u64,
    pub seconds: f64,
    pub imported_at: String,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsOverview {
    pub row_count: u64,
    pub distinct_senders: u64,
    pub distinct_recipients: u64,
    pub distinct_edrpou: u64,
    pub distinct_trademarks: u64,
    pub total_value_usd: f64,
    pub total_gross_kg: f64,
    pub total_net_kg: f64,
    pub total_quantity: f64,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsGroupRow {
    pub label: String,
    pub rows: u64,
    pub total_value_usd: f64,
    pub total_net_kg: f64,
    pub total_gross_kg: f64,
    pub total_quantity: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Analytics {
    pub overview: AnalyticsOverview,
    pub top_recipients: Vec<AnalyticsGroupRow>,
    pub top_senders: Vec<AnalyticsGroupRow>,
    pub top_trademarks: Vec<AnalyticsGroupRow>,
    pub top_product_codes: Vec<AnalyticsGroupRow>,
    pub top_origin_countries: Vec<AnalyticsGroupRow>,
}

pub struct Db {
    pub conn: Connection,
}

fn records_ddl() -> String {
    let fields: Vec<String> = COLUMNS.iter().map(|c| format!("{} TEXT", c.name)).collect();
    format!(
        "CREATE TABLE IF NOT EXISTS records (
            id INTEGER PRIMARY KEY,
            row_hash BLOB NOT NULL UNIQUE,
            source_file TEXT NOT NULL,
            year INTEGER,
            imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            {}
        )",
        fields.join(",\n            ")
    )
}

fn search_text_expr() -> String {
    SEARCH_COLUMNS
        .iter()
        .map(|c| format!("COALESCE({c},'')"))
        .collect::<Vec<_>>()
        .join(" || ' ' || ")
}

impl Db {
    pub fn open(path: &Path) -> Result<Db, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        let db = Db { conn };
        db.init().map_err(|e| e.to_string())?;
        Ok(db)
    }

    fn init(&self) -> rusqlite::Result<()> {
        self.conn.pragma_update(None, "journal_mode", "WAL")?;
        self.conn.pragma_update(None, "synchronous", "NORMAL")?;
        self.conn.pragma_update(None, "temp_store", "MEMORY")?;
        self.conn.pragma_update(None, "cache_size", -131072)?;
        self.conn.pragma_update(None, "mmap_size", 268435456i64)?;
        // Case-insensitive substring search with Cyrillic support:
        // SQLite's built-in LOWER/LIKE only handle ASCII case folding.
        self.conn.create_scalar_function(
            "cyr_contains",
            2,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let hay = ctx
                    .get_raw(0)
                    .as_str_or_null()
                    .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
                let needle = ctx
                    .get_raw(1)
                    .as_str_or_null()
                    .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
                Ok(match (hay, needle) {
                    (Some(h), Some(n)) => contains_ci(h, n),
                    _ => false,
                })
            },
        )?;
        self.conn.create_scalar_function(
            "num_value",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| {
                let raw = ctx
                    .get_raw(0)
                    .as_str_or_null()
                    .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
                Ok(raw.and_then(parse_number))
            },
        )?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT
            );",
        )?;
        // Search index settings changed between versions. On mismatch, the
        // table is recreated and rebuilt from the watermark, with progress on
        // the next import or startup.
        const FTS_SCHEMA_VERSION: &str = "3";
        if self.meta_get("fts_schema").as_deref() != Some(FTS_SCHEMA_VERSION) {
            self.conn
                .execute_batch("DROP TABLE IF EXISTS records_fts;")?;
            self.meta_set("fts_watermark", "0");
            self.meta_set("fts_schema", FTS_SCHEMA_VERSION);
        }
        self.conn.execute_batch(&format!(
            "{records};
            CREATE VIRTUAL TABLE IF NOT EXISTS records_fts USING fts5(
                search_text,
                content='',
                detail=none,
                columnsize=0,
                tokenize='unicode61 remove_diacritics 2'
            );
            CREATE TABLE IF NOT EXISTS import_log (
                id INTEGER PRIMARY KEY,
                file_name TEXT NOT NULL,
                total_rows INTEGER NOT NULL,
                imported INTEGER NOT NULL,
                duplicates INTEGER NOT NULL,
                seconds REAL NOT NULL,
                imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_records_year ON records(year);
            CREATE INDEX IF NOT EXISTS idx_records_product_code ON records(product_code);
            CREATE INDEX IF NOT EXISTS idx_records_edrpou ON records(edrpou);",
            records = records_ddl()
        ))?;
        // This column was added after early versions; add it without a migration.
        let _ = self
            .conn
            .execute("ALTER TABLE import_log ADD COLUMN file_hash TEXT", []);
        Ok(())
    }

    // ---------- meta ----------

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn meta_set(&self, key: &str, value: &str) {
        let _ = self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    fn meta_get_i64(&self, key: &str) -> i64 {
        self.meta_get(key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    // ---------- insert ----------

    /// Inserts a row batch in one transaction.
    /// Returns (inserted rows, skipped duplicates).
    pub fn insert_batch(
        &mut self,
        source_file: &str,
        records: &[ImportRecord],
    ) -> rusqlite::Result<(u64, u64)> {
        if records.is_empty() {
            return Ok((0, 0));
        }
        let col_names: Vec<&str> = COLUMNS.iter().map(|c| c.name).collect();
        let sql = format!(
            "INSERT OR IGNORE INTO records (row_hash, source_file, year, {}) VALUES ({})",
            col_names.join(", "),
            std::iter::repeat_n("?", 3 + col_names.len())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let tx = self.conn.transaction()?;
        let mut inserted: u64 = 0;
        {
            let mut stmt = tx.prepare_cached(&sql)?;
            for rec in records {
                stmt.raw_bind_parameter(1, &rec.hash[..])?;
                stmt.raw_bind_parameter(2, source_file)?;
                stmt.raw_bind_parameter(3, rec.year)?;
                for (i, v) in rec.values.iter().enumerate() {
                    stmt.raw_bind_parameter(4 + i, v.as_str())?;
                }
                inserted += stmt.raw_execute()? as u64;
            }
        }
        tx.commit()?;
        Ok((inserted, records.len() as u64 - inserted))
    }

    // ---------- FTS ----------

    /// Indexes all rows with an id above the watermark.
    /// Returns (indexed rows, cancelled).
    pub fn index_fts(
        &mut self,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> rusqlite::Result<(u64, bool)> {
        let max_id: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(id), 0) FROM records", [], |r| r.get(0))?;
        let start = self.meta_get_i64("fts_watermark");
        if start >= max_id {
            return Ok((0, false));
        }
        let span_total = (max_id - start) as u64;
        let insert_sql = format!(
            "INSERT INTO records_fts(rowid, search_text)
             SELECT id, {} FROM records WHERE id > ?1 AND id <= ?2",
            search_text_expr()
        );
        const CHUNK: i64 = 20_000;
        let mut watermark = start;
        let mut indexed: u64 = 0;
        while watermark < max_id {
            if cancel.load(Ordering::Relaxed) {
                return Ok((indexed, true));
            }
            let end = (watermark + CHUNK).min(max_id);
            let tx = self.conn.transaction()?;
            let n = tx.execute(&insert_sql, params![watermark, end])?;
            tx.execute(
                "INSERT INTO meta(key, value) VALUES ('fts_watermark', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![end.to_string()],
            )?;
            tx.commit()?;
            indexed += n as u64;
            watermark = end;
            progress((watermark - start) as u64, span_total);
        }
        Ok((indexed, false))
    }

    /// Number of rows not yet present in the search index.
    pub fn unindexed_rows(&self) -> u64 {
        let watermark = self.meta_get_i64("fts_watermark");
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE id > ?1",
                [watermark],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as u64
    }

    // ---------- search ----------

    fn build_where(&self, q: &Query) -> (String, String, Vec<Value>) {
        let mut joins = String::new();
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        // Shared MATCH expression: query text plus company and country filter
        // tokens. These columns are indexed, so FTS narrows the candidate set
        // first and cyr_contains performs the exact substring check.
        let mut match_expr = build_fts_query(&q.text);
        let f = &q.filters;
        let mut contains_clauses: Vec<(String, String)> = Vec::new();
        for (col, value) in [
            ("sender", &f.sender),
            ("recipient", &f.recipient),
            ("trade_country", &f.trade_country),
            ("dispatch_country", &f.dispatch_country),
            ("origin_country", &f.origin_country),
        ] {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if let Some(terms) = fts_prefix_terms(value) {
                if !match_expr.is_empty() {
                    match_expr.push(' ');
                }
                match_expr.push_str(&terms);
            }
            contains_clauses.push((format!("cyr_contains(r.{col}, ?)"), value.to_lowercase()));
        }
        if !match_expr.is_empty() {
            joins.push_str(" JOIN records_fts ON records_fts.rowid = r.id");
            clauses.push("records_fts MATCH ?".into());
            params.push(match_expr.into());
        }
        if let Some(year) = parse_year(&f.year) {
            clauses.push("r.year = ?".into());
            params.push(year.into());
        }
        let code = f.product_code.trim();
        if !code.is_empty() {
            clauses.push("r.product_code GLOB ?".into());
            params.push(format!("{}*", glob_escape(code)).into());
        }
        let edrpou = f.edrpou.trim();
        if !edrpou.is_empty() {
            clauses.push("r.edrpou = ?".into());
            params.push(edrpou.to_string().into());
        }
        for (clause, param) in contains_clauses {
            clauses.push(clause);
            params.push(param.into());
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        (joins, where_sql, params)
    }

    pub fn count(&self, q: &Query) -> rusqlite::Result<u64> {
        let (joins, where_sql, params) = self.build_where(q);
        let sql = format!("SELECT COUNT(*) FROM records r{joins}{where_sql}");
        let n: i64 = self
            .conn
            .query_row(&sql, params_from_iter(params), |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Result page: (row ids, RESULT_COLUMNS values).
    pub fn search_page(
        &self,
        q: &Query,
        limit: u64,
        offset: u64,
    ) -> rusqlite::Result<(Vec<i64>, Vec<Vec<String>>)> {
        let (joins, where_sql, mut params) = self.build_where(q);
        let select: Vec<String> = RESULT_COLUMNS.iter().map(|c| format!("r.{c}")).collect();
        // Without conditions, page by insertion order, which is instant at any
        // size. With conditions, sort by date after the result set is narrowed.
        let order = if q.is_empty() {
            "r.id DESC"
        } else {
            "r.declaration_date DESC, r.id DESC"
        };
        let sql = format!(
            "SELECT r.id, {} FROM records r{joins}{where_sql} ORDER BY {order} LIMIT ? OFFSET ?",
            select.join(", ")
        );
        params.push((limit as i64).into());
        params.push((offset as i64).into());
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        let mut ids = Vec::new();
        let mut data = Vec::new();
        while let Some(row) = rows.next()? {
            ids.push(row.get::<_, i64>(0)?);
            let mut values = Vec::with_capacity(RESULT_COLUMNS.len());
            for i in 0..RESULT_COLUMNS.len() {
                values.push(row.get::<_, Option<String>>(i + 1)?.unwrap_or_default());
            }
            data.push(values);
        }
        Ok((ids, data))
    }

    /// Export row batch using keyset pagination by id: all 41 columns plus file.
    pub fn export_batch(
        &self,
        q: &Query,
        last_id: i64,
        limit: u64,
    ) -> rusqlite::Result<(i64, Vec<Vec<String>>)> {
        let (joins, where_sql, mut params) = self.build_where(q);
        let select: Vec<String> = COLUMNS.iter().map(|c| format!("r.{}", c.name)).collect();
        let cond = if where_sql.is_empty() {
            " WHERE"
        } else {
            " AND"
        };
        let sql = format!(
            "SELECT r.id, {}, r.source_file FROM records r{joins}{where_sql}{cond} r.id > ? ORDER BY r.id LIMIT ?",
            select.join(", ")
        );
        params.push(last_id.into());
        params.push((limit as i64).into());
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        let mut data = Vec::new();
        let mut max_id = last_id;
        while let Some(row) = rows.next()? {
            max_id = row.get::<_, i64>(0)?;
            let mut values = Vec::with_capacity(COLUMNS.len() + 1);
            for i in 0..=COLUMNS.len() {
                values.push(row.get::<_, Option<String>>(i + 1)?.unwrap_or_default());
            }
            data.push(values);
        }
        Ok((max_id, data))
    }

    /// Full record card by id.
    pub fn record_card(&self, id: i64) -> rusqlite::Result<RecordCard> {
        let select: Vec<String> = COLUMNS.iter().map(|c| c.name.to_string()).collect();
        let sql = format!(
            "SELECT {}, source_file FROM records WHERE id = ?1",
            select.join(", ")
        );
        self.conn.query_row(&sql, [id], |row| {
            let mut fields = Vec::with_capacity(COLUMNS.len());
            for (i, col) in COLUMNS.iter().enumerate() {
                fields.push((
                    col.header,
                    row.get::<_, Option<String>>(i)?.unwrap_or_default(),
                ));
            }
            let source_file: String = row.get(COLUMNS.len())?;
            Ok(RecordCard {
                fields,
                source_file,
            })
        })
    }

    // ---------- analytics ----------

    pub fn analytics(&self, q: &Query, limit: u64) -> rusqlite::Result<Analytics> {
        let (joins, where_sql, params) = self.build_where(q);
        let sql = format!(
            "SELECT
                COUNT(*),
                COUNT(DISTINCT NULLIF(TRIM(r.sender), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.recipient), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.edrpou), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.trademark), '')),
                SUM(num_value(r.currency_control_value)),
                SUM(num_value(r.gross_kg)),
                SUM(num_value(r.net_kg)),
                SUM(num_value(r.quantity))
             FROM records r{joins}{where_sql}"
        );
        let overview = self
            .conn
            .query_row(&sql, params_from_iter(params.clone()), |row| {
                Ok(AnalyticsOverview {
                    row_count: row.get::<_, i64>(0)? as u64,
                    distinct_senders: row.get::<_, i64>(1)? as u64,
                    distinct_recipients: row.get::<_, i64>(2)? as u64,
                    distinct_edrpou: row.get::<_, i64>(3)? as u64,
                    distinct_trademarks: row.get::<_, i64>(4)? as u64,
                    total_value_usd: row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    total_gross_kg: row.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                    total_net_kg: row.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                    total_quantity: row.get::<_, Option<f64>>(8)?.unwrap_or(0.0),
                })
            })?;

        Ok(Analytics {
            overview,
            top_recipients: self.analytics_group(q, "recipient", limit)?,
            top_senders: self.analytics_group(q, "sender", limit)?,
            top_trademarks: self.analytics_group(q, "trademark", limit)?,
            top_product_codes: self.analytics_group(q, "product_code", limit)?,
            top_origin_countries: self.analytics_group(q, "origin_country", limit)?,
        })
    }

    fn analytics_group(
        &self,
        q: &Query,
        column: &str,
        limit: u64,
    ) -> rusqlite::Result<Vec<AnalyticsGroupRow>> {
        let (joins, where_sql, mut params) = self.build_where(q);
        let non_empty = format!("TRIM(COALESCE(r.{column}, '')) <> ''");
        let filter_sql = if where_sql.is_empty() {
            format!(" WHERE {non_empty}")
        } else {
            format!("{where_sql} AND {non_empty}")
        };
        let sql = format!(
            "SELECT
                TRIM(r.{column}) AS label,
                COUNT(*) AS rows_count,
                COALESCE(SUM(num_value(r.currency_control_value)), 0.0) AS total_value_usd,
                COALESCE(SUM(num_value(r.net_kg)), 0.0) AS total_net_kg,
                COALESCE(SUM(num_value(r.gross_kg)), 0.0) AS total_gross_kg,
                COALESCE(SUM(num_value(r.quantity)), 0.0) AS total_quantity
             FROM records r{joins}{filter_sql}
             GROUP BY TRIM(r.{column})
             ORDER BY total_value_usd DESC, rows_count DESC, label COLLATE NOCASE
             LIMIT ?"
        );
        params.push((limit as i64).into());
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), |row| {
            Ok(AnalyticsGroupRow {
                label: row.get(0)?,
                rows: row.get::<_, i64>(1)? as u64,
                total_value_usd: row.get(2)?,
                total_net_kg: row.get(3)?,
                total_gross_kg: row.get(4)?,
                total_quantity: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    // ---------- statistics ----------

    pub fn total_rows(&self) -> u64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    }

    pub fn add_import_log(
        &self,
        file_name: &str,
        total_rows: u64,
        imported: u64,
        duplicates: u64,
        seconds: f64,
        file_hash: Option<&str>,
    ) {
        let _ = self.conn.execute(
            "INSERT INTO import_log (file_name, total_rows, imported, duplicates, seconds, file_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                file_name,
                total_rows as i64,
                imported as i64,
                duplicates as i64,
                seconds,
                file_hash
            ],
        );
    }

    /// Full cleanup: removes all data and import logs, recreates the schema,
    /// and returns disk space via VACUUM. Settings such as language and theme
    /// are preserved.
    pub fn clear_all(&mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS records;
             DROP TABLE IF EXISTS records_fts;
             DROP TABLE IF EXISTS import_log;",
        )?;
        self.meta_set("fts_watermark", "0");
        self.init()?;
        self.conn.execute_batch("VACUUM;")?;
        Ok(())
    }

    /// Name of a previously imported file with the same content.
    pub fn find_import_by_hash(&self, file_hash: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT file_name FROM import_log WHERE file_hash = ?1 ORDER BY id DESC LIMIT 1",
                [file_hash],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn import_log(&self, limit: u64) -> Vec<ImportLogEntry> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT file_name, total_rows, imported, duplicates, seconds, imported_at
             FROM import_log ORDER BY id DESC LIMIT ?1",
        ) else {
            return Vec::new();
        };
        stmt.query_map([limit as i64], |row| {
            Ok(ImportLogEntry {
                file_name: row.get(0)?,
                total_rows: row.get::<_, i64>(1)? as u64,
                imported: row.get::<_, i64>(2)? as u64,
                duplicates: row.get::<_, i64>(3)? as u64,
                seconds: row.get(4)?,
                imported_at: row.get(5)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }
}

/// Builds an FTS5 query from user input.
/// Each word is an exact phrase; `word*` performs prefix search.
/// Numeric terms with 4+ digits are automatically treated as prefixes,
/// which is convenient for product codes.
pub fn build_fts_query(input: &str) -> String {
    fn flush(terms: &mut Vec<String>, current: &mut String, prefix: bool) {
        if current.is_empty() {
            return;
        }
        let all_digits = current.chars().all(|c| c.is_ascii_digit());
        let prefix = prefix || (all_digits && current.len() >= 4);
        let star = if prefix { "*" } else { "" };
        terms.push(format!("\"{current}\"{star}"));
        current.clear();
    }
    let mut terms: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in input.chars() {
        if terms.len() >= 32 {
            break;
        }
        if ch.is_alphanumeric() {
            current.push(ch);
        } else if ch == '*' {
            flush(&mut terms, &mut current, true);
        } else {
            flush(&mut terms, &mut current, false);
        }
    }
    flush(&mut terms, &mut current, false);
    terms.join(" ")
}

/// Prefix FTS terms for a filter value: `JYSK Ukraine` -> `"jysk"* "ukraine"*`.
/// Returns None when the value cannot produce reliable terms, such as 1-char tokens.
fn fts_prefix_terms(value: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in value.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            if current.chars().count() < 2 {
                return None;
            }
            terms.push(format!("\"{current}\"*"));
            current.clear();
        }
    }
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

/// Allocation-free case-insensitive substring search, including Cyrillic text.
/// `needle_lower` must already be lowercased.
pub fn contains_ci(hay: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let Some(first) = needle_lower.chars().next() else {
        return true;
    };
    for (i, c) in hay.char_indices() {
        if c.to_lowercase().next() != Some(first) {
            continue;
        }
        let mut h = hay[i..].chars().flat_map(char::to_lowercase);
        let mut n = needle_lower.chars();
        loop {
            let Some(nc) = n.next() else {
                return true;
            };
            if h.next() != Some(nc) {
                break;
            }
        }
    }
    false
}

pub fn parse_number(value: &str) -> Option<f64> {
    let mut compact = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_digit() || matches!(ch, '.' | ',' | '-' | '+') {
            compact.push(ch);
        }
    }
    if !compact.chars().any(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let dot_count = compact.matches('.').count();
    let comma_count = compact.matches(',').count();
    let decimal_sep = match (dot_count, comma_count) {
        (0, 0) => None,
        (0, 1) => decimal_separator_for_single(&compact, ','),
        (1, 0) => decimal_separator_for_single(&compact, '.'),
        (0, _) | (_, 0) => None,
        _ => {
            let last_dot = compact.rfind('.').unwrap_or(0);
            let last_comma = compact.rfind(',').unwrap_or(0);
            Some(if last_dot > last_comma { '.' } else { ',' })
        }
    };

    let mut normalized = String::with_capacity(compact.len());
    let mut sign_written = false;
    let mut decimal_written = false;
    for (i, ch) in compact.chars().enumerate() {
        if ch.is_ascii_digit() {
            normalized.push(ch);
        } else if matches!(ch, '-' | '+') && !sign_written && normalized.is_empty() && i == 0 {
            normalized.push(ch);
            sign_written = true;
        } else if Some(ch) == decimal_sep && !decimal_written {
            normalized.push('.');
            decimal_written = true;
        }
    }

    normalized.parse::<f64>().ok()
}

fn decimal_separator_for_single(value: &str, sep: char) -> Option<char> {
    let pos = value.rfind(sep)?;
    let before = value[..pos].chars().filter(|c| c.is_ascii_digit()).count();
    let after = value[pos + sep.len_utf8()..]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .count();
    if after == 0 || (after == 3 && before > 0) {
        None
    } else {
        Some(sep)
    }
}

fn parse_year(value: &str) -> Option<i64> {
    let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 4 {
        digits.parse().ok()
    } else {
        None
    }
}

fn glob_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '*' | '?' | '[' | ']' => {
                out.push('[');
                out.push(ch);
                out.push(']');
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Extracts a 20xx year from date text.
pub fn extract_year(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    for window_start in 0..bytes.len().saturating_sub(3) {
        let w = &bytes[window_start..window_start + 4];
        if w[0] == b'2' && w[1] == b'0' && w[2].is_ascii_digit() && w[3].is_ascii_digit() {
            // Not part of a longer number.
            let before_digit = window_start > 0 && bytes[window_start - 1].is_ascii_digit();
            let after_digit =
                window_start + 4 < bytes.len() && bytes[window_start + 4].is_ascii_digit();
            if !before_digit && !after_digit {
                return std::str::from_utf8(w).ok()?.parse().ok();
            }
        }
    }
    None
}
