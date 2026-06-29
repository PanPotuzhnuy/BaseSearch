use serde::{Deserialize, Serialize};

/// Domain-neutral role inferred from a source column.
///
/// These roles describe how a column can be searched, filtered, aggregated, or
/// displayed. They are not tied to one document layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColumnRole {
    Text,
    Number,
    Date,
    Year,
    Country,
    Code,
    Identifier,
    Money,
    Weight,
}

/// Optional semantic meaning attached by a profile.
///
/// A public Base Search import must work without any semantic field. Profiles
/// can add these hints for better analytics, but the raw columns remain the
/// source of truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticField {
    Date,
    DeclarationNumber,
    CompanyCode,
    Sender,
    Recipient,
    ProductCode,
    Description,
    Trademark,
    Country,
    OriginCountry,
    DispatchCountry,
    TradeCountry,
    Quantity,
    NetWeight,
    GrossWeight,
    Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceColumn {
    pub id: String,
    pub header: String,
    pub source_index: usize,
    pub role: ColumnRole,
    pub semantic: Option<SemanticField>,
    #[serde(default)]
    pub storage: ColumnStorage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColumnStorage {
    #[default]
    SourceJson,
    SchemaColumn(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableShape {
    pub columns: Vec<SourceColumn>,
}

impl TableShape {
    pub fn from_headers(headers: impl IntoIterator<Item = String>) -> Self {
        let mut seen = std::collections::HashMap::<String, usize>::new();
        let columns = headers
            .into_iter()
            .enumerate()
            .map(|(source_index, header)| {
                let header = normalize_header_for_display(&header, source_index);
                let base_id = stable_column_id(&header, source_index);
                let count = seen.entry(base_id.clone()).or_insert(0);
                let id = if *count == 0 {
                    base_id
                } else {
                    format!("{base_id}_{}", *count + 1)
                };
                *count += 1;
                let role = infer_role(&header);
                SourceColumn {
                    id,
                    header,
                    source_index,
                    role,
                    semantic: None,
                    storage: ColumnStorage::SourceJson,
                }
            })
            .collect();
        Self { columns }
    }

    pub fn with_semantics(
        mut self,
        semantics: impl IntoIterator<Item = (String, SemanticField)>,
    ) -> Self {
        let semantics = semantics
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        for column in &mut self.columns {
            if let Some(semantic) = semantics.get(&column.id).copied() {
                column.semantic = Some(semantic);
            }
        }
        self
    }
}

fn normalize_header_for_display(header: &str, source_index: usize) -> String {
    let trimmed = header.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        format!("Column {}", source_index + 1)
    } else {
        trimmed
    }
}

fn stable_column_id(header: &str, source_index: usize) -> String {
    let mut out = String::new();
    let mut last_sep = false;
    for ch in header.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            out.push(ch);
            last_sep = false;
        } else if !out.is_empty() && !last_sep {
            out.push('_');
            last_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        format!("column_{}", source_index + 1)
    } else {
        out
    }
}

fn infer_role(header: &str) -> ColumnRole {
    let normalized = stable_column_id(header, 0);
    let lower = normalized.as_str();
    if lower.contains("date") || lower.contains("дата") {
        ColumnRole::Date
    } else if lower == "year" || lower.contains("рік") || lower.contains("год") {
        ColumnRole::Year
    } else if lower.contains("country") || lower.contains("краї") || lower.contains("стра")
    {
        ColumnRole::Country
    } else if lower.contains("code") || lower.contains("код") || lower.contains("sku") {
        ColumnRole::Code
    } else if lower.contains("id") || lower.contains("number") || lower.contains("номер") {
        ColumnRole::Identifier
    } else if lower.contains("price")
        || lower.contains("value")
        || lower.contains("amount")
        || lower.contains("варт")
        || lower.contains("сум")
    {
        ColumnRole::Money
    } else if lower.contains("weight") || lower.contains("вага") || lower.contains("kg") {
        ColumnRole::Weight
    } else if lower.contains("qty") || lower.contains("quantity") || lower.contains("кільк") {
        ColumnRole::Number
    } else {
        ColumnRole::Text
    }
}

#[cfg(test)]
mod tests {
    use super::{ColumnRole, TableShape};

    #[test]
    fn table_shape_keeps_every_source_column_first_class() {
        let shape = TableShape::from_headers([
            "SKU".to_string(),
            "Price EUR".to_string(),
            "Origin country".to_string(),
            "SKU".to_string(),
            "".to_string(),
        ]);

        assert_eq!(shape.columns.len(), 5);
        assert_eq!(shape.columns[0].id, "sku");
        assert_eq!(shape.columns[1].role, ColumnRole::Money);
        assert_eq!(shape.columns[2].role, ColumnRole::Country);
        assert_eq!(shape.columns[3].id, "sku_2");
        assert_eq!(shape.columns[4].header, "Column 5");
    }
}
