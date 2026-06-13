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
    pub declaration_count: u64,
    pub distinct_senders: u64,
    pub distinct_recipients: u64,
    pub distinct_edrpou: u64,
    pub distinct_trademarks: u64,
    pub distinct_product_codes: u64,
    pub distinct_origin_countries: u64,
    pub distinct_dispatch_countries: u64,
    pub distinct_trade_countries: u64,
    pub total_value_usd: f64,
    pub total_gross_kg: f64,
    pub total_net_kg: f64,
    pub total_quantity: f64,
    pub avg_value_per_net_kg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalyticsFilterField {
    Recipient,
    Sender,
    Edrpou,
    ProductCode,
    Trademark,
    OriginCountry,
    DispatchCountry,
    TradeCountry,
    Description,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticsFilterAction {
    pub field: AnalyticsFilterField,
    pub value: String,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsGroupRow {
    pub label: String,
    pub rows: u64,
    pub declarations: u64,
    pub companies: u64,
    pub total_value_usd: f64,
    pub total_net_kg: f64,
    pub total_gross_kg: f64,
    pub total_quantity: f64,
    pub share_percent: f64,
    pub avg_value_per_net_kg: f64,
    pub filter_action: Option<AnalyticsFilterAction>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnalyticsSectionKind {
    #[default]
    Recipients,
    Senders,
    Edrpou,
    ProductCodes,
    Trademarks,
    ProductGroups,
    OriginCountries,
    DispatchCountries,
    TradeCountries,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsSection {
    pub kind: AnalyticsSectionKind,
    pub rows: Vec<AnalyticsGroupRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PriceMetricKind {
    #[default]
    ValuePerNetKg,
    RfvUsdKg,
    RmvNetUsdKg,
    RmvUsdExtraUnit,
    RmvGrossUsdKg,
    MinBaseUsdKg,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsPriceMetric {
    pub kind: PriceMetricKind,
    pub count: u64,
    pub average: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub weighted_average: f64,
    /// Робастные показатели: медиана и квартили устойчивы к выбросам
    /// и ошибкам в исходных данных, в отличие от min/max.
    pub median: f64,
    pub p25: f64,
    pub p75: f64,
}

/// SQLite-агрегат: собирает значения и возвращает "p25|p50|p75".
struct PercentilesAggregate;

impl rusqlite::functions::Aggregate<Vec<f64>, Option<String>> for PercentilesAggregate {
    fn init(&self, _ctx: &mut rusqlite::functions::Context<'_>) -> rusqlite::Result<Vec<f64>> {
        Ok(Vec::new())
    }

    fn step(
        &self,
        ctx: &mut rusqlite::functions::Context<'_>,
        acc: &mut Vec<f64>,
    ) -> rusqlite::Result<()> {
        if let Some(value) = ctx.get::<Option<f64>>(0)?
            && value.is_finite()
        {
            acc.push(value);
        }
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut rusqlite::functions::Context<'_>,
        acc: Option<Vec<f64>>,
    ) -> rusqlite::Result<Option<String>> {
        let mut values = acc.unwrap_or_default();
        if values.is_empty() {
            return Ok(None);
        }
        values.sort_unstable_by(f64::total_cmp);
        let pick = |p: f64| {
            let idx = ((values.len() - 1) as f64 * p).round() as usize;
            values[idx.min(values.len() - 1)]
        };
        Ok(Some(format!("{}|{}|{}", pick(0.25), pick(0.5), pick(0.75))))
    }
}

/// Analytics category computed independently, so the GUI can load only
/// the visible one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnalyticsScope {
    #[default]
    Companies,
    Products,
    Countries,
    Prices,
}

impl AnalyticsScope {
    pub const ALL: [AnalyticsScope; 4] = [
        AnalyticsScope::Companies,
        AnalyticsScope::Products,
        AnalyticsScope::Countries,
        AnalyticsScope::Prices,
    ];

    pub fn index(self) -> usize {
        match self {
            AnalyticsScope::Companies => 0,
            AnalyticsScope::Products => 1,
            AnalyticsScope::Countries => 2,
            AnalyticsScope::Prices => 3,
        }
    }
}

/// One month of import dynamics (chart data).
#[derive(Clone, Debug, Default)]
pub struct AnalyticsMonthRow {
    /// "2024-03"
    pub month: String,
    pub rows: u64,
    pub declarations: u64,
    pub total_value_usd: f64,
    pub total_net_kg: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Analytics {
    pub overview: AnalyticsOverview,
    pub months: Vec<AnalyticsMonthRow>,
    pub company_sections: Vec<AnalyticsSection>,
    pub product_sections: Vec<AnalyticsSection>,
    pub country_sections: Vec<AnalyticsSection>,
    pub price_sections: Vec<AnalyticsPriceMetric>,
    pub top_recipients: Vec<AnalyticsGroupRow>,
    pub top_senders: Vec<AnalyticsGroupRow>,
    pub top_trademarks: Vec<AnalyticsGroupRow>,
    pub top_product_codes: Vec<AnalyticsGroupRow>,
    pub top_origin_countries: Vec<AnalyticsGroupRow>,
}

/// SQLite aggregate: median of the values as a number.
struct MedianAggregate;

impl rusqlite::functions::Aggregate<Vec<f64>, Option<f64>> for MedianAggregate {
    fn init(&self, _ctx: &mut rusqlite::functions::Context<'_>) -> rusqlite::Result<Vec<f64>> {
        Ok(Vec::new())
    }

    fn step(
        &self,
        ctx: &mut rusqlite::functions::Context<'_>,
        acc: &mut Vec<f64>,
    ) -> rusqlite::Result<()> {
        if let Some(value) = ctx.get::<Option<f64>>(0)?
            && value.is_finite()
        {
            acc.push(value);
        }
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut rusqlite::functions::Context<'_>,
        acc: Option<Vec<f64>>,
    ) -> rusqlite::Result<Option<f64>> {
        let mut values = acc.unwrap_or_default();
        if values.is_empty() {
            return Ok(None);
        }
        values.sort_unstable_by(f64::total_cmp);
        Ok(Some(values[values.len() / 2]))
    }
}

/// One declaration flagged as potentially undervalued: its price per kg is
/// well below the median for the same product code.
#[derive(Clone, Debug, Default)]
pub struct UndervaluedRow {
    pub id: i64,
    pub declaration_date: String,
    pub declaration_number: String,
    pub recipient: String,
    pub product_code: String,
    pub description: String,
    pub price_per_kg: f64,
    pub code_median: f64,
    /// price_per_kg / code_median (0.3 means 30% of the typical price).
    pub ratio: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Undervaluation {
    pub rows: Vec<UndervaluedRow>,
    /// Number of distinct product codes that had enough samples to judge.
    pub checked_codes: u64,
}

/// Dimension for the pivot table (rows or columns).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PivotDim {
    Recipient,
    Sender,
    Edrpou,
    ProductCode,
    Trademark,
    OriginCountry,
    DispatchCountry,
    TradeCountry,
    Month,
    Year,
}

impl PivotDim {
    fn sql(self) -> &'static str {
        match self {
            PivotDim::Recipient => "TRIM(r.recipient)",
            PivotDim::Sender => "TRIM(r.sender)",
            PivotDim::Edrpou => "TRIM(r.edrpou)",
            PivotDim::ProductCode => "TRIM(r.product_code)",
            PivotDim::Trademark => "TRIM(r.trademark)",
            PivotDim::OriginCountry => "TRIM(r.origin_country)",
            PivotDim::DispatchCountry => "TRIM(r.dispatch_country)",
            PivotDim::TradeCountry => "TRIM(r.trade_country)",
            PivotDim::Month => "SUBSTR(TRIM(r.declaration_date), 1, 7)",
            PivotDim::Year => "CAST(r.year AS TEXT)",
        }
    }

    /// The filter field this dimension maps to, for drill-down clicks.
    pub fn filter_field(self) -> Option<AnalyticsFilterField> {
        match self {
            PivotDim::Recipient => Some(AnalyticsFilterField::Recipient),
            PivotDim::Sender => Some(AnalyticsFilterField::Sender),
            PivotDim::Edrpou => Some(AnalyticsFilterField::Edrpou),
            PivotDim::ProductCode => Some(AnalyticsFilterField::ProductCode),
            PivotDim::Trademark => Some(AnalyticsFilterField::Trademark),
            PivotDim::OriginCountry => Some(AnalyticsFilterField::OriginCountry),
            PivotDim::DispatchCountry => Some(AnalyticsFilterField::DispatchCountry),
            PivotDim::TradeCountry => Some(AnalyticsFilterField::TradeCountry),
            PivotDim::Month | PivotDim::Year => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PivotMetric {
    Value,
    Rows,
    NetKg,
}

impl PivotMetric {
    fn sql(self) -> &'static str {
        match self {
            PivotMetric::Value => "COALESCE(SUM(num_value(r.currency_control_value)), 0.0)",
            PivotMetric::Rows => "CAST(COUNT(*) AS REAL)",
            PivotMetric::NetKg => "COALESCE(SUM(num_value(r.net_kg)), 0.0)",
        }
    }
}

/// Cross-tab: a matrix of one dimension by another for a chosen metric.
#[derive(Clone, Debug, Default)]
pub struct PivotResult {
    pub row_labels: Vec<String>,
    pub col_labels: Vec<String>,
    /// cells[row][col].
    pub cells: Vec<Vec<f64>>,
    pub row_totals: Vec<f64>,
    pub col_totals: Vec<f64>,
    pub grand_total: f64,
    /// True when low-ranked rows/columns were folded into an "others" bucket.
    pub rows_truncated: bool,
    pub cols_truncated: bool,
}

/// Single-company dossier built for one EDRPOU: everything an analyst needs
/// to answer "tell me everything about this importer" on one screen.
#[derive(Clone, Debug, Default)]
pub struct CompanyProfile {
    pub edrpou: String,
    /// All recipient-name variants seen for this EDRPOU.
    pub names: Vec<String>,
    pub overview: AnalyticsOverview,
    pub months: Vec<AnalyticsMonthRow>,
    pub top_products: Vec<AnalyticsGroupRow>,
    pub top_senders: Vec<AnalyticsGroupRow>,
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
        // Перцентили одним проходом: "p25|p50|p75" или NULL без значений.
        self.conn.create_aggregate_function(
            "pctl_text",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            PercentilesAggregate,
        )?;
        // Медиана как число — для соединений и фильтров в SQL.
        self.conn.create_aggregate_function(
            "median_num",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            MedianAggregate,
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

    /// Full analytics across every scope (used by the CLI and tests).
    /// The GUI requests one scope at a time via [`Db::analytics_scoped`],
    /// which is several times cheaper on broad queries.
    pub fn analytics(&self, q: &Query, limit: u64) -> rusqlite::Result<Analytics> {
        let mut analytics = self.analytics_scoped(q, limit, Some(AnalyticsScope::Companies), 10)?;
        let products = self.analytics_scoped(q, limit, Some(AnalyticsScope::Products), 10)?;
        let countries = self.analytics_scoped(q, limit, Some(AnalyticsScope::Countries), 10)?;
        let prices = self.analytics_scoped(q, limit, Some(AnalyticsScope::Prices), 10)?;
        analytics.product_sections = products.product_sections;
        analytics.top_trademarks = products.top_trademarks;
        analytics.top_product_codes = products.top_product_codes;
        analytics.country_sections = countries.country_sections;
        analytics.top_origin_countries = countries.top_origin_countries;
        analytics.price_sections = prices.price_sections;
        Ok(analytics)
    }

    /// Overview, monthly dynamics and the sections of a single scope.
    /// `scope = None` computes only the overview and months (for the
    /// Overview tab). `hs_level` groups product codes by their first
    /// 2/4/6 digits; 10 keeps full codes.
    pub fn analytics_scoped(
        &self,
        q: &Query,
        limit: u64,
        scope: Option<AnalyticsScope>,
        hs_level: u8,
    ) -> rusqlite::Result<Analytics> {
        let overview = self.analytics_overview(q)?;
        let months = self.analytics_months(q)?;
        let mut analytics = Analytics {
            overview,
            months,
            ..Default::default()
        };
        let overview = &analytics.overview;
        match scope {
            None => {}
            Some(AnalyticsScope::Companies) => {
                analytics.company_sections = vec![
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::Recipients,
                        "r.recipient",
                        AnalyticsFilterField::Recipient,
                        limit,
                        overview,
                    )?,
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::Senders,
                        "r.sender",
                        AnalyticsFilterField::Sender,
                        limit,
                        overview,
                    )?,
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::Edrpou,
                        "r.edrpou",
                        AnalyticsFilterField::Edrpou,
                        limit,
                        overview,
                    )?,
                ];
                analytics.top_recipients =
                    section_rows(&analytics.company_sections, AnalyticsSectionKind::Recipients);
                analytics.top_senders =
                    section_rows(&analytics.company_sections, AnalyticsSectionKind::Senders);
            }
            Some(AnalyticsScope::Products) => {
                let code_expr = if hs_level >= 10 {
                    "r.product_code".to_string()
                } else {
                    format!("SUBSTR(TRIM(r.product_code), 1, {})", hs_level.clamp(2, 8))
                };
                analytics.product_sections = vec![
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::ProductCodes,
                        &code_expr,
                        AnalyticsFilterField::ProductCode,
                        limit,
                        overview,
                    )?,
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::Trademarks,
                        "r.trademark",
                        AnalyticsFilterField::Trademark,
                        limit,
                        overview,
                    )?,
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::ProductGroups,
                        "SUBSTR(TRIM(r.description), 1, 80)",
                        AnalyticsFilterField::Description,
                        limit,
                        overview,
                    )?,
                ];
                analytics.top_trademarks =
                    section_rows(&analytics.product_sections, AnalyticsSectionKind::Trademarks);
                analytics.top_product_codes = section_rows(
                    &analytics.product_sections,
                    AnalyticsSectionKind::ProductCodes,
                );
            }
            Some(AnalyticsScope::Countries) => {
                analytics.country_sections = vec![
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::OriginCountries,
                        "r.origin_country",
                        AnalyticsFilterField::OriginCountry,
                        limit,
                        overview,
                    )?,
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::DispatchCountries,
                        "r.dispatch_country",
                        AnalyticsFilterField::DispatchCountry,
                        limit,
                        overview,
                    )?,
                    self.analytics_group(
                        q,
                        AnalyticsSectionKind::TradeCountries,
                        "r.trade_country",
                        AnalyticsFilterField::TradeCountry,
                        limit,
                        overview,
                    )?,
                ];
                analytics.top_origin_countries = section_rows(
                    &analytics.country_sections,
                    AnalyticsSectionKind::OriginCountries,
                );
            }
            Some(AnalyticsScope::Prices) => {
                analytics.price_sections = vec![
                    self.price_metric(
                        q,
                        PriceMetricKind::ValuePerNetKg,
                        "CASE
                            WHEN num_value(r.currency_control_value) IS NOT NULL
                                AND num_value(r.net_kg) IS NOT NULL
                                AND num_value(r.net_kg) > 0
                            THEN num_value(r.currency_control_value) / num_value(r.net_kg)
                         END",
                    )?,
                    self.price_metric(q, PriceMetricKind::RfvUsdKg, "num_value(r.rfv_usd_kg)")?,
                    self.price_metric(
                        q,
                        PriceMetricKind::RmvNetUsdKg,
                        "num_value(r.rmv_net_usd_kg)",
                    )?,
                    self.price_metric(
                        q,
                        PriceMetricKind::RmvUsdExtraUnit,
                        "num_value(r.rmv_usd_extra_unit)",
                    )?,
                    self.price_metric(
                        q,
                        PriceMetricKind::RmvGrossUsdKg,
                        "num_value(r.rmv_gross_usd_kg)",
                    )?,
                    self.price_metric(
                        q,
                        PriceMetricKind::MinBaseUsdKg,
                        "num_value(r.min_base_usd_kg)",
                    )?,
                ];
            }
        }
        Ok(analytics)
    }

    fn analytics_overview(&self, q: &Query) -> rusqlite::Result<AnalyticsOverview> {
        let (joins, where_sql, params) = self.build_where(q);
        let sql = format!(
            "SELECT
                COUNT(*),
                COUNT(DISTINCT NULLIF(TRIM(r.declaration_number), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.sender), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.recipient), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.edrpou), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.trademark), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.product_code), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.origin_country), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.dispatch_country), '')),
                COUNT(DISTINCT NULLIF(TRIM(r.trade_country), '')),
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
                    declaration_count: row.get::<_, i64>(1)? as u64,
                    distinct_senders: row.get::<_, i64>(2)? as u64,
                    distinct_recipients: row.get::<_, i64>(3)? as u64,
                    distinct_edrpou: row.get::<_, i64>(4)? as u64,
                    distinct_trademarks: row.get::<_, i64>(5)? as u64,
                    distinct_product_codes: row.get::<_, i64>(6)? as u64,
                    distinct_origin_countries: row.get::<_, i64>(7)? as u64,
                    distinct_dispatch_countries: row.get::<_, i64>(8)? as u64,
                    distinct_trade_countries: row.get::<_, i64>(9)? as u64,
                    total_value_usd: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                    total_gross_kg: row.get::<_, Option<f64>>(11)?.unwrap_or(0.0),
                    total_net_kg: row.get::<_, Option<f64>>(12)?.unwrap_or(0.0),
                    total_quantity: row.get::<_, Option<f64>>(13)?.unwrap_or(0.0),
                    avg_value_per_net_kg: 0.0,
                })
            })?;
        Ok(AnalyticsOverview {
            avg_value_per_net_kg: ratio(overview.total_value_usd, overview.total_net_kg),
            ..overview
        })
    }

    /// Import dynamics grouped by month ("YYYY-MM" from the ISO date).
    /// Returns the most recent 48 months in chronological order.
    fn analytics_months(&self, q: &Query) -> rusqlite::Result<Vec<AnalyticsMonthRow>> {
        let (joins, where_sql, params) = self.build_where(q);
        let month_filter =
            "TRIM(r.declaration_date) GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]*'";
        let filter_sql = if where_sql.is_empty() {
            format!(" WHERE {month_filter}")
        } else {
            format!("{where_sql} AND {month_filter}")
        };
        let sql = format!(
            "SELECT
                SUBSTR(TRIM(r.declaration_date), 1, 7) AS month,
                COUNT(*) AS rows_count,
                COUNT(DISTINCT NULLIF(TRIM(r.declaration_number), '')) AS declarations_count,
                COALESCE(SUM(num_value(r.currency_control_value)), 0.0) AS total_value_usd,
                COALESCE(SUM(num_value(r.net_kg)), 0.0) AS total_net_kg
             FROM records r{joins}{filter_sql}
             GROUP BY month
             ORDER BY month DESC
             LIMIT 48"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), |row| {
            Ok(AnalyticsMonthRow {
                month: row.get(0)?,
                rows: row.get::<_, i64>(1)? as u64,
                declarations: row.get::<_, i64>(2)? as u64,
                total_value_usd: row.get(3)?,
                total_net_kg: row.get(4)?,
            })
        })?;
        let mut months: Vec<AnalyticsMonthRow> = rows.flatten().collect();
        months.reverse();
        Ok(months)
    }

    /// Full dossier for one company (by EDRPOU): name variants, headline
    /// numbers, monthly dynamics, and the top products / suppliers / origin
    /// countries. Scoped to the company's rows, so it is fast thanks to the
    /// EDRPOU index even on a multi-million-row database.
    pub fn company_profile(&self, edrpou: &str, limit: u64) -> rusqlite::Result<CompanyProfile> {
        let q = Query {
            text: String::new(),
            filters: Filters {
                edrpou: edrpou.trim().to_string(),
                ..Filters::default()
            },
        };
        let overview = self.analytics_overview(&q)?;
        let months = self.analytics_months(&q)?;
        let top_products = self
            .analytics_group(
                &q,
                AnalyticsSectionKind::ProductCodes,
                "r.product_code",
                AnalyticsFilterField::ProductCode,
                limit,
                &overview,
            )?
            .rows;
        let top_senders = self
            .analytics_group(
                &q,
                AnalyticsSectionKind::Senders,
                "r.sender",
                AnalyticsFilterField::Sender,
                limit,
                &overview,
            )?
            .rows;
        let top_origin_countries = self
            .analytics_group(
                &q,
                AnalyticsSectionKind::OriginCountries,
                "r.origin_country",
                AnalyticsFilterField::OriginCountry,
                limit,
                &overview,
            )?
            .rows;

        let mut names = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT TRIM(recipient) AS name, COUNT(*) AS n
             FROM records
             WHERE TRIM(edrpou) = ?1 AND TRIM(COALESCE(recipient, '')) <> ''
             GROUP BY name ORDER BY n DESC LIMIT 8",
        )?;
        let rows = stmt.query_map([edrpou.trim()], |row| row.get::<_, String>(0))?;
        for name in rows.flatten() {
            names.push(name);
        }

        Ok(CompanyProfile {
            edrpou: edrpou.trim().to_string(),
            names,
            overview,
            months,
            top_products,
            top_senders,
            top_origin_countries,
        })
    }

    /// Finds declarations whose customs value per kg is far below the median
    /// for the same product code — a classic signal of undervaluation. Only
    /// codes with at least `min_samples` priced rows are judged, so a lone
    /// declaration cannot flag itself. Rows are returned most-undervalued first.
    pub fn undervaluation(
        &self,
        q: &Query,
        threshold: f64,
        min_samples: u64,
        limit: u64,
    ) -> rusqlite::Result<Undervaluation> {
        let (joins, where_sql, params) = self.build_where(q);
        let cond = if where_sql.is_empty() { " WHERE" } else { " AND" };
        // priced: one row per declaration line with a usable $/kg.
        // code_stats: median $/kg and sample count per product code.
        let sql = format!(
            "WITH priced AS (
                SELECT r.id AS id,
                    TRIM(r.product_code) AS code,
                    num_value(r.currency_control_value) / num_value(r.net_kg) AS price,
                    r.declaration_date AS dt,
                    r.declaration_number AS num,
                    r.recipient AS recipient,
                    r.description AS descr
                FROM records r{joins}{where_sql}{cond}
                    TRIM(r.product_code) <> ''
                    AND num_value(r.net_kg) > 0
                    AND num_value(r.currency_control_value) > 0
             ),
             code_stats AS (
                SELECT code, median_num(price) AS med, COUNT(*) AS n
                FROM priced GROUP BY code HAVING n >= ?
             )
             SELECT p.id, p.dt, p.num, p.recipient, p.code, p.descr,
                    p.price, c.med, p.price / c.med AS ratio
             FROM priced p JOIN code_stats c ON c.code = p.code
             WHERE c.med > 0 AND p.price < c.med * ?
             ORDER BY ratio ASC
             LIMIT ?"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut bind: Vec<rusqlite::types::Value> = params;
        bind.push((min_samples as i64).into());
        bind.push(threshold.into());
        bind.push((limit as i64).into());
        let mut rows = stmt.query(params_from_iter(bind))?;
        let mut out = Vec::new();
        let mut codes = std::collections::HashSet::new();
        while let Some(row) = rows.next()? {
            let code: String = row.get(4)?;
            codes.insert(code.clone());
            out.push(UndervaluedRow {
                id: row.get(0)?,
                declaration_date: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                declaration_number: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                recipient: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                product_code: code,
                description: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                price_per_kg: row.get(6)?,
                code_median: row.get(7)?,
                ratio: row.get(8)?,
            });
        }
        Ok(Undervaluation {
            rows: out,
            checked_codes: codes.len() as u64,
        })
    }

    /// Cross-tabulation of `row_dim` by `col_dim` for `metric`, over the rows
    /// matching the query. Rows are limited to the top `max_rows` by total and
    /// columns to the top `max_cols`; the remainder is folded into an "others"
    /// bucket so the matrix stays readable.
    pub fn pivot(
        &self,
        q: &Query,
        row_dim: PivotDim,
        col_dim: PivotDim,
        metric: PivotMetric,
        max_rows: usize,
        max_cols: usize,
        others_label: &str,
    ) -> rusqlite::Result<PivotResult> {
        let (joins, where_sql, params) = self.build_where(q);
        let row_sql = row_dim.sql();
        let col_sql = col_dim.sql();
        let non_empty = format!("{row_sql} <> '' AND {col_sql} <> ''");
        let filter_sql = if where_sql.is_empty() {
            format!(" WHERE {non_empty}")
        } else {
            format!("{where_sql} AND {non_empty}")
        };
        let sql = format!(
            "SELECT {row_sql} AS rk, {col_sql} AS ck, {metric} AS v
             FROM records r{joins}{filter_sql}
             GROUP BY rk, ck",
            metric = metric.sql()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;

        // Accumulate into maps, then rank rows and columns by total.
        let mut row_totals: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let mut col_totals: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let mut triples: Vec<(String, String, f64)> = Vec::new();
        while let Some(row) = rows.next()? {
            let rk: String = row.get(0)?;
            let ck: String = row.get(1)?;
            let v: f64 = row.get(2)?;
            *row_totals.entry(rk.clone()).or_default() += v;
            *col_totals.entry(ck.clone()).or_default() += v;
            triples.push((rk, ck, v));
        }

        let rank = |totals: &std::collections::HashMap<String, f64>, limit: usize, sort_label: bool| {
            let mut items: Vec<(String, f64)> =
                totals.iter().map(|(k, v)| (k.clone(), *v)).collect();
            if sort_label {
                items.sort_by(|a, b| a.0.cmp(&b.0));
            } else {
                items.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            }
            let truncated = items.len() > limit;
            items.truncate(limit);
            (
                items.into_iter().map(|(k, _)| k).collect::<Vec<_>>(),
                truncated,
            )
        };

        // Months/years read naturally in chronological order; others by size.
        let col_chrono = matches!(col_dim, PivotDim::Month | PivotDim::Year);
        let (row_labels, rows_truncated) = rank(&row_totals, max_rows, false);
        let (col_labels, cols_truncated) = rank(&col_totals, max_cols, col_chrono);

        let row_index: std::collections::HashMap<&str, usize> = row_labels
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let col_index: std::collections::HashMap<&str, usize> = col_labels
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();

        let n_rows = row_labels.len() + usize::from(rows_truncated);
        let n_cols = col_labels.len() + usize::from(cols_truncated);
        let others_row = row_labels.len();
        let others_col = col_labels.len();
        let mut cells = vec![vec![0.0_f64; n_cols]; n_rows];
        for (rk, ck, v) in triples {
            let ri = row_index.get(rk.as_str()).copied().unwrap_or(others_row);
            let ci = col_index.get(ck.as_str()).copied().unwrap_or(others_col);
            if ri < n_rows && ci < n_cols {
                cells[ri][ci] += v;
            }
        }

        let mut final_row_labels = row_labels;
        if rows_truncated {
            final_row_labels.push(others_label.to_string());
        }
        let mut final_col_labels = col_labels;
        if cols_truncated {
            final_col_labels.push(others_label.to_string());
        }
        let row_tot: Vec<f64> = cells.iter().map(|r| r.iter().sum()).collect();
        let mut col_tot = vec![0.0_f64; n_cols];
        for r in &cells {
            for (ci, v) in r.iter().enumerate() {
                col_tot[ci] += v;
            }
        }
        let grand: f64 = row_tot.iter().sum();

        Ok(PivotResult {
            row_labels: final_row_labels,
            col_labels: final_col_labels,
            cells,
            row_totals: row_tot,
            col_totals: col_tot,
            grand_total: grand,
            rows_truncated,
            cols_truncated,
        })
    }

    fn analytics_group(
        &self,
        q: &Query,
        kind: AnalyticsSectionKind,
        label_expr: &str,
        filter_field: AnalyticsFilterField,
        limit: u64,
        overview: &AnalyticsOverview,
    ) -> rusqlite::Result<AnalyticsSection> {
        let (joins, where_sql, mut params) = self.build_where(q);
        let non_empty = format!("TRIM(COALESCE({label_expr}, '')) <> ''");
        let filter_sql = if where_sql.is_empty() {
            format!(" WHERE {non_empty}")
        } else {
            format!("{where_sql} AND {non_empty}")
        };
        let sql = format!(
            "SELECT
                TRIM({label_expr}) AS label,
                COUNT(*) AS rows_count,
                COUNT(DISTINCT NULLIF(TRIM(r.declaration_number), '')) AS declarations_count,
                COUNT(DISTINCT NULLIF(TRIM(r.edrpou), '')) AS companies_count,
                COALESCE(SUM(num_value(r.currency_control_value)), 0.0) AS total_value_usd,
                COALESCE(SUM(num_value(r.net_kg)), 0.0) AS total_net_kg,
                COALESCE(SUM(num_value(r.gross_kg)), 0.0) AS total_gross_kg,
                COALESCE(SUM(num_value(r.quantity)), 0.0) AS total_quantity
             FROM records r{joins}{filter_sql}
             GROUP BY TRIM({label_expr})
             ORDER BY total_value_usd DESC, total_net_kg DESC, rows_count DESC, label COLLATE NOCASE
             LIMIT ?"
        );
        params.push((limit as i64).into());
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), |row| {
            let label: String = row.get(0)?;
            let total_value_usd: f64 = row.get(4)?;
            let total_net_kg: f64 = row.get(5)?;
            let total_gross_kg: f64 = row.get(6)?;
            let total_quantity: f64 = row.get(7)?;
            let share_base = if overview.total_value_usd > 0.0 {
                overview.total_value_usd
            } else if overview.total_net_kg > 0.0 {
                overview.total_net_kg
            } else {
                overview.row_count as f64
            };
            let share_value = if overview.total_value_usd > 0.0 {
                total_value_usd
            } else if overview.total_net_kg > 0.0 {
                total_net_kg
            } else {
                row.get::<_, i64>(1)? as f64
            };
            Ok(AnalyticsGroupRow {
                filter_action: Some(AnalyticsFilterAction {
                    field: filter_field,
                    value: label.clone(),
                }),
                label,
                rows: row.get::<_, i64>(1)? as u64,
                declarations: row.get::<_, i64>(2)? as u64,
                companies: row.get::<_, i64>(3)? as u64,
                total_value_usd,
                total_net_kg,
                total_gross_kg,
                total_quantity,
                share_percent: ratio(share_value * 100.0, share_base),
                avg_value_per_net_kg: ratio(total_value_usd, total_net_kg),
            })
        })?;
        Ok(AnalyticsSection {
            kind,
            rows: rows.collect::<rusqlite::Result<Vec<_>>>()?,
        })
    }

    fn price_metric(
        &self,
        q: &Query,
        kind: PriceMetricKind,
        price_expr: &str,
    ) -> rusqlite::Result<AnalyticsPriceMetric> {
        let (joins, where_sql, params) = self.build_where(q);
        let sql = format!(
            "SELECT
                COUNT(price),
                AVG(price),
                MIN(price),
                MAX(price),
                SUM(CASE WHEN price IS NOT NULL AND weight IS NOT NULL AND weight > 0
                    THEN price * weight ELSE 0 END),
                SUM(CASE WHEN price IS NOT NULL AND weight IS NOT NULL AND weight > 0
                    THEN weight ELSE 0 END),
                pctl_text(price)
             FROM (
                SELECT {price_expr} AS price, num_value(r.net_kg) AS weight
                FROM records r{joins}{where_sql}
             )"
        );
        self.conn.query_row(&sql, params_from_iter(params), |row| {
            let weighted_sum = row.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
            let weighted_kg = row.get::<_, Option<f64>>(5)?.unwrap_or(0.0);
            let pctls: Option<String> = row.get(6)?;
            let mut parts = pctls
                .as_deref()
                .unwrap_or("")
                .split('|')
                .map(|p| p.parse::<f64>().unwrap_or(0.0));
            let p25 = parts.next().unwrap_or(0.0);
            let median = parts.next().unwrap_or(0.0);
            let p75 = parts.next().unwrap_or(0.0);
            Ok(AnalyticsPriceMetric {
                kind,
                count: row.get::<_, i64>(0)? as u64,
                average: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                minimum: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                maximum: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                weighted_average: ratio(weighted_sum, weighted_kg),
                median,
                p25,
                p75,
            })
        })
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

pub fn analytics_should_run(q: &Query) -> bool {
    !q.is_empty()
}

fn section_rows(
    sections: &[AnalyticsSection],
    kind: AnalyticsSectionKind,
) -> Vec<AnalyticsGroupRow> {
    sections
        .iter()
        .find(|section| section.kind == kind)
        .map(|section| section.rows.clone())
        .unwrap_or_default()
}

fn ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator.abs() <= f64::EPSILON {
        0.0
    } else {
        numerator / denominator
    }
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
