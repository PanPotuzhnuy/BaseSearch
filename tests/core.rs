//! Core integration tests: import, deduplication, FTS search,
//! Cyrillic-aware filters, and CSV/XLSX export.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use base_search::db::{
    AnalyticsFilterField, AnalyticsScope, AnalyticsSectionKind, Db, Filters, PivotDim, PivotLimits,
    PivotMetric, PriceMetricKind, Query, analytics_should_run, build_fts_query, extract_year,
    fts_prefix_terms, parse_number, pivot_filter_action,
};
use base_search::export;
use base_search::import::{self, collapse_ws, normalize_date, normalize_value};
use base_search::schema::{COLUMNS, RESULT_COLUMNS, col_index};
use calamine::Reader;

#[test]
fn public_product_name_is_base_search() {
    assert_eq!(base_search::i18n::UA.app_title, "Base Search");
    assert_eq!(base_search::i18n::RU.app_title, "Base Search");
    assert_eq!(base_search::i18n::EN.app_title, "Base Search");
}

fn result_col(name: &str) -> usize {
    RESULT_COLUMNS
        .iter()
        .position(|column| *column == name)
        .unwrap_or_else(|| panic!("missing result column {name}"))
}

#[test]
fn result_table_exposes_all_source_columns() {
    let expected: Vec<&str> = COLUMNS
        .iter()
        .map(|column| column.name)
        .chain(std::iter::once("source_file"))
        .collect();
    assert_eq!(RESULT_COLUMNS.as_slice(), expected.as_slice());
}

#[test]
fn pivot_dimension_labels_map_to_filter_actions() {
    let action = pivot_filter_action(PivotDim::TradeCountry, "IE").unwrap();
    assert_eq!(action.field, AnalyticsFilterField::TradeCountry);
    assert_eq!(action.value, "IE");

    let action = pivot_filter_action(PivotDim::Recipient, "ТОВ ЕППЛ УКРАЇНА").unwrap();
    assert_eq!(action.field, AnalyticsFilterField::Recipient);
    assert_eq!(action.value, "ТОВ ЕППЛ УКРАЇНА");

    assert_eq!(pivot_filter_action(PivotDim::Month, "2024-03"), None);
    assert_eq!(pivot_filter_action(PivotDim::Year, "2024"), None);
}

/// Creates a test XLSX file with the full schema column set.
fn write_test_xlsx(path: &Path, rows: &[Vec<(&str, &str)>]) {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    for (col, def) in COLUMNS.iter().enumerate() {
        sheet.write_string(0, col as u16, def.header).unwrap();
    }
    for (r, row) in rows.iter().enumerate() {
        for (name, value) in row {
            let col = col_index(name).unwrap() as u16;
            sheet.write_string(r as u32 + 1, col, *value).unwrap();
        }
    }
    workbook.save(path).unwrap();
}

fn write_owned_test_xlsx(path: &Path, rows: &[Vec<(String, String)>]) {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    for (col, def) in COLUMNS.iter().enumerate() {
        sheet.write_string(0, col as u16, def.header).unwrap();
    }
    for (r, row) in rows.iter().enumerate() {
        for (name, value) in row {
            let col = col_index(name).unwrap() as u16;
            sheet
                .write_string(r as u32 + 1, col, value.as_str())
                .unwrap();
        }
    }
    workbook.save(path).unwrap();
}

fn sample_rows() -> Vec<Vec<(&'static str, &'static str)>> {
    vec![
        vec![
            ("declaration_number", "24UA100110000001U1"),
            ("declaration_date", "15.03.2024"),
            ("sender", "GUANGZHOU TRADING CO., LTD"),
            ("edrpou", "12345678"),
            ("recipient", "ТОВ «Вінтаж Імпорт»"),
            ("product_code", "0810500000"),
            ("description", "Ківі свіжі, врожай 2023 року"),
            ("trade_country", "CN"),
        ],
        vec![
            ("declaration_number", "24UA100110000002U2"),
            ("declaration_date", "16.03.2024"),
            ("sender", "BODEGAS RIOJA S.A."),
            ("edrpou", "87654321"),
            ("recipient", "ТОВ «Вино Світу»"),
            ("product_code", "2204101100"),
            ("description", "Вино виноградне ігристе, біле"),
            ("trade_country", "ES"),
        ],
        vec![
            ("declaration_number", "24UA100110000003U3"),
            ("declaration_date", "17.03.2024"),
            ("sender", "SIEMENS AG"),
            ("edrpou", "11112222"),
            ("recipient", "ТОВ «Електро Трейд»"),
            ("product_code", "8504405500"),
            ("description", "Перетворювач напруги статичний"),
            ("trade_country", "DE"),
        ],
    ]
}

#[test]
fn import_search_filter_export() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("test.xlsx");
    let db_path = dir.path().join("data").join("test.db");
    write_test_xlsx(&xlsx, &sample_rows());

    let cancel = AtomicBool::new(false);
    let mut db = Db::open(&db_path).unwrap();
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.total_rows, 3);
    assert_eq!(summary.imported, 3);
    assert_eq!(summary.duplicates, 0);
    assert_eq!(db.total_rows(), 3);
    assert_eq!(db.unindexed_rows(), 0);

    // A file with the same rows plus one new row: overlapping rows are removed
    // by row-level deduplication, and the new row is inserted.
    let xlsx2 = dir.path().join("test2.xlsx");
    let mut rows2 = sample_rows();
    rows2.push(vec![
        ("declaration_number", "24UA100110000004U4"),
        ("declaration_date", "18.03.2024"),
        ("sender", "NEW SENDER LLC"),
        ("edrpou", "99999999"),
        ("recipient", "ТОВ «Нове»"),
        ("product_code", "0810500001"),
        ("description", "Ківі свіжі друга партія"),
        ("trade_country", "CN"),
    ]);
    write_test_xlsx(&xlsx2, &rows2);
    let summary2 = import::import_file(&mut db, &xlsx2, &cancel, &mut |_, _, _| {});
    assert_eq!(summary2.error, None);
    assert_eq!(summary2.imported, 1);
    assert_eq!(summary2.duplicates, 3);
    assert_eq!(db.total_rows(), 4);

    // FTS: exact word matching, case-insensitive for Cyrillic text.
    let q = |text: &str| Query {
        text: text.into(),
        ..Default::default()
    };
    assert_eq!(db.count(&q("виноградне")).unwrap(), 1);
    assert_eq!(db.count(&q("ВИНОГРАДНЕ")).unwrap(), 1);
    // Explicit prefix search with an asterisk.
    assert_eq!(db.count(&q("виноград*")).unwrap(), 1);
    // Numeric product codes are automatically treated as prefixes.
    assert_eq!(db.count(&q("8504")).unwrap(), 1);
    // Search by declaration number.
    assert_eq!(db.count(&q("24UA100110000002U2")).unwrap(), 1);
    // Multiple words are combined as AND.
    assert_eq!(db.count(&q("вино біле")).unwrap(), 1);
    assert_eq!(db.count(&q("вино червоне")).unwrap(), 0);

    // The date is normalized to ISO and the year is extracted.
    let (_, rows) = db.search_page(&q("8504"), 10, 0).unwrap();
    assert_eq!(rows[0][result_col("declaration_date")], "2024-03-17");

    // Filters.
    let filters = Filters {
        year: "2024".into(),
        ..Default::default()
    };
    let fq = Query {
        text: String::new(),
        filters: filters.clone(),
    };
    assert_eq!(db.count(&fq).unwrap(), 4);

    // Recipient filter: Cyrillic text in a different case.
    let filters = Filters {
        recipient: "вінтаж".into(),
        ..Default::default()
    };
    assert_eq!(
        db.count(&Query {
            text: String::new(),
            filters
        })
        .unwrap(),
        1
    );

    // Product code filter: prefix matching.
    let filters = Filters {
        product_code: "2204".into(),
        ..Default::default()
    };
    assert_eq!(
        db.count(&Query {
            text: String::new(),
            filters
        })
        .unwrap(),
        1
    );

    // EDRPOU filter: exact match.
    let filters = Filters {
        edrpou: "11112222".into(),
        ..Default::default()
    };
    assert_eq!(
        db.count(&Query {
            text: String::new(),
            filters
        })
        .unwrap(),
        1
    );

    // Combination: text query plus country filter, with case-insensitive text.
    let filters = Filters {
        trade_country: "es".into(),
        ..Default::default()
    };
    assert_eq!(
        db.count(&Query {
            text: "вино".into(),
            filters
        })
        .unwrap(),
        1
    );
    // The same query with a different country must return no rows.
    let filters = Filters {
        trade_country: "cn".into(),
        ..Default::default()
    };
    assert_eq!(
        db.count(&Query {
            text: "вино".into(),
            filters
        })
        .unwrap(),
        0
    );

    // Record details card.
    let (ids, _) = db.search_page(&q("8504"), 10, 0).unwrap();
    let card = db.record_card(ids[0]).unwrap();
    assert_eq!(card.source_file, "test.xlsx");
    assert!(
        card.fields
            .iter()
            .any(|(h, v)| *h == "Опис товару" && v.contains("Перетворювач"))
    );

    // CSV export: BOM plus all rows.
    let csv_path = dir.path().join("out.csv");
    let n = export::export(&db, &Query::default(), &csv_path, &cancel, |_, _| {}).unwrap();
    assert_eq!(n, 4);
    let bytes = std::fs::read(&csv_path).unwrap();
    assert_eq!(&bytes[..3], b"\xEF\xBB\xBF");
    let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
    assert!(text.contains("Перетворювач напруги"));
    assert!(text.starts_with("Час оформлення;"));

    // XLSX export: opens successfully and contains all data rows.
    let xlsx_out = dir.path().join("out.xlsx");
    let n = export::export(&db, &Query::default(), &xlsx_out, &cancel, |_, _| {}).unwrap();
    assert_eq!(n, 4);
    let mut wb: calamine::Xlsx<_> = calamine::open_workbook(&xlsx_out).unwrap();
    let range = wb.worksheet_range_at(0).unwrap().unwrap();
    assert_eq!(range.height(), 5); // header + 4 rows
}

#[test]
fn analytics_summarizes_filtered_rows_by_value_and_company() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("analytics.xlsx");
    let db_path = dir.path().join("data").join("analytics.db");
    write_test_xlsx(
        &xlsx,
        &[
            vec![
                ("declaration_number", "24UA100110000101U1"),
                ("declaration_date", "15.03.2024"),
                ("sender", "APPLE DISTRIBUTION INTERNATIONAL LTD"),
                ("edrpou", "11111111"),
                ("recipient", "ТОВ «АЙФОН УКРАЇНА»"),
                ("product_code", "8517130000"),
                ("description", "Apple iPhone 15 smartphone"),
                ("trade_country", "IE"),
                ("origin_country", "CN"),
                ("quantity", "10"),
                ("gross_kg", "12.5"),
                ("net_kg", "10"),
                ("currency_control_value", "1 200.50"),
                ("trademark", "Apple"),
            ],
            vec![
                ("declaration_number", "24UA100110000102U2"),
                ("declaration_date", "16.03.2024"),
                ("sender", "APPLE OPERATIONS EUROPE"),
                ("edrpou", "22222222"),
                ("recipient", "ТОВ «ТЕХНО ІМПОРТ»"),
                ("product_code", "8517130000"),
                ("description", "Apple iPhone parts"),
                ("trade_country", "IE"),
                ("origin_country", "CN"),
                ("quantity", "2"),
                ("gross_kg", "3,5"),
                ("net_kg", "2,5"),
                ("currency_control_value", "300,25"),
                ("trademark", "Apple"),
            ],
            vec![
                ("declaration_number", "24UA100110000103U3"),
                ("declaration_date", "17.03.2024"),
                ("sender", "SAMSUNG ELECTRONICS"),
                ("edrpou", "33333333"),
                ("recipient", "ТОВ «ТЕХНО ІМПОРТ»"),
                ("product_code", "8517130000"),
                ("description", "Samsung smartphone"),
                ("trade_country", "KR"),
                ("origin_country", "VN"),
                ("quantity", "5"),
                ("gross_kg", "7"),
                ("net_kg", "6"),
                ("currency_control_value", "700"),
                ("trademark", "Samsung"),
            ],
            vec![
                ("declaration_number", "25UA100110000104U4"),
                ("declaration_date", "12.01.2025"),
                ("sender", "APPLE DISTRIBUTION INTERNATIONAL LTD"),
                ("edrpou", "11111111"),
                ("recipient", "ТОВ «АЙФОН УКРАЇНА»"),
                ("product_code", "8517130000"),
                ("description", "Apple iPhone 16 smartphone"),
                ("trade_country", "IE"),
                ("origin_country", "CN"),
                ("quantity", "20"),
                ("gross_kg", "25"),
                ("net_kg", "20"),
                ("currency_control_value", "2 400"),
                ("trademark", "Apple"),
            ],
        ],
    );

    let cancel = AtomicBool::new(false);
    let mut db = Db::open(&db_path).unwrap();
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.imported, 4);

    let q = Query {
        text: "Apple".into(),
        filters: Filters {
            year: "2024".into(),
            ..Default::default()
        },
    };
    let analytics = db.analytics(&q, 5).unwrap();
    assert_eq!(analytics.overview.row_count, 2);
    assert_eq!(analytics.overview.distinct_senders, 2);
    assert_eq!(analytics.overview.distinct_recipients, 2);
    assert_eq!(analytics.overview.distinct_trademarks, 1);
    assert_close(analytics.overview.total_value_usd, 1500.75);
    assert_close(analytics.overview.total_gross_kg, 16.0);
    assert_close(analytics.overview.total_net_kg, 12.5);
    assert_close(analytics.overview.total_quantity, 12.0);

    assert_eq!(analytics.top_recipients[0].label, "ТОВ «АЙФОН УКРАЇНА»");
    assert_eq!(analytics.top_recipients[0].rows, 1);
    assert_close(analytics.top_recipients[0].total_value_usd, 1200.50);
    assert_eq!(
        analytics.top_senders[0].label,
        "APPLE DISTRIBUTION INTERNATIONAL LTD"
    );
    assert_eq!(analytics.top_trademarks[0].label, "Apple");
    assert_eq!(analytics.top_product_codes[0].label, "8517130000");
    assert_eq!(analytics.top_origin_countries[0].label, "CN");
    assert_eq!(analytics.top_origin_countries[0].rows, 2);

    // Monthly dynamics: both Apple-2024 rows fall into the same month.
    assert_eq!(analytics.months.len(), 1);
    assert_eq!(analytics.months[0].month, "2024-03");
    assert_eq!(analytics.months[0].rows, 2);
    assert_eq!(analytics.months[0].declarations, 2);
    assert_close(analytics.months[0].total_value_usd, 1500.75);

    // Without the year filter the months are listed chronologically.
    let q_all = Query {
        text: "Apple".into(),
        ..Default::default()
    };
    let analytics_all = db.analytics(&q_all, 5).unwrap();
    let months: Vec<&str> = analytics_all
        .months
        .iter()
        .map(|m| m.month.as_str())
        .collect();
    assert_eq!(months, vec!["2024-03", "2025-01"]);
    assert_eq!(analytics_all.months[1].rows, 1);
    assert_close(analytics_all.months[1].total_value_usd, 2400.0);

    // Robust price stats: per-kg prices for the three Apple rows are
    // 120.0, 120.05 and 120.1, so the median is the middle value.
    let price = &analytics_all.price_sections[0];
    assert_eq!(price.kind, PriceMetricKind::ValuePerNetKg);
    assert_eq!(price.count, 3);
    assert_close(price.median, 120.05);
    assert_close(price.p25, 120.05);
    assert_close(price.p75, 120.1);

    // Product codes grouped at the 4-digit HS level.
    let products = db
        .analytics_scoped(&q_all, 5, Some(AnalyticsScope::Products), 4)
        .unwrap();
    assert_eq!(products.top_product_codes[0].label, "8517");
    assert_eq!(products.top_product_codes[0].rows, 3);

    // Overview-only scope skips the heavy section queries.
    let overview_only = db.analytics_scoped(&q_all, 5, None, 10).unwrap();
    assert_eq!(overview_only.overview.row_count, 3);
    assert!(overview_only.company_sections.is_empty());
    assert!(overview_only.price_sections.is_empty());

    // Company dossier for EDRPOU 11111111 (the Apple importer, 2 rows total).
    let profile = db.company_profile("11111111", 10).unwrap();
    assert_eq!(profile.edrpou, "11111111");
    assert_eq!(profile.names, vec!["ТОВ «АЙФОН УКРАЇНА»".to_string()]);
    assert_eq!(profile.overview.row_count, 2);
    assert_close(profile.overview.total_value_usd, 3600.5);
    assert_eq!(profile.top_products[0].label, "8517130000");
    assert_eq!(
        profile.top_senders[0].label,
        "APPLE DISTRIBUTION INTERNATIONAL LTD"
    );
    assert_eq!(profile.top_origin_countries[0].label, "CN");
    // Both months in which this company imported are present.
    let months: Vec<&str> = profile.months.iter().map(|m| m.month.as_str()).collect();
    assert_eq!(months, vec!["2024-03", "2025-01"]);

    // Pivot: recipients (rows) by origin country (columns), counting rows.
    let pivot = db
        .pivot(
            &q_all,
            PivotDim::Recipient,
            PivotDim::OriginCountry,
            PivotMetric::Rows,
            PivotLimits { rows: 25, cols: 18 },
            "others",
        )
        .unwrap();
    // Two recipients, one origin country.
    assert_eq!(pivot.col_labels, vec!["CN".to_string()]);
    assert_eq!(pivot.row_labels.len(), 2);
    assert_eq!(pivot.grand_total, 3.0);
    // The matrix and totals are internally consistent.
    let sum_cells: f64 = pivot.cells.iter().flat_map(|r| r.iter()).sum();
    assert_eq!(sum_cells, pivot.grand_total);
    assert_eq!(pivot.col_totals[0], 3.0);

    // Pivot by month with the value metric: two months, value matches overview.
    let pivot_m = db
        .pivot(
            &q_all,
            PivotDim::Recipient,
            PivotDim::Month,
            PivotMetric::Value,
            PivotLimits { rows: 25, cols: 18 },
            "others",
        )
        .unwrap();
    assert_eq!(
        pivot_m.col_labels,
        vec!["2024-03".to_string(), "2025-01".to_string()]
    );
    // Apple value only: 1200.50 + 300.25 (2024-03) + 2400 (2025-01).
    assert_close(pivot_m.grand_total, 3900.75);
}

#[test]
fn undervaluation_flags_rows_below_code_median() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("under.xlsx");
    let db_path = dir.path().join("data").join("under.db");
    // Six rows with the same product code: five around 10 $/kg and one at
    // 1 $/kg (10 USD / 10 kg) which should be flagged.
    let mut rows = Vec::new();
    for (i, (value, net)) in [
        ("100", "10"),
        ("105", "10"),
        ("98", "10"),
        ("102", "10"),
        ("110", "10"),
        ("10", "10"), // the outlier: 1 $/kg vs ~10 median
    ]
    .iter()
    .enumerate()
    {
        rows.push(vec![
            (
                "declaration_number",
                Box::leak(format!("24UA{i:09}U1").into_boxed_str()) as &str,
            ),
            ("declaration_date", "15.03.2024"),
            ("sender", "FOREIGN SUPPLIER LTD"),
            ("edrpou", "55550000"),
            ("recipient", "ТОВ «Тест»"),
            ("product_code", "1234567890"),
            ("description", "Тестовий товар"),
            ("net_kg", net),
            ("currency_control_value", value),
        ]);
    }
    write_test_xlsx(&xlsx, &rows);

    let cancel = AtomicBool::new(false);
    let mut db = Db::open(&db_path).unwrap();
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.imported, 6);

    let uv = db.undervaluation(&Query::default(), 0.5, 5, 100).unwrap();
    assert_eq!(uv.checked_codes, 1);
    assert_eq!(uv.rows.len(), 1);
    let flagged = &uv.rows[0];
    assert_eq!(flagged.product_code, "1234567890");
    assert_close(flagged.price_per_kg, 1.0);
    assert_close(flagged.code_median, 10.2); // median of {9.8,10,10.2,10.5,11,1} sorted -> index 3 = 10.2
    assert!(flagged.ratio < 0.5);
}

#[test]
fn analytics_section_can_load_all_group_rows_beyond_visible_top() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("many_recipients.xlsx");
    let db_path = dir.path().join("data").join("many_recipients.db");
    let rows: Vec<Vec<(String, String)>> = (1..=12)
        .map(|idx| {
            vec![
                (
                    "declaration_number".to_string(),
                    format!("24UA100110{:06}U1", idx),
                ),
                ("declaration_date".to_string(), "15.03.2024".to_string()),
                (
                    "sender".to_string(),
                    "APPLE DISTRIBUTION INTERNATIONAL LTD".to_string(),
                ),
                ("edrpou".to_string(), format!("{idx:08}")),
                ("recipient".to_string(), format!("APPLE IMPORTER {idx:02}")),
                ("product_code".to_string(), "8517130000".to_string()),
                (
                    "description".to_string(),
                    "Apple iPhone smartphone".to_string(),
                ),
                ("trade_country".to_string(), "IE".to_string()),
                ("dispatch_country".to_string(), "IE".to_string()),
                ("origin_country".to_string(), "CN".to_string()),
                ("quantity".to_string(), "1".to_string()),
                ("gross_kg".to_string(), "1.2".to_string()),
                ("net_kg".to_string(), "1".to_string()),
                (
                    "currency_control_value".to_string(),
                    (idx * 100).to_string(),
                ),
                ("trademark".to_string(), "Apple".to_string()),
            ]
        })
        .collect();
    write_owned_test_xlsx(&xlsx, &rows);

    let cancel = AtomicBool::new(false);
    let mut db = Db::open(&db_path).unwrap();
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.imported, 12);

    let q = Query {
        text: "Apple".into(),
        filters: Filters {
            year: "2024".into(),
            ..Default::default()
        },
    };
    let top = db
        .analytics_scoped(&q, 5, Some(AnalyticsScope::Companies), 10)
        .unwrap();
    let top_recipients = analytics_section(&top.company_sections, AnalyticsSectionKind::Recipients);
    assert_eq!(top_recipients.rows.len(), 5);

    let full = db
        .analytics_section(&q, AnalyticsSectionKind::Recipients, 10, 50)
        .unwrap();
    assert_eq!(full.rows.len(), 12);
    assert_eq!(full.rows[0].label, "APPLE IMPORTER 12");
    assert_eq!(full.rows[11].label, "APPLE IMPORTER 01");
    let filter = full.rows[0].filter_action.as_ref().unwrap();
    assert_eq!(filter.field, AnalyticsFilterField::Recipient);
    assert_eq!(filter.value, "APPLE IMPORTER 12");
}

#[test]
fn analytics_builds_decision_sections_for_trade_questions() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("analytics_sections.xlsx");
    let db_path = dir.path().join("data").join("analytics_sections.db");
    write_test_xlsx(
        &xlsx,
        &[
            vec![
                ("declaration_number", "24UA100110000201U1"),
                ("declaration_date", "15.03.2024"),
                ("sender", "APPLE DISTRIBUTION INTERNATIONAL LTD"),
                ("edrpou", "11111111"),
                ("recipient", "IPHONE UKRAINE LLC"),
                ("product_code", "8517130000"),
                ("description", "Apple iPhone 15 smartphone"),
                ("trade_country", "IE"),
                ("dispatch_country", "IE"),
                ("origin_country", "CN"),
                ("quantity", "10"),
                ("gross_kg", "12.5"),
                ("net_kg", "10"),
                ("currency_control_value", "1 200.50"),
                ("rfv_usd_kg", "120.05"),
                ("rmv_net_usd_kg", "119.5"),
                ("trademark", "Apple"),
            ],
            vec![
                ("declaration_number", "24UA100110000201U1"),
                ("declaration_date", "16.03.2024"),
                ("sender", "APPLE OPERATIONS EUROPE"),
                ("edrpou", "22222222"),
                ("recipient", "TECH IMPORT LLC"),
                ("product_code", "8517790000"),
                ("description", "Apple iPhone parts"),
                ("trade_country", "IE"),
                ("dispatch_country", "PL"),
                ("origin_country", "US"),
                ("quantity", "2"),
                ("gross_kg", "3,5"),
                ("net_kg", "2,5"),
                ("currency_control_value", "300,25"),
                ("rfv_usd_kg", "120.1"),
                ("rmv_net_usd_kg", "121"),
                ("trademark", "Apple"),
            ],
            vec![
                ("declaration_number", "24UA100110000202U2"),
                ("declaration_date", "17.03.2024"),
                ("sender", "APPLE OPERATIONS EUROPE"),
                ("edrpou", "22222222"),
                ("recipient", "TECH IMPORT LLC"),
                ("product_code", "8517790000"),
                ("description", "Apple service replacement unit"),
                ("trade_country", "US"),
                ("dispatch_country", "US"),
                ("origin_country", "US"),
                ("quantity", "1"),
                ("gross_kg", "bad"),
                ("net_kg", ""),
                ("currency_control_value", "not a number"),
                ("rfv_usd_kg", ""),
                ("rmv_net_usd_kg", "bad"),
                ("trademark", "Apple"),
            ],
            vec![
                ("declaration_number", "24UA100110000203U3"),
                ("declaration_date", "17.03.2024"),
                ("sender", "SAMSUNG ELECTRONICS"),
                ("edrpou", "33333333"),
                ("recipient", "TECH IMPORT LLC"),
                ("product_code", "8517130000"),
                ("description", "Samsung smartphone"),
                ("trade_country", "KR"),
                ("dispatch_country", "KR"),
                ("origin_country", "VN"),
                ("quantity", "5"),
                ("gross_kg", "7"),
                ("net_kg", "6"),
                ("currency_control_value", "700"),
                ("trademark", "Samsung"),
            ],
        ],
    );

    let cancel = AtomicBool::new(false);
    let mut db = Db::open(&db_path).unwrap();
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.imported, 4);

    let q = Query {
        text: "Apple".into(),
        filters: Filters {
            year: "2024".into(),
            ..Default::default()
        },
    };
    let analytics = db.analytics(&q, 10).unwrap();
    assert_eq!(analytics.overview.row_count, 3);
    assert_eq!(analytics.overview.declaration_count, 2);
    assert_eq!(analytics.overview.distinct_senders, 2);
    assert_eq!(analytics.overview.distinct_recipients, 2);
    assert_eq!(analytics.overview.distinct_edrpou, 2);
    assert_eq!(analytics.overview.distinct_product_codes, 2);
    assert_eq!(analytics.overview.distinct_trademarks, 1);
    assert_eq!(analytics.overview.distinct_origin_countries, 2);
    assert_eq!(analytics.overview.distinct_dispatch_countries, 3);
    assert_eq!(analytics.overview.distinct_trade_countries, 2);
    assert_close(analytics.overview.total_value_usd, 1500.75);
    assert_close(analytics.overview.total_net_kg, 12.5);
    assert_close(analytics.overview.avg_value_per_net_kg, 120.06);

    let recipients = analytics_section(
        &analytics.company_sections,
        AnalyticsSectionKind::Recipients,
    );
    assert_eq!(recipients.rows[0].label, "IPHONE UKRAINE LLC");
    assert_close(recipients.rows[0].share_percent, 79.9933);
    assert_eq!(recipients.rows[0].declarations, 1);
    assert_close(recipients.rows[0].avg_value_per_net_kg, 120.05);
    let filter = recipients.rows[0].filter_action.as_ref().unwrap();
    assert_eq!(filter.field, AnalyticsFilterField::Recipient);
    assert_eq!(filter.value, "IPHONE UKRAINE LLC");

    let codes = analytics_section(
        &analytics.product_sections,
        AnalyticsSectionKind::ProductCodes,
    );
    assert_eq!(codes.rows[0].label, "8517130000");
    assert_eq!(codes.rows[0].companies, 1);
    assert_eq!(codes.rows[1].label, "8517790000");
    assert_eq!(codes.rows[1].companies, 1);

    let trademarks = analytics_section(
        &analytics.product_sections,
        AnalyticsSectionKind::Trademarks,
    );
    assert_eq!(trademarks.rows[0].label, "Apple");
    assert_eq!(trademarks.rows[0].companies, 2);

    let origin = analytics_section(
        &analytics.country_sections,
        AnalyticsSectionKind::OriginCountries,
    );
    assert_eq!(origin.rows[0].label, "CN");
    assert_eq!(origin.rows[1].label, "US");
    let dispatch = analytics_section(
        &analytics.country_sections,
        AnalyticsSectionKind::DispatchCountries,
    );
    assert_eq!(dispatch.rows.len(), 3);
    let trade = analytics_section(
        &analytics.country_sections,
        AnalyticsSectionKind::TradeCountries,
    );
    assert_eq!(trade.rows[0].label, "IE");

    let value_per_kg = price_metric(&analytics.price_sections, PriceMetricKind::ValuePerNetKg);
    assert_eq!(value_per_kg.count, 2);
    assert_close(value_per_kg.average, 120.075);
    assert_close(value_per_kg.minimum, 120.05);
    assert_close(value_per_kg.maximum, 120.1);
    assert_close(value_per_kg.weighted_average, 120.06);

    let rfv = price_metric(&analytics.price_sections, PriceMetricKind::RfvUsdKg);
    assert_eq!(rfv.count, 2);
    assert_close(rfv.average, 120.075);
    assert_close(rfv.minimum, 120.05);
    assert_close(rfv.maximum, 120.1);
    let rmv = price_metric(&analytics.price_sections, PriceMetricKind::RmvNetUsdKg);
    assert_eq!(rmv.count, 2);
    assert_close(rmv.average, 120.25);

    assert!(!analytics_should_run(&Query::default()));
    assert!(analytics_should_run(&q));
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.0001,
        "expected {expected}, got {actual}"
    );
}

fn analytics_section(
    sections: &[base_search::db::AnalyticsSection],
    kind: AnalyticsSectionKind,
) -> &base_search::db::AnalyticsSection {
    sections
        .iter()
        .find(|section| section.kind == kind)
        .unwrap_or_else(|| panic!("missing section {kind:?}"))
}

fn price_metric(
    metrics: &[base_search::db::AnalyticsPriceMetric],
    kind: PriceMetricKind,
) -> &base_search::db::AnalyticsPriceMetric {
    metrics
        .iter()
        .find(|metric| metric.kind == kind)
        .unwrap_or_else(|| panic!("missing metric {kind:?}"))
}

/// Registry-style format: repeated headers, a declaration number split into
/// three parts, and an Excel serial date.
#[test]
fn import_registry_format() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("registry.xlsx");
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    let headers = [
        "Тип декларації",
        "Тип декларації",
        "Номер декларації",
        "Номер декларації",
        "Номер декларації",
        "Дата оформлення",
        "Митниця оформлення",
        "Торгуюча країна",
        "Код товара",
        "Найменування товару",
        "Отримувач",
        "Отримувач",
        "Відпправник",
        "Вага брутто, кг",
        "Вага нетто, кг",
    ];
    for (c, h) in headers.iter().enumerate() {
        sheet.write_string(0, c as u16, *h).unwrap();
    }
    let row = [
        "40",
        "ДЕ",
        "UA209230",
        "2024",
        "102880",
        "",
        "ЛЬВІВСЬКА МИТНИЦЯ",
        "ПОЛЬЩА",
        "9405500000",
        "Освітлювальні прилади неелектричні",
        "37642136",
        "ТОВ ЮСК УКРАЇНА",
        "JYSK SP Z O O",
        "",
        "",
    ];
    for (c, v) in row.iter().enumerate() {
        if !v.is_empty() {
            sheet.write_string(1, c as u16, *v).unwrap();
        }
    }
    sheet.write_number(1, 5, 45597.0).unwrap(); // date = 2024-11-01
    sheet.write_number(1, 13, 27.95).unwrap();
    sheet.write_number(1, 14, 27.391).unwrap();
    workbook.save(&xlsx).unwrap();

    let db_path = dir.path().join("test.db");
    let mut db = Db::open(&db_path).unwrap();
    let cancel = AtomicBool::new(false);
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.imported, 1);

    let q = Query {
        text: "освітлювальні".into(),
        ..Default::default()
    };
    let (ids, rows) = db.search_page(&q, 10, 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][result_col("declaration_date")], "2024-11-01"); // date converted from serial number
    assert_eq!(
        rows[0][result_col("declaration_number")],
        "UA209230/2024/102880"
    ); // declaration number joined
    assert_eq!(rows[0][result_col("edrpou")], "37642136"); // EDRPOU is read from the first recipient column

    let card = db.record_card(ids[0]).unwrap();
    let get = |h: &str| {
        card.fields
            .iter()
            .find(|(fh, _)| *fh == h)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(get("Одержувач"), "ТОВ ЮСК УКРАЇНА");
    assert_eq!(get("Відправник"), "JYSK SP Z O O");
    assert_eq!(get("Тип"), "40/ДЕ");
    assert_eq!(get("Брутто, кг."), "27.95");

    // The year is extracted from the converted date.
    let filters = Filters {
        year: "2024".into(),
        ..Default::default()
    };
    assert_eq!(
        db.count(&Query {
            text: String::new(),
            filters
        })
        .unwrap(),
        1
    );
}

/// Registry-style variant: product-name column, separate recipient code/name
/// columns, and known source typos in declaration and sender headers.
#[test]
fn import_registry_format_im12_variant() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("registry12.xlsx");
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    let headers = [
        "Тип декларації",
        "Тип декларації",
        "Номер декларації",
        "Номер деклараціх",
        "Номер декларації",
        "Дата оформлення",
        "Митниця оформлення",
        "Код товару",
        "Назва товару",
        "Код фірми отримувача",
        "Назва фірми отримувача",
        "Назва фірми відправиника",
        "Вартість, $/кг",
        "Ознака товару  в контейнері",
        "Метод визначення митної вартості",
        "Фактурна вартість, $",
        "Мито, грн.",
        "Акциз, грн.",
        "ПДВ, грн.",
    ];
    for (c, h) in headers.iter().enumerate() {
        sheet.write_string(0, c as u16, *h).unwrap();
    }
    let row = [
        "40",
        "АА",
        "UA209060",
        "",
        "1479",
        "",
        "ЛЬВІВСЬКА МИТНИЦЯ",
        "7005103000",
        "СКЛО CLIMAGUARD PREMIUM2",
        "32818783",
        "ТОВ ГЮАЛОС",
        "GUARDIAN CZESTOCHOWA SP Z O O",
        "",
        "0",
        "1",
        "",
        "120.5",
        "0",
        "",
    ];
    for (c, v) in row.iter().enumerate() {
        if !v.is_empty() {
            sheet.write_string(1, c as u16, *v).unwrap();
        }
    }
    sheet.write_number(1, 3, 2024.0).unwrap(); // middle declaration-number part
    sheet.write_number(1, 5, 45627.0).unwrap(); // date = 2024-12-01
    sheet.write_number(1, 12, 0.5756).unwrap();
    sheet.write_number(1, 15, 8473.2).unwrap();
    sheet.write_number(1, 18, 74216.87).unwrap();
    workbook.save(&xlsx).unwrap();

    let db_path = dir.path().join("test.db");
    let mut db = Db::open(&db_path).unwrap();
    let cancel = AtomicBool::new(false);
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.imported, 1);

    let q = Query {
        text: "climaguard".into(),
        ..Default::default()
    };
    let (ids, rows) = db.search_page(&q, 10, 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][result_col("declaration_date")], "2024-12-01");
    assert_eq!(
        rows[0][result_col("declaration_number")],
        "UA209060/2024/1479"
    ); // all 3 parts, including the typo column
    assert_eq!(rows[0][result_col("edrpou")], "32818783");

    let card = db.record_card(ids[0]).unwrap();
    let get = |h: &str| {
        card.fields
            .iter()
            .find(|(fh, _)| *fh == h)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(get("Одержувач"), "ТОВ ГЮАЛОС");
    assert_eq!(get("Відправник"), "GUARDIAN CZESTOCHOWA SP Z O O");
    assert_eq!(get("Тип"), "40/АА");
    assert_eq!(get("РФВ Дол/кг."), "0.5756");
    assert_eq!(get("ФВ вал.контр"), "8473.2");
    assert_eq!(get("Особ.перем."), "0");
    assert_eq!(get("43"), "1");
    assert_eq!(get("3001"), "120.5");
    assert_eq!(get("3002"), "0");
    assert_eq!(get("9610"), "74216.87");
}

/// Generic detector: an external export with Russian headers and title rows
/// above the actual table header.
#[test]
fn import_generic_format_with_title_rows() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("generic.xlsx");
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    // Two title rows before the table header.
    sheet
        .write_string(0, 0, "Реестр импортных операций")
        .unwrap();
    sheet.write_string(1, 0, "за март 2024 года").unwrap();
    let headers = [
        "Дата",
        "Номер декларации",
        "Отправитель",
        "Получатель",
        "Код ТН ВЭД",
        "Описание товара",
        "Страна происхождения",
        "Вес нетто",
    ];
    for (c, h) in headers.iter().enumerate() {
        sheet.write_string(2, c as u16, *h).unwrap();
    }
    let row = [
        "15.03.2024",
        "UA100100/2024/55555",
        "ACME GMBH",
        "ООО Ромашка",
        "8504405500",
        "Преобразователь напряжения статический",
        "DE",
        "12.5",
    ];
    for (c, v) in row.iter().enumerate() {
        sheet.write_string(3, c as u16, *v).unwrap();
    }
    workbook.save(&xlsx).unwrap();

    let db_path = dir.path().join("test.db");
    let mut db = Db::open(&db_path).unwrap();
    let cancel = AtomicBool::new(false);
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(summary.error, None);
    assert_eq!(summary.imported, 1);

    let q = Query {
        text: "преобразователь".into(),
        ..Default::default()
    };
    let (ids, rows) = db.search_page(&q, 10, 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][result_col("declaration_date")], "2024-03-15");
    assert_eq!(
        rows[0][result_col("declaration_number")],
        "UA100100/2024/55555"
    );
    let card = db.record_card(ids[0]).unwrap();
    assert!(
        card.fields
            .iter()
            .any(|(h, v)| *h == "Відправник" && v == "ACME GMBH")
    );
}

/// Reimporting the same file is skipped by content hash without parsing Excel.
#[test]
fn duplicate_file_skipped_by_content_hash() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("data.xlsx");
    write_test_xlsx(&xlsx, &sample_rows());
    let db_path = dir.path().join("test.db");
    let mut db = Db::open(&db_path).unwrap();
    let cancel = AtomicBool::new(false);

    let first = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(first.error, None);
    assert_eq!(first.imported, 3);
    assert_eq!(first.skipped_duplicate_of, None);

    // The same file is skipped completely.
    let second = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    assert_eq!(second.error, None);
    assert_eq!(second.skipped_duplicate_of, Some("data.xlsx".to_string()));
    assert_eq!(second.total_rows, 0);

    // The same table under another filename is skipped and points to the original.
    let copy = dir.path().join("copy.xlsx");
    std::fs::copy(&xlsx, &copy).unwrap();
    let third = import::import_file(&mut db, &copy, &cancel, &mut |_, _, _| {});
    assert_eq!(third.skipped_duplicate_of, Some("data.xlsx".to_string()));
    assert_eq!(db.total_rows(), 3);
}

#[test]
fn missing_required_columns() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("bad.xlsx");
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet.write_string(0, 0, "Номер МД").unwrap();
    sheet.write_string(0, 1, "Что-то другое").unwrap();
    sheet.write_string(1, 0, "24UA1").unwrap();
    workbook.save(&xlsx).unwrap();

    let db_path = dir.path().join("test.db");
    let mut db = Db::open(&db_path).unwrap();
    let cancel = AtomicBool::new(false);
    let summary = import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});
    let err = summary.error.expect("должна быть ошибка");
    assert!(err.starts_with("__MISSING__"));
    assert!(err.contains("Дата"));
    assert!(err.contains("Опис товару"));
    assert_eq!(db.total_rows(), 0);
}

#[test]
fn contains_ci_works() {
    use base_search::db::contains_ci;
    assert!(contains_ci("ТОВ «Вінтаж Імпорт»", "вінтаж"));
    assert!(contains_ci("SIEMENS AG", "siemens"));
    assert!(contains_ci("ВИНОГРАД", "виноград"));
    assert!(contains_ci("abc", ""));
    assert!(!contains_ci("ТОВ «Вінтаж»", "вино"));
    assert!(!contains_ci("", "вино"));
    assert!(contains_ci("ааб", "аб")); // overlapping prefix
}

#[test]
fn fts_prefix_terms_keeps_long_tokens() {
    // Company names full of one- and two-letter tokens must still produce FTS
    // prefix terms from their distinctive long words — otherwise the filter
    // falls back to a full-table scan (the v1.1 slowdown).
    let terms = fts_prefix_terms("JYSK SP Z O O METEORYTOWA 13 GDANSK").unwrap();
    assert!(terms.contains("\"JYSK\"*"));
    assert!(terms.contains("\"METEORYTOWA\"*"));
    assert!(terms.contains("\"GDANSK\"*"));
    // Short tokens are dropped, not fatal.
    assert!(!terms.contains("\"Z\""));
    assert!(!terms.contains("\"SP\"*"));
    // A name made only of short tokens yields no usable terms.
    assert_eq!(fts_prefix_terms("S A"), None);
    assert_eq!(fts_prefix_terms(""), None);
}

/// A sender/recipient filter must return the same rows whether or not FTS
/// narrowing kicks in, even when the name is full of short tokens.
#[test]
fn sender_filter_with_short_tokens_is_correct() {
    let dir = tempfile::tempdir().unwrap();
    let xlsx = dir.path().join("senders.xlsx");
    let db_path = dir.path().join("data").join("senders.db");
    write_test_xlsx(
        &xlsx,
        &[
            vec![
                ("declaration_number", "24UA0000001U1"),
                ("declaration_date", "15.03.2024"),
                ("sender", "JYSK SP Z O O METEORYTOWA 13 GDANSK"),
                ("edrpou", "10000001"),
                ("recipient", "ТОВ «Перший»"),
                ("product_code", "9405500000"),
                ("description", "Світильники"),
            ],
            vec![
                ("declaration_number", "24UA0000002U2"),
                ("declaration_date", "16.03.2024"),
                ("sender", "OTHER COMPANY LLC"),
                ("edrpou", "10000002"),
                ("recipient", "ТОВ «Другий»"),
                ("product_code", "9405500000"),
                ("description", "Світильники інші"),
            ],
        ],
    );
    let cancel = AtomicBool::new(false);
    let mut db = Db::open(&db_path).unwrap();
    import::import_file(&mut db, &xlsx, &cancel, &mut |_, _, _| {});

    let filters = Filters {
        sender: "JYSK SP Z O O METEORYTOWA 13 GDANSK".into(),
        ..Default::default()
    };
    let q = Query {
        text: String::new(),
        filters,
    };
    assert_eq!(db.count(&q).unwrap(), 1);
    let (_, rows) = db.search_page(&q, 10, 0).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][result_col("edrpou")], "10000001");
}

#[test]
fn fts_query_builder() {
    assert_eq!(build_fts_query("вино, сок"), "\"вино\" \"сок\"");
    assert_eq!(build_fts_query("вин*"), "\"вин\"*");
    assert_eq!(build_fts_query("8504"), "\"8504\"*");
    assert_eq!(build_fts_query("код 850440"), "\"код\" \"850440\"*");
    assert_eq!(build_fts_query("  "), "");
    assert_eq!(build_fts_query("24UA100110"), "\"24UA100110\"");
}

#[test]
fn value_normalization() {
    use calamine::Data;
    assert_eq!(normalize_value(&Data::Float(8504405500.0)), "8504405500");
    assert_eq!(normalize_value(&Data::Float(12.5)), "12.5");
    assert_eq!(
        normalize_value(&Data::String("  а   б\n в ".into())),
        "а б в"
    );
    assert_eq!(normalize_value(&Data::Empty), "");
    assert_eq!(collapse_ws("  один\tдва  "), "один два");
    assert_eq!(normalize_date("31.12.2024"), "2024-12-31");
    assert_eq!(normalize_date("1.3.2024"), "2024-03-01");
    assert_eq!(normalize_date("2024-12-31"), "2024-12-31");
    assert_eq!(normalize_date("не дата"), "не дата");
    assert_eq!(extract_year("2024-03-15"), Some(2024));
    assert_eq!(extract_year("15.03.2024"), Some(2024));
    assert_eq!(extract_year("120245"), None);
    assert_eq!(extract_year(""), None);
    assert_eq!(parse_number("1 234,56"), Some(1234.56));
    assert_eq!(parse_number("1,234.56"), Some(1234.56));
    assert_eq!(parse_number("$ 300,25"), Some(300.25));
    assert_eq!(parse_number(""), None);
}
