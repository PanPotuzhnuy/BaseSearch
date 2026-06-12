//! Column definitions for customs Excel files and their database mapping.

pub struct ColumnDef {
    /// Source Excel column header as it appears in customs data.
    pub header: &'static str,
    /// SQLite column name.
    pub name: &'static str,
}

pub const COLUMNS: [ColumnDef; 41] = [
    ColumnDef {
        header: "Час оформлення",
        name: "clearance_time",
    },
    ColumnDef {
        header: "Назва ПМО",
        name: "customs_office",
    },
    ColumnDef {
        header: "Тип",
        name: "declaration_type",
    },
    ColumnDef {
        header: "Номер МД",
        name: "declaration_number",
    },
    ColumnDef {
        header: "Дата",
        name: "declaration_date",
    },
    ColumnDef {
        header: "Відправник",
        name: "sender",
    },
    ColumnDef {
        header: "ЕДРПОУ",
        name: "edrpou",
    },
    ColumnDef {
        header: "Одержувач",
        name: "recipient",
    },
    ColumnDef {
        header: "№",
        name: "item_number",
    },
    ColumnDef {
        header: "Код товару",
        name: "product_code",
    },
    ColumnDef {
        header: "Опис товару",
        name: "description",
    },
    ColumnDef {
        header: "Кр.торг.",
        name: "trade_country",
    },
    ColumnDef {
        header: "Кр.відпр.",
        name: "dispatch_country",
    },
    ColumnDef {
        header: "Кр.пох.",
        name: "origin_country",
    },
    ColumnDef {
        header: "Умови пост.",
        name: "delivery_terms",
    },
    ColumnDef {
        header: "Місце пост",
        name: "delivery_place",
    },
    ColumnDef {
        header: "К-ть",
        name: "quantity",
    },
    ColumnDef {
        header: "Один.вим.",
        name: "unit",
    },
    ColumnDef {
        header: "Брутто, кг.",
        name: "gross_kg",
    },
    ColumnDef {
        header: "Нетто, кг.",
        name: "net_kg",
    },
    ColumnDef {
        header: "Вага по МД",
        name: "declaration_weight",
    },
    ColumnDef {
        header: "ФВ вал.контр",
        name: "currency_control_value",
    },
    ColumnDef {
        header: "Особ.перем.",
        name: "movement_feature",
    },
    ColumnDef {
        header: "43",
        name: "field_43",
    },
    ColumnDef {
        header: "43_01",
        name: "field_43_01",
    },
    ColumnDef {
        header: "РФВ Дол/кг.",
        name: "rfv_usd_kg",
    },
    ColumnDef {
        header: "Вага.один.",
        name: "unit_weight",
    },
    ColumnDef {
        header: "Вага різн.",
        name: "weight_difference",
    },
    ColumnDef {
        header: "Контракт",
        name: "contract",
    },
    ColumnDef {
        header: "3001",
        name: "field_3001",
    },
    ColumnDef {
        header: "3002",
        name: "field_3002",
    },
    ColumnDef {
        header: "9610",
        name: "field_9610",
    },
    ColumnDef {
        header: "Торг.марк.",
        name: "trademark",
    },
    ColumnDef {
        header: "РМВ Нетто Дол/кг.",
        name: "rmv_net_usd_kg",
    },
    ColumnDef {
        header: "РМВ Дол/дод.од.",
        name: "rmv_usd_extra_unit",
    },
    ColumnDef {
        header: "РМВ Брутто Дол/кг",
        name: "rmv_gross_usd_kg",
    },
    ColumnDef {
        header: "Призн.Зед",
        name: "zed_purpose",
    },
    ColumnDef {
        header: "Мін.База Дол/кг.",
        name: "min_base_usd_kg",
    },
    ColumnDef {
        header: "Різн.мін.база",
        name: "min_base_difference",
    },
    ColumnDef {
        header: "пільгова",
        name: "preferential",
    },
    ColumnDef {
        header: "повна",
        name: "full_rate",
    },
];

/// Index of the date column in COLUMNS.
pub const DATE_COL: usize = 4;

/// Files are not imported without these columns.
pub const REQUIRED_HEADERS: [&str; 7] = [
    "Номер МД",
    "Дата",
    "Відправник",
    "ЕДРПОУ",
    "Одержувач",
    "Код товару",
    "Опис товару",
];

/// Columns included in the full-text search index.
/// Countries are included so country filters can be accelerated through FTS.
pub const SEARCH_COLUMNS: [&str; 9] = [
    "description",
    "sender",
    "recipient",
    "trademark",
    "declaration_number",
    "product_code",
    "trade_country",
    "dispatch_country",
    "origin_country",
];

/// Result table columns by database name. source_file is added separately.
pub const RESULT_COLUMNS: [&str; 15] = [
    "declaration_date",
    "declaration_number",
    "sender",
    "recipient",
    "description",
    "product_code",
    "edrpou",
    "trade_country",
    "dispatch_country",
    "origin_country",
    "quantity",
    "gross_kg",
    "net_kg",
    "trademark",
    "source_file",
];

pub fn col_index(name: &str) -> Option<usize> {
    COLUMNS.iter().position(|c| c.name == name)
}

/// Source-file header for a database column name; source_file maps to file.
pub fn header_for(name: &str) -> &'static str {
    if name == "source_file" {
        return "Файл";
    }
    COLUMNS
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.header)
        .unwrap_or("")
}
