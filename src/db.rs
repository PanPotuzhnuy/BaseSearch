//! Public SQLite facade.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use rusqlite::Connection;
use rusqlite::types::Value;

use crate::domain::table::TableShape;
use crate::search::{FieldInfo, field_catalog_for_context, result_field_catalog_for_context};
use crate::storage::extra::{parse_extra, remember_extra_header};
use crate::storage::normalize::normalize_text_key;
use crate::storage::{
    analytics_repo, connection as storage_connection, fts_index, import_log, maintenance, meta,
    query_plan, record_writer, result_repo, table_shape,
};

pub use crate::db_types::*;
pub use crate::storage::maintenance::{DatabaseStorageInfo, WalCheckpointInfo};
pub use crate::storage::normalize::{extract_year, parse_number};
pub use crate::storage::records::canonical_record_hash;
pub use crate::storage::search_text::{build_fts_query, contains_ci, fts_prefix_terms};

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db, String> {
        Ok(Db {
            conn: storage_connection::open(path)?,
        })
    }

    // ---------- meta ----------

    pub fn meta_get(&self, key: &str) -> Option<String> {
        meta::get(&self.conn, key)
    }

    pub fn meta_set(&self, key: &str, value: &str) {
        meta::set(&self.conn, key, value);
    }

    fn meta_get_i64(&self, key: &str) -> i64 {
        meta::get_i64(&self.conn, key)
    }

    pub fn diagnostic_execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
        self.conn.execute_batch(sql)
    }

    pub fn diagnostic_execute(&self, sql: &str) -> rusqlite::Result<usize> {
        self.conn.execute(sql, [])
    }

    pub fn diagnostic_query_rows(
        &self,
        sql: &str,
        max_rows: usize,
    ) -> rusqlite::Result<Vec<Vec<String>>> {
        let mut stmt = self.conn.prepare(sql)?;
        let n_cols = stmt.column_count();
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while out.len() < max_rows {
            let Some(row) = rows.next()? else {
                break;
            };
            let mut cells = Vec::with_capacity(n_cols);
            for i in 0..n_cols {
                cells.push(sql_value_to_text(row.get::<_, Value>(i)?));
            }
            out.push(cells);
        }
        Ok(out)
    }

    // ---------- insert ----------

    pub fn begin_import_file(&mut self) -> rusqlite::Result<()> {
        record_writer::begin_import_file(&self.conn)
    }

    pub fn commit_import_file(&mut self) -> rusqlite::Result<()> {
        record_writer::commit_import_file(&self.conn)
    }

    pub fn rollback_import_file(&mut self) {
        record_writer::rollback_import_file(&self.conn);
    }

    /// Inserts a row batch. Duplicates are inserted and flagged.
    /// Returns (inserted physical rows, duplicate rows).
    pub fn insert_batch(
        &mut self,
        source_file: &str,
        records: &[ImportRecord],
    ) -> rusqlite::Result<(u64, u64)> {
        record_writer::insert_batch(&self.conn, source_file, records)
    }

    // ---------- FTS ----------

    /// Indexes all rows with an id above the watermark.
    /// Returns (indexed rows, cancelled).
    pub fn index_fts(
        &mut self,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> rusqlite::Result<(u64, bool)> {
        fts_index::index(&mut self.conn, cancel, &mut progress)
    }

    /// Number of rows not yet present in the search index.
    pub fn unindexed_rows(&self) -> u64 {
        fts_index::unindexed_rows(&self.conn)
    }

    /// Searchable field catalog for the current database, including imported
    /// source columns preserved in each row's canonical fields or JSON payload.
    pub fn field_catalog(&self) -> rusqlite::Result<Vec<FieldInfo>> {
        let extra_headers = self.extra_headers()?;
        Ok(field_catalog_for_context(
            self.table_shape().as_ref(),
            extra_headers,
        ))
    }

    pub fn result_fields(&self) -> rusqlite::Result<Vec<FieldInfo>> {
        let extra_headers = self.extra_headers()?;
        Ok(result_field_catalog_for_context(
            self.table_shape().as_ref(),
            extra_headers,
        ))
    }

    pub fn field_catalog_cached(&self) -> Vec<FieldInfo> {
        let extra_headers = self.cached_extra_headers();
        field_catalog_for_context(self.table_shape().as_ref(), extra_headers)
    }

    pub fn result_fields_cached(&self) -> Vec<FieldInfo> {
        let extra_headers = self.cached_extra_headers();
        result_field_catalog_for_context(self.table_shape().as_ref(), extra_headers)
    }

    pub fn table_shape(&self) -> Option<TableShape> {
        table_shape::get(&self.conn)
    }

    pub fn remember_table_shape(&self, shape: &TableShape) -> TableShape {
        table_shape::merge(&self.conn, shape)
    }

    /// Assigns or clears the analytical meaning of a shape column by id, so the
    /// user can tell analytics which generic column is the value, country, etc.
    /// Returns true when the column existed. Used by the column-mapping UI.
    pub fn set_column_semantic(
        &self,
        column_id: &str,
        semantic: Option<crate::domain::table::SemanticField>,
    ) -> bool {
        let Some(mut shape) = table_shape::get(&self.conn) else {
            return false;
        };
        let Some(column) = shape
            .columns
            .iter_mut()
            .find(|column| column.id == column_id)
        else {
            return false;
        };
        column.semantic = semantic;
        table_shape::set(&self.conn, &shape);
        true
    }

    pub fn extra_headers(&self) -> rusqlite::Result<Vec<String>> {
        if let Some(cached) = self.extra_headers_cache() {
            return Ok(cached);
        }
        let headers = self.scan_extra_headers()?;
        self.store_extra_headers(&headers);
        Ok(headers)
    }

    pub fn cached_extra_headers(&self) -> Vec<String> {
        self.extra_headers_cache().unwrap_or_default()
    }

    pub fn remember_extra_headers<I, S>(&self, headers: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let existing = self.extra_headers_cache().unwrap_or_default();
        let mut seen = HashSet::new();
        let mut merged = Vec::new();
        for header in existing.iter().map(String::as_str) {
            remember_extra_header(&mut seen, &mut merged, header);
        }
        for header in headers {
            remember_extra_header(&mut seen, &mut merged, header.as_ref());
        }
        self.store_extra_headers(&merged);
    }

    fn extra_headers_cache(&self) -> Option<Vec<String>> {
        meta::get_string_vec(&self.conn, meta::EXTRA_HEADERS_KEY)
    }

    fn store_extra_headers(&self, headers: &[String]) {
        meta::set_string_vec(&self.conn, meta::EXTRA_HEADERS_KEY, headers);
    }

    fn scan_extra_headers(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT extra FROM records
             WHERE extra IS NOT NULL AND TRIM(extra) <> ''",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        let mut seen = HashSet::new();
        let mut headers = Vec::new();
        for raw in rows {
            for (header, _) in parse_extra(raw?.as_deref()) {
                let key = normalize_text_key(&header);
                if !key.is_empty() && seen.insert(key) {
                    headers.push(header);
                }
            }
        }
        Ok(headers)
    }

    // ---------- search ----------

    fn filter_plan(
        &self,
        q: &Query,
        unique_only: bool,
    ) -> rusqlite::Result<query_plan::FilterPlan> {
        query_plan::build_filter_plan(q, unique_only, self.meta_get_i64("fts_watermark"))
    }

    pub fn count(&self, q: &Query) -> rusqlite::Result<u64> {
        result_repo::count(&self.conn, self.filter_plan(q, false)?)
    }

    /// Legacy fixed-schema result page.
    pub fn search_page(&self, q: &Query, limit: u64, offset: u64) -> rusqlite::Result<SearchPage> {
        result_repo::legacy_search_page(&self.conn, q, self.filter_plan(q, false)?, limit, offset)
    }

    pub fn search_page_dynamic(
        &self,
        q: &Query,
        limit: u64,
        offset: u64,
    ) -> rusqlite::Result<DynamicSearchPage> {
        let fields = self.result_fields_cached();
        result_repo::dynamic_search_page(
            &self.conn,
            q,
            fields,
            self.filter_plan(q, false)?,
            limit,
            offset,
        )
    }

    /// Legacy fixed-schema export row batch using keyset pagination by id.
    pub fn export_batch(
        &self,
        q: &Query,
        last_id: i64,
        limit: u64,
    ) -> rusqlite::Result<(i64, Vec<Vec<String>>)> {
        result_repo::legacy_export_batch(&self.conn, self.filter_plan(q, false)?, last_id, limit)
    }

    pub fn export_batch_dynamic(
        &self,
        q: &Query,
        last_id: i64,
        limit: u64,
    ) -> rusqlite::Result<(Vec<FieldInfo>, i64, Vec<Vec<String>>)> {
        let fields = self.result_fields_cached();
        let (max_id, data) = self.export_batch_fields(q, last_id, limit, &fields)?;
        Ok((fields, max_id, data))
    }

    pub fn export_batch_fields(
        &self,
        q: &Query,
        last_id: i64,
        limit: u64,
        fields: &[FieldInfo],
    ) -> rusqlite::Result<(i64, Vec<Vec<String>>)> {
        result_repo::export_batch_fields(
            &self.conn,
            fields,
            self.filter_plan(q, false)?,
            last_id,
            limit,
        )
    }

    /// Full record card by id.
    pub fn record_card(&self, id: i64) -> rusqlite::Result<RecordCard> {
        if self.table_shape().is_none() {
            return result_repo::legacy_record_card(&self.conn, id);
        }
        let fields = self.result_fields_cached();
        result_repo::record_card(&self.conn, fields, id)
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
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::Edrpou,
                        hs_level,
                        limit,
                        overview,
                    )?,
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::Recipients,
                        hs_level,
                        limit,
                        overview,
                    )?,
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::Senders,
                        hs_level,
                        limit,
                        overview,
                    )?,
                ];
                analytics.top_recipients = section_rows(
                    &analytics.company_sections,
                    AnalyticsSectionKind::Recipients,
                );
                analytics.top_senders =
                    section_rows(&analytics.company_sections, AnalyticsSectionKind::Senders);
            }
            Some(AnalyticsScope::Products) => {
                analytics.product_sections = vec![
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::ProductCodes,
                        hs_level,
                        limit,
                        overview,
                    )?,
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::Trademarks,
                        hs_level,
                        limit,
                        overview,
                    )?,
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::ProductGroups,
                        hs_level,
                        limit,
                        overview,
                    )?,
                ];
                analytics.top_trademarks = section_rows(
                    &analytics.product_sections,
                    AnalyticsSectionKind::Trademarks,
                );
                analytics.top_product_codes = section_rows(
                    &analytics.product_sections,
                    AnalyticsSectionKind::ProductCodes,
                );
            }
            Some(AnalyticsScope::Countries) => {
                analytics.country_sections = vec![
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::OriginCountries,
                        hs_level,
                        limit,
                        overview,
                    )?,
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::DispatchCountries,
                        hs_level,
                        limit,
                        overview,
                    )?,
                    self.analytics_section_with_overview(
                        q,
                        AnalyticsSectionKind::TradeCountries,
                        hs_level,
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
                analytics.price_sections = self.analytics_price_metrics(q)?;
            }
        }
        Ok(analytics)
    }

    pub fn analytics_section(
        &self,
        q: &Query,
        kind: AnalyticsSectionKind,
        hs_level: u8,
        limit: u64,
    ) -> rusqlite::Result<AnalyticsSection> {
        let overview = self.analytics_overview(q)?;
        self.analytics_section_with_overview(q, kind, hs_level, limit, &overview)
    }

    fn analytics_section_with_overview(
        &self,
        q: &Query,
        kind: AnalyticsSectionKind,
        hs_level: u8,
        limit: u64,
        overview: &AnalyticsOverview,
    ) -> rusqlite::Result<AnalyticsSection> {
        analytics_repo::section(
            &self.conn,
            self.filter_plan(q, true)?,
            kind,
            hs_level,
            limit,
            overview,
        )
    }

    fn analytics_overview(&self, q: &Query) -> rusqlite::Result<AnalyticsOverview> {
        analytics_repo::overview(&self.conn, self.filter_plan(q, true)?)
    }

    /// Import dynamics grouped by month ("YYYY-MM" from the ISO date).
    /// Returns the most recent 48 months in chronological order.
    fn analytics_months(&self, q: &Query) -> rusqlite::Result<Vec<AnalyticsMonthRow>> {
        analytics_repo::months(&self.conn, self.filter_plan(q, true)?)
    }

    /// Full dossier for one company (by EDRPOU): name variants, headline
    /// numbers, monthly dynamics, and the top products / suppliers / origin
    /// countries. Scoped to the company's rows, so it is fast thanks to the
    /// EDRPOU index even on a multi-million-row database.
    pub fn company_profile(&self, edrpou: &str, limit: u64) -> rusqlite::Result<CompanyProfile> {
        analytics_repo::company_profile(&self.conn, edrpou, limit)
    }

    /// Finds rows whose source value per kg is far below the median for the
    /// same product code — a classic signal of undervaluation. Only
    /// codes with at least `min_samples` priced rows are judged, so a lone
    /// single row cannot flag itself. Rows are returned most-undervalued first.
    pub fn undervaluation(
        &self,
        q: &Query,
        threshold: f64,
        min_samples: u64,
        limit: u64,
    ) -> rusqlite::Result<Undervaluation> {
        analytics_repo::undervaluation(
            &self.conn,
            self.filter_plan(q, true)?,
            threshold,
            min_samples,
            limit,
        )
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
        limits: PivotLimits,
        others_label: &str,
    ) -> rusqlite::Result<PivotResult> {
        analytics_repo::pivot(
            &self.conn,
            self.filter_plan(q, true)?,
            row_dim,
            col_dim,
            metric,
            limits,
            others_label,
        )
    }

    fn analytics_price_metrics(&self, q: &Query) -> rusqlite::Result<Vec<AnalyticsPriceMetric>> {
        analytics_repo::price_metrics(&self.conn, q, self.meta_get_i64("fts_watermark"))
    }

    // ---------- statistics ----------

    pub fn total_rows(&self) -> u64 {
        record_writer::total_rows(&self.conn)
    }

    pub fn add_import_log(&self, entry: ImportLogWrite<'_>) {
        import_log::add(&self.conn, entry);
    }

    pub fn storage_info(&self, db_path: &Path) -> rusqlite::Result<DatabaseStorageInfo> {
        maintenance::storage_info(&self.conn, db_path)
    }

    pub fn checkpoint_wal_truncate(&self) -> rusqlite::Result<WalCheckpointInfo> {
        maintenance::checkpoint_wal_truncate(&self.conn)
    }

    pub fn vacuum_database(&self) -> rusqlite::Result<()> {
        maintenance::vacuum_database(&self.conn)
    }

    /// Full cleanup: removes all records and import logs, then returns disk
    /// space via VACUUM. Settings such as language and theme are preserved.
    pub fn clear_all(&mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| -> rusqlite::Result<()> {
            self.conn.execute_batch(
                "DELETE FROM records_fts;
                 DELETE FROM records;
                 DELETE FROM import_log;",
            )?;
            meta::delete(&self.conn, meta::EXTRA_HEADERS_KEY)?;
            meta::delete(&self.conn, table_shape::TABLE_SHAPE_KEY)?;
            self.meta_set("fts_watermark", "0");
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT;")?,
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                return Err(err);
            }
        }
        self.conn.execute_batch("VACUUM;")?;
        Ok(())
    }

    /// Name of a previously imported file with the same content.
    pub fn find_import_by_hash(&self, file_hash: &str) -> Option<String> {
        import_log::find_by_hash(&self.conn, file_hash)
    }

    pub fn import_log(&self, limit: u64) -> Vec<ImportLogEntry> {
        import_log::list(&self.conn, limit)
    }
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

fn sql_value_to_text(value: Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Integer(x) => x.to_string(),
        Value::Real(x) => x.to_string(),
        Value::Text(s) => s,
        Value::Blob(b) => format!("<blob {}>", b.len()),
    }
}
