use rusqlite::types::Value;

use crate::db::Query;
use crate::storage::normalize::{normalize_country_key, parse_year};
use crate::storage::search_sql;
use crate::storage::search_text::{
    build_fts_query, fts_prefix_terms, glob_escape, plain_search_terms, product_code_search_prefix,
    search_text_expr_with_prefix,
};

#[derive(Clone)]
pub(crate) struct FilterPlan {
    pub(crate) joins: String,
    pub(crate) where_sql: String,
    pub(crate) params: Vec<Value>,
}

pub(crate) fn build_filter_plan(
    q: &Query,
    unique_only: bool,
    fts_watermark: i64,
) -> rusqlite::Result<FilterPlan> {
    let joins = String::new();
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();

    let text_code_prefix = product_code_search_prefix(&q.text);
    let mut match_expr = if text_code_prefix.is_some() {
        String::new()
    } else {
        build_fts_query(&q.text)
    };
    let f = &q.filters;
    let mut contains_clauses: Vec<(String, String)> = Vec::new();
    let trademark = f.trademark.trim();
    if !trademark.is_empty()
        && let Some(terms) = fts_prefix_terms(trademark)
    {
        if !match_expr.is_empty() {
            match_expr.push(' ');
        }
        match_expr.push_str(&terms);
    }
    for (col, value) in [
        ("description", &f.description),
        ("sender", &f.sender),
        ("recipient", &f.recipient),
    ] {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if let Some(terms) = fts_prefix_terms(value) {
            if !match_expr.is_empty() {
                match_expr.push(' ');
            }
            match_expr.push_str(&terms);
        }
        contains_clauses.push((format!("cyr_contains(r.{col}, ?)"), value.to_lowercase()));
    }
    if !match_expr.is_empty() {
        let mut fts_clause =
            "(r.id IN (SELECT rowid FROM records_fts WHERE records_fts MATCH ?)".to_string();
        params.push(match_expr.into());
        let mut tail_clauses = vec!["r.id > ?".to_string()];
        let mut tail_params: Vec<Value> = vec![fts_watermark.into()];
        if text_code_prefix.is_none() {
            for term in plain_search_terms(&q.text) {
                tail_clauses.push(format!(
                    "cyr_contains({}, ?)",
                    search_text_expr_with_prefix("r.")
                ));
                tail_params.push(term.into());
            }
        }
        fts_clause.push_str(" OR (");
        fts_clause.push_str(&tail_clauses.join(" AND "));
        fts_clause.push_str("))");
        clauses.push(fts_clause);
        params.extend(tail_params);
    }
    if let Some(year) = parse_year(&f.year) {
        clauses.push("r.year = ?".into());
        params.push(year.into());
    }
    if let Some(code) = text_code_prefix {
        clauses.push("r.product_code GLOB ?".into());
        params.push(format!("{}*", glob_escape(code)).into());
    }
    let code = f.product_code.trim();
    if !code.is_empty() {
        clauses.push("r.product_code GLOB ?".into());
        params.push(format!("{}*", glob_escape(code)).into());
    }
    let edrpou = f.edrpou.trim();
    if !edrpou.is_empty() {
        clauses.push("r.edrpou = ?".into());
        params.push(edrpou.to_string().into());
    }
    if !trademark.is_empty() {
        clauses.push("text_key(r.trademark) = text_key(?)".into());
        params.push(trademark.to_string().into());
    }
    // Country filters compare against the normalized key column materialized at
    // import, so there is no per-row country normalization at query time.
    for (key_col, value) in [
        ("trade_key", &f.trade_country),
        ("dispatch_key", &f.dispatch_country),
        ("origin_key", &f.origin_country),
    ] {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        clauses.push(format!("r.{key_col} = ?"));
        params.push(normalize_country_key(value).into());
    }
    for (clause, param) in contains_clauses {
        clauses.push(clause);
        params.push(param.into());
    }
    if let Some(advanced) = &q.advanced
        && let Some((clause, advanced_params)) = search_sql::compile_query_expr(advanced)?
    {
        clauses.push(clause);
        params.extend(advanced_params);
    }
    if unique_only {
        clauses.push("r.dup_first_file IS NULL".into());
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    Ok(FilterPlan {
        joins,
        where_sql,
        params,
    })
}

#[cfg(test)]
mod tests {
    use super::build_filter_plan;
    use crate::db::{Filters, Query};

    #[test]
    fn empty_query_builds_empty_filter_plan() {
        let plan = build_filter_plan(&Query::default(), false, 0).unwrap();
        assert!(plan.joins.is_empty());
        assert!(plan.where_sql.is_empty());
        assert!(plan.params.is_empty());
    }

    #[test]
    fn unique_plan_filters_duplicates() {
        let plan = build_filter_plan(&Query::default(), true, 0).unwrap();
        assert_eq!(plan.where_sql, " WHERE r.dup_first_file IS NULL");
    }

    #[test]
    fn product_code_text_uses_range_scan_without_fts() {
        let plan = build_filter_plan(
            &Query {
                text: "8504".to_string(),
                ..Default::default()
            },
            false,
            0,
        )
        .unwrap();
        assert_eq!(plan.where_sql, " WHERE r.product_code GLOB ?");
        assert_eq!(plan.params.len(), 1);
    }

    #[test]
    fn text_query_uses_fts_and_unindexed_tail() {
        let plan = build_filter_plan(
            &Query {
                text: "apple phone".to_string(),
                ..Default::default()
            },
            false,
            42,
        )
        .unwrap();
        assert!(plan.where_sql.contains("records_fts MATCH"));
        assert!(plan.where_sql.contains("r.id > ?"));
        assert!(plan.params.len() >= 2);
    }

    #[test]
    fn structured_filters_are_added_to_plan() {
        let plan = build_filter_plan(
            &Query {
                filters: Filters {
                    year: "2024".to_string(),
                    edrpou: "12345678".to_string(),
                    origin_country: "CN".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            false,
            0,
        )
        .unwrap();
        assert!(plan.where_sql.contains("r.year = ?"));
        assert!(plan.where_sql.contains("r.edrpou = ?"));
        assert!(plan.where_sql.contains("r.origin_key = ?"));
    }
}
