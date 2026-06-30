use std::path::{Path, PathBuf};
use std::time::Instant;

use duckdb::{Connection as DuckConnection, params as duck_params};
use rusqlite::Connection as SqliteConnection;

use crate::db::Query;
use crate::olap::{OlapBenchmarkOptions, OlapBenchmarkReport, OlapScenarioReport};

const PROJECTION_COLUMNS: [&str; 17] = [
    "id",
    "year",
    "declaration_number",
    "sender_label",
    "recipient_label",
    "edrpou_label",
    "product_code",
    "description",
    "trademark_label",
    "origin_key",
    "dispatch_key",
    "trade_key",
    "month",
    "value_num",
    "net_kg_num",
    "gross_kg_num",
    "quantity_num",
];

#[derive(Debug, Clone)]
pub struct DuckProjectionBuild {
    pub projection_path: PathBuf,
    pub rows: u64,
    pub elapsed_ms: f64,
}

pub fn default_projection_path(sqlite_path: &Path) -> PathBuf {
    let mut path = sqlite_path.to_path_buf();
    path.set_extension("duckdb");
    path
}

pub fn build_projection(
    sqlite_path: &Path,
    projection_path: &Path,
) -> Result<DuckProjectionBuild, String> {
    if projection_path.exists() {
        std::fs::remove_file(projection_path).map_err(|err| {
            format!(
                "Could not replace DuckDB projection {}: {err}",
                projection_path.display()
            )
        })?;
    }
    let started = Instant::now();
    let sqlite = SqliteConnection::open(sqlite_path).map_err(|err| err.to_string())?;
    let duck = DuckConnection::open(projection_path).map_err(|err| err.to_string())?;
    prepare_projection_schema(&duck)?;

    let sql = format!(
        "SELECT {} FROM records ORDER BY id",
        PROJECTION_COLUMNS.join(", ")
    );
    let mut stmt = sqlite.prepare(&sql).map_err(|err| err.to_string())?;
    let mut rows = stmt.query([]).map_err(|err| err.to_string())?;
    let mut appender = duck.appender("records").map_err(|err| err.to_string())?;
    let mut inserted = 0u64;
    while let Some(row) = rows.next().map_err(|err| err.to_string())? {
        let id: i64 = row.get(0).map_err(|err| err.to_string())?;
        let year: Option<i64> = row.get(1).map_err(|err| err.to_string())?;
        let declaration_number: Option<String> = row.get(2).map_err(|err| err.to_string())?;
        let sender_label: Option<String> = row.get(3).map_err(|err| err.to_string())?;
        let recipient_label: Option<String> = row.get(4).map_err(|err| err.to_string())?;
        let edrpou_label: Option<String> = row.get(5).map_err(|err| err.to_string())?;
        let product_code: Option<String> = row.get(6).map_err(|err| err.to_string())?;
        let description: Option<String> = row.get(7).map_err(|err| err.to_string())?;
        let trademark_label: Option<String> = row.get(8).map_err(|err| err.to_string())?;
        let origin_key: Option<String> = row.get(9).map_err(|err| err.to_string())?;
        let dispatch_key: Option<String> = row.get(10).map_err(|err| err.to_string())?;
        let trade_key: Option<String> = row.get(11).map_err(|err| err.to_string())?;
        let month: Option<String> = row.get(12).map_err(|err| err.to_string())?;
        let value_num: Option<f64> = row.get(13).map_err(|err| err.to_string())?;
        let net_kg_num: Option<f64> = row.get(14).map_err(|err| err.to_string())?;
        let gross_kg_num: Option<f64> = row.get(15).map_err(|err| err.to_string())?;
        let quantity_num: Option<f64> = row.get(16).map_err(|err| err.to_string())?;
        appender
            .append_row(duck_params![
                id,
                year,
                declaration_number,
                sender_label,
                recipient_label,
                edrpou_label,
                product_code,
                description,
                trademark_label,
                origin_key,
                dispatch_key,
                trade_key,
                month,
                value_num,
                net_kg_num,
                gross_kg_num,
                quantity_num,
            ])
            .map_err(|err| err.to_string())?;
        inserted += 1;
    }
    appender.flush().map_err(|err| err.to_string())?;
    drop(appender);
    write_projection_meta(&duck, sqlite_path, inserted)?;
    Ok(DuckProjectionBuild {
        projection_path: projection_path.to_path_buf(),
        rows: inserted,
        elapsed_ms: round_ms(started.elapsed().as_secs_f64() * 1000.0),
    })
}

pub fn run_duckdb_benchmark(
    projection_path: &Path,
    query: &Query,
    options: &OlapBenchmarkOptions,
) -> Result<OlapBenchmarkReport, String> {
    let conn = DuckConnection::open(projection_path).map_err(|err| err.to_string())?;
    let total_database_rows = query_count(&conn, "SELECT COUNT(*) FROM records")?;
    let filter = DuckFilter::from_query(query);
    let mut scenarios = Vec::new();
    scenarios.push(measure_duck_scenario(
        options,
        "Search count",
        "search",
        "Counts rows matching the projection filter. Text search is LIKE-based, not FTS.",
        || {
            query_count(
                &conn,
                &format!("SELECT COUNT(*) FROM records {}", filter.where_sql()),
            )
        },
    )?);
    scenarios.push(measure_duck_scenario(
        options,
        "First result page",
        "search",
        "Reads the first projected rows for a matching filter.",
        || {
            query_count(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM (SELECT id FROM records {} ORDER BY id LIMIT {})",
                    filter.where_sql(),
                    options.page_limit.clamp(1, 500)
                ),
            )
        },
    )?);
    scenarios.push(measure_duck_scenario(
        options,
        "Analytics overview",
        "olap",
        "Computes headline totals and distinct counts from the DuckDB projection.",
        || {
            query_count(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM (
                    SELECT
                        COUNT(*),
                        COUNT(DISTINCT declaration_number),
                        COUNT(DISTINCT recipient_label),
                        COUNT(DISTINCT sender_label),
                        COUNT(DISTINCT edrpou_label),
                        SUM(value_num),
                        SUM(net_kg_num),
                        SUM(gross_kg_num),
                        SUM(quantity_num)
                    FROM records {}
                )",
                    filter.where_sql()
                ),
            )
        },
    )?);
    scenarios.push(measure_duck_scenario(
        options,
        "Companies aggregation",
        "olap",
        "Groups by recipient, sender, and company id using columnar scans.",
        || {
            Ok(
                count_group_rows(&conn, &filter, "recipient_label", options.section_limit)?
                    + count_group_rows(&conn, &filter, "sender_label", options.section_limit)?
                    + count_group_rows(&conn, &filter, "edrpou_label", options.section_limit)?,
            )
        },
    )?);
    scenarios.push(measure_duck_scenario(
        options,
        "Products aggregation",
        "olap",
        "Groups by product code and trademark using columnar scans.",
        || {
            Ok(
                count_group_rows(&conn, &filter, "product_code", options.section_limit)?
                    + count_group_rows(&conn, &filter, "trademark_label", options.section_limit)?,
            )
        },
    )?);
    scenarios.push(measure_duck_scenario(
        options,
        "Countries aggregation",
        "olap",
        "Groups by origin, dispatch, and trade countries.",
        || {
            Ok(
                count_group_rows(&conn, &filter, "origin_key", options.section_limit)?
                    + count_group_rows(&conn, &filter, "dispatch_key", options.section_limit)?
                    + count_group_rows(&conn, &filter, "trade_key", options.section_limit)?,
            )
        },
    )?);
    scenarios.push(measure_duck_scenario(
        options,
        "Price metrics",
        "olap",
        "Calculates available price-per-weight metrics from projected numeric columns.",
        || {
            query_count(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM (
                    SELECT AVG(value_num / NULLIF(net_kg_num, 0))
                    FROM records
                    {} value_num IS NOT NULL AND net_kg_num IS NOT NULL AND net_kg_num > 0
                )",
                    filter.where_extra_sql()
                ),
            )
        },
    )?);
    scenarios.push(measure_duck_scenario(
        options,
        "Pivot: recipient by month",
        "olap",
        "Builds a compact recipient/month value matrix from grouped rows.",
        || {
            query_count(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM (
                    SELECT recipient_label, month, SUM(value_num)
                    FROM records
                    {} recipient_label IS NOT NULL AND recipient_label <> ''
                      AND month IS NOT NULL AND month <> ''
                    GROUP BY recipient_label, month
                    LIMIT {}
                )",
                    filter.where_extra_sql(),
                    options
                        .pivot_rows
                        .saturating_mul(options.pivot_cols)
                        .clamp(1, 10_000)
                ),
            )
        },
    )?);

    Ok(OlapBenchmarkReport {
        backend: "duckdb",
        total_database_rows,
        unindexed_rows: 0,
        query: query.clone(),
        query_is_empty: query.is_empty(),
        scenarios,
    })
}

fn prepare_projection_schema(conn: &DuckConnection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE records (
            id BIGINT,
            year BIGINT,
            declaration_number VARCHAR,
            sender_label VARCHAR,
            recipient_label VARCHAR,
            edrpou_label VARCHAR,
            product_code VARCHAR,
            description VARCHAR,
            trademark_label VARCHAR,
            origin_key VARCHAR,
            dispatch_key VARCHAR,
            trade_key VARCHAR,
            month VARCHAR,
            value_num DOUBLE,
            net_kg_num DOUBLE,
            gross_kg_num DOUBLE,
            quantity_num DOUBLE
        );
        CREATE TABLE projection_meta(key VARCHAR PRIMARY KEY, value VARCHAR);",
    )
    .map_err(|err| err.to_string())
}

fn write_projection_meta(
    conn: &DuckConnection,
    sqlite_path: &Path,
    rows: u64,
) -> Result<(), String> {
    let source = sqlite_path.display().to_string();
    let rows = rows.to_string();
    let built_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        "INSERT INTO projection_meta VALUES ('source_sqlite', ?), ('rows', ?), ('built_at', ?)",
        duck_params![source, rows, built_at],
    )
    .map(|_| ())
    .map_err(|err| err.to_string())
}

fn count_group_rows(
    conn: &DuckConnection,
    filter: &DuckFilter,
    field: &str,
    limit: u64,
) -> Result<u64, String> {
    query_count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM (
                SELECT {field}, COUNT(*) rows_count, SUM(value_num) total_value, SUM(net_kg_num) net
                FROM records
                {} {field} IS NOT NULL AND {field} <> ''
                GROUP BY {field}
                ORDER BY total_value DESC NULLS LAST, net DESC NULLS LAST, rows_count DESC
                LIMIT {}
            )",
            filter.where_extra_sql(),
            limit.clamp(1, 200)
        ),
    )
}

fn query_count(conn: &DuckConnection, sql: &str) -> Result<u64, String> {
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|value| value.max(0) as u64)
        .map_err(|err| err.to_string())
}

fn measure_duck_scenario(
    options: &OlapBenchmarkOptions,
    name: &'static str,
    category: &'static str,
    note: &'static str,
    mut run: impl FnMut() -> Result<u64, String>,
) -> Result<OlapScenarioReport, String> {
    for _ in 0..options.warmups {
        run()?;
    }
    let repeat = options.repeat.max(1);
    let mut runs_ms = Vec::with_capacity(repeat);
    let mut output_rows = 0;
    for _ in 0..repeat {
        let started = Instant::now();
        output_rows = run()?;
        runs_ms.push(round_ms(started.elapsed().as_secs_f64() * 1000.0));
    }
    Ok(OlapScenarioReport {
        name,
        category,
        output_rows,
        average_ms: round_ms(runs_ms.iter().sum::<f64>() / runs_ms.len() as f64),
        minimum_ms: runs_ms.iter().copied().fold(f64::INFINITY, f64::min),
        maximum_ms: runs_ms.iter().copied().fold(0.0, f64::max),
        runs_ms,
        note,
    })
}

struct DuckFilter {
    conditions: Vec<String>,
}

impl DuckFilter {
    fn from_query(query: &Query) -> Self {
        let mut conditions = Vec::new();
        let text = query.text.trim();
        if !text.is_empty() {
            let needle = sql_string(&text.to_ascii_lowercase());
            conditions.push(format!(
                "lower(coalesce(description, '') || ' ' || coalesce(sender_label, '') || ' ' ||
                 coalesce(recipient_label, '') || ' ' || coalesce(product_code, '') || ' ' ||
                 coalesce(trademark_label, '')) LIKE '%' || {needle} || '%'"
            ));
        }
        let filters = &query.filters;
        push_eq_i64(&mut conditions, "year", &filters.year);
        push_prefix(&mut conditions, "product_code", &filters.product_code);
        push_contains(&mut conditions, "trademark_label", &filters.trademark);
        push_contains(&mut conditions, "description", &filters.description);
        push_contains(&mut conditions, "sender_label", &filters.sender);
        push_contains(&mut conditions, "recipient_label", &filters.recipient);
        push_contains(&mut conditions, "edrpou_label", &filters.edrpou);
        push_eq_text(&mut conditions, "trade_key", &filters.trade_country);
        push_eq_text(&mut conditions, "dispatch_key", &filters.dispatch_country);
        push_eq_text(&mut conditions, "origin_key", &filters.origin_country);
        Self { conditions }
    }

    fn where_sql(&self) -> String {
        if self.conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", self.conditions.join(" AND "))
        }
    }

    fn where_extra_sql(&self) -> String {
        if self.conditions.is_empty() {
            "WHERE".to_string()
        } else {
            format!("WHERE {} AND", self.conditions.join(" AND "))
        }
    }
}

fn push_eq_i64(conditions: &mut Vec<String>, field: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if let Ok(parsed) = value.parse::<i64>() {
        conditions.push(format!("{field} = {parsed}"));
    }
}

fn push_eq_text(conditions: &mut Vec<String>, field: &str, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        conditions.push(format!(
            "upper(coalesce({field}, '')) = {}",
            sql_string(&value.to_ascii_uppercase())
        ));
    }
}

fn push_prefix(conditions: &mut Vec<String>, field: &str, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        conditions.push(format!(
            "coalesce({field}, '') LIKE {} || '%'",
            sql_string(value)
        ));
    }
}

fn push_contains(conditions: &mut Vec<String>, field: &str, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        conditions.push(format!(
            "lower(coalesce({field}, '')) LIKE '%' || {} || '%'",
            sql_string(&value.to_ascii_lowercase())
        ));
    }
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn round_ms(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
