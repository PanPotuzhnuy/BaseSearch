use serde::{Deserialize, Serialize};

use crate::search::{FieldInfo, QueryExpr};

/// Filter values; an empty string means the filter is not set.
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filters {
    pub year: String,
    pub product_code: String,
    pub trademark: String,
    pub description: String,
    pub sender: String,
    pub recipient: String,
    pub edrpou: String,
    pub trade_country: String,
    pub dispatch_country: String,
    pub origin_country: String,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        [
            &self.year,
            &self.product_code,
            &self.trademark,
            &self.description,
            &self.sender,
            &self.recipient,
            &self.edrpou,
            &self.trade_country,
            &self.dispatch_country,
            &self.origin_country,
        ]
        .iter()
        .all(|value| value.trim().is_empty())
    }

    pub fn clear(&mut self) {
        *self = Filters::default();
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub text: String,
    pub filters: Filters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advanced: Option<QueryExpr>,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
            && self.filters.is_empty()
            && self.advanced.as_ref().is_none_or(QueryExpr::is_empty)
    }
}

/// One row prepared for insertion during import.
pub struct ImportRecord {
    pub hash: [u8; 16],
    pub year: Option<i64>,
    pub values: Vec<String>,
    /// Source columns not stored in compatibility schema fields. JSON array of
    /// [header, value] pairs.
    pub extra: Option<String>,
}

pub struct RecordCard {
    pub fields: Vec<(String, String)>,
    pub source_file: String,
    /// Extra source columns this file had beyond the known schema, in file order.
    pub extra: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct ImportLogEntry {
    pub file_name: String,
    pub total_rows: u64,
    pub imported: u64,
    pub duplicates: u64,
    pub seconds: f64,
    pub imported_at: String,
    pub quality: ImportQuality,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportQuality {
    pub layout: String,
    pub header_row: u64,
    pub source_columns: u64,
    pub recognized_columns: u64,
    pub extra_columns: u64,
    pub non_empty_cells: u64,
    pub empty_cells: u64,
    pub warnings: Vec<String>,
}

impl ImportQuality {
    pub fn filled_percent(&self) -> f64 {
        let total = self.non_empty_cells + self.empty_cells;
        if total == 0 {
            0.0
        } else {
            self.non_empty_cells as f64 * 100.0 / total as f64
        }
    }

    pub(crate) fn warnings_text(&self) -> String {
        self.warnings.join("\n")
    }

    pub(crate) fn with_warnings_text(mut self, warnings: String) -> ImportQuality {
        self.warnings = warnings
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        self
    }
}

pub struct ImportLogWrite<'a> {
    pub file_name: &'a str,
    pub total_rows: u64,
    pub imported: u64,
    pub duplicates: u64,
    pub seconds: f64,
    pub file_hash: Option<&'a str>,
    pub quality: &'a ImportQuality,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsOverview {
    pub row_count: u64,
    pub declaration_count: u64,
    pub distinct_senders: u64,
    pub distinct_recipients: u64,
    pub distinct_edrpou: u64,
    pub distinct_trademarks: u64,
    pub distinct_product_codes: u64,
    pub distinct_origin_countries: u64,
    pub distinct_dispatch_countries: u64,
    pub distinct_trade_countries: u64,
    pub total_value_usd: f64,
    pub total_gross_kg: f64,
    pub total_net_kg: f64,
    pub total_quantity: f64,
    pub avg_value_per_net_kg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalyticsFilterField {
    Recipient,
    Sender,
    Edrpou,
    ProductCode,
    Trademark,
    OriginCountry,
    DispatchCountry,
    TradeCountry,
    Description,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticsFilterAction {
    pub field: AnalyticsFilterField,
    pub value: String,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsGroupRow {
    pub label: String,
    pub rows: u64,
    pub declarations: u64,
    pub companies: u64,
    pub total_value_usd: f64,
    pub total_net_kg: f64,
    pub total_gross_kg: f64,
    pub total_quantity: f64,
    pub share_percent: f64,
    pub avg_value_per_net_kg: f64,
    pub filter_action: Option<AnalyticsFilterAction>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnalyticsSectionKind {
    #[default]
    Recipients,
    Senders,
    Edrpou,
    ProductCodes,
    Trademarks,
    ProductGroups,
    OriginCountries,
    DispatchCountries,
    TradeCountries,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsSection {
    pub kind: AnalyticsSectionKind,
    pub rows: Vec<AnalyticsGroupRow>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PriceMetricKind {
    #[default]
    ValuePerNetKg,
    RfvUsdKg,
    RmvNetUsdKg,
    RmvUsdExtraUnit,
    RmvGrossUsdKg,
    MinBaseUsdKg,
}

#[derive(Clone, Debug, Default)]
pub struct AnalyticsPriceMetric {
    pub kind: PriceMetricKind,
    pub count: u64,
    pub average: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub weighted_average: f64,
    /// Robust statistics: median and quartiles are less sensitive to outliers
    /// and source-data mistakes than min/max.
    pub median: f64,
    pub p25: f64,
    pub p75: f64,
}

/// Analytics category computed independently, so the GUI can load only
/// the visible one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnalyticsScope {
    #[default]
    Companies,
    Products,
    Countries,
    Prices,
}

impl AnalyticsScope {
    pub const ALL: [AnalyticsScope; 4] = [
        AnalyticsScope::Companies,
        AnalyticsScope::Products,
        AnalyticsScope::Countries,
        AnalyticsScope::Prices,
    ];

    pub fn index(self) -> usize {
        match self {
            AnalyticsScope::Companies => 0,
            AnalyticsScope::Products => 1,
            AnalyticsScope::Countries => 2,
            AnalyticsScope::Prices => 3,
        }
    }
}

/// One month of import dynamics (chart data).
#[derive(Clone, Debug, Default)]
pub struct AnalyticsMonthRow {
    /// "2024-03"
    pub month: String,
    pub rows: u64,
    pub declarations: u64,
    pub total_value_usd: f64,
    pub total_net_kg: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Analytics {
    pub overview: AnalyticsOverview,
    pub months: Vec<AnalyticsMonthRow>,
    pub company_sections: Vec<AnalyticsSection>,
    pub product_sections: Vec<AnalyticsSection>,
    pub country_sections: Vec<AnalyticsSection>,
    pub price_sections: Vec<AnalyticsPriceMetric>,
    pub top_recipients: Vec<AnalyticsGroupRow>,
    pub top_senders: Vec<AnalyticsGroupRow>,
    pub top_trademarks: Vec<AnalyticsGroupRow>,
    pub top_product_codes: Vec<AnalyticsGroupRow>,
    pub top_origin_countries: Vec<AnalyticsGroupRow>,
}

/// One row flagged as potentially undervalued: its price per kg is well below
/// the median for the same product code.
#[derive(Clone, Debug, Default)]
pub struct UndervaluedRow {
    pub id: i64,
    pub declaration_date: String,
    pub declaration_number: String,
    pub recipient: String,
    pub sender: String,
    pub edrpou: String,
    pub product_code: String,
    pub description: String,
    pub source_value: f64,
    pub net_kg: f64,
    pub price_per_kg: f64,
    pub code_median: f64,
    pub code_p25: f64,
    pub code_p75: f64,
    pub code_sample_count: u64,
    pub estimated_gap: f64,
    /// price_per_kg / code_median (0.3 means 30% of the typical price).
    pub ratio: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Undervaluation {
    pub rows: Vec<UndervaluedRow>,
    /// Number of distinct product codes that had enough samples to judge.
    pub checked_codes: u64,
    /// Priced rows in those judged product codes.
    pub checked_rows: u64,
    pub flagged_rows: u64,
    pub flagged_codes: u64,
    pub flagged_value: f64,
    pub estimated_gap: f64,
}

/// Dimension for the pivot table (rows or columns).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PivotDim {
    Recipient,
    Sender,
    Edrpou,
    ProductCode,
    Trademark,
    OriginCountry,
    DispatchCountry,
    TradeCountry,
    Month,
    Year,
}

impl PivotDim {
    /// The filter field this dimension maps to, for drill-down clicks.
    pub fn filter_field(self) -> Option<AnalyticsFilterField> {
        match self {
            PivotDim::Recipient => Some(AnalyticsFilterField::Recipient),
            PivotDim::Sender => Some(AnalyticsFilterField::Sender),
            PivotDim::Edrpou => Some(AnalyticsFilterField::Edrpou),
            PivotDim::ProductCode => Some(AnalyticsFilterField::ProductCode),
            PivotDim::Trademark => Some(AnalyticsFilterField::Trademark),
            PivotDim::OriginCountry => Some(AnalyticsFilterField::OriginCountry),
            PivotDim::DispatchCountry => Some(AnalyticsFilterField::DispatchCountry),
            PivotDim::TradeCountry => Some(AnalyticsFilterField::TradeCountry),
            PivotDim::Month | PivotDim::Year => None,
        }
    }
}

pub fn pivot_filter_action(
    dim: PivotDim,
    value: impl Into<String>,
) -> Option<AnalyticsFilterAction> {
    dim.filter_field().map(|field| AnalyticsFilterAction {
        field,
        value: value.into(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PivotMetric {
    Value,
    Rows,
    NetKg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PivotLimits {
    pub rows: usize,
    pub cols: usize,
}

/// Cross-tab: a matrix of one dimension by another for a chosen metric.
#[derive(Clone, Debug, Default)]
pub struct PivotResult {
    pub row_labels: Vec<String>,
    pub col_labels: Vec<String>,
    /// cells[row][col].
    pub cells: Vec<Vec<f64>>,
    pub row_totals: Vec<f64>,
    pub col_totals: Vec<f64>,
    pub grand_total: f64,
    /// True when low-ranked rows/columns were folded into an "others" bucket.
    pub rows_truncated: bool,
    pub cols_truncated: bool,
    /// True when clicking a row or column label can safely apply a legacy
    /// filter. Generic source-column dimensions are calculated correctly but
    /// are not yet mapped to the old filter fields.
    pub row_filterable: bool,
    pub col_filterable: bool,
}

/// Single-company dossier built for one EDRPOU: everything an analyst needs
/// to answer "tell me everything about this importer" on one screen.
#[derive(Clone, Debug, Default)]
pub struct CompanyProfile {
    pub edrpou: String,
    /// All recipient-name variants seen for this EDRPOU.
    pub names: Vec<String>,
    pub overview: AnalyticsOverview,
    pub months: Vec<AnalyticsMonthRow>,
    pub top_products: Vec<AnalyticsGroupRow>,
    pub top_senders: Vec<AnalyticsGroupRow>,
    pub top_origin_countries: Vec<AnalyticsGroupRow>,
    pub product_sections: Vec<AnalyticsSection>,
    pub country_sections: Vec<AnalyticsSection>,
    pub price_sections: Vec<AnalyticsPriceMetric>,
}

pub type SearchPage = (Vec<i64>, Vec<Vec<String>>, Vec<Option<String>>);
pub type DynamicSearchPage = (
    Vec<FieldInfo>,
    Vec<i64>,
    Vec<Vec<String>>,
    Vec<Option<String>>,
);

pub fn analytics_should_run(q: &Query) -> bool {
    !q.is_empty()
}
