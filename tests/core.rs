//! Core integration tests: import, deduplication, FTS search,
//! Cyrillic-aware filters, and CSV/XLSX export.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use base_search::db::{Db, Filters, Query, build_fts_query, extract_year};
use base_search::export;
use base_search::import::{self, collapse_ws, normalize_date, normalize_value};
use base_search::schema::{COLUMNS, col_index};
use calamine::Reader;

#[test]
fn public_product_name_is_base_search() {
    assert_eq!(base_search::i18n::UA.app_title, "Base Search");
    assert_eq!(base_search::i18n::RU.app_title, "Base Search");
    assert_eq!(base_search::i18n::EN.app_title, "Base Search");
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
    assert_eq!(rows[0][0], "2024-03-17");

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
    assert_eq!(rows[0][0], "2024-11-01"); // date converted from serial number
    assert_eq!(rows[0][1], "UA209230/2024/102880"); // declaration number joined
    assert_eq!(rows[0][6], "37642136"); // EDRPOU is read from the first recipient column

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
    ];
    for (c, v) in row.iter().enumerate() {
        if !v.is_empty() {
            sheet.write_string(1, c as u16, *v).unwrap();
        }
    }
    sheet.write_number(1, 3, 2024.0).unwrap(); // middle declaration-number part
    sheet.write_number(1, 5, 45627.0).unwrap(); // date = 2024-12-01
    sheet.write_number(1, 12, 0.5756).unwrap();
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
    assert_eq!(rows[0][0], "2024-12-01");
    assert_eq!(rows[0][1], "UA209060/2024/1479"); // all 3 parts, including the typo column
    assert_eq!(rows[0][6], "32818783");

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
    assert_eq!(rows[0][0], "2024-03-15");
    assert_eq!(rows[0][1], "UA100100/2024/55555");
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
}
