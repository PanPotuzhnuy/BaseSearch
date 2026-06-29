use rusqlite::types::Value;

use crate::search::{FieldInfo, FieldRef};

pub(crate) struct SourceFieldSelect {
    pub(crate) expressions: Vec<String>,
    pub(crate) params: Vec<Value>,
}

pub(crate) fn select_for_fields(fields: &[FieldInfo], row_alias: &str) -> SourceFieldSelect {
    let mut expressions = Vec::with_capacity(fields.len());
    let mut params = Vec::new();
    for field in fields {
        match &field.source {
            FieldRef::Column(name) => expressions.push(format!("{row_alias}.{name}")),
            FieldRef::Extra(header) => {
                expressions.push(format!("extra_value({row_alias}.extra, ?)"));
                params.push(header.clone().into());
            }
        }
    }
    SourceFieldSelect {
        expressions,
        params,
    }
}

pub(crate) fn is_source_file_field(field: &FieldInfo) -> bool {
    matches!(&field.source, FieldRef::Column(name) if name == "source_file")
}

#[cfg(test)]
mod tests {
    use super::select_for_fields;
    use rusqlite::types::Value;

    use crate::search::{FieldInfo, FieldKind, FieldRef, operators_for_kind};

    fn field(id: &str, source: FieldRef) -> FieldInfo {
        FieldInfo {
            id: id.to_string(),
            label: id.to_string(),
            kind: FieldKind::Text,
            source,
            operators: operators_for_kind(FieldKind::Text).to_vec(),
        }
    }

    #[test]
    fn select_plan_reads_schema_and_json_backed_source_fields() {
        let fields = [
            field(
                "source:product",
                FieldRef::Column("description".to_string()),
            ),
            field("source:sku", FieldRef::Extra("SKU".to_string())),
        ];

        let plan = select_for_fields(&fields, "r");

        assert_eq!(
            plan.expressions,
            vec![
                "r.description".to_string(),
                "extra_value(r.extra, ?)".to_string()
            ]
        );
        assert_eq!(plan.params, vec![Value::Text("SKU".to_string())]);
    }
}
