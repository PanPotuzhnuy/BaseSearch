use rusqlite::types::Value;

use crate::schema::RESULT_COLUMNS;
use crate::search::{
    ConditionOp, ConditionValue, FieldKind, FieldRef, LogicOp, QueryCondition, QueryExpr,
    field_catalog, field_kind_for_column,
};
use crate::storage::normalize::{
    normalize_country_key, normalize_text_key, parse_number, parse_year,
};
use crate::storage::search_text::glob_escape;

#[derive(Clone)]
struct SearchFieldSql {
    expr: String,
    extra_header: Option<String>,
    kind: FieldKind,
}

pub(crate) fn compile_query_expr(
    expr: &QueryExpr,
) -> rusqlite::Result<Option<(String, Vec<Value>)>> {
    match expr {
        QueryExpr::Group(group) => {
            let mut clauses = Vec::new();
            let mut params = Vec::new();
            for child in &group.children {
                if let Some((clause, child_params)) = compile_query_expr(child)? {
                    clauses.push(clause);
                    params.extend(child_params);
                }
            }
            if clauses.is_empty() {
                return Ok(None);
            }
            let joiner = match group.op {
                LogicOp::And => " AND ",
                LogicOp::Or => " OR ",
            };
            let mut clause = format!("({})", clauses.join(joiner));
            if group.negated {
                clause = format!("NOT ({clause})");
            }
            Ok(Some((clause, params)))
        }
        QueryExpr::Condition(condition) => compile_condition(condition),
    }
}

fn compile_condition(condition: &QueryCondition) -> rusqlite::Result<Option<(String, Vec<Value>)>> {
    if condition.is_empty() {
        return Ok(None);
    }
    let field = search_field_sql(&condition.field)?;
    validate_condition_operator(field.kind, condition.op)?;

    let mut params = Vec::new();
    let clause = match condition.op {
        ConditionOp::Contains => {
            let value = condition
                .value
                .single()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid_search_input("contains requires a value"))?;
            push_field_params(&field, &mut params);
            params.push(value.to_lowercase().into());
            format!("cyr_contains({}, ?)", field.expr)
        }
        ConditionOp::Equals => {
            let value = condition
                .value
                .single()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid_search_input("equals requires a value"))?;
            compile_equal_clause(&field, value, &mut params)?
        }
        ConditionOp::StartsWith => {
            let value = condition
                .value
                .single()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid_search_input("starts with requires a value"))?;
            push_field_params(&field, &mut params);
            match field.kind {
                FieldKind::Code => {
                    params.push(format!("{}*", glob_escape(value)).into());
                    format!("{} GLOB ?", field.expr)
                }
                FieldKind::Text => {
                    params.push(format!("{}*", glob_escape(&normalize_text_key(value))).into());
                    format!("text_key({}) GLOB ?", field.expr)
                }
                _ => {
                    return Err(invalid_search_input(
                        "starts with is only valid for text and code fields",
                    ));
                }
            }
        }
        ConditionOp::IsAnyOf => {
            let values = condition
                .value
                .list()
                .ok_or_else(|| invalid_search_input("is any of requires a list"))?;
            let mut parts = Vec::new();
            for value in values.iter().map(|value| value.trim()) {
                if value.is_empty() {
                    continue;
                }
                parts.push(compile_equal_clause(&field, value, &mut params)?);
            }
            if parts.is_empty() {
                return Ok(None);
            }
            format!("({})", parts.join(" OR "))
        }
        ConditionOp::Range => compile_range_clause(&field, &condition.value, &mut params)?,
        ConditionOp::IsEmpty => {
            push_field_params(&field, &mut params);
            format!("TRIM(COALESCE({}, '')) = ''", field.expr)
        }
        ConditionOp::IsNotEmpty => {
            push_field_params(&field, &mut params);
            format!("TRIM(COALESCE({}, '')) <> ''", field.expr)
        }
    };
    let clause = if condition.negated {
        format!("NOT ({clause})")
    } else {
        clause
    };
    Ok(Some((format!("({clause})"), params)))
}

fn search_field_sql(field: &FieldRef) -> rusqlite::Result<SearchFieldSql> {
    match field {
        FieldRef::Column(name) if name == "year" => Ok(SearchFieldSql {
            expr: "r.year".to_string(),
            extra_header: None,
            kind: FieldKind::Year,
        }),
        FieldRef::Column(name) if RESULT_COLUMNS.contains(&name.as_str()) => Ok(SearchFieldSql {
            expr: format!("r.{name}"),
            extra_header: None,
            kind: field_kind_for_column(name),
        }),
        FieldRef::Column(name) => Err(invalid_search_input(&format!(
            "Unknown search field: {name}"
        ))),
        FieldRef::Extra(header) if header.trim().is_empty() => {
            Err(invalid_search_input("Extra search field header is empty"))
        }
        FieldRef::Extra(header) => Ok(SearchFieldSql {
            expr: "extra_value(r.extra, ?)".to_string(),
            extra_header: Some(header.trim().to_string()),
            kind: field_catalog([header.trim().to_string()])
                .pop()
                .map(|field| field.kind)
                .unwrap_or(FieldKind::Text),
        }),
    }
}

fn validate_condition_operator(kind: FieldKind, op: ConditionOp) -> rusqlite::Result<()> {
    let allowed = match kind {
        FieldKind::Text => matches!(
            op,
            ConditionOp::Contains
                | ConditionOp::Equals
                | ConditionOp::StartsWith
                | ConditionOp::IsAnyOf
                | ConditionOp::IsEmpty
                | ConditionOp::IsNotEmpty
        ),
        FieldKind::Code => matches!(
            op,
            ConditionOp::StartsWith
                | ConditionOp::Equals
                | ConditionOp::IsAnyOf
                | ConditionOp::IsEmpty
                | ConditionOp::IsNotEmpty
        ),
        FieldKind::Country => matches!(
            op,
            ConditionOp::Equals
                | ConditionOp::IsAnyOf
                | ConditionOp::IsEmpty
                | ConditionOp::IsNotEmpty
        ),
        FieldKind::Number | FieldKind::Date | FieldKind::Year => matches!(
            op,
            ConditionOp::Equals
                | ConditionOp::Range
                | ConditionOp::IsEmpty
                | ConditionOp::IsNotEmpty
        ),
    };
    if allowed {
        Ok(())
    } else {
        Err(invalid_search_input(&format!(
            "{} is not valid for {:?} fields",
            op.label(),
            kind
        )))
    }
}

fn push_field_params(field: &SearchFieldSql, params: &mut Vec<Value>) {
    if let Some(header) = &field.extra_header {
        params.push(header.clone().into());
    }
}

fn compile_equal_clause(
    field: &SearchFieldSql,
    value: &str,
    params: &mut Vec<Value>,
) -> rusqlite::Result<String> {
    push_field_params(field, params);
    match field.kind {
        FieldKind::Text => {
            params.push(value.to_string().into());
            Ok(format!("text_key({}) = text_key(?)", field.expr))
        }
        FieldKind::Code | FieldKind::Date => {
            params.push(value.to_string().into());
            Ok(format!("TRIM(COALESCE({}, '')) = ?", field.expr))
        }
        FieldKind::Country => {
            params.push(normalize_country_key(value).into());
            Ok(format!("country_key({}) = ?", field.expr))
        }
        FieldKind::Number => {
            let number = parse_number(value)
                .ok_or_else(|| invalid_search_input("number comparison requires a number"))?;
            params.push(number.into());
            Ok(format!("num_value({}) = ?", field.expr))
        }
        FieldKind::Year => {
            let year = parse_year(value)
                .ok_or_else(|| invalid_search_input("year comparison requires a 4-digit year"))?;
            params.push(year.into());
            Ok(format!("{} = ?", field.expr))
        }
    }
}

fn compile_range_clause(
    field: &SearchFieldSql,
    value: &ConditionValue,
    params: &mut Vec<Value>,
) -> rusqlite::Result<String> {
    let ConditionValue::Range { from, to } = value else {
        return Err(invalid_search_input("range requires from/to values"));
    };
    let mut parts = Vec::new();
    if let Some(from) = from
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_field_params(field, params);
        parts.push(compile_range_bound(field, ">=", from, params)?);
    }
    if let Some(to) = to
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_field_params(field, params);
        parts.push(compile_range_bound(field, "<=", to, params)?);
    }
    if parts.is_empty() {
        return Err(invalid_search_input("range requires at least one boundary"));
    }
    Ok(format!("({})", parts.join(" AND ")))
}

fn compile_range_bound(
    field: &SearchFieldSql,
    cmp: &str,
    value: &str,
    params: &mut Vec<Value>,
) -> rusqlite::Result<String> {
    match field.kind {
        FieldKind::Number => {
            let number = parse_number(value)
                .ok_or_else(|| invalid_search_input("number range requires numeric bounds"))?;
            params.push(number.into());
            Ok(format!("num_value({}) {cmp} ?", field.expr))
        }
        FieldKind::Year => {
            let year = parse_year(value)
                .ok_or_else(|| invalid_search_input("year range requires 4-digit years"))?;
            params.push(year.into());
            Ok(format!("{} {cmp} ?", field.expr))
        }
        FieldKind::Date => {
            params.push(value.to_string().into());
            Ok(format!("TRIM(COALESCE({}, '')) {cmp} ?", field.expr))
        }
        _ => Err(invalid_search_input(
            "range is only valid for number, date, and year fields",
        )),
    }
}

fn invalid_search_input(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.to_string())
}
