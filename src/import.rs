//! Imports Excel files (.xlsx and .xlsb) into the database through calamine.
//!
//! Files are read as a cell stream, so import uses very little memory even for
//! files that are hundreds of megabytes. Before parsing, the file content hash
//! is calculated so repeat imports of the same file can be skipped quickly.
//!
//! Supported column layouts:
//! - format A: 41-column customs layout, with extra columns and noisy headers
//!   tolerated;
//! - format B: registry-style layout with split declaration numbers, repeated
//!   headers, and known source typos;
//! - generic layout: heuristic matching through a multilingual header alias
//!   dictionary, with the header row searched near the beginning of the sheet.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use calamine::{Data, Reader, Sheets, open_workbook_auto};
use chrono::Timelike;
use xxhash_rust::xxh3::Xxh3;

use crate::db::{Db, ImportRecord, extract_year};
use crate::schema::{COLUMNS, DATE_COL, REQUIRED_HEADERS};

const BATCH_SIZE: usize = 8192;
/// Number of first sheet rows scanned while searching for the header row.
const HEADER_SCAN_ROWS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImportPhase {
    /// File reading and parsing.
    Reading,
    /// Writing rows to the database.
    Inserting,
    /// Full-text index construction.
    Indexing,
}

#[derive(Clone, Debug, Default)]
pub struct FileSummary {
    pub file_name: String,
    pub total_rows: u64,
    pub imported: u64,
    pub duplicates: u64,
    pub seconds: f64,
    pub error: Option<String>,
    pub cancelled: bool,
    /// Whole-file skip because this content was already imported.
    /// Stores the previously imported filename.
    pub skipped_duplicate_of: Option<String>,
}

/// Source for a schema column value in a file row.
enum ColSrc {
    /// The file does not contain this column.
    Missing,
    Cell(usize),
    /// Several file columns joined with a separator, such as `UA100290/2024/102794`.
    Join(Vec<usize>, &'static str),
}

// ---------- header mapping ----------

/// Header index that keeps repeated header positions, for example one logical
/// field may appear once as an organization code and again as a company name.
struct HeaderIndex {
    positions: HashMap<String, Vec<usize>>,
}

impl HeaderIndex {
    fn new(headers: &[String]) -> HeaderIndex {
        let mut positions: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, h) in headers.iter().enumerate() {
            if !h.is_empty() {
                positions.entry(h.clone()).or_default().push(i);
            }
        }
        HeaderIndex { positions }
    }

    fn get(&self, header: &str, occurrence: usize) -> Option<usize> {
        self.positions.get(header)?.get(occurrence).copied()
    }

    /// Exact match first; otherwise a file header that starts with the requested
    /// header and continues with a non-alphanumeric character. Used only when
    /// exactly one such candidate exists.
    fn get_fuzzy(&self, header: &str, occurrence: usize) -> Option<usize> {
        if let Some(i) = self.get(header, occurrence) {
            return Some(i);
        }
        let mut candidates = self.positions.iter().filter(|(h, _)| {
            h.strip_prefix(header)
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| !c.is_alphanumeric())
        });
        let (_, positions) = candidates.next()?;
        if candidates.next().is_some() {
            return None; // ambiguous
        }
        positions.get(occurrence).copied()
    }

    fn has(&self, header: &str) -> bool {
        self.positions.contains_key(header)
    }

    /// First found item from a list of candidates: (header, occurrence number).
    fn first_of(&self, candidates: &[(&str, usize)]) -> Option<usize> {
        candidates.iter().find_map(|(h, occ)| self.get(h, *occ))
    }

    fn get_norm(&self, header: &str, occurrence: usize) -> Option<usize> {
        if let Some(pos) = self.get(header, occurrence) {
            return Some(pos);
        }
        let wanted = norm_header(header);
        let mut positions: Vec<usize> = self
            .positions
            .iter()
            .filter(|(h, _)| norm_header(h) == wanted)
            .flat_map(|(_, positions)| positions.iter().copied())
            .collect();
        positions.sort_unstable();
        positions.get(occurrence).copied()
    }

    fn first_norm_of(&self, candidates: &[(&str, usize)]) -> Option<usize> {
        candidates
            .iter()
            .find_map(|(h, occ)| self.get_norm(h, *occ))
    }

    /// All columns whose header starts with the prefix, in file order.
    /// This handles split declaration-number columns, including known typos in
    /// the middle part.
    fn all_with_prefix(&self, prefix: &str) -> Vec<usize> {
        let mut all: Vec<usize> = self
            .positions
            .iter()
            .filter(|(h, _)| h.starts_with(prefix))
            .flat_map(|(_, positions)| positions.iter().copied())
            .collect();
        all.sort_unstable();
        all
    }

    fn all_norm_with_prefixes(&self, prefixes: &[&str]) -> Vec<usize> {
        let prefixes: Vec<String> = prefixes.iter().map(|p| norm_header(p)).collect();
        let mut all: Vec<usize> = self
            .positions
            .iter()
            .filter(|(h, _)| {
                let header = norm_header(h);
                prefixes.iter().any(|prefix| header.starts_with(prefix))
            })
            .flat_map(|(_, positions)| positions.iter().copied())
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    }
}

/// Format A: all required columns are present.
fn map_format_a(idx: &HeaderIndex) -> Option<Vec<ColSrc>> {
    if !REQUIRED_HEADERS
        .iter()
        .all(|h| idx.get_fuzzy(h, 0).is_some())
    {
        return None;
    }
    Some(
        COLUMNS
            .iter()
            .map(|c| match idx.get_fuzzy(c.header, 0) {
                Some(i) => ColSrc::Cell(i),
                None => ColSrc::Missing,
            })
            .collect(),
    )
}

/// Format B: registry-style export.
/// Header names vary between files: product-name variants, repeated recipient
/// headers versus separate recipient code/name columns, and known source typos.
/// Each field is therefore resolved through a list of known variants.
fn map_format_b(idx: &HeaderIndex) -> Option<Vec<ColSrc>> {
    let decl_number_parts = idx.all_with_prefix("Номер деклараці");
    let description = idx.first_of(&[("Найменування товару", 0), ("Назва товару", 0)]);
    let product_code = idx.first_of(&[("Код товара", 0), ("Код товару", 0)]);
    if decl_number_parts.is_empty()
        || !idx.has("Дата оформлення")
        || description.is_none()
        || product_code.is_none()
    {
        return None;
    }
    let opt = |pos: Option<usize>| pos.map(ColSrc::Cell).unwrap_or(ColSrc::Missing);
    let sender = idx.first_of(&[
        ("Відпправник", 0),
        ("Відправник", 0),
        ("Назва фірми відправиника", 0),
        ("Назва фірми відправника", 0),
    ]);
    let edrpou = idx.first_of(&[("Отримувач", 0), ("Код фірми отримувача", 0)]);
    let recipient = idx.first_of(&[("Отримувач", 1), ("Назва фірми отримувача", 0)]);
    let item_number = idx.first_of(&[("Номер товара", 0), ("Номер товару", 0)]);
    let quantity = idx.first_of(&[
        ("Дод.од", 0),
        ("Додатковоа одиниця виміру", 0),
        ("Додаткова одиниця виміру", 0),
    ]);
    let unit = idx.first_of(&[
        ("Дод.од", 1),
        ("Додатковоа одиниця виміру", 1),
        ("Додаткова одиниця виміру", 1),
    ]);
    let value_usd = idx.first_of(&[("Фактурнаа вартість, $", 0), ("Фактурна вартість, $", 0)]);
    let price_usd_kg = idx.first_of(&[("Ціна, $/кг", 0), ("Вартість, $/кг", 0)]);
    let movement_feature = idx.first_of(&[
        ("Ознака товару в контейнері", 0),
        ("Ознака товару у контейнері", 0),
    ]);
    let customs_value_method = idx.first_of(&[("Метод визначення митної вартості", 0)]);
    let duty_uah = idx.first_of(&[("Мито, грн.", 0), ("Мито, грн", 0)]);
    let excise_uah = idx.first_of(&[("Акциз, грн.", 0), ("Акциз, грн", 0)]);
    let vat_uah = idx.first_of(&[("ПДВ, грн.", 0), ("ПДВ, грн", 0)]);
    let decl_type_parts = idx.all_with_prefix("Тип деклараці");
    let join = |parts: &Vec<usize>| {
        if parts.is_empty() {
            ColSrc::Missing
        } else {
            ColSrc::Join(parts.clone(), "/")
        }
    };
    Some(
        COLUMNS
            .iter()
            .map(|c| match c.name {
                "customs_office" => opt(idx.get("Митниця оформлення", 0)),
                "declaration_type" => join(&decl_type_parts),
                "declaration_number" => join(&decl_number_parts),
                "declaration_date" => opt(idx.get("Дата оформлення", 0)),
                "sender" => opt(sender),
                "edrpou" => opt(edrpou),
                "recipient" => opt(recipient),
                "item_number" => opt(item_number),
                "product_code" => opt(product_code),
                "description" => opt(description),
                "trade_country" => opt(idx.get("Торгуюча країна", 0)),
                "dispatch_country" => opt(idx.get("Країна відправлення", 0)),
                "origin_country" => opt(idx.get("Країна походження", 0)),
                "delivery_terms" => opt(idx.get("Умови поставки", 0)),
                "delivery_place" => opt(idx.get("Умови поставки", 1)),
                "quantity" => opt(quantity),
                "unit" => opt(unit),
                "gross_kg" => opt(idx.get("Вага брутто, кг", 0)),
                "net_kg" => opt(idx.get("Вага нетто, кг", 0)),
                "currency_control_value" => opt(value_usd),
                "movement_feature" => opt(movement_feature),
                "field_43" => opt(customs_value_method),
                "rfv_usd_kg" => opt(price_usd_kg),
                "field_3001" => opt(duty_uah),
                "field_3002" => opt(excise_uah),
                "field_9610" => opt(vat_uah),
                _ => ColSrc::Missing,
            })
            .collect(),
    )
}

/// Wide customs export/import layout used by newer 2026 files.
///
/// These files have 50+ columns and repeated participant headers. For imports,
/// the recipient code and recipient name are split into neighboring columns;
/// for exports, the sender code and sender name are split the same way. The
/// generic detector cannot infer that relationship from headers alone, so this
/// layout is mapped explicitly.
fn map_wide_customs(headers: &[String], idx: &HeaderIndex) -> Option<Vec<ColSrc>> {
    if headers.len() < 45 {
        return None;
    }
    let decl_number_parts = idx.all_norm_with_prefixes(&["Номер декларации", "Номер декларації"]);
    let declaration_date = idx.first_norm_of(&[("Дата оформления", 0), ("Дата оформлення", 0)]);
    let product_code =
        idx.first_norm_of(&[("КОД ТОВАРУ", 0), ("Код товара", 0), ("Код товару", 0)]);
    let description = idx.first_norm_of(&[
        ("ОПИС ТОВАРУ", 0),
        ("Описание товара", 0),
        ("Опис товару", 0),
        ("Найменування товару", 0),
        ("Назва товару", 0),
    ]);
    let value_usd =
        idx.first_norm_of(&[("Фактурная стоимость, $", 0), ("Фактурна вартість, $", 0)]);
    if decl_number_parts.len() < 3
        || declaration_date.is_none()
        || product_code.is_none()
        || description.is_none()
        || value_usd.is_none()
    {
        return None;
    }

    let decl_type_parts = idx.all_norm_with_prefixes(&["Тип декларации", "Тип декларації"]);
    let recipient_code = idx.first_norm_of(&[("Получатель", 0)]);
    let sender_code = if recipient_code.is_none() {
        idx.first_norm_of(&[("Отправитель", 0)])
    } else {
        None
    };
    let recipient_name = idx.first_norm_of(&[
        ("ОТРИМУВАЧ", 0),
        ("Отримувач", 0),
        ("Получатель", 1),
        ("Назва фірми отримувача", 0),
    ]);
    let sender_name = if recipient_code.is_some() {
        idx.first_norm_of(&[
            ("ВІДПРАВНИК", 0),
            ("Відправник", 0),
            ("Отправитель", 0),
            ("Назва фірми відправника", 0),
            ("Назва фірми відправиника", 0),
        ])
    } else {
        idx.first_norm_of(&[
            ("ВІДПРАВНИК", 0),
            ("Відправник", 0),
            ("Отправитель", 1),
            ("Назва фірми відправника", 0),
            ("Назва фірми відправиника", 0),
        ])
    };
    if recipient_code.is_none() && sender_code.is_none() {
        return None;
    }

    let opt = |pos: Option<usize>| pos.map(ColSrc::Cell).unwrap_or(ColSrc::Missing);
    let join = |parts: &Vec<usize>| {
        if parts.is_empty() {
            ColSrc::Missing
        } else {
            ColSrc::Join(parts.clone(), "/")
        }
    };

    Some(
        COLUMNS
            .iter()
            .map(|c| match c.name {
                "clearance_time" => {
                    opt(idx.first_norm_of(&[("Час оформлення", 0), ("Время оформления", 0)]))
                }
                "customs_office" => opt(idx.first_norm_of(&[
                    ("Таможня оформления", 0),
                    ("Митниця оформлення", 0),
                    ("Назва ПМО", 0),
                ])),
                "declaration_type" => join(&decl_type_parts),
                "declaration_number" => join(&decl_number_parts),
                "declaration_date" => opt(declaration_date),
                "sender" => opt(sender_name.or_else(|| idx.first_norm_of(&[("Відправник", 0)]))),
                "edrpou" => opt(recipient_code.or(sender_code)),
                "recipient" => opt(recipient_name),
                "item_number" => {
                    opt(idx.first_norm_of(&[("Номер товара", 0), ("Номер товару", 0), ("№", 0)]))
                }
                "product_code" => opt(product_code),
                "description" => opt(description),
                "trade_country" => opt(idx.first_norm_of(&[
                    ("Торгующая страна", 0),
                    ("Торгуюча країна", 0),
                    ("Кр.торг.", 0),
                ])),
                "dispatch_country" => opt(idx.first_norm_of(&[
                    ("Страна отправления", 0),
                    ("Країна відправлення", 0),
                    ("Кр.відпр.", 0),
                ])),
                "origin_country" => opt(idx.first_norm_of(&[
                    ("КРАЇНА ПОХОДЖЕННЯ", 0),
                    ("Страна происхождения", 0),
                    ("Країна походження", 0),
                    ("КРАЇНА ПРИХНАЧЕННЯ", 0),
                    ("КРАЇНА ПРИЗНАЧЕННЯ", 0),
                    ("Страна назначения", 0),
                ])),
                "delivery_terms" => opt(idx
                    .get_norm("Условия поставки", 0)
                    .or_else(|| idx.get_norm("Умови поставки", 0))),
                "delivery_place" => opt(idx
                    .get_norm("Условия поставки", 1)
                    .or_else(|| idx.get_norm("Умови поставки", 1))),
                "quantity" => opt(idx
                    .get_norm("Доп.ед", 0)
                    .or_else(|| idx.get_norm("Дод.од", 0))),
                "unit" => opt(idx
                    .get_norm("Доп.ед", 1)
                    .or_else(|| idx.get_norm("Дод.од", 1))),
                "gross_kg" => opt(idx.first_norm_of(&[
                    ("Вес брутто, кг", 0),
                    ("Вага брутто, кг", 0),
                    ("Брутто, кг.", 0),
                ])),
                "net_kg" => opt(idx.first_norm_of(&[
                    ("Вес нетто, кг", 0),
                    ("Вага нетто, кг", 0),
                    ("Нетто, кг.", 0),
                ])),
                "currency_control_value" => opt(value_usd),
                "movement_feature" => opt(idx.first_norm_of(&[
                    ("ОЗНАКА ТОВАРУ В КОНТЕЙНЕРІ", 0),
                    ("Признак товара в контейнере", 0),
                    ("Ознака товару в контейнері", 0),
                ])),
                "field_43" => {
                    opt(idx.first_norm_of(&[("Метод", 0), ("Метод визначення митної вартості", 0)]))
                }
                "rfv_usd_kg" => opt(idx.first_norm_of(&[
                    ("ЦІНА, $/кг", 0),
                    ("Цена, $/кг", 0),
                    ("Вартість, $/кг", 0),
                ])),
                "field_3001" => opt(idx.first_norm_of(&[
                    ("Пошлина, грн.", 0),
                    ("Пошлина, грн", 0),
                    ("Мито, грн.", 0),
                ])),
                "field_3002" => opt(idx.first_norm_of(&[("Акциз, грн.", 0), ("Акциз, грн", 0)])),
                "field_9610" => opt(idx.first_norm_of(&[("ПДВ, грн.", 0), ("ПДВ, грн", 0)])),
                _ => ColSrc::Missing,
            })
            .collect(),
    )
}

/// Generic detector dictionary: schema column name -> normalized header aliases.
/// Aliases are lowercase and stripped of spaces and punctuation.
const GENERIC_ALIASES: &[(&str, &[&str])] = &[
    ("clearance_time", &["часоформлення", "времяоформления"]),
    (
        "customs_office",
        &[
            "назвапмо",
            "митницяоформлення",
            "митниця",
            "таможня",
            "таможняоформления",
        ],
    ),
    (
        "declaration_type",
        &["тип", "типдекларації", "типдекларации"],
    ),
    (
        "declaration_number",
        &[
            "номермд",
            "номердекларації",
            "номердекларации",
            "номергтд",
            "номермитноїдекларації",
            "номертаможеннойдекларации",
        ],
    ),
    (
        "declaration_date",
        &["дата", "датаоформлення", "датаоформления", "датамд", "date"],
    ),
    (
        "sender",
        &[
            "відправник",
            "отправитель",
            "відпправник",
            "назвафірмивідправника",
            "назвафірмивідправиника",
            "sender",
            "експортер",
            "экспортер",
            "exporter",
        ],
    ),
    (
        "edrpou",
        &[
            "едрпоу",
            "єдрпоу",
            "кодєдрпоу",
            "кодедрпоу",
            "кодфірмиотримувача",
            "кодотримувача",
            "окпо",
            "кодпоєдрпоу",
        ],
    ),
    (
        "recipient",
        &[
            "одержувач",
            "отримувач",
            "получатель",
            "назвафірмиотримувача",
            "імпортер",
            "импортер",
            "importer",
        ],
    ),
    ("item_number", &["номертовару", "номертовара", "нп", "пп"]),
    (
        "product_code",
        &[
            "кодтовару",
            "кодтовара",
            "кодтнвэд",
            "кодтнвед",
            "кодтнзед",
            "кодуктзед",
            "hscode",
            "код",
        ],
    ),
    (
        "description",
        &[
            "опистовару",
            "описаниетовара",
            "найменуваннятовару",
            "назватовару",
            "наименованиетовара",
            "описание",
            "опис",
            "опистовара",
            "description",
            "товар",
            "найменування",
            "наименование",
        ],
    ),
    (
        "trade_country",
        &[
            "крторг",
            "торгуючакраїна",
            "странаторговли",
            "країнаторгівлі",
            "торгующаястрана",
        ],
    ),
    (
        "dispatch_country",
        &["крвідпр", "країнавідправлення", "странаотправления"],
    ),
    (
        "origin_country",
        &[
            "крпох",
            "країнапоходження",
            "странапроисхождения",
            "країнапоходженнятовару",
        ],
    ),
    (
        "delivery_terms",
        &["умовипост", "умовипоставки", "условияпоставки"],
    ),
    (
        "delivery_place",
        &["місцепост", "місцепоставки", "местопоставки"],
    ),
    (
        "quantity",
        &["кть", "кількість", "количество", "додод", "qty"],
    ),
    ("unit", &["одинвим", "одиницявиміру", "единицаизмерения"]),
    (
        "gross_kg",
        &[
            "бруттокг",
            "вагабруттокг",
            "весбруттокг",
            "вагабрутто",
            "весбрутто",
            "брутто",
        ],
    ),
    (
        "net_kg",
        &[
            "неттокг",
            "ваганеттокг",
            "веснеттокг",
            "ваганетто",
            "веснетто",
            "нетто",
        ],
    ),
    (
        "trademark",
        &[
            "торгмарк",
            "торговамарка",
            "торговаямарка",
            "тм",
            "trademark",
        ],
    ),
    (
        "contract",
        &["контракт", "номерконтракта", "номерконтракту"],
    ),
];

fn norm_header(h: &str) -> String {
    h.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Generic detector for similar exports.
/// Requires at least a description plus one of product code, declaration number,
/// sender, or recipient.
fn map_generic(headers: &[String]) -> Option<Vec<ColSrc>> {
    let mut alias_to_target: HashMap<&str, &str> = HashMap::new();
    for (target, aliases) in GENERIC_ALIASES {
        for alias in *aliases {
            alias_to_target.entry(alias).or_insert(target);
        }
    }
    let mut found: HashMap<&str, usize> = HashMap::new();
    for (i, header) in headers.iter().enumerate() {
        let norm = norm_header(header);
        if norm.is_empty() {
            continue;
        }
        if let Some(target) = alias_to_target.get(norm.as_str()) {
            found.entry(target).or_insert(i);
        }
    }
    if !found.contains_key("description") {
        return None;
    }
    if !["product_code", "declaration_number", "sender", "recipient"]
        .iter()
        .any(|k| found.contains_key(k))
    {
        return None;
    }
    Some(
        COLUMNS
            .iter()
            .map(|c| match found.get(c.name) {
                Some(i) => ColSrc::Cell(*i),
                None => ColSrc::Missing,
            })
            .collect(),
    )
}

fn detect_mapping(headers: &[String]) -> Option<Vec<ColSrc>> {
    let idx = HeaderIndex::new(headers);
    map_format_a(&idx)
        .or_else(|| map_format_b(&idx))
        .or_else(|| map_wide_customs(headers, &idx))
        .or_else(|| map_generic(headers))
}

/// Source columns the mapping does not consume, paired with their header names,
/// in file order. These are preserved per row in the `extra` payload so the app
/// keeps every column of differently shaped files, not only the known schema.
fn unmapped_columns(headers: &[String], mapping: &[ColSrc]) -> Vec<(usize, String)> {
    let mut consumed = std::collections::HashSet::new();
    for src in mapping {
        match src {
            ColSrc::Cell(i) => {
                consumed.insert(*i);
            }
            ColSrc::Join(parts, _) => {
                for p in parts {
                    consumed.insert(*p);
                }
            }
            ColSrc::Missing => {}
        }
    }
    headers
        .iter()
        .enumerate()
        .filter(|(i, header)| !header.is_empty() && !consumed.contains(i))
        .map(|(i, header)| (i, header.clone()))
        .collect()
}

// ---------- import ----------

/// File content hash, streamed without loading the whole file into memory.
pub fn file_content_hash(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Xxh3::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:032x}", hasher.digest128()))
}

/// Imports one file. progress(phase, done, total); total == 0 means unknown.
pub fn import_file(
    db: &mut Db,
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(ImportPhase, u64, u64),
) -> FileSummary {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mut summary = FileSummary {
        file_name: file_name.clone(),
        ..Default::default()
    };
    let started = Instant::now();
    progress(ImportPhase::Reading, 0, 0);

    // Whole-file deduplication: identical content is not parsed again.
    let file_hash = match file_content_hash(path) {
        Ok(hash) => {
            if let Some(previous) = db.find_import_by_hash(&hash) {
                summary.skipped_duplicate_of = Some(previous);
                summary.seconds = started.elapsed().as_secs_f64();
                return summary;
            }
            hash
        }
        Err(e) => {
            summary.error = Some(e);
            return summary;
        }
    };

    let mut committed = false;
    match db.begin_import_file() {
        Ok(()) => match import_file_inner(db, path, &file_name, cancel, progress, &mut summary) {
            Ok(()) if !summary.cancelled => match db.commit_import_file() {
                Ok(()) => {
                    committed = true;
                }
                Err(e) => {
                    summary.error = Some(e.to_string());
                    db.rollback_import_file();
                }
            },
            Ok(()) => {
                db.rollback_import_file();
                summary.imported = 0;
                summary.duplicates = 0;
            }
            Err(e) => {
                summary.error = Some(e);
                db.rollback_import_file();
                summary.imported = 0;
                summary.duplicates = 0;
            }
        },
        Err(e) => summary.error = Some(e.to_string()),
    }
    if committed {
        progress(ImportPhase::Indexing, 0, 0);
        match db.index_fts(cancel, |done, total| {
            progress(ImportPhase::Indexing, done, total)
        }) {
            Ok((_, fts_cancelled)) => {
                summary.cancelled |= fts_cancelled;
            }
            Err(e) => summary.error = Some(e.to_string()),
        }
    }
    summary.seconds = started.elapsed().as_secs_f64();
    if committed {
        // Store the hash only for fully imported files, so interrupted imports
        // can be retried.
        db.add_import_log(
            &file_name,
            summary.total_rows,
            summary.imported,
            summary.duplicates,
            summary.seconds,
            Some(file_hash.as_str()),
        );
    }
    summary
}

fn import_file_inner(
    db: &mut Db,
    path: &Path,
    file_name: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(ImportPhase, u64, u64),
    summary: &mut FileSummary,
) -> Result<(), String> {
    let mut workbook = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let sheet = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| "В файле нет листов / У файлі немає аркушів".to_string())?;

    let mut sink = RowSink {
        db,
        file_name,
        cancel,
        progress,
        summary,
        total_rows_hint: 0,
        mapping: None,
        extra_cols: Vec::new(),
        scanned: Vec::new(),
        first_row_headers: Vec::new(),
        batch: Vec::with_capacity(BATCH_SIZE),
        rows_seen: 0,
    };

    match &mut workbook {
        Sheets::Xlsx(xlsx) => {
            let mut reader = xlsx
                .worksheet_cells_reader(&sheet)
                .map_err(|e| e.to_string())?;
            let dims = reader.dimensions();
            sink.total_rows_hint = (dims.end.0.saturating_sub(dims.start.0)) as u64;
            let mut assembler = RowAssembler::default();
            while let Some(cell) = reader.next_cell().map_err(|e| e.to_string())? {
                let (row, col) = cell.get_position();
                let data: Data = cell.get_value().clone().into();
                if let Some(done_row) = assembler.push(row, col, data)
                    && !sink.row(done_row)?
                {
                    return sink.finish();
                }
            }
            if let Some(done_row) = assembler.take() {
                sink.row(done_row)?;
            }
            sink.finish()
        }
        Sheets::Xlsb(xlsb) => {
            let mut reader = xlsb
                .worksheet_cells_reader(&sheet)
                .map_err(|e| e.to_string())?;
            let dims = reader.dimensions();
            sink.total_rows_hint = (dims.end.0.saturating_sub(dims.start.0)) as u64;
            let mut assembler = RowAssembler::default();
            while let Some(cell) = reader.next_cell().map_err(|e| e.to_string())? {
                let (row, col) = cell.get_position();
                let data: Data = cell.get_value().clone().into();
                if let Some(done_row) = assembler.push(row, col, data)
                    && !sink.row(done_row)?
                {
                    return sink.finish();
                }
            }
            if let Some(done_row) = assembler.take() {
                sink.row(done_row)?;
            }
            sink.finish()
        }
        // Old .xls and .ods files are uncommon, so read them as full ranges.
        other => {
            let range = other.worksheet_range(&sheet).map_err(|e| e.to_string())?;
            sink.total_rows_hint = (range.height().saturating_sub(1)) as u64;
            for row in range.rows() {
                if !sink.row(row.to_vec())? {
                    break;
                }
            }
            sink.finish()
        }
    }
}

/// Assembles a cell stream into rows. Gaps between cells are filled with
/// `Data::Empty`; fully empty sheet rows are not emitted by the reader.
#[derive(Default)]
struct RowAssembler {
    current_row: Option<u32>,
    cells: Vec<Data>,
}

impl RowAssembler {
    fn push(&mut self, row: u32, col: u32, value: Data) -> Option<Vec<Data>> {
        let mut finished = None;
        match self.current_row {
            Some(current) if current == row => {}
            Some(_) => finished = Some(std::mem::take(&mut self.cells)),
            None => {}
        }
        self.current_row = Some(row);
        let col = col as usize;
        if self.cells.len() < col {
            self.cells.resize(col, Data::Empty);
        }
        if self.cells.len() == col {
            self.cells.push(value);
        } else {
            self.cells[col] = value;
        }
        finished
    }

    fn take(&mut self) -> Option<Vec<Data>> {
        self.current_row.take()?;
        Some(std::mem::take(&mut self.cells))
    }
}

/// Row sink: finds the header row, normalizes data, and writes batches.
struct RowSink<'a> {
    db: &'a mut Db,
    file_name: &'a str,
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(ImportPhase, u64, u64),
    summary: &'a mut FileSummary,
    total_rows_hint: u64,
    mapping: Option<Vec<ColSrc>>,
    /// Source columns not consumed by the mapping: (column index, header name).
    /// Captured verbatim per row so no source data is lost on import.
    extra_cols: Vec<(usize, String)>,
    scanned: Vec<Vec<Data>>,
    first_row_headers: Vec<String>,
    batch: Vec<ImportRecord>,
    rows_seen: u64,
}

impl RowSink<'_> {
    /// Ok(false) means import was cancelled and no more rows are needed.
    fn row(&mut self, row: Vec<Data>) -> Result<bool, String> {
        self.rows_seen += 1;
        if self.mapping.is_none() {
            let headers: Vec<String> = row.iter().map(header_text).collect();
            if self.first_row_headers.is_empty() {
                self.first_row_headers = headers.clone();
            }
            if let Some(mapping) = detect_mapping(&headers) {
                self.extra_cols = unmapped_columns(&headers, &mapping);
                self.mapping = Some(mapping);
                self.scanned.clear(); // rows above the header are title noise
                return Ok(true);
            }
            self.scanned.push(row);
            if self.scanned.len() >= HEADER_SCAN_ROWS {
                return Err(self.missing_error());
            }
            return Ok(true);
        }
        self.data_row(row)
    }

    fn data_row(&mut self, row: Vec<Data>) -> Result<bool, String> {
        let mapping = self.mapping.as_ref().expect("mapping установлен");
        let mut values: Vec<String> = Vec::with_capacity(COLUMNS.len());
        for (i, src) in mapping.iter().enumerate() {
            let value = match src {
                ColSrc::Missing => String::new(),
                ColSrc::Cell(pos) => row
                    .get(*pos)
                    .map(|d| normalize_cell(d, i == DATE_COL))
                    .unwrap_or_default(),
                ColSrc::Join(parts, sep) => parts
                    .iter()
                    .filter_map(|pos| row.get(*pos))
                    .map(normalize_value)
                    .filter(|v| !v.is_empty())
                    .collect::<Vec<_>>()
                    .join(sep),
            };
            values.push(value);
        }
        if values.iter().all(|v| v.is_empty()) {
            return Ok(true);
        }
        values[DATE_COL] = normalize_date(&values[DATE_COL]);
        let extra = self.collect_extra(&row);
        self.summary.total_rows += 1;
        self.batch.push(ImportRecord {
            // Hash the full source row so rows that differ only in unmapped
            // columns are not treated as duplicates.
            hash: row_hash_cells(&row),
            year: extract_year(&values[DATE_COL]),
            values,
            extra,
        });
        if self.batch.len() >= BATCH_SIZE {
            self.flush_batch()?;
            if self.cancel.load(Ordering::Relaxed) {
                self.summary.cancelled = true;
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Builds the `extra` JSON payload (unmapped columns) for one source row.
    fn collect_extra(&self, row: &[Data]) -> Option<String> {
        if self.extra_cols.is_empty() {
            return None;
        }
        let pairs: Vec<(&str, String)> = self
            .extra_cols
            .iter()
            .filter_map(|(idx, name)| {
                let value = row.get(*idx).map(normalize_value).unwrap_or_default();
                (!value.is_empty()).then_some((name.as_str(), value))
            })
            .collect();
        if pairs.is_empty() {
            None
        } else {
            serde_json::to_string(&pairs).ok()
        }
    }

    fn flush_batch(&mut self) -> Result<(), String> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let (inserted, duplicates) = self
            .db
            .insert_batch(self.file_name, &self.batch)
            .map_err(|e| e.to_string())?;
        self.summary.imported += inserted;
        self.summary.duplicates += duplicates;
        self.batch.clear();
        (self.progress)(ImportPhase::Inserting, self.rows_seen, self.total_rows_hint);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        if self.mapping.is_none() {
            return Err(self.missing_error());
        }
        self.flush_batch()?;
        Ok(())
    }

    fn missing_error(&self) -> String {
        let idx = HeaderIndex::new(&self.first_row_headers);
        let missing: Vec<&str> = REQUIRED_HEADERS
            .iter()
            .filter(|h| !idx.has(h))
            .copied()
            .collect();
        format!("__MISSING__{}", missing.join(", "))
    }
}

// ---------- value normalization ----------

fn header_text(data: &Data) -> String {
    match data {
        Data::String(s) => collapse_ws(s),
        Data::Empty => String::new(),
        other => collapse_ws(&other.to_string()),
    }
}

/// Converts a cell value to a clean string: integer-like numbers without ".0",
/// ISO dates, and collapsed whitespace.
pub fn normalize_value(data: &Data) -> String {
    normalize_cell(data, false)
}

/// `expect_date` marks a date column; Excel serial numbers become dates.
pub fn normalize_cell(data: &Data, expect_date: bool) -> String {
    match data {
        Data::Empty | Data::Error(_) => String::new(),
        Data::String(s) => collapse_ws(s),
        Data::Float(f) => {
            if expect_date && let Some(date) = excel_serial_to_iso(*f) {
                return date;
            }
            float_to_string(*f)
        }
        Data::Int(i) => {
            if expect_date && let Some(date) = excel_serial_to_iso(*i as f64) {
                return date;
            }
            i.to_string()
        }
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(ndt) => {
                if dt.as_f64() < 1.0 {
                    // Time only: a fractional day without a date.
                    ndt.format("%H:%M:%S").to_string()
                } else if ndt.hour() == 0 && ndt.minute() == 0 && ndt.second() == 0 {
                    ndt.format("%Y-%m-%d").to_string()
                } else {
                    ndt.format("%Y-%m-%d %H:%M:%S").to_string()
                }
            }
            None => float_to_string(dt.as_f64()),
        },
        Data::DateTimeIso(s) => collapse_ws(s),
        Data::DurationIso(s) => collapse_ws(s),
    }
}

/// Excel serial date (days since 1899-12-30) -> ISO date.
/// The range is limited to plausible years (1968-2064).
pub fn excel_serial_to_iso(serial: f64) -> Option<String> {
    if !serial.is_finite() || !(25000.0..=60000.0).contains(&serial) {
        return None;
    }
    let days = serial.trunc() as i64;
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?;
    let date = base.checked_add_signed(chrono::Duration::days(days))?;
    let secs = ((serial - days as f64) * 86400.0).round() as u32;
    if secs > 0 && secs < 86400 {
        let time = chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, 0)?;
        Some(format!(
            "{} {}",
            date.format("%Y-%m-%d"),
            time.format("%H:%M:%S")
        ))
    } else {
        Some(date.format("%Y-%m-%d").to_string())
    }
}

fn float_to_string(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 {
        (f as i64).to_string()
    } else {
        f.to_string()
    }
}

pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true; // consumes leading whitespace
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// "31.12.2024" / "31/12/2024" / "31-12-2024" -> "2024-12-31".
/// Existing ISO dates and unrecognized text are returned unchanged.
pub fn normalize_date(value: &str) -> String {
    let parts: Vec<&str> = value.split(['.', '/', '-']).collect();
    if parts.len() == 3
        && parts[0].len() <= 2
        && parts[1].len() <= 2
        && parts[2].len() == 4
        && let (Ok(d), Ok(m), Ok(y)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        )
        && (1..=31).contains(&d)
        && (1..=12).contains(&m)
    {
        return format!("{y:04}-{m:02}-{d:02}");
    }
    value.to_string()
}

/// File row hash. Trailing empty cells are trimmed so the hash does not depend
/// on the reading mode: streaming cells or full range.
pub fn row_hash_cells(row: &[Data]) -> [u8; 16] {
    let mut end = row.len();
    while end > 0 && matches!(row[end - 1], Data::Empty) {
        end -= 1;
    }
    let mut hasher = Xxh3::new();
    for (i, cell) in row[..end].iter().enumerate() {
        if i > 0 {
            hasher.update(&[0x1f]);
        }
        hasher.update(normalize_value(cell).as_bytes());
    }
    hasher.digest128().to_le_bytes()
}
