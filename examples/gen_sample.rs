//! Generates a small synthetic customs dataset as an .xlsx file, so the desktop
//! and web interfaces can be tried without real data.
//!
//! Usage: cargo run --example gen_sample -- <out.xlsx>
//!
//! The data is deterministic (no randomness) so repeated runs produce the same
//! file. Headers use the standard 41-column customs layout the importer detects.

use rust_xlsxwriter::Workbook;

const HEADERS: [&str; 41] = [
    "Час оформлення",
    "Назва ПМО",
    "Тип",
    "Номер МД",
    "Дата",
    "Відправник",
    "ЕДРПОУ",
    "Одержувач",
    "№",
    "Код товару",
    "Опис товару",
    "Кр.торг.",
    "Кр.відпр.",
    "Кр.пох.",
    "Умови пост.",
    "Місце пост",
    "К-ть",
    "Один.вим.",
    "Брутто, кг.",
    "Нетто, кг.",
    "Вага по МД",
    "ФВ вал.контр",
    "Особ.перем.",
    "43",
    "43_01",
    "РФВ Дол/кг.",
    "Вага.один.",
    "Вага різн.",
    "Контракт",
    "3001",
    "3002",
    "9610",
    "Торг.марк.",
    "РМВ Нетто Дол/кг.",
    "РМВ Дол/дод.од.",
    "РМВ Брутто Дол/кг",
    "Призн.Зед",
    "Мін.База Дол/кг.",
    "Різн.мін.база",
    "пільгова",
    "повна",
];

/// Recipient company paired with a stable EDRPOU code.
const RECIPIENTS: [(&str, &str); 6] = [
    ("DEMO IMPORT LLC", "30215600"),
    ("NORTHWIND TRADE", "41882300"),
    ("GLOBEX UA", "38771200"),
    ("ACME DEVICES", "44190077"),
    ("SILK ROAD LOGISTICS", "39400512"),
    ("CARPATHIA RETAIL", "42655810"),
];

const SENDERS: [&str; 6] = [
    "SHENZHEN BRIGHT CO., LTD",
    "APPLE DISTRIBUTION INTERNATIONAL",
    "SAMSUNG ELECTRONICS GMBH",
    "VIETNAM PRECISION IND.",
    "POLARIS EUROPE SP. Z O.O.",
    "MEDITERRA TRADING SRL",
];

/// Product code paired with a description.
const GOODS: [(&str, &str); 7] = [
    ("8517120000", "Smartphones, cellular network telephones"),
    ("8504401100", "Power adapters and chargers for telephones"),
    ("8471300000", "Portable computers, tablets"),
    ("8518300000", "Headphones and earphones"),
    ("9403200000", "Metal furniture, office shelving"),
    ("6109100000", "Cotton t-shirts, knitted"),
    ("2204210000", "Wine of fresh grapes, bottled"),
];

const BRANDS: [&str; 6] = ["Apple", "Samsung", "Anker", "Xiaomi", "IKEA", "Generic"];

/// Country code paired with itself for origin/dispatch/trade variety.
const COUNTRIES: [&str; 6] = ["CN", "VN", "DE", "PL", "IT", "TR"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sample.xlsx".to_string());

    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    for (col, header) in HEADERS.iter().enumerate() {
        sheet.write_string(0, col as u16, *header)?;
    }

    let rows = 220u32;
    for i in 0..rows {
        let r = i + 1;
        let idx = i as usize;
        let (recipient, edrpou) = RECIPIENTS[idx % RECIPIENTS.len()];
        let sender = SENDERS[(idx * 7) % SENDERS.len()];
        let (code, desc) = GOODS[(idx * 3) % GOODS.len()];
        let brand = BRANDS[(idx * 5) % BRANDS.len()];
        let origin = COUNTRIES[(idx * 11) % COUNTRIES.len()];
        let dispatch = COUNTRIES[(idx * 13 + 2) % COUNTRIES.len()];

        let year = if idx.is_multiple_of(9) { 2025 } else { 2024 };
        let month = (idx % 12) + 1;
        let day = (idx % 27) + 1;
        let date = format!("{year:04}-{month:02}-{day:02}");

        let quantity = 40.0 + (idx % 25) as f64 * 12.0;
        let net_kg = 180.0 + (idx % 60) as f64 * 14.5;
        let gross_kg = (net_kg * 1.08).round();
        // Base price per kg varies by product, with a per-row wobble; a few rows
        // are deliberately cheap so the price spread looks realistic.
        let base_price = 6.0 + ((idx * 3) % 7) as f64 * 5.5;
        let wobble = ((idx % 5) as f64 - 2.0) * 0.8;
        let price = (base_price + wobble).max(1.5);
        let value = (price * net_kg).round();

        sheet.write_string(r, 1, "Kyiv City Customs")?;
        sheet.write_string(r, 2, "IM 40")?;
        sheet.write_string(r, 3, format!("UA100290/{year}/{:06}", 100000 + i))?;
        sheet.write_string(r, 4, &date)?;
        sheet.write_string(r, 5, sender)?;
        sheet.write_string(r, 6, edrpou)?;
        sheet.write_string(r, 7, recipient)?;
        sheet.write_string(r, 8, "1")?;
        sheet.write_string(r, 9, code)?;
        sheet.write_string(r, 10, desc)?;
        sheet.write_string(r, 11, origin)?;
        sheet.write_string(r, 12, dispatch)?;
        sheet.write_string(r, 13, origin)?;
        sheet.write_string(r, 14, "CIF")?;
        sheet.write_number(r, 16, quantity)?;
        sheet.write_string(r, 17, "pcs")?;
        sheet.write_number(r, 18, gross_kg)?;
        sheet.write_number(r, 19, net_kg)?;
        sheet.write_number(r, 21, value)?;
        sheet.write_number(r, 25, (price * 100.0).round() / 100.0)?;
        sheet.write_string(r, 28, format!("CT-{:04}", 2000 + (idx % 40)))?;
        sheet.write_string(r, 32, brand)?;
    }

    workbook.save(&out)?;
    println!("Wrote {rows} rows to {out}");
    Ok(())
}
