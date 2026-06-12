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

const USAGE: &str = "base-search-cli — техническая проверка базы Base Search

Использование:
  base-search-cli stats  <db>
  base-search-cli peek   <file.xlsx|file.xlsb>
  base-search-cli import <db> <file.xlsx|file.xlsb> [...]
  base-search-cli search <db> [запрос...] [--limit N] [--year Y] [--code C]
                     [--sender S] [--recipient R] [--edrpou E]
  base-search-cli analytics <db> [запрос...] [--year Y] [--code C]
                       [--sender S] [--recipient R] [--edrpou E]
  base-search-cli export <db> <out.csv|out.xlsx> [запрос...]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("stats") if args.len() == 2 => cmd_stats(Path::new(&args[1])),
        Some("peek") if args.len() == 2 => cmd_peek(Path::new(&args[1])),
        Some("import") if args.len() >= 3 => cmd_import(Path::new(&args[1]), &args[2..]),
        Some("search") if args.len() >= 2 => cmd_search(Path::new(&args[1]), &args[2..]),
        Some("analytics") if args.len() >= 2 => cmd_analytics(Path::new(&args[1]), &args[2..]),
        Some("export") if args.len() >= 3 => cmd_export(Path::new(&args[1]), &args[2], &args[3..]),
        Some("sql") if args.len() == 3 => cmd_sql(Path::new(&args[1]), &args[2]),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_stats(db_path: &Path) -> Result<(), String> {
    let db = Db::open(db_path)?;
    println!("База: {}", db_path.display());
    println!("Строк: {}", db.total_rows());
    println!("Не проиндексировано: {}", db.unindexed_rows());
    let log = db.import_log(20);
    if !log.is_empty() {
        println!("Последние импорты:");
        for e in log {
            println!(
                "  {}  строк {}  добавлено {}  дубликатов {}  {:.1}с  {}",
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
    println!("Листы: {names:?}");
    for (i, name) in names.iter().enumerate().take(3) {
        if let Some(Ok(range)) = wb.worksheet_range_at(i) {
            println!(
                "-- Лист {i} «{name}»: {} строк x {} колонок",
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
                    ImportPhase::Reading => "чтение",
                    ImportPhase::Inserting => "запись",
                    ImportPhase::Indexing => "индексация",
                };
                if total > 0 {
                    println!("   {name}: {done} / {total}");
                } else {
                    println!("   {name}...");
                }
            }
        });
        match (&summary.error, &summary.skipped_duplicate_of) {
            (Some(e), _) => println!("   ОШИБКА: {e}"),
            (None, Some(previous)) => {
                println!("   пропущен: файл уже импортирован (совпадает с «{previous}»)")
            }
            (None, None) => println!(
                "   готово: строк {}, добавлено {}, дубликатов {}, за {:.1}с ({:.0} строк/с)",
                summary.total_rows,
                summary.imported,
                summary.duplicates,
                started.elapsed().as_secs_f64(),
                summary.total_rows as f64 / started.elapsed().as_secs_f64().max(0.001)
            ),
        }
    }
    println!("Всего строк в базе: {}", db.total_rows());
    Ok(())
}

fn parse_query(args: &[String]) -> (Query, u64) {
    let mut q = Query::default();
    let mut limit = 10u64;
    let mut filters = Filters::default();
    let mut words: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let take = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_default()
        };
        match args[i].as_str() {
            "--limit" => limit = take(&mut i).parse().unwrap_or(10),
            "--year" => filters.year = take(&mut i),
            "--code" => filters.product_code = take(&mut i),
            "--sender" => filters.sender = take(&mut i),
            "--recipient" => filters.recipient = take(&mut i),
            "--edrpou" => filters.edrpou = take(&mut i),
            word => words.push(word),
        }
        i += 1;
    }
    q.text = words.join(" ");
    q.filters = filters;
    (q, limit)
}

/// Completes the search index after an interrupted import or migration.
fn ensure_indexed(db: &mut Db) -> Result<(), String> {
    if db.unindexed_rows() > 0 {
        eprintln!("Индекс перестраивается...");
        let cancel = AtomicBool::new(false);
        db.index_fts(&cancel, |_, _| {})
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn cmd_search(db_path: &Path, args: &[String]) -> Result<(), String> {
    let mut db = Db::open(db_path)?;
    ensure_indexed(&mut db)?;
    let (q, limit) = parse_query(args);
    let started = Instant::now();
    let total = db.count(&q).map_err(|e| e.to_string())?;
    let count_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let (_ids, rows) = db.search_page(&q, limit, 0).map_err(|e| e.to_string())?;
    let page_ms = started.elapsed().as_millis();
    println!("Найдено: {total} (count {count_ms} мс, страница {page_ms} мс)");
    for row in &rows {
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
    Ok(())
}

fn cmd_analytics(db_path: &Path, args: &[String]) -> Result<(), String> {
    let mut db = Db::open(db_path)?;
    ensure_indexed(&mut db)?;
    let (q, _) = parse_query(args);
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
        "Value: {:.2} $  net: {:.3} kg  gross: {:.3} kg  avg: {:.2} $/kg  quantity: {:.3}",
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
            "  {} | rows {} | decl {} | companies {} | share {:.1}% | {:.2} $ | net {:.3} kg | {:.2} $/kg",
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
    let (q, _) = parse_query(args);
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let mut last_print = Instant::now();
    let written = export::export(&db, &q, Path::new(out), &cancel, |done, total| {
        if last_print.elapsed().as_secs() >= 2 {
            last_print = Instant::now();
            println!("  {done} / {total}");
        }
    })
    .map_err(|e| format!("{e:?}"))?;
    println!(
        "Экспортировано {written} строк в {out} за {:.1}с",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
