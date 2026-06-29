//! Resolves analytics SQL expressions for semantic fields against the recorded
//! table shape, so analytics is driven by the shape rather than hardcoded column
//! names.
//!
//! For a recognized customs column the semantic resolves to its materialized
//! typed column (`storage::derived`), so customs analytics stays fully typed and
//! unchanged. For a column the user assigned a meaning to that lives in the
//! `extra` JSON (a generic table), the semantic resolves to a normalized
//! expression over that JSON value. When the shape carries no column for a
//! semantic, it falls back to the customs profile.

use crate::domain::table::{ColumnStorage, SemanticField, TableShape};
use crate::schema::column_for_semantic;
use crate::storage::derived;

pub(crate) struct AnalyticsColumns {
    shape: Option<TableShape>,
}

enum Source {
    /// A physical column in the `records` table.
    Schema(String),
    /// A value inside the `extra` JSON, keyed by header.
    Extra(String),
}

impl AnalyticsColumns {
    pub(crate) fn new(shape: Option<TableShape>) -> Self {
        Self { shape }
    }

    fn source_for(&self, field: SemanticField) -> Option<Source> {
        if let Some(shape) = &self.shape
            && let Some(column) = shape.columns.iter().find(|c| c.semantic == Some(field))
        {
            return Some(match &column.storage {
                ColumnStorage::SchemaColumn(name) => Source::Schema(name.clone()),
                ColumnStorage::SourceJson => Source::Extra(column.header.clone()),
            });
        }
        column_for_semantic(field).map(|name| Source::Schema(name.to_string()))
    }

    pub(crate) fn is_schema_backed(&self, field: SemanticField) -> bool {
        matches!(self.source_for(field), Some(Source::Schema(_)))
    }

    /// Numeric (`REAL`/`NULL`) expression for a semantic, or `None` if the shape
    /// has no column for it.
    pub(crate) fn number(&self, field: SemanticField) -> Option<String> {
        Some(match self.source_for(field)? {
            Source::Schema(name) => derived::num_column_for(&name)
                .map(|column| format!("r.{column}"))
                .unwrap_or_else(|| format!("num_value(r.{name})")),
            Source::Extra(header) => {
                format!("num_value(extra_value(r.extra, '{}'))", escape(&header))
            }
        })
    }

    /// Cleaned grouping-label expression for a semantic.
    pub(crate) fn label(&self, field: SemanticField) -> Option<String> {
        Some(match self.source_for(field)? {
            Source::Schema(name) => derived::label_column_for(&name)
                .map(|column| format!("r.{column}"))
                .unwrap_or_else(|| format!("label_value(r.{name})")),
            Source::Extra(header) => {
                format!("label_value(extra_value(r.extra, '{}'))", escape(&header))
            }
        })
    }

    /// Normalized country-key expression for a semantic.
    pub(crate) fn country_key(&self, field: SemanticField) -> Option<String> {
        Some(match self.source_for(field)? {
            Source::Schema(name) => derived::key_column_for(&name)
                .map(|column| format!("r.{column}"))
                .unwrap_or_else(|| format!("country_key(r.{name})")),
            Source::Extra(header) => {
                format!("country_key(extra_value(r.extra, '{}'))", escape(&header))
            }
        })
    }

    /// Month-key expression (`YYYY-MM`) for a date semantic.
    pub(crate) fn month(&self, field: SemanticField) -> Option<String> {
        Some(match self.source_for(field)? {
            Source::Schema(name) => derived::month_column_for(&name)
                .map(|column| format!("r.{column}"))
                .unwrap_or_else(|| format!("month_key(r.{name})")),
            Source::Extra(header) => {
                format!("month_key(extra_value(r.extra, '{}'))", escape(&header))
            }
        })
    }
}

/// Escapes a header for inlining as a single-quoted SQL string literal. Headers
/// come from imported files (not from query-time user input) and are escaped
/// here so a quote in a header cannot break the expression.
fn escape(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::AnalyticsColumns;
    use crate::domain::table::{
        ColumnRole, ColumnStorage, SemanticField, SourceColumn, TableShape,
    };

    fn column(
        id: &str,
        header: &str,
        semantic: SemanticField,
        storage: ColumnStorage,
    ) -> SourceColumn {
        SourceColumn {
            id: id.to_string(),
            header: header.to_string(),
            source_index: 0,
            role: ColumnRole::Text,
            semantic: Some(semantic),
            storage,
        }
    }

    #[test]
    fn customs_semantics_resolve_to_typed_columns() {
        let shape = TableShape {
            columns: vec![
                column(
                    "value",
                    "ФВ вал.контр",
                    SemanticField::Value,
                    ColumnStorage::SchemaColumn("currency_control_value".to_string()),
                ),
                column(
                    "sender",
                    "Відправник",
                    SemanticField::Sender,
                    ColumnStorage::SchemaColumn("sender".to_string()),
                ),
                column(
                    "origin",
                    "Кр.пох.",
                    SemanticField::OriginCountry,
                    ColumnStorage::SchemaColumn("origin_country".to_string()),
                ),
            ],
        };
        let cols = AnalyticsColumns::new(Some(shape));
        assert_eq!(
            cols.number(SemanticField::Value).as_deref(),
            Some("r.value_num")
        );
        assert_eq!(
            cols.label(SemanticField::Sender).as_deref(),
            Some("r.sender_label")
        );
        assert_eq!(
            cols.country_key(SemanticField::OriginCountry).as_deref(),
            Some("r.origin_key")
        );
        assert_eq!(cols.month(SemanticField::Date).as_deref(), Some("r.month"));
    }

    #[test]
    fn missing_shape_falls_back_to_customs_profile() {
        let cols = AnalyticsColumns::new(None);
        assert_eq!(
            cols.number(SemanticField::Value).as_deref(),
            Some("r.value_num")
        );
        assert_eq!(
            cols.label(SemanticField::Recipient).as_deref(),
            Some("r.recipient_label")
        );
        assert_eq!(cols.month(SemanticField::Date).as_deref(), Some("r.month"));
    }

    #[test]
    fn user_assigned_extra_column_resolves_to_json_expression() {
        let shape = TableShape {
            columns: vec![
                column(
                    "price_eur",
                    "Price EUR",
                    SemanticField::Value,
                    ColumnStorage::SourceJson,
                ),
                column(
                    "ship_from",
                    "Ship From",
                    SemanticField::OriginCountry,
                    ColumnStorage::SourceJson,
                ),
                column(
                    "order_date",
                    "Order Date",
                    SemanticField::Date,
                    ColumnStorage::SourceJson,
                ),
            ],
        };
        let cols = AnalyticsColumns::new(Some(shape));
        assert_eq!(
            cols.number(SemanticField::Value).as_deref(),
            Some("num_value(extra_value(r.extra, 'Price EUR'))")
        );
        assert_eq!(
            cols.country_key(SemanticField::OriginCountry).as_deref(),
            Some("country_key(extra_value(r.extra, 'Ship From'))")
        );
        assert_eq!(
            cols.month(SemanticField::Date).as_deref(),
            Some("month_key(extra_value(r.extra, 'Order Date'))")
        );
    }

    #[test]
    fn header_quote_is_escaped() {
        let shape = TableShape {
            columns: vec![column(
                "x",
                "O'Hara value",
                SemanticField::Value,
                ColumnStorage::SourceJson,
            )],
        };
        let cols = AnalyticsColumns::new(Some(shape));
        assert_eq!(
            cols.number(SemanticField::Value).as_deref(),
            Some("num_value(extra_value(r.extra, 'O''Hara value'))")
        );
    }
}
