use std::collections::HashMap;

use rusqlite::{Connection, params_from_iter};

use crate::db::{
    AnalyticsFilterAction, AnalyticsFilterField, AnalyticsGroupRow, AnalyticsMonthRow,
    AnalyticsOverview, AnalyticsPriceMetric, AnalyticsSection, AnalyticsSectionKind,
    CompanyProfile, PivotDim, PivotLimits, PivotMetric, PivotResult, PriceMetricKind, Query,
    Undervaluation, UndervaluedRow,
};
use crate::domain::table::SemanticField;
use crate::storage::analytics_columns::AnalyticsColumns;
use crate::storage::query_plan::{self, FilterPlan};
use crate::storage::table_shape;

pub(crate) fn overview(conn: &Connection, plan: FilterPlan) -> rusqlite::Result<AnalyticsOverview> {
    let joins = &plan.joins;
    let where_sql = &plan.where_sql;
    let params = plan.params;
    // Columns are resolved through the recorded table shape: typed customs
    // columns for recognized fields, or normalized expressions over the `extra`
    // JSON for fields the user assigned on a generic table.
    let cols = AnalyticsColumns::new(table_shape::get(conn));
    let label = |field| cols.label(field).unwrap_or_else(|| "''".to_string());
    let country = |field| cols.country_key(field).unwrap_or_else(|| "''".to_string());
    let number = |field| cols.number(field).unwrap_or_else(|| "NULL".to_string());
    let sender = label(SemanticField::Sender);
    let recipient = label(SemanticField::Recipient);
    let edrpou = label(SemanticField::CompanyCode);
    let declaration = label(SemanticField::DeclarationNumber);
    let trademark = label(SemanticField::Trademark);
    let product = label(SemanticField::ProductCode);
    let origin = country(SemanticField::OriginCountry);
    let dispatch = country(SemanticField::DispatchCountry);
    let trade = country(SemanticField::TradeCountry);
    let value = number(SemanticField::Value);
    let gross = number(SemanticField::GrossWeight);
    let net = number(SemanticField::NetWeight);
    let quantity = number(SemanticField::Quantity);
    let sql = format!(
        "SELECT
            COUNT(*),
            COUNT(DISTINCT NULLIF({declaration}, '')),
            COUNT(DISTINCT NULLIF({sender}, '')),
            COUNT(DISTINCT NULLIF({recipient}, '')),
            COUNT(DISTINCT NULLIF({edrpou}, '')),
            COUNT(DISTINCT NULLIF({trademark}, '')),
            COUNT(DISTINCT NULLIF({product}, '')),
            COUNT(DISTINCT NULLIF({origin}, '')),
            COUNT(DISTINCT NULLIF({dispatch}, '')),
            COUNT(DISTINCT NULLIF({trade}, '')),
            SUM({value}),
            SUM({gross}),
            SUM({net}),
            SUM({quantity})
         FROM records r{joins}{where_sql}"
    );
    let overview = conn.query_row(&sql, params_from_iter(params), |row| {
        Ok(AnalyticsOverview {
            row_count: row.get::<_, i64>(0)? as u64,
            declaration_count: row.get::<_, i64>(1)? as u64,
            distinct_senders: row.get::<_, i64>(2)? as u64,
            distinct_recipients: row.get::<_, i64>(3)? as u64,
            distinct_edrpou: row.get::<_, i64>(4)? as u64,
            distinct_trademarks: row.get::<_, i64>(5)? as u64,
            distinct_product_codes: row.get::<_, i64>(6)? as u64,
            distinct_origin_countries: row.get::<_, i64>(7)? as u64,
            distinct_dispatch_countries: row.get::<_, i64>(8)? as u64,
            distinct_trade_countries: row.get::<_, i64>(9)? as u64,
            total_value_usd: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
            total_gross_kg: row.get::<_, Option<f64>>(11)?.unwrap_or(0.0),
            total_net_kg: row.get::<_, Option<f64>>(12)?.unwrap_or(0.0),
            total_quantity: row.get::<_, Option<f64>>(13)?.unwrap_or(0.0),
            avg_value_per_net_kg: 0.0,
        })
    })?;
    Ok(AnalyticsOverview {
        avg_value_per_net_kg: ratio(overview.total_value_usd, overview.total_net_kg),
        ..overview
    })
}

pub(crate) fn months(
    conn: &Connection,
    plan: FilterPlan,
) -> rusqlite::Result<Vec<AnalyticsMonthRow>> {
    let joins = &plan.joins;
    let where_sql = &plan.where_sql;
    let params = plan.params;
    let cols = AnalyticsColumns::new(table_shape::get(conn));
    let month = cols
        .month(SemanticField::Date)
        .unwrap_or_else(|| "''".to_string());
    let declaration = cols
        .label(SemanticField::DeclarationNumber)
        .unwrap_or_else(|| "''".to_string());
    let value = cols
        .number(SemanticField::Value)
        .unwrap_or_else(|| "NULL".to_string());
    let net = cols
        .number(SemanticField::NetWeight)
        .unwrap_or_else(|| "NULL".to_string());
    let month_filter = format!("{month} <> ''");
    let filter_sql = if where_sql.is_empty() {
        format!(" WHERE {month_filter}")
    } else {
        format!("{where_sql} AND {month_filter}")
    };
    let sql = format!(
        "SELECT
            {month} AS month,
            COUNT(*) AS rows_count,
            COUNT(DISTINCT NULLIF({declaration}, '')) AS declarations_count,
            COALESCE(SUM({value}), 0.0) AS total_value_usd,
            COALESCE(SUM({net}), 0.0) AS total_net_kg
         FROM records r{joins}{filter_sql}
         GROUP BY {month}
         ORDER BY {month} DESC
         LIMIT 48"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok(AnalyticsMonthRow {
            month: row.get(0)?,
            rows: row.get::<_, i64>(1)? as u64,
            declarations: row.get::<_, i64>(2)? as u64,
            total_value_usd: row.get(3)?,
            total_net_kg: row.get(4)?,
        })
    })?;
    let mut months: Vec<AnalyticsMonthRow> = rows.flatten().collect();
    months.reverse();
    Ok(months)
}

pub(crate) fn section(
    conn: &Connection,
    plan: FilterPlan,
    kind: AnalyticsSectionKind,
    hs_level: u8,
    limit: u64,
    overview: &AnalyticsOverview,
) -> rusqlite::Result<AnalyticsSection> {
    let cols = AnalyticsColumns::new(table_shape::get(conn));
    let Some(grouping) = section_grouping(&cols, kind, hs_level) else {
        return Ok(AnalyticsSection {
            kind,
            rows: Vec::new(),
        });
    };
    let label_sql = grouping.label_sql;
    let declaration = cols
        .label(SemanticField::DeclarationNumber)
        .unwrap_or_else(|| "''".to_string());
    let company = cols
        .label(SemanticField::CompanyCode)
        .unwrap_or_else(|| "''".to_string());
    let value = cols
        .number(SemanticField::Value)
        .unwrap_or_else(|| "NULL".to_string());
    let net = cols
        .number(SemanticField::NetWeight)
        .unwrap_or_else(|| "NULL".to_string());
    let gross = cols
        .number(SemanticField::GrossWeight)
        .unwrap_or_else(|| "NULL".to_string());
    let quantity = cols
        .number(SemanticField::Quantity)
        .unwrap_or_else(|| "NULL".to_string());
    let joins = &plan.joins;
    let where_sql = &plan.where_sql;
    let mut params = plan.params;
    let non_empty = format!("{label_sql} <> ''");
    let filter_sql = if where_sql.is_empty() {
        format!(" WHERE {non_empty}")
    } else {
        format!("{where_sql} AND {non_empty}")
    };
    let sql = format!(
        "SELECT
            {label_sql} AS label,
            COUNT(*) AS rows_count,
            COUNT(DISTINCT NULLIF({declaration}, '')) AS declarations_count,
            COUNT(DISTINCT NULLIF({company}, '')) AS companies_count,
            COALESCE(SUM({value}), 0.0) AS total_value_usd,
            COALESCE(SUM({net}), 0.0) AS total_net_kg,
            COALESCE(SUM({gross}), 0.0) AS total_gross_kg,
            COALESCE(SUM({quantity}), 0.0) AS total_quantity
         FROM records r{joins}{filter_sql}
         GROUP BY {label_sql}
         ORDER BY total_value_usd DESC, total_net_kg DESC, rows_count DESC, label COLLATE NOCASE
         LIMIT ?"
    );
    params.push((limit as i64).into());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        let label: String = row.get(0)?;
        let total_value_usd: f64 = row.get(4)?;
        let total_net_kg: f64 = row.get(5)?;
        let total_gross_kg: f64 = row.get(6)?;
        let total_quantity: f64 = row.get(7)?;
        let share_base = if overview.total_value_usd > 0.0 {
            overview.total_value_usd
        } else if overview.total_net_kg > 0.0 {
            overview.total_net_kg
        } else {
            overview.row_count as f64
        };
        let share_value = if overview.total_value_usd > 0.0 {
            total_value_usd
        } else if overview.total_net_kg > 0.0 {
            total_net_kg
        } else {
            row.get::<_, i64>(1)? as f64
        };
        Ok(AnalyticsGroupRow {
            filter_action: grouping.filter_field.map(|field| AnalyticsFilterAction {
                field,
                value: label.clone(),
            }),
            label,
            rows: row.get::<_, i64>(1)? as u64,
            declarations: row.get::<_, i64>(2)? as u64,
            companies: row.get::<_, i64>(3)? as u64,
            total_value_usd,
            total_net_kg,
            total_gross_kg,
            total_quantity,
            share_percent: ratio(share_value * 100.0, share_base),
            avg_value_per_net_kg: ratio(total_value_usd, total_net_kg),
        })
    })?;
    Ok(AnalyticsSection {
        kind,
        rows: rows.collect::<rusqlite::Result<Vec<_>>>()?,
    })
}

pub(crate) fn price_metrics(
    conn: &Connection,
    q: &Query,
    fts_watermark: i64,
) -> rusqlite::Result<Vec<AnalyticsPriceMetric>> {
    let cols = AnalyticsColumns::new(table_shape::get(conn));
    let plan = query_plan::build_filter_plan(q, true, fts_watermark)?;
    price_metrics_for_plan(conn, plan, &cols)
}

pub(crate) fn company_profile(
    conn: &Connection,
    identifier: &str,
    limit: u64,
) -> rusqlite::Result<CompanyProfile> {
    let identifier = identifier.trim();
    let cols = AnalyticsColumns::new(table_shape::get(conn));
    let Some(company) = cols.label(SemanticField::CompanyCode) else {
        return Ok(CompanyProfile {
            edrpou: identifier.to_string(),
            ..Default::default()
        });
    };
    let plan = FilterPlan {
        joins: String::new(),
        where_sql: format!(" WHERE {company} = label_value(?) AND r.dup_first_file IS NULL"),
        params: vec![identifier.to_string().into()],
    };
    let overview = overview(conn, plan.clone())?;
    let months = months(conn, plan.clone())?;
    let product_sections = vec![
        section(
            conn,
            plan.clone(),
            AnalyticsSectionKind::ProductCodes,
            10,
            limit,
            &overview,
        )?,
        section(
            conn,
            plan.clone(),
            AnalyticsSectionKind::Trademarks,
            10,
            limit,
            &overview,
        )?,
        section(
            conn,
            plan.clone(),
            AnalyticsSectionKind::ProductGroups,
            10,
            limit,
            &overview,
        )?,
    ];
    let country_sections = vec![
        section(
            conn,
            plan.clone(),
            AnalyticsSectionKind::OriginCountries,
            10,
            limit,
            &overview,
        )?,
        section(
            conn,
            plan.clone(),
            AnalyticsSectionKind::DispatchCountries,
            10,
            limit,
            &overview,
        )?,
        section(
            conn,
            plan.clone(),
            AnalyticsSectionKind::TradeCountries,
            10,
            limit,
            &overview,
        )?,
    ];
    let price_sections = price_metrics_for_plan(conn, plan.clone(), &cols)?;
    let top_products = section_rows(&product_sections, AnalyticsSectionKind::ProductCodes);
    let top_origin_countries =
        section_rows(&country_sections, AnalyticsSectionKind::OriginCountries);
    let top_senders = section(
        conn,
        plan.clone(),
        AnalyticsSectionKind::Senders,
        10,
        limit,
        &overview,
    )?
    .rows;
    let names = profile_names(conn, plan, &cols)?;

    Ok(CompanyProfile {
        edrpou: identifier.to_string(),
        names,
        overview,
        months,
        top_products,
        top_senders,
        top_origin_countries,
        product_sections,
        country_sections,
        price_sections,
    })
}

fn section_rows(
    sections: &[AnalyticsSection],
    kind: AnalyticsSectionKind,
) -> Vec<AnalyticsGroupRow> {
    sections
        .iter()
        .find(|section| section.kind == kind)
        .map(|section| section.rows.clone())
        .unwrap_or_default()
}

fn profile_names(
    conn: &Connection,
    plan: FilterPlan,
    cols: &AnalyticsColumns,
) -> rusqlite::Result<Vec<String>> {
    let Some(recipient) = cols.label(SemanticField::Recipient) else {
        return Ok(Vec::new());
    };
    let joins = &plan.joins;
    let where_sql = &plan.where_sql;
    let params = plan.params;
    let filter_sql = if where_sql.is_empty() {
        format!(" WHERE {recipient} <> ''")
    } else {
        format!("{where_sql} AND {recipient} <> ''")
    };
    let sql = format!(
        "SELECT {recipient} AS name, COUNT(*) AS n
         FROM records r{joins}{filter_sql}
         GROUP BY {recipient}
         ORDER BY n DESC, name COLLATE NOCASE
         LIMIT 8"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| row.get::<_, String>(0))?;
    rows.collect()
}

fn price_metrics_for_plan(
    conn: &Connection,
    plan: FilterPlan,
    cols: &AnalyticsColumns,
) -> rusqlite::Result<Vec<AnalyticsPriceMetric>> {
    let value = cols
        .number(SemanticField::Value)
        .unwrap_or_else(|| "NULL".to_string());
    let net = cols
        .number(SemanticField::NetWeight)
        .unwrap_or_else(|| "NULL".to_string());
    let metric = |kind, price_expr| price_metric(conn, plan.clone(), kind, price_expr, &net);
    Ok(vec![
        metric(
            PriceMetricKind::ValuePerNetKg,
            &format!(
                "CASE
                    WHEN {value} IS NOT NULL
                        AND {net} IS NOT NULL
                        AND {net} > 0
                    THEN {value} / {net}
                 END"
            ),
        )?,
        metric(PriceMetricKind::RfvUsdKg, "r.rfv_num")?,
        metric(PriceMetricKind::RmvNetUsdKg, "r.rmv_net_num")?,
        metric(PriceMetricKind::RmvUsdExtraUnit, "r.rmv_extra_num")?,
        metric(PriceMetricKind::RmvGrossUsdKg, "r.rmv_gross_num")?,
        metric(PriceMetricKind::MinBaseUsdKg, "r.min_base_num")?,
    ])
}

pub(crate) fn pivot(
    conn: &Connection,
    plan: FilterPlan,
    row_dim: PivotDim,
    col_dim: PivotDim,
    metric: PivotMetric,
    limits: PivotLimits,
    others_label: &str,
) -> rusqlite::Result<PivotResult> {
    let cols = AnalyticsColumns::new(table_shape::get(conn));
    let Some(row_dim_sql) = pivot_dim_sql(&cols, row_dim) else {
        return Ok(empty_pivot());
    };
    let Some(col_dim_sql) = pivot_dim_sql(&cols, col_dim) else {
        return Ok(empty_pivot());
    };
    let Some(metric_sql) = pivot_metric_sql(&cols, metric) else {
        return Ok(empty_pivot());
    };

    let joins = &plan.joins;
    let where_sql = &plan.where_sql;
    let params = plan.params;
    let row_sql = row_dim_sql.expr;
    let col_sql = col_dim_sql.expr;
    let non_empty = format!("{row_sql} <> '' AND {col_sql} <> ''");
    let filter_sql = if where_sql.is_empty() {
        format!(" WHERE {non_empty}")
    } else {
        format!("{where_sql} AND {non_empty}")
    };
    let sql = format!(
        "SELECT {row_sql} AS rk, {col_sql} AS ck, {metric_sql} AS v
         FROM records r{joins}{filter_sql}
         GROUP BY {row_sql}, {col_sql}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;

    let mut row_totals: HashMap<String, f64> = HashMap::new();
    let mut col_totals: HashMap<String, f64> = HashMap::new();
    let mut triples: Vec<(String, String, f64)> = Vec::new();
    while let Some(row) = rows.next()? {
        let rk: String = row.get(0)?;
        let ck: String = row.get(1)?;
        let v: f64 = row.get(2)?;
        *row_totals.entry(rk.clone()).or_default() += v;
        *col_totals.entry(ck.clone()).or_default() += v;
        triples.push((rk, ck, v));
    }

    let col_chrono = matches!(col_dim, PivotDim::Month | PivotDim::Year);
    let (row_labels, rows_truncated) = rank_pivot_labels(&row_totals, limits.rows, false);
    let (col_labels, cols_truncated) = rank_pivot_labels(&col_totals, limits.cols, col_chrono);

    let row_index: HashMap<&str, usize> = row_labels
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();
    let col_index: HashMap<&str, usize> = col_labels
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();

    let n_rows = row_labels.len() + usize::from(rows_truncated);
    let n_cols = col_labels.len() + usize::from(cols_truncated);
    let others_row = row_labels.len();
    let others_col = col_labels.len();
    let mut cells = vec![vec![0.0_f64; n_cols]; n_rows];
    for (rk, ck, v) in triples {
        let ri = row_index.get(rk.as_str()).copied().unwrap_or(others_row);
        let ci = col_index.get(ck.as_str()).copied().unwrap_or(others_col);
        if ri < n_rows && ci < n_cols {
            cells[ri][ci] += v;
        }
    }

    let mut final_row_labels = row_labels;
    if rows_truncated {
        final_row_labels.push(others_label.to_string());
    }
    let mut final_col_labels = col_labels;
    if cols_truncated {
        final_col_labels.push(others_label.to_string());
    }
    let row_totals: Vec<f64> = cells.iter().map(|r| r.iter().sum()).collect();
    let mut col_totals = vec![0.0_f64; n_cols];
    for r in &cells {
        for (ci, v) in r.iter().enumerate() {
            col_totals[ci] += v;
        }
    }
    let grand_total: f64 = row_totals.iter().sum();

    Ok(PivotResult {
        row_labels: final_row_labels,
        col_labels: final_col_labels,
        cells,
        row_totals,
        col_totals,
        grand_total,
        rows_truncated,
        cols_truncated,
        row_filterable: row_dim_sql.filterable,
        col_filterable: col_dim_sql.filterable,
    })
}

pub(crate) fn undervaluation(
    conn: &Connection,
    plan: FilterPlan,
    threshold: f64,
    min_samples: u64,
    limit: u64,
) -> rusqlite::Result<Undervaluation> {
    let cols = AnalyticsColumns::new(table_shape::get(conn));
    let Some(product) = cols.label(SemanticField::ProductCode) else {
        return Ok(Undervaluation::default());
    };
    let Some(value) = cols.number(SemanticField::Value) else {
        return Ok(Undervaluation::default());
    };
    let Some(net) = cols.number(SemanticField::NetWeight) else {
        return Ok(Undervaluation::default());
    };

    let label = |field| cols.label(field).unwrap_or_else(|| "''".to_string());
    let date = label(SemanticField::Date);
    let declaration = label(SemanticField::DeclarationNumber);
    let recipient = label(SemanticField::Recipient);
    let sender = label(SemanticField::Sender);
    let company = label(SemanticField::CompanyCode);
    let description = label(SemanticField::Description);

    let joins = &plan.joins;
    let where_sql = &plan.where_sql;
    let params = plan.params;
    let cond = if where_sql.is_empty() {
        " WHERE"
    } else {
        " AND"
    };
    let cte = format!(
        "WITH priced AS (
            SELECT r.id AS id,
                {product} AS code,
                {value} AS source_value,
                {net} AS net_kg,
                {value} / {net} AS price,
                {date} AS dt,
                {declaration} AS num,
                {recipient} AS recipient,
                {sender} AS sender,
                {company} AS edrpou,
                {description} AS descr
            FROM records r{joins}{where_sql}{cond}
                {product} <> ''
                AND {net} > 0
                AND {value} > 0
         ),
         code_stats AS (
            SELECT code, median_num(price) AS med, pctl_text(price) AS pctls, COUNT(*) AS n
            FROM priced GROUP BY code HAVING n >= ?
         ),
         flagged AS (
            SELECT p.id, p.dt, p.num, p.recipient, p.sender, p.edrpou, p.code, p.descr,
                p.source_value, p.net_kg, p.price, c.med, c.pctls, c.n,
                p.price / c.med AS ratio,
                MAX((c.med * p.net_kg) - p.source_value, 0.0) AS estimated_gap
            FROM priced p JOIN code_stats c ON c.code = p.code
            WHERE c.med > 0 AND p.price < c.med * ?
         )
         "
    );

    let summary_sql = format!(
        "{cte}
         SELECT
            COALESCE((SELECT SUM(n) FROM code_stats), 0),
            COALESCE((SELECT COUNT(*) FROM code_stats), 0),
            COALESCE((SELECT COUNT(*) FROM flagged), 0),
            COALESCE((SELECT COUNT(DISTINCT code) FROM flagged), 0),
            COALESCE((SELECT SUM(source_value) FROM flagged), 0.0),
            COALESCE((SELECT SUM(estimated_gap) FROM flagged), 0.0)"
    );
    let mut summary_bind = params.clone();
    summary_bind.push((min_samples as i64).into());
    summary_bind.push(threshold.into());
    let summary = conn.query_row(
        &summary_sql,
        params_from_iter(summary_bind),
        |row| -> rusqlite::Result<(u64, u64, u64, u64, f64, f64)> {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
            ))
        },
    )?;

    let sql = format!(
        "{cte}
         SELECT id, dt, num, recipient, sender, edrpou, code, descr,
                source_value, net_kg, price, med, pctls, n, ratio, estimated_gap
         FROM flagged
         ORDER BY ratio ASC, estimated_gap DESC
         LIMIT ?"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut bind = params;
    bind.push((min_samples as i64).into());
    bind.push(threshold.into());
    bind.push((limit as i64).into());
    let mut rows = stmt.query(params_from_iter(bind))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let pctls: Option<String> = row.get(12)?;
        let mut parts = pctls
            .as_deref()
            .unwrap_or("")
            .split('|')
            .map(|p| p.parse::<f64>().unwrap_or(0.0));
        out.push(UndervaluedRow {
            id: row.get(0)?,
            declaration_date: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            declaration_number: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            recipient: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            sender: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            edrpou: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            product_code: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            description: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
            source_value: row.get::<_, Option<f64>>(8)?.unwrap_or(0.0),
            net_kg: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
            price_per_kg: row.get(10)?,
            code_median: row.get(11)?,
            code_p25: parts.next().unwrap_or(0.0),
            code_p75: {
                let _median = parts.next();
                parts.next().unwrap_or(0.0)
            },
            code_sample_count: row.get::<_, i64>(13)? as u64,
            ratio: row.get(14)?,
            estimated_gap: row.get::<_, Option<f64>>(15)?.unwrap_or(0.0),
        });
    }
    Ok(Undervaluation {
        rows: out,
        checked_rows: summary.0,
        checked_codes: summary.1,
        flagged_rows: summary.2,
        flagged_codes: summary.3,
        flagged_value: summary.4,
        estimated_gap: summary.5,
    })
}

struct SectionGrouping {
    label_sql: String,
    filter_field: Option<AnalyticsFilterField>,
}

/// Final grouping/label SQL for a section. The expression is resolved from the
/// recorded source shape first, so generic files can drive analytics once the
/// user assigns semantic meanings to their columns.
fn section_grouping(
    cols: &AnalyticsColumns,
    kind: AnalyticsSectionKind,
    hs_level: u8,
) -> Option<SectionGrouping> {
    let grouping = |semantic, filter_field| {
        Some(SectionGrouping {
            label_sql: cols.label(semantic)?,
            filter_field: cols.is_schema_backed(semantic).then_some(filter_field),
        })
    };
    let country_grouping = |semantic, filter_field| {
        Some(SectionGrouping {
            label_sql: cols.country_key(semantic)?,
            filter_field: cols.is_schema_backed(semantic).then_some(filter_field),
        })
    };
    match kind {
        AnalyticsSectionKind::Recipients => {
            grouping(SemanticField::Recipient, AnalyticsFilterField::Recipient)
        }
        AnalyticsSectionKind::Senders => {
            grouping(SemanticField::Sender, AnalyticsFilterField::Sender)
        }
        AnalyticsSectionKind::Edrpou => {
            grouping(SemanticField::CompanyCode, AnalyticsFilterField::Edrpou)
        }
        AnalyticsSectionKind::ProductCodes => {
            let product = cols.label(SemanticField::ProductCode)?;
            let expr = if hs_level >= 10 {
                product
            } else {
                format!(
                    "label_value(SUBSTR({product}, 1, {}))",
                    hs_level.clamp(2, 8)
                )
            };
            Some(SectionGrouping {
                label_sql: expr,
                filter_field: cols
                    .is_schema_backed(SemanticField::ProductCode)
                    .then_some(AnalyticsFilterField::ProductCode),
            })
        }
        AnalyticsSectionKind::Trademarks => {
            grouping(SemanticField::Trademark, AnalyticsFilterField::Trademark)
        }
        AnalyticsSectionKind::ProductGroups => {
            let description = cols.label(SemanticField::Description)?;
            Some(SectionGrouping {
                label_sql: format!("label_value(SUBSTR({description}, 1, 80))"),
                filter_field: cols
                    .is_schema_backed(SemanticField::Description)
                    .then_some(AnalyticsFilterField::Description),
            })
        }
        AnalyticsSectionKind::OriginCountries => country_grouping(
            SemanticField::OriginCountry,
            AnalyticsFilterField::OriginCountry,
        ),
        AnalyticsSectionKind::DispatchCountries => country_grouping(
            SemanticField::DispatchCountry,
            AnalyticsFilterField::DispatchCountry,
        ),
        AnalyticsSectionKind::TradeCountries => country_grouping(
            SemanticField::TradeCountry,
            AnalyticsFilterField::TradeCountry,
        ),
    }
}

fn price_metric(
    conn: &Connection,
    plan: FilterPlan,
    kind: PriceMetricKind,
    price_expr: &str,
    weight_expr: &str,
) -> rusqlite::Result<AnalyticsPriceMetric> {
    let joins = &plan.joins;
    let where_sql = &plan.where_sql;
    let params = plan.params;
    let sql = format!(
        "SELECT
            COUNT(price),
            AVG(price),
            MIN(price),
            MAX(price),
            SUM(CASE WHEN price IS NOT NULL AND weight IS NOT NULL AND weight > 0
                THEN price * weight ELSE 0 END),
            SUM(CASE WHEN price IS NOT NULL AND weight IS NOT NULL AND weight > 0
                THEN weight ELSE 0 END),
            pctl_text(price)
         FROM (
            SELECT {price_expr} AS price, {weight_expr} AS weight
            FROM records r{joins}{where_sql}
         )"
    );
    conn.query_row(&sql, params_from_iter(params), |row| {
        let weighted_sum = row.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
        let weighted_kg = row.get::<_, Option<f64>>(5)?.unwrap_or(0.0);
        let pctls: Option<String> = row.get(6)?;
        let mut parts = pctls
            .as_deref()
            .unwrap_or("")
            .split('|')
            .map(|p| p.parse::<f64>().unwrap_or(0.0));
        let p25 = parts.next().unwrap_or(0.0);
        let median = parts.next().unwrap_or(0.0);
        let p75 = parts.next().unwrap_or(0.0);
        Ok(AnalyticsPriceMetric {
            kind,
            count: row.get::<_, i64>(0)? as u64,
            average: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
            minimum: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
            maximum: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
            weighted_average: ratio(weighted_sum, weighted_kg),
            median,
            p25,
            p75,
        })
    })
}

struct PivotDimSql {
    expr: String,
    filterable: bool,
}

fn pivot_dim_sql(cols: &AnalyticsColumns, dim: PivotDim) -> Option<PivotDimSql> {
    let semantic = match dim {
        PivotDim::Recipient => Some((SemanticField::Recipient, false)),
        PivotDim::Sender => Some((SemanticField::Sender, false)),
        PivotDim::Edrpou => Some((SemanticField::CompanyCode, false)),
        PivotDim::ProductCode => Some((SemanticField::ProductCode, false)),
        PivotDim::Trademark => Some((SemanticField::Trademark, false)),
        PivotDim::OriginCountry => Some((SemanticField::OriginCountry, true)),
        PivotDim::DispatchCountry => Some((SemanticField::DispatchCountry, true)),
        PivotDim::TradeCountry => Some((SemanticField::TradeCountry, true)),
        PivotDim::Month | PivotDim::Year => None,
    };
    if let Some((field, is_country)) = semantic {
        let expr = if is_country {
            cols.country_key(field)?
        } else {
            cols.label(field)?
        };
        return Some(PivotDimSql {
            expr,
            filterable: cols.is_schema_backed(field) && dim.filter_field().is_some(),
        });
    }

    match dim {
        PivotDim::Month => Some(PivotDimSql {
            expr: cols.month(SemanticField::Date)?,
            filterable: false,
        }),
        PivotDim::Year => {
            let expr = if cols.is_schema_backed(SemanticField::Date) {
                "CAST(r.year AS TEXT)".to_string()
            } else {
                format!("SUBSTR({}, 1, 4)", cols.month(SemanticField::Date)?)
            };
            Some(PivotDimSql {
                expr,
                filterable: false,
            })
        }
        _ => None,
    }
}

fn pivot_metric_sql(cols: &AnalyticsColumns, metric: PivotMetric) -> Option<String> {
    match metric {
        PivotMetric::Rows => Some("CAST(COUNT(*) AS REAL)".to_string()),
        PivotMetric::Value => cols
            .number(SemanticField::Value)
            .map(|expr| format!("COALESCE(SUM({expr}), 0.0)")),
        PivotMetric::NetKg => cols
            .number(SemanticField::NetWeight)
            .map(|expr| format!("COALESCE(SUM({expr}), 0.0)")),
    }
}

fn rank_pivot_labels(
    totals: &HashMap<String, f64>,
    limit: usize,
    sort_label: bool,
) -> (Vec<String>, bool) {
    let mut items: Vec<(String, f64)> = totals.iter().map(|(k, v)| (k.clone(), *v)).collect();
    if sort_label {
        items.sort_by(|a, b| a.0.cmp(&b.0));
    } else {
        items.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    }
    let truncated = items.len() > limit;
    items.truncate(limit);
    (
        items.into_iter().map(|(k, _)| k).collect::<Vec<_>>(),
        truncated,
    )
}

fn empty_pivot() -> PivotResult {
    PivotResult {
        row_filterable: false,
        col_filterable: false,
        ..Default::default()
    }
}

fn ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator.abs() <= f64::EPSILON {
        0.0
    } else {
        numerator / denominator
    }
}
