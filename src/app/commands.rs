use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::App;
use super::state::{OpKind, OpState, StatusLine};
use crate::workers;

pub(super) fn pick_and_import(app: &mut App, ctx: &egui::Context) {
    let t = app.t();
    let files = rfd::FileDialog::new()
        .set_title(t.choose_files)
        .add_filter(t.excel_files, &["xlsx", "xlsb", "xls"])
        .pick_files();
    let Some(files) = files else { return };
    if files.is_empty() {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    app.op = Some(OpState {
        kind: OpKind::Import,
        cancel: cancel.clone(),
        last_event: None,
        export_progress: (0, 0),
    });
    app.status = StatusLine::default();
    workers::spawn_import(
        app.db_path.clone(),
        files,
        cancel,
        app.msg_tx.clone(),
        ctx.clone(),
    );
}

pub(super) fn pick_and_export(app: &mut App, ctx: &egui::Context) {
    let t = app.t();
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
    app.op = Some(OpState {
        kind: OpKind::Export,
        cancel: cancel.clone(),
        last_event: None,
        export_progress: (0, 0),
    });
    app.status = StatusLine::default();
    workers::spawn_export(
        app.db_path.clone(),
        app.active_query.clone(),
        dest,
        cancel,
        app.msg_tx.clone(),
        ctx.clone(),
    );
}

pub(super) fn save_report_html(app: &mut App, html: String) {
    let dest = rfd::FileDialog::new()
        .set_title("Export report")
        .add_filter("HTML report", &["html"])
        .set_file_name("base_search_report.html")
        .save_file();
    let Some(mut dest) = dest else { return };
    if dest.extension().is_none() {
        dest.set_extension("html");
    }
    match std::fs::write(&dest, html) {
        Ok(()) => {
            app.status = StatusLine {
                text: format!("Report exported: {}", dest.display()),
                is_error: false,
            };
        }
        Err(err) => {
            app.status = StatusLine {
                text: format!("{}: {err}", app.t().error),
                is_error: true,
            };
        }
    }
}

pub(super) fn start_clear_db(app: &mut App, ctx: &egui::Context) {
    app.op = Some(OpState {
        kind: OpKind::Clear,
        cancel: Arc::new(AtomicBool::new(false)),
        last_event: None,
        export_progress: (0, 0),
    });
    app.status = StatusLine::default();
    workers::spawn_clear_db(app.db_path.clone(), app.msg_tx.clone(), ctx.clone());
}

pub(super) fn start_optimize_db(app: &mut App, ctx: &egui::Context) {
    app.op = Some(OpState {
        kind: OpKind::Maintenance,
        cancel: Arc::new(AtomicBool::new(false)),
        last_event: None,
        export_progress: (0, 0),
    });
    app.status = StatusLine::default();
    workers::spawn_optimize_db(app.db_path.clone(), app.msg_tx.clone(), ctx.clone());
}
