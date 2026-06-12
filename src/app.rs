//! Graphical interface: search bar, filters, paginated table, record card,
//! import/export progress, and settings.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui_extras::{Column, TableBuilder};

use crate::db::{Analytics, AnalyticsGroupRow, Db, Filters, Query, RecordCard};
use crate::export::ExportError;
use crate::i18n::{Lang, Tr, fmt, group_digits, tr};
use crate::import::{FileSummary, ImportPhase};
use crate::schema::{RESULT_COLUMNS, header_for};
use crate::workers::{self, ImportEvent, Msg, PAGE_SIZE, WorkerReq};

/// Interface accent color.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(37, 99, 235);
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Action from the table row context menu.
enum RowMenuAction {
    CopyCell(String),
    CopyRow(usize),
    CopySelected,
    FilterSender(String),
    FilterRecipient(String),
    FilterCode(String),
    FilterEdrpou(String),
}

type QuickAction = (&'static str, &'static str, fn(String) -> RowMenuAction);

/// Visual cell type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    /// Primary text, such as descriptions and companies.
    Normal,
    /// Secondary text, such as dates, countries, and organization codes.
    Weak,
    /// Product code: monospace and accented.
    Code,
    /// Numbers: monospace and right-aligned.
    Number,
}

/// Result column width and visual style.
fn col_spec(name: &str) -> (f32, CellKind) {
    match name {
        "clearance_time" => (130.0, CellKind::Weak),
        "customs_office" => (190.0, CellKind::Weak),
        "declaration_type" => (72.0, CellKind::Weak),
        "declaration_date" => (88.0, CellKind::Weak),
        "declaration_number" => (150.0, CellKind::Weak),
        "sender" => (195.0, CellKind::Normal),
        "recipient" => (195.0, CellKind::Normal),
        "item_number" => (58.0, CellKind::Number),
        "description" => (440.0, CellKind::Normal),
        "product_code" => (104.0, CellKind::Code),
        "edrpou" => (88.0, CellKind::Weak),
        "trade_country" | "dispatch_country" | "origin_country" => (76.0, CellKind::Weak),
        "delivery_terms" => (92.0, CellKind::Weak),
        "delivery_place" => (140.0, CellKind::Weak),
        "quantity" => (76.0, CellKind::Number),
        "unit" => (72.0, CellKind::Weak),
        "gross_kg"
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
        | "full_rate" => (112.0, CellKind::Number),
        "contract" => (150.0, CellKind::Weak),
        "trademark" => (110.0, CellKind::Weak),
        "source_file" => (140.0, CellKind::Weak),
        _ => (110.0, CellKind::Normal),
    }
}

fn trunc_label(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('\u{2026}');
    }
    out
}

pub fn default_db_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    exe_dir.join("data").join("base_search.db")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Import,
    Export,
    Clear,
}

struct OpState {
    kind: OpKind,
    cancel: Arc<AtomicBool>,
    last_event: Option<ImportEvent>,
    export_progress: (u64, u64),
}

#[derive(Default)]
struct StatusLine {
    text: String,
    is_error: bool,
}

pub struct App {
    lang: Lang,
    db_path: PathBuf,
    /// Lightweight connection for instant operations, such as cards and settings.
    lite_db: Option<Db>,

    query_text: String,
    filters: Filters,
    show_filters: bool,
    active_query: Query,
    page: u64,
    total: Option<u64>,
    rows: Vec<Vec<String>>,
    row_ids: Vec<i64>,
    analytics: Option<Analytics>,
    show_analytics: bool,
    selected: HashSet<usize>,
    select_anchor: Option<usize>,
    visible_cols: Vec<bool>,
    search_gen: u64,
    search_in_flight: bool,
    last_search_ms: Option<u64>,

    db_total_rows: Option<u64>,
    status: StatusLine,

    op: Option<OpState>,
    import_report: Option<Vec<FileSummary>>,

    card: Option<RecordCard>,
    card_open: bool,
    show_settings: bool,
    confirm_clear: bool,

    msg_rx: Receiver<Msg>,
    msg_tx: Sender<Msg>,
    search_tx: Sender<WorkerReq>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);

        let db_path = default_db_path();
        let lite_db = Db::open(&db_path).ok();

        let lang = lite_db
            .as_ref()
            .and_then(|db| db.meta_get("lang"))
            .map(|c| Lang::from_code(&c))
            .unwrap_or_default();
        let theme = lite_db.as_ref().and_then(|db| db.meta_get("theme"));
        cc.egui_ctx.set_theme(match theme.as_deref() {
            Some("dark") => egui::Theme::Dark,
            _ => egui::Theme::Light,
        });
        if let Some(zoom) = lite_db
            .as_ref()
            .and_then(|db| db.meta_get("zoom"))
            .and_then(|z| z.parse::<f32>().ok())
        {
            cc.egui_ctx.set_zoom_factor(zoom.clamp(0.6, 2.0));
        }
        let hidden: HashSet<String> = lite_db
            .as_ref()
            .and_then(|db| db.meta_get("hidden_cols"))
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default();
        let visible_cols = RESULT_COLUMNS
            .iter()
            .map(|name| !hidden.contains(*name))
            .collect();

        let (msg_tx, msg_rx) = channel::<Msg>();
        let (search_tx, search_rx) = channel::<WorkerReq>();
        workers::spawn_search_worker(
            db_path.clone(),
            search_rx,
            msg_tx.clone(),
            cc.egui_ctx.clone(),
        );

        let mut app = App {
            lang,
            db_path,
            lite_db,
            query_text: String::new(),
            filters: Filters::default(),
            show_filters: false,
            active_query: Query::default(),
            page: 0,
            total: None,
            rows: Vec::new(),
            row_ids: Vec::new(),
            analytics: None,
            show_analytics: false,
            selected: HashSet::new(),
            select_anchor: None,
            visible_cols,
            search_gen: 0,
            search_in_flight: false,
            last_search_ms: None,
            db_total_rows: None,
            status: StatusLine::default(),
            op: None,
            import_report: None,
            card: None,
            card_open: false,
            show_settings: false,
            confirm_clear: false,
            msg_rx,
            msg_tx,
            search_tx,
        };
        let _ = app.search_tx.send(WorkerReq::Stats);
        app.start_search(true);

        // Repair the search index if the previous run was interrupted.
        if let Some(db) = &app.lite_db
            && db.unindexed_rows() > 0
        {
            let cancel = Arc::new(AtomicBool::new(false));
            app.op = Some(OpState {
                kind: OpKind::Import,
                cancel: cancel.clone(),
                last_event: None,
                export_progress: (0, 0),
            });
            workers::spawn_index_repair(
                app.db_path.clone(),
                cancel,
                app.msg_tx.clone(),
                cc.egui_ctx.clone(),
            );
        }
        app
    }

    fn t(&self) -> &'static Tr {
        tr(self.lang)
    }

    fn persist(&self, key: &str, value: &str) {
        if let Some(db) = &self.lite_db {
            db.meta_set(key, value);
        }
    }

    fn persist_hidden_cols(&self) {
        let hidden: Vec<&str> = RESULT_COLUMNS
            .iter()
            .zip(&self.visible_cols)
            .filter(|(_, v)| !**v)
            .map(|(n, _)| *n)
            .collect();
        self.persist("hidden_cols", &hidden.join(","));
    }

    fn start_search(&mut self, reset_page: bool) {
        if reset_page {
            self.page = 0;
        }
        self.active_query = Query {
            text: self.query_text.clone(),
            filters: self.filters.clone(),
        };
        self.search_gen += 1;
        self.search_in_flight = true;
        if self.show_analytics {
            self.analytics = None;
        }
        let _ = self.search_tx.send(WorkerReq::Search {
            q: Box::new(self.active_query.clone()),
            page: self.page,
            generation: self.search_gen,
            include_analytics: self.show_analytics,
        });
    }

    fn goto_page(&mut self, page: u64) {
        self.page = page;
        self.search_gen += 1;
        self.search_in_flight = true;
        let _ = self.search_tx.send(WorkerReq::Search {
            q: Box::new(self.active_query.clone()),
            page,
            generation: self.search_gen,
            include_analytics: false,
        });
    }

    fn page_count(&self) -> u64 {
        self.total
            .map(|t| t.div_ceil(PAGE_SIZE).max(1))
            .unwrap_or(1)
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                Msg::SearchDone {
                    generation,
                    ids,
                    rows,
                    total,
                    analytics,
                    ms,
                } => {
                    if generation == self.search_gen {
                        self.row_ids = ids;
                        self.rows = rows;
                        self.total = Some(total);
                        if let Some(analytics) = analytics {
                            self.analytics = Some(*analytics);
                        } else if !self.show_analytics || self.active_query.is_empty() {
                            self.analytics = None;
                        }
                        self.last_search_ms = Some(ms);
                        self.search_in_flight = false;
                        self.selected.clear();
                        self.select_anchor = None;
                    }
                }
                Msg::SearchError {
                    generation,
                    message,
                } => {
                    if generation == self.search_gen {
                        self.search_in_flight = false;
                        self.status = StatusLine {
                            text: format!("{}: {message}", self.t().error),
                            is_error: true,
                        };
                    }
                }
                Msg::Stats(total) => self.db_total_rows = Some(total),
                Msg::Import(ev) => {
                    if let Some(op) = &mut self.op {
                        op.last_event = Some(ev);
                    }
                }
                Msg::ImportDone(summaries, total_rows) => {
                    self.op = None;
                    self.db_total_rows = Some(total_rows);
                    if !summaries.is_empty() {
                        let imported: u64 = summaries.iter().map(|s| s.imported).sum();
                        let dups: u64 = summaries.iter().map(|s| s.duplicates).sum();
                        let errors = summaries.iter().filter(|s| s.error.is_some()).count();
                        self.status = StatusLine {
                            text: fmt(
                                self.t().import_done,
                                &[
                                    &group_digits(imported),
                                    &group_digits(dups),
                                    &errors.to_string(),
                                ],
                            ),
                            is_error: errors > 0,
                        };
                        self.import_report = Some(summaries);
                    }
                    let _ = self.search_tx.send(WorkerReq::Stats);
                    self.start_search(true);
                }
                Msg::ExportProgress(done, total) => {
                    if let Some(op) = &mut self.op {
                        op.export_progress = (done, total);
                    }
                }
                Msg::ExportDone(result) => {
                    self.op = None;
                    self.status = match result {
                        Ok((written, path)) => StatusLine {
                            text: format!(
                                "{} \u{2192} {}",
                                fmt(self.t().export_done, &[&group_digits(written)]),
                                path.display()
                            ),
                            is_error: false,
                        },
                        Err(ExportError::TooManyRowsForXlsx(_)) => StatusLine {
                            text: self.t().xlsx_limit.to_string(),
                            is_error: true,
                        },
                        Err(ExportError::Cancelled) => StatusLine {
                            text: self.t().cancelled.to_string(),
                            is_error: false,
                        },
                        Err(ExportError::Other(e)) => StatusLine {
                            text: format!("{}: {e}", self.t().error),
                            is_error: true,
                        },
                    };
                }
                Msg::DbCleared(result) => {
                    self.op = None;
                    self.status = match result {
                        Ok(()) => StatusLine {
                            text: self.t().db_cleared.to_string(),
                            is_error: false,
                        },
                        Err(e) => StatusLine {
                            text: format!("{}: {e}", self.t().error),
                            is_error: true,
                        },
                    };
                    let _ = self.search_tx.send(WorkerReq::Stats);
                    self.start_search(true);
                }
                Msg::Fatal(message) => {
                    self.status = StatusLine {
                        text: format!("{}: {message}", self.t().error),
                        is_error: true,
                    };
                }
            }
        }
    }

    fn pick_and_import(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let files = rfd::FileDialog::new()
            .set_title(t.choose_files)
            .add_filter(t.excel_files, &["xlsx", "xlsb", "xls"])
            .pick_files();
        let Some(files) = files else { return };
        if files.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.op = Some(OpState {
            kind: OpKind::Import,
            cancel: cancel.clone(),
            last_event: None,
            export_progress: (0, 0),
        });
        self.status = StatusLine::default();
        workers::spawn_import(
            self.db_path.clone(),
            files,
            cancel,
            self.msg_tx.clone(),
            ctx.clone(),
        );
    }

    fn pick_and_export(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let dest = rfd::FileDialog::new()
            .set_title(t.save_as)
            .add_filter("CSV", &["csv"])
            .add_filter("Excel", &["xlsx"])
            .set_file_name("base_search_export.csv")
            .save_file();
        let Some(mut dest) = dest else { return };
        if dest.extension().is_none() {
            dest.set_extension("csv");
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.op = Some(OpState {
            kind: OpKind::Export,
            cancel: cancel.clone(),
            last_event: None,
            export_progress: (0, 0),
        });
        self.status = StatusLine::default();
        workers::spawn_export(
            self.db_path.clone(),
            self.active_query.clone(),
            dest,
            cancel,
            self.msg_tx.clone(),
            ctx.clone(),
        );
    }

    fn start_clear_db(&mut self, ctx: &egui::Context) {
        self.op = Some(OpState {
            kind: OpKind::Clear,
            cancel: Arc::new(AtomicBool::new(false)),
            last_event: None,
            export_progress: (0, 0),
        });
        self.status = StatusLine::default();
        workers::spawn_clear_db(self.db_path.clone(), self.msg_tx.clone(), ctx.clone());
    }

    fn open_card(&mut self, row_index: usize) {
        let Some(id) = self.row_ids.get(row_index).copied() else {
            return;
        };
        if let Some(db) = &self.lite_db
            && let Ok(card) = db.record_card(id)
        {
            self.card = Some(card);
            self.card_open = true;
        }
    }

    fn handle_row_click(&mut self, i: usize, modifiers: egui::Modifiers) {
        if modifiers.ctrl || modifiers.command {
            if !self.selected.insert(i) {
                self.selected.remove(&i);
            }
            self.select_anchor = Some(i);
        } else if modifiers.shift && self.select_anchor.is_some() {
            let anchor = self.select_anchor.unwrap();
            let (lo, hi) = (anchor.min(i), anchor.max(i));
            self.selected = (lo..=hi).collect();
        } else {
            self.selected.clear();
            self.selected.insert(i);
            self.select_anchor = Some(i);
        }
    }

    /// Copies selected rows as TSV using visible columns, ready to paste into Excel.
    fn copy_selected_rows(&self, ctx: &egui::Context) {
        let mut indices: Vec<usize> = self.selected.iter().copied().collect();
        indices.sort_unstable();
        let lines: Vec<String> = indices
            .iter()
            .filter_map(|i| self.rows.get(*i))
            .map(|row| {
                row.iter()
                    .zip(&self.visible_cols)
                    .filter(|(_, v)| **v)
                    .map(|(value, _)| value.as_str())
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect();
        if !lines.is_empty() {
            ctx.copy_text(lines.join("\n"));
        }
    }

    fn apply_menu_action(&mut self, ctx: &egui::Context, action: RowMenuAction) {
        let quick_filter = |this: &mut Self, set: &dyn Fn(&mut Filters, String), value: String| {
            this.query_text.clear();
            this.filters.clear();
            set(&mut this.filters, value);
            this.show_filters = true;
            this.start_search(true);
        };
        match action {
            RowMenuAction::CopyCell(value) => ctx.copy_text(value),
            RowMenuAction::CopyRow(i) => {
                if let Some(row) = self.rows.get(i) {
                    ctx.copy_text(row.join("\t"));
                }
            }
            RowMenuAction::CopySelected => self.copy_selected_rows(ctx),
            RowMenuAction::FilterSender(v) => {
                quick_filter(self, &|f, v| f.sender = v, v);
            }
            RowMenuAction::FilterRecipient(v) => {
                quick_filter(self, &|f, v| f.recipient = v, v);
            }
            RowMenuAction::FilterCode(v) => {
                quick_filter(self, &|f, v| f.product_code = v, v);
            }
            RowMenuAction::FilterEdrpou(v) => {
                quick_filter(self, &|f, v| f.edrpou = v, v);
            }
        }
    }

    // ---------- panels ----------

    fn ui_toolbar(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let t = self.t();
        let mut do_search = false;
        let mut do_import = false;
        let mut do_export = false;
        let mut analytics_toggled = false;
        let frame = egui::Frame::side_top_panel(&ctx.global_style()).inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 10,
            bottom: 8,
        });
        egui::Panel::top("toolbar")
            .frame(frame)
            .show_inside(root, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(t.app_title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("\u{2699}").on_hover_text(t.settings).clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        ui.separator();
                        if let Some(total) = self.db_total_rows {
                            ui.label(
                                egui::RichText::new(fmt(t.db_rows, &[&group_digits(total)])).weak(),
                            );
                        }
                    });
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    let busy = self.op.is_some();
                    if ui.add_enabled(!busy, egui::Button::new(t.import)).clicked() {
                        do_import = true;
                    }
                    let can_export = !busy && self.total.unwrap_or(0) > 0;
                    if ui
                        .add_enabled(can_export, egui::Button::new(t.export))
                        .clicked()
                    {
                        do_export = true;
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button(t.columns_btn, |ui| {
                            for (i, name) in RESULT_COLUMNS.iter().enumerate() {
                                let mut v = self.visible_cols[i];
                                if ui.checkbox(&mut v, header_for(name)).changed() {
                                    let visible_count =
                                        self.visible_cols.iter().filter(|x| **x).count();
                                    if v || visible_count > 1 {
                                        self.visible_cols[i] = v;
                                        self.persist_hidden_cols();
                                    }
                                }
                            }
                        });
                        let analytics_btn = ui.selectable_label(self.show_analytics, t.analytics);
                        if analytics_btn.clicked() {
                            self.show_analytics = !self.show_analytics;
                            analytics_toggled = true;
                            if !self.show_analytics {
                                self.analytics = None;
                            }
                        }
                        let filters_btn = ui.selectable_label(self.show_filters, t.filters);
                        if filters_btn.clicked() {
                            self.show_filters = !self.show_filters;
                        }
                        let find_btn = egui::Button::new(
                            egui::RichText::new(t.find).color(egui::Color32::WHITE),
                        )
                        .fill(ACCENT);
                        if ui.add(find_btn).clicked() {
                            do_search = true;
                        }
                        let edit = egui::TextEdit::singleline(&mut self.query_text)
                            .hint_text(t.search_hint)
                            .desired_width(ui.available_width());
                        let response = ui.add(edit);
                        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            do_search = true;
                        }
                    });
                });

                if self.show_filters {
                    ui.add_space(6.0);
                    if self.ui_filters(ui) {
                        do_search = true;
                    }
                }
                ui.add_space(2.0);
            });
        if do_search {
            self.start_search(true);
        } else if analytics_toggled && self.show_analytics {
            self.start_search(false);
        }
        if do_import {
            self.pick_and_import(&ctx);
        }
        if do_export {
            self.pick_and_export(&ctx);
        }
    }

    /// Renders filters and returns true when a search should be started.
    fn ui_filters(&mut self, ui: &mut egui::Ui) -> bool {
        let t = self.t();
        let mut search = false;
        ui.horizontal_wrapped(|ui| {
            filter_field(ui, t.year, &mut self.filters.year, 60.0, &mut search);
            filter_field(
                ui,
                t.product_code,
                &mut self.filters.product_code,
                110.0,
                &mut search,
            );
            filter_field(ui, t.edrpou, &mut self.filters.edrpou, 100.0, &mut search);
            filter_field(ui, t.sender, &mut self.filters.sender, 180.0, &mut search);
            filter_field(
                ui,
                t.recipient,
                &mut self.filters.recipient,
                180.0,
                &mut search,
            );
            filter_field(
                ui,
                t.trade_country,
                &mut self.filters.trade_country,
                80.0,
                &mut search,
            );
            filter_field(
                ui,
                t.dispatch_country,
                &mut self.filters.dispatch_country,
                80.0,
                &mut search,
            );
            filter_field(
                ui,
                t.origin_country,
                &mut self.filters.origin_country,
                80.0,
                &mut search,
            );
            ui.vertical(|ui| {
                ui.label(" ");
                if ui.button(t.clear_filters).clicked() {
                    self.filters.clear();
                    search = true;
                }
            });
        });
        search
    }

    fn ui_status_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status").show_inside(root, |ui| {
            ui.add_space(4.0);
            if self.op.is_some() {
                self.ui_progress(ui);
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                if self.search_in_flight {
                    ui.spinner();
                    ui.label(self.t().searching);
                } else if !self.status.text.is_empty() {
                    let color = if self.status.is_error {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.colored_label(color, &self.status.text);
                } else if let (Some(total), Some(ms)) = (self.total, self.last_search_ms) {
                    let start = self.page * PAGE_SIZE + 1;
                    let end = (self.page * PAGE_SIZE + self.rows.len() as u64).min(total);
                    if total > 0 {
                        let mut text = fmt(
                            self.t().rows_of,
                            &[
                                &group_digits(start),
                                &group_digits(end),
                                &group_digits(total),
                            ],
                        );
                        text.push_str("  \u{00B7}  ");
                        text.push_str(&fmt(self.t().search_ms, &[&ms.to_string()]));
                        if self.selected.len() > 1 {
                            text.push_str("  \u{00B7}  ");
                            text.push_str(&fmt(
                                self.t().selected_n,
                                &[&self.selected.len().to_string()],
                            ));
                        }
                        ui.label(text);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.ui_pagination(ui);
                });
            });
            ui.add_space(4.0);
        });
    }

    fn ui_progress(&mut self, ui: &mut egui::Ui) {
        let Some(op) = &self.op else { return };
        let t = self.t();
        let mut cancel_clicked = false;
        ui.horizontal(|ui| {
            match op.kind {
                OpKind::Clear => {
                    ui.spinner();
                    ui.label(t.clearing);
                }
                OpKind::Export => {
                    let (done, total) = op.export_progress;
                    ui.label(t.exporting);
                    let frac = if total > 0 {
                        done as f32 / total as f32
                    } else {
                        0.0
                    };
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_width(ui.available_width() - 110.0)
                            .text(format!("{} / {}", group_digits(done), group_digits(total))),
                    );
                }
                OpKind::Import => {
                    if let Some(ev) = &op.last_event {
                        let phase = match ev.phase {
                            ImportPhase::Reading => t.reading_file,
                            ImportPhase::Inserting => t.writing_rows,
                            ImportPhase::Indexing => t.indexing,
                        };
                        let label = if ev.file_name.is_empty() {
                            phase.to_string()
                        } else {
                            format!(
                                "{} \u{00B7} {}",
                                fmt(
                                    t.file_of,
                                    &[
                                        &ev.file_idx.to_string(),
                                        &ev.file_count.to_string(),
                                        &ev.file_name
                                    ]
                                ),
                                phase
                            )
                        };
                        ui.label(label);
                        if ev.total > 0 {
                            let frac = ev.done as f32 / ev.total as f32;
                            ui.add(
                                egui::ProgressBar::new(frac)
                                    .desired_width(ui.available_width() - 110.0)
                                    .text(format!(
                                        "{} / {}",
                                        group_digits(ev.done),
                                        group_digits(ev.total)
                                    )),
                            );
                        } else {
                            ui.spinner();
                            if ev.done > 0 {
                                ui.label(group_digits(ev.done));
                            }
                        }
                    } else {
                        ui.spinner();
                        ui.label(t.reading_file);
                    }
                }
            }
            if op.kind != OpKind::Clear {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t.cancel).clicked() {
                        cancel_clicked = true;
                    }
                });
            }
        });
        if cancel_clicked && let Some(op) = &self.op {
            op.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn ui_pagination(&mut self, ui: &mut egui::Ui) {
        let pages = self.page_count();
        let page = self.page;
        let mut goto: Option<u64> = None;
        // right_to_left draws from the end.
        if ui
            .add_enabled(page + 1 < pages, egui::Button::new("⏭"))
            .clicked()
        {
            goto = Some(pages - 1);
        }
        if ui
            .add_enabled(page + 1 < pages, egui::Button::new("▶"))
            .clicked()
        {
            goto = Some(page + 1);
        }
        ui.label(format!(
            "{} / {}",
            group_digits(page + 1),
            group_digits(pages)
        ));
        if ui.add_enabled(page > 0, egui::Button::new("◀")).clicked() {
            goto = Some(page - 1);
        }
        if ui.add_enabled(page > 0, egui::Button::new("⏮")).clicked() {
            goto = Some(0);
        }
        if let Some(p) = goto {
            self.goto_page(p);
        }
    }

    fn ui_analytics_panel(&mut self, root: &mut egui::Ui) {
        if !self.show_analytics {
            return;
        }
        egui::Panel::right("analytics_panel")
            .resizable(true)
            .default_size(380.0)
            .size_range(300.0..=560.0)
            .show_inside(root, |ui| {
                let t = self.t();
                ui.horizontal(|ui| {
                    ui.heading(t.analytics);
                    if self.search_in_flight {
                        ui.spinner();
                    }
                });
                ui.separator();

                if self.active_query.is_empty() {
                    ui.label(egui::RichText::new(t.analytics_hint).weak());
                    return;
                }

                let Some(analytics) = &self.analytics else {
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t.searching);
                    });
                    return;
                };

                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("analytics_overview")
                        .num_columns(2)
                        .striped(true)
                        .spacing([16.0, 7.0])
                        .show(ui, |ui| {
                            metric(ui, t.rows_label, group_digits(analytics.overview.row_count));
                            metric(
                                ui,
                                t.unique_senders,
                                group_digits(analytics.overview.distinct_senders),
                            );
                            metric(
                                ui,
                                t.unique_recipients,
                                group_digits(analytics.overview.distinct_recipients),
                            );
                            metric(
                                ui,
                                t.unique_edrpou,
                                group_digits(analytics.overview.distinct_edrpou),
                            );
                            metric(
                                ui,
                                t.unique_trademarks,
                                group_digits(analytics.overview.distinct_trademarks),
                            );
                            metric(
                                ui,
                                t.total_value,
                                format!("{} $", fmt_decimal(analytics.overview.total_value_usd, 2)),
                            );
                            metric(
                                ui,
                                t.gross_weight,
                                fmt_decimal(analytics.overview.total_gross_kg, 3),
                            );
                            metric(
                                ui,
                                t.net_weight,
                                fmt_decimal(analytics.overview.total_net_kg, 3),
                            );
                            metric(
                                ui,
                                t.quantity,
                                fmt_decimal(analytics.overview.total_quantity, 3),
                            );
                        });

                    analytics_group_table(ui, t.top_recipients, &analytics.top_recipients);
                    analytics_group_table(ui, t.top_senders, &analytics.top_senders);
                    analytics_group_table(ui, t.top_trademarks, &analytics.top_trademarks);
                    analytics_group_table(ui, t.top_product_codes, &analytics.top_product_codes);
                    analytics_group_table(
                        ui,
                        t.top_origin_countries,
                        &analytics.top_origin_countries,
                    );
                });
            });
    }

    fn ui_table(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(root, |ui| {
            if self.rows.is_empty() {
                let text = match self.total {
                    Some(0) if self.active_query.is_empty() => self.t().db_empty,
                    Some(0) => self.t().nothing_found,
                    _ => self.t().enter_query_hint,
                };
                ui.add_space((ui.available_height() * 0.35).max(0.0));
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("🔍").size(42.0).weak());
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(text).size(16.0).weak());
                });
                return;
            }
            let visible: Vec<usize> = (0..RESULT_COLUMNS.len())
                .filter(|i| self.visible_cols[*i])
                .collect();
            // Read modifiers from the click event itself: the keyboard state at
            // frame time may no longer contain Shift/Ctrl.
            let modifiers = ui.input(|i| {
                i.events
                    .iter()
                    .rev()
                    .find_map(|e| match e {
                        egui::Event::PointerButton {
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers,
                            ..
                        } => Some(*modifiers),
                        _ => None,
                    })
                    .unwrap_or(i.modifiers)
            });
            let dark = ui.visuals().dark_mode;
            let code_color = if dark {
                egui::Color32::from_rgb(132, 170, 255)
            } else {
                ACCENT
            };
            let mut open_card: Option<usize> = None;
            let mut clicked_row: Option<usize> = None;
            let mut menu_action: Option<RowMenuAction> = None;
            let t = self.t();
            let n_selected = self.selected.len();
            let text_height = egui::TextStyle::Body.resolve(ui.style()).size + 9.0;
            egui::ScrollArea::horizontal().show(ui, |ui| {
                let mut table = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(0.0);
                for idx in &visible {
                    let (width, _) = col_spec(RESULT_COLUMNS[*idx]);
                    table = table.column(Column::initial(width).at_least(40.0).clip(true));
                }
                table
                    .header(28.0, |mut header| {
                        for idx in &visible {
                            let name = RESULT_COLUMNS[*idx];
                            let (_, kind) = col_spec(name);
                            header.col(|ui| {
                                if kind == CellKind::Number {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.strong(header_for(name));
                                        },
                                    );
                                } else {
                                    ui.strong(header_for(name));
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(text_height, self.rows.len(), |mut row| {
                            let i = row.index();
                            row.set_selected(self.selected.contains(&i));
                            let mut clicked = false;
                            let mut double = false;
                            for idx in &visible {
                                let value = &self.rows[i][*idx];
                                let (_, kind) = col_spec(RESULT_COLUMNS[*idx]);
                                let (_, response) = row.col(|ui| {
                                    let rich = match kind {
                                        CellKind::Normal => egui::RichText::new(value),
                                        CellKind::Weak => egui::RichText::new(value).weak(),
                                        CellKind::Code => {
                                            egui::RichText::new(value).monospace().color(code_color)
                                        }
                                        CellKind::Number => egui::RichText::new(value).monospace(),
                                    };
                                    let label = egui::Label::new(rich).selectable(false).truncate();
                                    if kind == CellKind::Number {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.add(label);
                                            },
                                        );
                                    } else {
                                        ui.add(label);
                                    }
                                });
                                clicked |= response.clicked();
                                double |= response.double_clicked();
                                response.context_menu(|ui| {
                                    let cells = &self.rows[i];
                                    if n_selected > 1
                                        && ui
                                            .button(fmt(
                                                t.copy_selected,
                                                &[&n_selected.to_string()],
                                            ))
                                            .clicked()
                                    {
                                        menu_action = Some(RowMenuAction::CopySelected);
                                        ui.close();
                                    }
                                    if ui.button(t.copy_value).clicked() {
                                        menu_action = Some(RowMenuAction::CopyCell(value.clone()));
                                        ui.close();
                                    }
                                    if ui.button(t.copy_row).clicked() {
                                        menu_action = Some(RowMenuAction::CopyRow(i));
                                        ui.close();
                                    }
                                    ui.separator();
                                    let quick: [QuickAction; 4] = [
                                        (t.flt_sender, "sender", RowMenuAction::FilterSender),
                                        (
                                            t.flt_recipient,
                                            "recipient",
                                            RowMenuAction::FilterRecipient,
                                        ),
                                        (t.flt_code, "product_code", RowMenuAction::FilterCode),
                                        (t.flt_edrpou, "edrpou", RowMenuAction::FilterEdrpou),
                                    ];
                                    for (label, column, make) in quick {
                                        let Some(col) = result_col_index(column) else {
                                            continue;
                                        };
                                        let cell = cells[col].trim();
                                        if cell.is_empty() {
                                            continue;
                                        }
                                        let text = format!("{label}: {}", trunc_label(cell, 24));
                                        if ui.button(text).clicked() {
                                            menu_action = Some(make(cell.to_string()));
                                            ui.close();
                                        }
                                    }
                                });
                            }
                            if double {
                                open_card = Some(i);
                            } else if clicked {
                                clicked_row = Some(i);
                            }
                        });
                    });
            });
            if let Some(i) = clicked_row {
                self.handle_row_click(i, modifiers);
            }
            if let Some(i) = open_card {
                self.selected.clear();
                self.selected.insert(i);
                self.select_anchor = Some(i);
                self.open_card(i);
            }
            if let Some(action) = menu_action {
                let ctx = ui.ctx().clone();
                self.apply_menu_action(&ctx, action);
            }
        });
    }

    fn ui_card_window(&mut self, ctx: &egui::Context) {
        if !self.card_open {
            return;
        }
        let t = self.t();
        let mut open = self.card_open;
        if let Some(card) = &self.card {
            egui::Window::new(t.details)
                .open(&mut open)
                .default_size([640.0, 660.0])
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}: {}", t.file_col, card.source_file))
                                .weak(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(t.copy_all).clicked() {
                                let text: String = card
                                    .fields
                                    .iter()
                                    .filter(|(_, v)| !v.is_empty())
                                    .map(|(h, v)| format!("{h}: {v}"))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                ctx.copy_text(text);
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        egui::Grid::new("card_grid")
                            .num_columns(2)
                            .striped(true)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                for (header, value) in &card.fields {
                                    ui.label(egui::RichText::new(*header).strong());
                                    if value.is_empty() {
                                        ui.label(egui::RichText::new("\u{2014}").weak());
                                    } else {
                                        ui.add(egui::Label::new(value).wrap());
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                });
        }
        self.card_open = open;
        if !self.card_open {
            self.card = None;
        }
    }

    fn ui_import_report(&mut self, ctx: &egui::Context) {
        let Some(report) = &self.import_report else {
            return;
        };
        let t = self.t();
        let mut open = true;
        egui::Window::new(t.import_report)
            .open(&mut open)
            .default_width(560.0)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("report_grid")
                    .num_columns(2)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for s in report {
                            ui.label(egui::RichText::new(&s.file_name).strong());
                            if let Some(err) = &s.error {
                                let text = if let Some(cols) = err.strip_prefix("__MISSING__") {
                                    fmt(t.missing_cols, &[cols])
                                } else {
                                    err.clone()
                                };
                                ui.colored_label(ui.visuals().error_fg_color, text);
                            } else if let Some(previous) = &s.skipped_duplicate_of {
                                ui.label(
                                    egui::RichText::new(fmt(t.file_skipped, &[previous])).weak(),
                                );
                            } else {
                                let mut text = fmt(
                                    t.file_result,
                                    &[
                                        &group_digits(s.imported),
                                        &group_digits(s.duplicates),
                                        &format!("{:.1}", s.seconds),
                                    ],
                                );
                                if s.cancelled {
                                    text.push_str(" \u{00B7} ");
                                    text.push_str(t.cancelled);
                                }
                                ui.label(text);
                            }
                            ui.end_row();
                        }
                    });
            });
        if !open {
            self.import_report = None;
        }
    }

    fn ui_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let t = self.t();
        let mut open = true;
        egui::Window::new(format!("\u{2699} {}", t.settings))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("settings_grid")
                    .num_columns(2)
                    .spacing([24.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(t.language);
                        let mut lang = self.lang;
                        egui::ComboBox::from_id_salt("settings_lang")
                            .width(150.0)
                            .selected_text(lang.label())
                            .show_ui(ui, |ui| {
                                for l in Lang::ALL {
                                    ui.selectable_value(&mut lang, l, l.label());
                                }
                            });
                        if lang != self.lang {
                            self.lang = lang;
                            self.persist("lang", lang.code());
                        }
                        ui.end_row();

                        ui.label(t.theme_label);
                        ui.horizontal(|ui| {
                            let dark = ui.visuals().dark_mode;
                            if ui.selectable_label(!dark, t.theme_light).clicked() && dark {
                                ctx.set_theme(egui::Theme::Light);
                                self.persist("theme", "light");
                            }
                            if ui.selectable_label(dark, t.theme_dark).clicked() && !dark {
                                ctx.set_theme(egui::Theme::Dark);
                                self.persist("theme", "dark");
                            }
                        });
                        ui.end_row();

                        ui.label(t.zoom_label);
                        ui.horizontal(|ui| {
                            let zoom = ctx.zoom_factor();
                            let mut new_zoom = zoom;
                            if ui.button("\u{2212}").clicked() {
                                new_zoom = (zoom - 0.1).max(0.6);
                            }
                            ui.label(format!("{:.0}%", zoom * 100.0));
                            if ui.button("+").clicked() {
                                new_zoom = (zoom + 0.1).min(2.0);
                            }
                            if (new_zoom - zoom).abs() > f32::EPSILON {
                                ctx.set_zoom_factor(new_zoom);
                                self.persist("zoom", &format!("{new_zoom:.2}"));
                            }
                            ui.label(egui::RichText::new("Ctrl + / \u{2212}").weak().small());
                        });
                        ui.end_row();
                    });

                ui.separator();
                ui.label(egui::RichText::new(t.db_section).strong());
                ui.add_space(4.0);
                egui::Grid::new("settings_db_grid")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(t.db_file_label).weak());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(self.db_path.display().to_string()).small(),
                            )
                            .wrap(),
                        );
                        ui.end_row();
                        ui.label(egui::RichText::new(t.db_size_label).weak());
                        let size = std::fs::metadata(&self.db_path)
                            .map(|m| m.len())
                            .unwrap_or(0);
                        ui.label(format!("{:.2} GB", size as f64 / (1u64 << 30) as f64));
                        ui.end_row();
                    });
                ui.add_space(8.0);
                let busy = self.op.is_some();
                let clear_btn =
                    egui::Button::new(egui::RichText::new(t.clear_db).color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(200, 50, 50));
                if ui.add_enabled(!busy, clear_btn).clicked() {
                    self.confirm_clear = true;
                }
                ui.add_space(6.0);
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{}: {APP_VERSION}", t.version_label))
                        .weak()
                        .small(),
                );
            });
        self.show_settings = open;
    }

    fn ui_confirm_clear(&mut self, ctx: &egui::Context) {
        if !self.confirm_clear {
            return;
        }
        let t = self.t();
        egui::Window::new(t.clear_db)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(t.clear_confirm);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let yes_btn = egui::Button::new(
                        egui::RichText::new(t.clear_yes).color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(200, 50, 50));
                    if ui.add(yes_btn).clicked() {
                        self.confirm_clear = false;
                        self.show_settings = false;
                        self.start_clear_db(ctx);
                    }
                    if ui.button(t.cancel).clicked() {
                        self.confirm_clear = false;
                    }
                });
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.drain_messages();
        // Ctrl+C copies selected rows when focus is not inside a text field.
        let copy_requested = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if copy_requested && !self.selected.is_empty() && !self.card_open {
            self.copy_selected_rows(&ctx);
        }
        self.ui_toolbar(root);
        self.ui_status_bar(root);
        self.ui_analytics_panel(root);
        self.ui_table(root);
        self.ui_card_window(&ctx);
        self.ui_import_report(&ctx);
        self.ui_settings_window(&ctx);
        self.ui_confirm_clear(&ctx);
        // Safety repaint: refresh regularly while a background operation runs.
        if self.op.is_some() || self.search_in_flight {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    ui.label(egui::RichText::new(label).weak());
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(egui::RichText::new(value).strong().monospace());
    });
    ui.end_row();
}

fn analytics_group_table(ui: &mut egui::Ui, title: &str, rows: &[AnalyticsGroupRow]) {
    ui.add_space(14.0);
    ui.label(egui::RichText::new(title).strong());
    ui.add_space(3.0);
    if rows.is_empty() {
        ui.label(egui::RichText::new("\u{2014}").weak());
        return;
    }
    egui::Grid::new(("analytics_group", title))
        .num_columns(3)
        .striped(true)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            for row in rows {
                ui.add(
                    egui::Label::new(egui::RichText::new(&row.label))
                        .truncate()
                        .selectable(false),
                )
                .on_hover_text(&row.label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(group_digits(row.rows)).monospace());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{} $", fmt_decimal(row.total_value_usd, 2)))
                            .monospace(),
                    );
                });
                ui.end_row();
            }
        });
}

fn fmt_decimal(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let mut s = format!("{value:.decimals$}");
    if let Some(dot) = s.find('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.len() == dot + 1 {
            s.pop();
        }
    }
    let (sign, body) = s
        .strip_prefix('-')
        .map(|rest| ("-", rest))
        .unwrap_or(("", s.as_str()));
    let (int_part, frac_part) = body.split_once('.').unwrap_or((body, ""));
    let mut grouped = String::with_capacity(s.len() + s.len() / 3);
    grouped.push_str(sign);
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i).is_multiple_of(3) {
            grouped.push('\u{202F}');
        }
        grouped.push(ch);
    }
    if !frac_part.is_empty() {
        grouped.push('.');
        grouped.push_str(frac_part);
    }
    grouped
}

fn result_col_index(name: &str) -> Option<usize> {
    RESULT_COLUMNS.iter().position(|column| *column == name)
}

fn filter_field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32, search: &mut bool) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().weak());
        let response = ui.add(egui::TextEdit::singleline(value).desired_width(width));
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            *search = true;
        }
    });
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // System Segoe UI: native look and complete Cyrillic coverage.
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\segoeui.ttf") {
        fonts.font_data.insert(
            "segoe".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe".to_owned());
    }
    // Consolas for codes and numbers.
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\consola.ttf") {
        fonts.font_data.insert(
            "consolas".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "consolas".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.5, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(14.5, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(19.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(13.5, FontFamily::Monospace),
        );
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        style.visuals.hyperlink_color = ACCENT;
        style.visuals.slider_trailing_fill = true;
    });
    // Table striping with more contrast than the default.
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals.faint_bg_color = egui::Color32::from_gray(34);
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.visuals.faint_bg_color = egui::Color32::from_gray(244);
    });
}
