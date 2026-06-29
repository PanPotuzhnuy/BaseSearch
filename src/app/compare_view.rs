use super::format::{fmt_compact, fmt_decimal};
use super::ui_text::query_summary;
use crate::db::{Analytics, Query};
use crate::i18n::{Lang, group_digits, tr};

pub(super) fn analytics_compare_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Порівняння",
        _ => "Compare",
    }
}

pub(super) fn compare_hint(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => {
            "Порівняйте поточний запит з іншим товаром, компанією або роком. Фільтри зліва зберігаються, якщо не змінити текст чи рік."
        }
        _ => {
            "Compare the current query with another product, company, or year. Current filters are reused unless you override text or year."
        }
    }
}

pub(super) fn compare_text_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Порівняти з:",
        _ => "Compare with:",
    }
}

pub(super) fn compare_previous_year_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Попередній рік",
        _ => "Previous year",
    }
}

pub(super) fn compare_run_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Порівняти",
        _ => "Compare",
    }
}

pub(super) fn compare_empty(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Вкажіть текст або рік для порівняння і натисніть «Порівняти».",
        _ => "Enter a text or year to compare with, then click Compare.",
    }
}

pub(super) fn compare_ui(
    ui: &mut egui::Ui,
    left: &Analytics,
    right: &Analytics,
    left_query: &Query,
    right_query: &Query,
    lang: Lang,
) {
    ui.columns(2, |cols| {
        compare_side_card(
            &mut cols[0],
            query_summary(left_query, tr(lang)),
            left,
            lang,
        );
        compare_side_card(
            &mut cols[1],
            query_summary(right_query, tr(lang)),
            right,
            lang,
        );
    });
    ui.add_space(10.0);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(compare_delta_title(lang)).strong());
            ui.add_space(4.0);
            egui::Grid::new("compare_delta_grid")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    compare_metric_row(
                        ui,
                        tr(lang).rows_label,
                        left.overview.row_count as f64,
                        right.overview.row_count as f64,
                        0,
                    );
                    compare_metric_row(
                        ui,
                        tr(lang).declarations_label,
                        left.overview.declaration_count as f64,
                        right.overview.declaration_count as f64,
                        0,
                    );
                    compare_metric_row(
                        ui,
                        tr(lang).total_value,
                        left.overview.total_value_usd,
                        right.overview.total_value_usd,
                        2,
                    );
                    compare_metric_row(
                        ui,
                        tr(lang).net_weight,
                        left.overview.total_net_kg,
                        right.overview.total_net_kg,
                        2,
                    );
                    compare_metric_row(
                        ui,
                        tr(lang).avg_value_kg,
                        left.overview.avg_value_per_net_kg,
                        right.overview.avg_value_per_net_kg,
                        2,
                    );
                    compare_metric_row(
                        ui,
                        tr(lang).unique_edrpou,
                        left.overview.distinct_edrpou as f64,
                        right.overview.distinct_edrpou as f64,
                        0,
                    );
                });
        });
}

fn compare_side_card(ui: &mut egui::Ui, title: String, analytics: &Analytics, lang: Lang) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(title).strong());
            ui.add_space(4.0);
            ui.label(format!(
                "{}: {}",
                tr(lang).rows_label,
                group_digits(analytics.overview.row_count)
            ));
            ui.label(format!(
                "{}: {}",
                tr(lang).total_value,
                fmt_compact(analytics.overview.total_value_usd)
            ));
            ui.label(format!(
                "{}: {} kg",
                tr(lang).net_weight,
                fmt_compact(analytics.overview.total_net_kg)
            ));
            ui.label(format!(
                "{}: {}",
                tr(lang).avg_value_kg,
                fmt_decimal(analytics.overview.avg_value_per_net_kg, 2)
            ));
        });
}

fn compare_delta_title(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Різниця",
        _ => "Difference",
    }
}

fn compare_metric_row(ui: &mut egui::Ui, label: &str, left: f64, right: f64, decimals: usize) {
    ui.label(label);
    ui.label(egui::RichText::new(format_metric(left, decimals)).monospace());
    ui.label(egui::RichText::new(format_metric(right, decimals)).monospace());
    let delta = right - left;
    let pct = if left.abs() > f64::EPSILON {
        delta / left * 100.0
    } else {
        0.0
    };
    let text = if left.abs() > f64::EPSILON {
        format!("{} ({:+.1}%)", format_metric(delta, decimals), pct)
    } else {
        format_metric(delta, decimals)
    };
    ui.label(egui::RichText::new(text).monospace().strong());
    ui.end_row();
}

fn format_metric(value: f64, decimals: usize) -> String {
    if decimals == 0 {
        let rounded = value.round();
        if rounded < 0.0 {
            format!("-{}", group_digits((-rounded) as u64))
        } else {
            group_digits(rounded as u64)
        }
    } else {
        fmt_decimal(value, decimals)
    }
}
