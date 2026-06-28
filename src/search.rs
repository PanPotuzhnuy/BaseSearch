//! V2 search model: typed query expressions, field metadata, and UI helpers.

use serde::{Deserialize, Serialize};

use crate::schema::{RESULT_COLUMNS, header_for};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryExpr {
    Group(QueryGroup),
    Condition(QueryCondition),
}

impl QueryExpr {
    pub fn is_empty(&self) -> bool {
        match self {
            QueryExpr::Group(group) => group.is_empty(),
            QueryExpr::Condition(condition) => condition.is_empty(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryGroup {
    pub op: LogicOp,
    pub negated: bool,
    pub children: Vec<QueryExpr>,
}

impl Default for QueryGroup {
    fn default() -> Self {
        Self {
            op: LogicOp::And,
            negated: false,
            children: Vec::new(),
        }
    }
}

impl QueryGroup {
    pub fn is_empty(&self) -> bool {
        self.children.iter().all(QueryExpr::is_empty)
    }

    pub fn as_expr(self) -> Option<QueryExpr> {
        (!self.is_empty()).then_some(QueryExpr::Group(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogicOp {
    And,
    Or,
}

impl LogicOp {
    pub fn as_str(self) -> &'static str {
        match self {
            LogicOp::And => "AND",
            LogicOp::Or => "OR",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryCondition {
    pub field: FieldRef,
    pub op: ConditionOp,
    pub value: ConditionValue,
    pub negated: bool,
}

impl QueryCondition {
    pub fn is_empty(&self) -> bool {
        match self.op {
            ConditionOp::IsEmpty | ConditionOp::IsNotEmpty => false,
            ConditionOp::Contains | ConditionOp::Equals | ConditionOp::StartsWith => self
                .value
                .single()
                .is_none_or(|value| value.trim().is_empty()),
            ConditionOp::IsAnyOf => self
                .value
                .list()
                .is_none_or(|values| values.iter().all(|value| value.trim().is_empty())),
            ConditionOp::Range => self.value.range().is_none_or(|(from, to)| {
                from.map_or("", String::as_str).trim().is_empty()
                    && to.map_or("", String::as_str).trim().is_empty()
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldRef {
    Column(String),
    Extra(String),
}

impl FieldRef {
    pub fn id(&self) -> String {
        match self {
            FieldRef::Column(name) => name.clone(),
            FieldRef::Extra(header) => format!("extra:{header}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionOp {
    Contains,
    Equals,
    StartsWith,
    IsAnyOf,
    Range,
    IsEmpty,
    IsNotEmpty,
}

impl ConditionOp {
    pub fn label(self) -> &'static str {
        match self {
            ConditionOp::Contains => "contains",
            ConditionOp::Equals => "equals",
            ConditionOp::StartsWith => "starts with",
            ConditionOp::IsAnyOf => "is any of",
            ConditionOp::Range => "is between",
            ConditionOp::IsEmpty => "is empty",
            ConditionOp::IsNotEmpty => "is not empty",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionValue {
    None,
    Single(String),
    List(Vec<String>),
    Range {
        from: Option<String>,
        to: Option<String>,
    },
}

impl ConditionValue {
    pub fn single(&self) -> Option<&str> {
        match self {
            ConditionValue::Single(value) => Some(value),
            _ => None,
        }
    }

    pub fn list(&self) -> Option<&[String]> {
        match self {
            ConditionValue::List(values) => Some(values),
            _ => None,
        }
    }

    pub fn range(&self) -> Option<(Option<&String>, Option<&String>)> {
        match self {
            ConditionValue::Range { from, to } => Some((from.as_ref(), to.as_ref())),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub source: FieldRef,
    pub operators: Vec<ConditionOp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Code,
    Country,
    Number,
    Date,
    Year,
}

pub fn field_catalog(extra_headers: impl IntoIterator<Item = String>) -> Vec<FieldInfo> {
    let mut fields = Vec::new();
    fields.push(field_info("year", "Year".to_string(), FieldKind::Year));
    for name in RESULT_COLUMNS {
        fields.push(field_info(
            name,
            header_for(name).to_string(),
            field_kind_for_column(name),
        ));
    }
    for header in extra_headers {
        let trimmed = header.trim();
        if trimmed.is_empty() {
            continue;
        }
        let kind = infer_extra_field_kind(trimmed);
        fields.push(FieldInfo {
            id: format!("extra:{trimmed}"),
            label: trimmed.to_string(),
            kind,
            source: FieldRef::Extra(trimmed.to_string()),
            operators: operators_for_kind(kind).to_vec(),
        });
    }
    fields
}

pub fn default_field_catalog() -> Vec<FieldInfo> {
    field_catalog(Vec::<String>::new())
}

pub fn field_kind_for_column(name: &str) -> FieldKind {
    match name {
        "year" => FieldKind::Year,
        "product_code" | "declaration_number" | "edrpou" => FieldKind::Code,
        "trade_country" | "dispatch_country" | "origin_country" => FieldKind::Country,
        "declaration_date" => FieldKind::Date,
        "quantity"
        | "gross_kg"
        | "net_kg"
        | "declaration_weight"
        | "currency_control_value"
        | "rfv_usd_kg"
        | "unit_weight"
        | "weight_difference"
        | "rmv_net_usd_kg"
        | "rmv_usd_extra_unit"
        | "rmv_gross_usd_kg"
        | "min_base_usd_kg"
        | "min_base_difference"
        | "preferential"
        | "full_rate" => FieldKind::Number,
        _ => FieldKind::Text,
    }
}

pub fn operators_for_kind(kind: FieldKind) -> &'static [ConditionOp] {
    match kind {
        FieldKind::Text => &[
            ConditionOp::Contains,
            ConditionOp::Equals,
            ConditionOp::StartsWith,
            ConditionOp::IsAnyOf,
            ConditionOp::IsEmpty,
            ConditionOp::IsNotEmpty,
        ],
        FieldKind::Code => &[
            ConditionOp::StartsWith,
            ConditionOp::Equals,
            ConditionOp::IsAnyOf,
            ConditionOp::IsEmpty,
            ConditionOp::IsNotEmpty,
        ],
        FieldKind::Country => &[
            ConditionOp::Equals,
            ConditionOp::IsAnyOf,
            ConditionOp::IsEmpty,
            ConditionOp::IsNotEmpty,
        ],
        FieldKind::Number | FieldKind::Date | FieldKind::Year => &[
            ConditionOp::Equals,
            ConditionOp::Range,
            ConditionOp::IsEmpty,
            ConditionOp::IsNotEmpty,
        ],
    }
}

pub fn default_condition_for_field(field: &FieldInfo) -> QueryCondition {
    let op = match field.kind {
        FieldKind::Text => ConditionOp::Contains,
        FieldKind::Code => ConditionOp::StartsWith,
        FieldKind::Country | FieldKind::Number | FieldKind::Date | FieldKind::Year => {
            ConditionOp::Equals
        }
    };
    QueryCondition {
        field: field.source.clone(),
        op,
        value: default_value_for_op(op),
        negated: false,
    }
}

pub fn default_value_for_op(op: ConditionOp) -> ConditionValue {
    match op {
        ConditionOp::IsEmpty | ConditionOp::IsNotEmpty => ConditionValue::None,
        ConditionOp::IsAnyOf => ConditionValue::List(vec![String::new()]),
        ConditionOp::Range => ConditionValue::Range {
            from: None,
            to: None,
        },
        ConditionOp::Contains | ConditionOp::Equals | ConditionOp::StartsWith => {
            ConditionValue::Single(String::new())
        }
    }
}

pub fn ensure_value_matches_operator(condition: &mut QueryCondition) {
    let expected = default_value_for_op(condition.op);
    let valid = matches!(
        (&condition.op, &condition.value),
        (
            ConditionOp::IsEmpty | ConditionOp::IsNotEmpty,
            ConditionValue::None
        ) | (
            ConditionOp::Contains | ConditionOp::Equals | ConditionOp::StartsWith,
            ConditionValue::Single(_)
        ) | (ConditionOp::IsAnyOf, ConditionValue::List(_))
            | (ConditionOp::Range, ConditionValue::Range { .. })
    );
    if !valid {
        condition.value = expected;
    }
}

pub fn field_label(field: &FieldRef, catalog: &[FieldInfo]) -> String {
    let id = field.id();
    catalog
        .iter()
        .find(|info| info.id == id)
        .map(|info| info.label.clone())
        .unwrap_or(id)
}

pub fn expr_label(expr: &QueryExpr, catalog: &[FieldInfo]) -> String {
    match expr {
        QueryExpr::Group(group) => {
            let mut text = format!("{} group", group.op.as_str());
            if group.negated {
                text.insert_str(0, "NOT ");
            }
            text
        }
        QueryExpr::Condition(condition) => {
            let mut text = format!(
                "{} {} {}",
                field_label(&condition.field, catalog),
                condition.op.label(),
                value_label(&condition.value)
            );
            if condition.negated {
                text.insert_str(0, "NOT ");
            }
            text.trim().to_string()
        }
    }
}

fn field_info(name: &str, label: String, kind: FieldKind) -> FieldInfo {
    FieldInfo {
        id: name.to_string(),
        label: if label.is_empty() {
            name.to_string()
        } else {
            label
        },
        kind,
        source: FieldRef::Column(name.to_string()),
        operators: operators_for_kind(kind).to_vec(),
    }
}

fn infer_extra_field_kind(header: &str) -> FieldKind {
    let lower = header.to_lowercase();
    if lower.contains("date") || lower.contains("дата") {
        FieldKind::Date
    } else if lower.contains("country") || lower.contains("кра") || lower.contains("стра") {
        FieldKind::Country
    } else if lower.contains("code") || lower.contains("код") || lower.contains("number") {
        FieldKind::Code
    } else if lower.contains("kg")
        || lower.contains("кг")
        || lower.contains("price")
        || lower.contains("value")
        || lower.contains("amount")
        || lower.contains("quantity")
        || lower.contains("qty")
    {
        FieldKind::Number
    } else {
        FieldKind::Text
    }
}

fn value_label(value: &ConditionValue) -> String {
    match value {
        ConditionValue::None => String::new(),
        ConditionValue::Single(value) => value.clone(),
        ConditionValue::List(values) => values.join(", "),
        ConditionValue::Range { from, to } => {
            let from = from.as_deref().unwrap_or("");
            let to = to.as_deref().unwrap_or("");
            format!("{from}..{to}")
        }
    }
}
