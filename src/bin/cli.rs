//! Command-line utility for database checks without the GUI:
//! import, search, export, and statistics.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use base_search::db::{
    AnalyticsGroupRow, AnalyticsPriceMetric, AnalyticsSection, AnalyticsSectionKind, Db, Filters,
    PriceMetricKind, Query,
};
use base_search::export;
use base_search::import::{self, ImportPhase};
use base_search::schema::RESULT_COLUMNS;
use base_search::web;

const USAGE: &str = "base-search-cli - technical database checks for Base Search

Usage:
  base-search-cli stats  <db>
  base-search-cli peek   <file.xlsx|file.xlsb>
  base-search-cli import <db> <file.xlsx|file.xlsb> [...]
  base-search-cli search <db> [query...] [--limit N] [--year Y] [--code C]
                     [--sender S] [--recipient R] [--edrpou E]
                     [--trademark T] [--description D]
                     [--repeat N] [--warmups N] [--no-print-rows] [--json]
  base-search-cli analytics <db> [query...] [--year Y] [--code C]
                       [--sender S] [--recipient R] [--edrpou E]
                       [--trademark T] [--description D]
  base-search-cli export <db> <out.csv|out.xlsx> [query...]
  base-search-cli web [db] [--host 127.0.0.1] [--port 7832] [--no-open]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("stats") if args.len() == 2 => cmd_stats(Path::new(&args[1])),
        Some("peek") if args.len() == 2 => cmd_peek(Path::new(&args[1])),
        Some("import") if args.len() >= 3 => cmd_import(Path::new(&args[1]), &args[2..]),
        Some("search") if args.len() >= 2 => cmd_search(Path::new(&args[1]), &args[2..]),
        Some("analytics") if args.len() >= 2 => cmd_analytics(Path::new(&args[1]), &args[2..]),
        Some("export") if args.len() >= 3 => cmd_export(Path::new(&args[1]), &args[2], &args[3..]),
        Some("web") => cmd_web(&args[1..]),
        Some("sql") if args.len() == 3 => cmd_sql(Path::new(&args[1]), &args[2]),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_web(args: &[String]) -> Result<(), String> {
    let mut config = web::WebConfig::new(base_search::app::default_db_path());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => {
                i += 1;
                config.host = args
                    .get(i)
                    .ok_or_else(|| "--host requires a value".to_string())?
                    .clone();
            }
            "--port" => {
                i += 1;
                config.port = args
                    .get(i)
                    .ok_or_else(|| "--port requires a value".to_string())?
                    .parse()
                    .map_err(|_| "--port must be a number from 0 to 65535".to_string())?;
            }
            "--no-open" => config.open_browser = false,
            "--token" => {
                i += 1;
                config.token = Some(
                    args.get(i)
                        .ok_or_else(|| "--token requires a value".to_string())?
                        .clone(),
                );
            }
            value if !value.starts_with("--") => config.db_path = PathBuf::from(value),
            other => return Err(format!("Unknown web option: {other}")),
        }
        i += 1;
    }
    web::run(config)
}

fn cmd_stats(db_path: &Path) -> Result<(), String> {
    let db = Db::open(db_path)?;
    println!("Database: {}", db_path.display());
    println!("Rows: {}", db.total_rows());
    println!("Unindexed rows: {}", db.unindexed_rows());
    let log = db.import_log(20);
    if !log.is_empty() {
        println!("Recent imports:");
        for e in log {
            println!(
                "  {}  rows {}  imported {}  duplicates {}  {:.1}s  {}",
                e.file_name, e.total_rows, e.imported, e.duplicates, e.seconds, e.imported_at
            );
        }
    }
    Ok(())
}

/// Diagnostic query: arbitrary SELECT, limited to 50 printed rows.
fn cmd_sql(db_path: &Path, sql: &str) -> Result<(), String> {
    let db = Db::open(db_path)?;
    let mut stmt = db.conn.prepare(sql).map_err(|e| e.to_string())?;
    let n_cols = stmt.column_count();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut printed = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut cells = Vec::with_capacity(n_cols);
        for i in 0..n_cols {
            cells.push(
                row.get::<_, rusqlite::types::Value>(i)
                    .map(|v| match v {
                        rusqlite::types::Value::Null => "NULL".to_string(),
                        rusqlite::types::Value::Integer(x) => x.to_string(),
                        rusqlite::types::Value::Real(x) => x.to_string(),
                        rusqlite::types::Value::Text(s) => s,
                        rusqlite::types::Value::Blob(b) => format!("<blob {}>", b.len()),
                    })
                    .unwrap_or_default(),
            );
        }
        println!("{}", cells.join(" | "));
        printed += 1;
        if printed >= 50 {
            break;
        }
    }
    Ok(())
}

fn cmd_peek(path: &Path) -> Result<(), String> {
    use calamine::Reader;
    let mut wb = calamine::open_workbook_auto(path).map_err(|e| e.to_string())?;
    let names: Vec<String> = wb.sheet_names().to_vec();
    println!("Sheets: {names:?}");
    for (i, name) in names.iter().enumerate().take(3) {
        if let Some(Ok(range)) = wb.worksheet_range_at(i) {
            println!(
                "-- Sheet {i} \"{name}\": {} rows x {} columns",
                range.height(),
                range.width()
            );
            let mut rows = range.rows();
            let headers: Vec<String> = rows
                .next()
                .map(|r| r.iter().map(|d| d.to_string()).collect())
                .unwrap_or_default();
            let sample: Vec<String> = rows
                .next()
                .map(|r| {
                    r.iter()
                        .map(|d| d.to_string().chars().take(40).collect())
                        .collect()
                })
                .unwrap_or_default();
            for (c, h) in headers.iter().enumerate() {
                println!(
                    "   [{c:2}] {h} = {}",
                    sample.get(c).cloned().unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}

fn cmd_import(db_path: &Path, files: &[String]) -> Result<(), String> {
    let mut db = Db::open(db_path)?;
    let cancel = AtomicBool::new(false);
    for file in files {
        let path = PathBuf::from(file);
        println!("== {}", path.display());
        let started = Instant::now();
        let mut last_phase: Option<ImportPhase> = None;
        let mut last_print = Instant::now();
        let summary = import::import_file(&mut db, &path, &cancel, &mut |phase, done, total| {
            let phase_changed = last_phase != Some(phase);
            if phase_changed || last_print.elapsed().as_secs() >= 2 {
                last_phase = Some(phase);
                last_print = Instant::now();
                let name = match phase {
                    ImportPhase::Reading => "reading",
                    ImportPhase::Inserting => "inserting",
                    ImportPhase::Indexing => "indexing",
                };
                if total > 0 {
                    println!("   {name}: {done} / {total}");
                } else {
                    println!("   {name}...");
                }
            }
        });
        match (&summary.error, &summary.skipped_duplicate_of) {
            (Some(e), _) => println!("   ERROR: {e}"),
            (None, Some(previous)) => {
                println!("   skipped: file was already imported (matches \"{previous}\")")
            }
            (None, None) => println!(
                "   done: rows {}, imported {}, duplicates {}, in {:.1}s ({:.0} rows/s)",
                summary.total_rows,
                summary.imported,
                summary.duplicates,
                started.elapsed().as_secs_f64(),
                summary.total_rows as f64 / started.elapsed().as_secs_f64().max(0.001)
            ),
        }
    }
    println!("Total rows in database: {}", db.total_rows());
    Ok(())
}

fn parse_query(args: &[String]) -> Result<(Query, u64), String> {
    parse_query_with_options(args, true)
}

fn parse_export_query(args: &[String]) -> Result<Query, String> {
    parse_query_with_options(args, false).map(|(query, _)| query)
}

fn parse_query_with_options(args: &[String], allow_limit: bool) -> Result<(Query, u64), String> {
    let mut q = Query::default();
    let mut limit = 10u64;
    let mut filters = Filters::default();
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let take = |i: &mut usize, flag: &str| -> Result<String, String> {
            *i += 1;
            match args.get(*i) {
                Some(value) if !value.starts_with("--") => Ok(value.clone()),
                _ => Err(format!("{flag} requires a value")),
            }
        };
        match args[i].as_str() {
            "--limit" if allow_limit => {
                let value = take(&mut i, "--limit")?;
                limit = value
                    .parse()
                    .map_err(|_| "--limit must be a positive integer".to_string())?;
                if limit == 0 {
                    return Err("--limit must be a positive integer".to_string());
                }
            }
            "--limit" => return Err("--limit is not supported by export".to_string()),
            "--year" => filters.year = take(&mut i, "--year")?,
            "--code" => filters.product_code = take(&mut i, "--code")?,
            "--sender" => filters.sender = take(&mut i, "--sender")?,
            "--recipient" => filters.recipient = take(&mut i, "--recipient")?,
            "--edrpou" => filters.edrpou = take(&mut i, "--edrpou")?,
            "--trademark" => filters.trademark = take(&mut i, "--trademark")?,
            "--description" => filters.description = take(&mut i, "--description")?,
            flag if flag.starts_with("--") => return Err(format!("Unknown query option: {flag}")),
            word => words.push(word.to_string()),
        }
        i += 1;
    }
    q.text = words.join(" ");
    q.filters = filters;
    Ok((q, limit))
}

#[derive(Debug, Clone)]
struct SearchOptions {
    repeat: usize,
    warmups: usize,
    print_rows: bool,
    json: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            repeat: 1,
            warmups: 0,
            print_rows: true,
            json: false,
        }
    }
}

#[derive(Debug)]
struct SearchRun {
    count_ms: f64,
    page_ms: f64,
    total: u64,
    page_rows: usize,
    rows: Option<Vec<Vec<String>>>,
}

fn parse_search_args(args: &[String]) -> Result<(Vec<String>, SearchOptions), String> {
    let mut query_args = Vec::new();
    let mut options = SearchOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repeat" => {
                i += 1;
                options.repeat = parse_positive_usize(args.get(i), "--repeat")?;
            }
            "--warmups" => {
                i += 1;
                options.warmups = parse_usize(args.get(i), "--warmups")?;
            }
            "--no-print-rows" => options.print_rows = false,
            "--json" => {
                options.json = true;
                options.print_rows = false;
            }
            arg => query_args.push(arg.to_string()),
        }
        i += 1;
    }
    Ok((query_args, options))
}

fn parse_usize(value: Option<&String>, flag: &str) -> Result<usize, String> {
    value
        .ok_or_else(|| format!("{flag} requires a value"))?
        .parse()
        .map_err(|_| format!("{flag} must be a non-negative integer"))
}

fn parse_positive_usize(value: Option<&String>, flag: &str) -> Result<usize, String> {
    let parsed = parse_usize(value, flag)?;
    if parsed == 0 {
        return Err(format!("{flag} must be at least 1"));
    }
    Ok(parsed)
}

/// Completes the search index after an interrupted import or migration.
fn ensure_indexed(db: &mut Db) -> Result<(), String> {
    if db.unindexed_rows() > 0 {
        eprintln!("Rebuilding search index...");
        let cancel = AtomicBool::new(false);
        db.index_fts(&cancel, |_, _| {})
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn cmd_search(db_path: &Path, args: &[String]) -> Result<(), String> {
    let mut db = Db::open(db_path)?;
    ensure_indexed(&mut db)?;
    let (query_args, options) = parse_search_args(args)?;
    let (q, limit) = parse_query(&query_args)?;
    for _ in 0..options.warmups {
        run_search_once(&db, &q, limit, false)?;
    }
    let mut runs = Vec::with_capacity(options.repeat);
    for idx in 0..options.repeat {
        let keep_rows = options.print_rows && options.repeat == 1 && idx == 0;
        runs.push(run_search_once(&db, &q, limit, keep_rows)?);
    }
    if options.json {
        print_search_json(&q, limit, &options, &runs);
        return Ok(());
    }
    if options.repeat == 1 {
        let run = &runs[0];
        println!(
            "Found: {} (count {:.3} ms, page {:.3} ms)",
            run.total, run.count_ms, run.page_ms
        );
    } else {
        for (idx, run) in runs.iter().enumerate() {
            println!(
                "Run {}: found {} (count {:.3} ms, page {:.3} ms, rows {})",
                idx + 1,
                run.total,
                run.count_ms,
                run.page_ms,
                run.page_rows
            );
        }
        print_search_summary(&runs);
    }
    if options.print_rows
        && options.repeat == 1
        && let Some(rows) = &runs[0].rows
    {
        print_search_rows(rows);
    }
    Ok(())
}

fn run_search_once(db: &Db, q: &Query, limit: u64, keep_rows: bool) -> Result<SearchRun, String> {
    let started = Instant::now();
    let total = db.count(q).map_err(|e| e.to_string())?;
    let count_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let (_ids, rows, _dups) = db.search_page(q, limit, 0).map_err(|e| e.to_string())?;
    let page_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(SearchRun {
        count_ms,
        page_ms,
        total,
        page_rows: rows.len(),
        rows: keep_rows.then_some(rows),
    })
}

fn print_search_rows(rows: &[Vec<String>]) {
    for row in rows {
        let desc: String = result_value(row, "description").chars().take(60).collect();
        println!(
            "  {} | {} | {} | {} | {} | {}",
            result_value(row, "declaration_date"),
            result_value(row, "declaration_number"),
            trunc(result_value(row, "sender"), 25),
            trunc(result_value(row, "recipient"), 25),
            result_value(row, "product_code"),
            desc
        );
    }
}

fn print_search_summary(runs: &[SearchRun]) {
    println!(
        "Summary: count avg {:.3} ms min {:.3} max {:.3}; page avg {:.3} ms min {:.3} max {:.3}",
        avg_ms(runs, |run| run.count_ms),
        min_ms(runs, |run| run.count_ms),
        max_ms(runs, |run| run.count_ms),
        avg_ms(runs, |run| run.page_ms),
        min_ms(runs, |run| run.page_ms),
        max_ms(runs, |run| run.page_ms),
    );
}

fn print_search_json(q: &Query, limit: u64, options: &SearchOptions, runs: &[SearchRun]) {
    let total = runs.first().map(|run| run.total).unwrap_or(0);
    let page_rows = runs.first().map(|run| run.page_rows).unwrap_or(0);
    print!(
        "{{\"query\":\"{}\",\"limit\":{},\"repeat\":{},\"warmups\":{},\"total\":{},\"page_rows\":{},",
        json_escape(&q.text),
        limit,
        options.repeat,
        options.warmups,
        total,
        page_rows
    );
    print!(
        "\"count_ms\":{{\"avg\":{:.3},\"min\":{:.3},\"max\":{:.3}}},",
        avg_ms(runs, |run| run.count_ms),
        min_ms(runs, |run| run.count_ms),
        max_ms(runs, |run| run.count_ms),
    );
    print!(
        "\"page_ms\":{{\"avg\":{:.3},\"min\":{:.3},\"max\":{:.3}}},\"runs\":[",
        avg_ms(runs, |run| run.page_ms),
        min_ms(runs, |run| run.page_ms),
        max_ms(runs, |run| run.page_ms),
    );
    for (idx, run) in runs.iter().enumerate() {
        if idx > 0 {
            print!(",");
        }
        print!(
            "{{\"count_ms\":{:.3},\"page_ms\":{:.3},\"total\":{},\"page_rows\":{}}}",
            run.count_ms, run.page_ms, run.total, run.page_rows
        );
    }
    println!("]}}");
}

fn avg_ms(runs: &[SearchRun], value: impl Fn(&SearchRun) -> f64) -> f64 {
    if runs.is_empty() {
        return 0.0;
    }
    runs.iter().map(value).sum::<f64>() / runs.len() as f64
}

fn min_ms(runs: &[SearchRun], value: impl Fn(&SearchRun) -> f64) -> f64 {
    runs.iter().map(value).fold(f64::INFINITY, f64::min)
}

fn max_ms(runs: &[SearchRun], value: impl Fn(&SearchRun) -> f64) -> f64 {
    runs.iter().map(value).fold(0.0, f64::max)
}

fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            '\n' => "\\n".chars().collect(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            ch => vec![ch],
        })
        .collect()
}

fn cmd_analytics(db_path: &Path, args: &[String]) -> Result<(), String> {
    let mut db = Db::open(db_path)?;
    ensure_indexed(&mut db)?;
    let (q, _) = parse_query(args)?;
    let started = Instant::now();
    let analytics = db.analytics(&q, 10).map_err(|e| e.to_string())?;
    println!(
        "Rows: {}  declarations: {}  recipients: {}  senders: {}  EDRPOU: {}",
        analytics.overview.row_count,
        analytics.overview.declaration_count,
        analytics.overview.distinct_recipients,
        analytics.overview.distinct_senders,
        analytics.overview.distinct_edrpou
    );
    println!(
        "Value: {:.2}  net: {:.3} kg  gross: {:.3} kg  value/kg: {:.2}  quantity: {:.3}",
        analytics.overview.total_value_usd,
        analytics.overview.total_net_kg,
        analytics.overview.total_gross_kg,
        analytics.overview.avg_value_per_net_kg,
        analytics.overview.total_quantity
    );
    println!(
        "Product codes: {}  trademarks: {}  origin countries: {}  dispatch countries: {}  trade countries: {}",
        analytics.overview.distinct_product_codes,
        analytics.overview.distinct_trademarks,
        analytics.overview.distinct_origin_countries,
        analytics.overview.distinct_dispatch_countries,
        analytics.overview.distinct_trade_countries
    );
    print_sections("Companies", &analytics.company_sections);
    print_sections("Goods", &analytics.product_sections);
    print_sections("Countries", &analytics.country_sections);
    print_prices(&analytics.price_sections);
    println!("Done in {} ms", started.elapsed().as_millis());
    Ok(())
}

fn print_sections(group: &str, sections: &[AnalyticsSection]) {
    for section in sections {
        if section.rows.is_empty() {
            continue;
        }
        println!("\n{group} / {}:", analytics_section_title(section.kind));
        print_group(&section.rows);
    }
}

fn print_group(rows: &[AnalyticsGroupRow]) {
    if rows.is_empty() {
        return;
    }
    for row in rows {
        println!(
            "  {} | rows {} | decl {} | companies {} | share {:.1}% | value {:.2} | net {:.3} kg | value/kg {:.2}",
            row.label,
            row.rows,
            row.declarations,
            row.companies,
            row.share_percent,
            row.total_value_usd,
            row.total_net_kg,
            row.avg_value_per_net_kg
        );
    }
}

fn print_prices(metrics: &[AnalyticsPriceMetric]) {
    if metrics.is_empty() {
        return;
    }
    println!("\nPrices:");
    for metric in metrics {
        if metric.count == 0 {
            continue;
        }
        println!(
            "  {} | values {} | avg {:.4} | weighted {:.4} | min {:.4} | max {:.4}",
            price_metric_title(metric.kind),
            metric.count,
            metric.average,
            metric.weighted_average,
            metric.minimum,
            metric.maximum
        );
    }
}

fn analytics_section_title(kind: AnalyticsSectionKind) -> &'static str {
    match kind {
        AnalyticsSectionKind::Recipients => "Recipients / who received",
        AnalyticsSectionKind::Senders => "Senders",
        AnalyticsSectionKind::Edrpou => "EDRPOU",
        AnalyticsSectionKind::ProductCodes => "Product codes",
        AnalyticsSectionKind::Trademarks => "Trademarks",
        AnalyticsSectionKind::ProductGroups => "Description groups",
        AnalyticsSectionKind::OriginCountries => "Origin countries",
        AnalyticsSectionKind::DispatchCountries => "Dispatch countries",
        AnalyticsSectionKind::TradeCountries => "Trade countries",
    }
}

fn price_metric_title(kind: PriceMetricKind) -> &'static str {
    match kind {
        PriceMetricKind::ValuePerNetKg => "Value / net kg",
        PriceMetricKind::RfvUsdKg => "RFV $/kg",
        PriceMetricKind::RmvNetUsdKg => "RMV net $/kg",
        PriceMetricKind::RmvUsdExtraUnit => "RMV extra unit",
        PriceMetricKind::RmvGrossUsdKg => "RMV gross $/kg",
        PriceMetricKind::MinBaseUsdKg => "Minimum base $/kg",
    }
}

fn result_value<'a>(row: &'a [String], name: &str) -> &'a str {
    RESULT_COLUMNS
        .iter()
        .position(|column| *column == name)
        .and_then(|idx| row.get(idx))
        .map(String::as_str)
        .unwrap_or("")
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn cmd_export(db_path: &Path, out: &str, args: &[String]) -> Result<(), String> {
    let mut db = Db::open(db_path)?;
    ensure_indexed(&mut db)?;
    let q = parse_export_query(args)?;
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let mut last_print = Instant::now();
    let written = export::export(&db, &q, Path::new(out), &cancel, |done, total| {
        if last_print.elapsed().as_secs() >= 2 {
            last_print = Instant::now();
            println!("  {done} / {total}");
        }
    })
    .map_err(export_error_message)?;
    println!(
        "Exported {written} rows to {out} in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn export_error_message(err: export::ExportError) -> String {
    match err {
        export::ExportError::TooManyRowsForXlsx(rows) => {
            format!("{rows} rows exceed the XLSX row limit; export CSV instead")
        }
        export::ExportError::UnsupportedExtension(ext) if ext.is_empty() => {
            "Unsupported export extension. Use .csv or .xlsx.".to_string()
        }
        export::ExportError::UnsupportedExtension(ext) => {
            format!("Unsupported export extension: .{ext}. Use .csv or .xlsx.")
        }
        export::ExportError::Cancelled => "Export cancelled".to_string(),
        export::ExportError::Other(message) => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_query_rejects_missing_filter_values() {
        assert!(parse_query(&args(&["--year"])).is_err());
        assert!(parse_query(&args(&["--code"])).is_err());
    }

    #[test]
    fn parse_query_rejects_invalid_limit() {
        assert!(parse_query(&args(&["--limit", "nope"])).is_err());
    }

    #[test]
    fn parse_query_rejects_unknown_flags() {
        assert!(parse_query(&args(&["--unknown"])).is_err());
    }

    #[test]
    fn parse_export_query_rejects_limit() {
        assert!(parse_export_query(&args(&["--limit", "10"])).is_err());
    }
}
