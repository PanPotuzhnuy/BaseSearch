use rusqlite::{Connection, params_from_iter};

use crate::db::{DynamicSearchPage, Query, RecordCard, SearchPage};
use crate::schema::{COLUMNS, RESULT_COLUMNS};
use crate::search::FieldInfo;
use crate::storage::extra::parse_extra;
use crate::storage::query_plan::FilterPlan;
use crate::storage::search_text::product_code_search_prefix;
use crate::storage::source_fields;

pub(crate) fn count(conn: &Connection, plan: FilterPlan) -> rusqlite::Result<u64> {
    let sql = format!(
        "SELECT COUNT(*) FROM records r{}{}",
        plan.joins, plan.where_sql
    );
    let n: i64 = conn.query_row(&sql, params_from_iter(plan.params), |r| r.get(0))?;
    Ok(n as u64)
}

pub(crate) fn legacy_search_page(
    conn: &Connection,
    q: &Query,
    mut plan: FilterPlan,
    limit: u64,
    offset: u64,
) -> rusqlite::Result<SearchPage> {
    let select: Vec<String> = RESULT_COLUMNS.iter().map(|c| format!("r.{c}")).collect();
    let order = result_order(q);
    let sql = format!(
        "SELECT r.id, {select}, r.dup_first_file FROM records r{joins}{where_sql} ORDER BY {order} LIMIT ? OFFSET ?",
        select = select.join(", "),
        joins = plan.joins,
        where_sql = plan.where_sql,
    );
    plan.params.push((limit as i64).into());
    plan.params.push((offset as i64).into());
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(plan.params))?;
    let mut ids = Vec::new();
    let mut data = Vec::new();
    let mut dups = Vec::new();
    while let Some(row) = rows.next()? {
        ids.push(row.get::<_, i64>(0)?);
        let mut values = Vec::with_capacity(RESULT_COLUMNS.len());
        for i in 0..RESULT_COLUMNS.len() {
            values.push(row.get::<_, Option<String>>(i + 1)?.unwrap_or_default());
        }
        data.push(values);
        dups.push(row.get::<_, Option<String>>(RESULT_COLUMNS.len() + 1)?);
    }
    Ok((ids, data, dups))
}

pub(crate) fn dynamic_search_page(
    conn: &Connection,
    q: &Query,
    fields: Vec<FieldInfo>,
    plan: FilterPlan,
    limit: u64,
    offset: u64,
) -> rusqlite::Result<DynamicSearchPage> {
    let field_select = source_fields::select_for_fields(&fields, "r");
    let order = result_order(q);
    let sql = format!(
        "SELECT r.id, {select}, r.dup_first_file FROM records r{joins}{where_sql} ORDER BY {order} LIMIT ? OFFSET ?",
        select = field_select.expressions.join(", "),
        joins = plan.joins,
        where_sql = plan.where_sql,
    );
    let mut final_params = field_select.params;
    final_params.extend(plan.params);
    final_params.push((limit as i64).into());
    final_params.push((offset as i64).into());
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(final_params))?;
    let mut ids = Vec::new();
    let mut data = Vec::new();
    let mut dups = Vec::new();
    while let Some(row) = rows.next()? {
        ids.push(row.get::<_, i64>(0)?);
        let mut values = Vec::with_capacity(fields.len());
        for i in 0..fields.len() {
            values.push(row.get::<_, Option<String>>(i + 1)?.unwrap_or_default());
        }
        data.push(values);
        dups.push(row.get::<_, Option<String>>(fields.len() + 1)?);
    }
    Ok((fields, ids, data, dups))
}

pub(crate) fn legacy_export_batch(
    conn: &Connection,
    mut plan: FilterPlan,
    last_id: i64,
    limit: u64,
) -> rusqlite::Result<(i64, Vec<Vec<String>>)> {
    let select: Vec<String> = COLUMNS.iter().map(|c| format!("r.{}", c.name)).collect();
    let cond = keyset_condition_prefix(&plan.where_sql);
    let sql = format!(
        "SELECT r.id, {select}, r.source_file FROM records r{joins}{where_sql}{cond} r.id > ? ORDER BY r.id LIMIT ?",
        select = select.join(", "),
        joins = plan.joins,
        where_sql = plan.where_sql,
    );
    plan.params.push(last_id.into());
    plan.params.push((limit as i64).into());
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(plan.params))?;
    let mut data = Vec::new();
    let mut max_id = last_id;
    while let Some(row) = rows.next()? {
        max_id = row.get::<_, i64>(0)?;
        let mut values = Vec::with_capacity(COLUMNS.len() + 1);
        for i in 0..=COLUMNS.len() {
            values.push(row.get::<_, Option<String>>(i + 1)?.unwrap_or_default());
        }
        data.push(values);
    }
    Ok((max_id, data))
}

pub(crate) fn export_batch_fields(
    conn: &Connection,
    fields: &[FieldInfo],
    plan: FilterPlan,
    last_id: i64,
    limit: u64,
) -> rusqlite::Result<(i64, Vec<Vec<String>>)> {
    let field_select = source_fields::select_for_fields(fields, "r");
    let cond = keyset_condition_prefix(&plan.where_sql);
    let sql = format!(
        "SELECT r.id, {select} FROM records r{joins}{where_sql}{cond} r.id > ? ORDER BY r.id LIMIT ?",
        select = field_select.expressions.join(", "),
        joins = plan.joins,
        where_sql = plan.where_sql,
    );
    let mut final_params = field_select.params;
    final_params.extend(plan.params);
    final_params.push(last_id.into());
    final_params.push((limit as i64).into());
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(final_params))?;
    let mut data = Vec::new();
    let mut max_id = last_id;
    while let Some(row) = rows.next()? {
        max_id = row.get::<_, i64>(0)?;
        let mut values = Vec::with_capacity(fields.len());
        for i in 0..fields.len() {
            values.push(row.get::<_, Option<String>>(i + 1)?.unwrap_or_default());
        }
        data.push(values);
    }
    Ok((max_id, data))
}

pub(crate) fn record_card(
    conn: &Connection,
    fields: Vec<FieldInfo>,
    id: i64,
) -> rusqlite::Result<RecordCard> {
    let card_fields: Vec<FieldInfo> = fields
        .into_iter()
        .filter(|field| !source_fields::is_source_file_field(field))
        .collect();
    let field_select = source_fields::select_for_fields(&card_fields, "r");
    let sql = format!(
        "SELECT {}, r.source_file FROM records r WHERE r.id = ?",
        field_select.expressions.join(", ")
    );
    let mut params = field_select.params;
    params.push(id.into());
    conn.query_row(&sql, params_from_iter(params), |row| {
        let mut fields = Vec::with_capacity(card_fields.len());
        for (i, field) in card_fields.iter().enumerate() {
            fields.push((
                field.label.clone(),
                row.get::<_, Option<String>>(i)?.unwrap_or_default(),
            ));
        }
        let source_file: String = row.get(card_fields.len())?;
        Ok(RecordCard {
            fields,
            source_file,
            extra: Vec::new(),
        })
    })
}

pub(crate) fn legacy_record_card(conn: &Connection, id: i64) -> rusqlite::Result<RecordCard> {
    let select: Vec<String> = COLUMNS.iter().map(|c| c.name.to_string()).collect();
    let sql = format!(
        "SELECT {}, source_file, extra FROM records WHERE id = ?1",
        select.join(", ")
    );
    conn.query_row(&sql, [id], |row| {
        let mut fields = Vec::with_capacity(COLUMNS.len());
        for (i, col) in COLUMNS.iter().enumerate() {
            fields.push((
                col.header.to_string(),
                row.get::<_, Option<String>>(i)?.unwrap_or_default(),
            ));
        }
        let source_file: String = row.get(COLUMNS.len())?;
        let extra = parse_extra(row.get::<_, Option<String>>(COLUMNS.len() + 1)?.as_deref());
        Ok(RecordCard {
            fields,
            source_file,
            extra,
        })
    })
}

fn keyset_condition_prefix(where_sql: &str) -> &'static str {
    if where_sql.is_empty() {
        " WHERE"
    } else {
        " AND"
    }
}

fn result_order(q: &Query) -> &'static str {
    if uses_fast_result_order(q) {
        "r.id DESC"
    } else {
        "r.declaration_date DESC, r.id DESC"
    }
}

fn uses_fast_result_order(q: &Query) -> bool {
    if q.is_empty() || product_code_search_prefix(&q.text).is_some() {
        return true;
    }
    if !q.text.trim().is_empty() {
        return false;
    }
    let f = &q.filters;
    [
        &f.trademark,
        &f.description,
        &f.sender,
        &f.recipient,
        &f.trade_country,
        &f.dispatch_country,
        &f.origin_country,
    ]
    .iter()
    .all(|value| value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::uses_fast_result_order;
    use crate::db::{Filters, Query};

    #[test]
    fn fast_order_is_only_used_for_structural_queries() {
        assert!(uses_fast_result_order(&Query::default()));
        assert!(uses_fast_result_order(&Query {
            filters: Filters {
                year: "2024".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }));
        assert!(!uses_fast_result_order(&Query {
            text: "apple phone".to_string(),
            ..Default::default()
        }));
        assert!(!uses_fast_result_order(&Query {
            filters: Filters {
                sender: "ACME".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }));
    }
}
