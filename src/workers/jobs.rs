use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::time::Instant;

use super::{ImportEvent, Msg};
use crate::db::{Db, Query};
use crate::export;
use crate::import::{self, ImportPhase};

pub fn spawn_import(
    db_path: PathBuf,
    files: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    tx: Sender<Msg>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let mut db = match Db::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let _ = tx.send(Msg::Fatal(e));
                ctx.request_repaint();
                return;
            }
        };
        let count = files.len();
        let mut summaries = Vec::with_capacity(count);
        for (i, path) in files.iter().enumerate() {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let summary = import::import_file(&mut db, path, &cancel, &mut |phase, done, total| {
                let _ = tx.send(Msg::Import(ImportEvent {
                    file_idx: i + 1,
                    file_count: count,
                    file_name: file_name.clone(),
                    phase,
                    done,
                    total,
                }));
                ctx.request_repaint();
            });
            summaries.push(summary);
        }
        let total_rows = db.total_rows();
        let _ = tx.send(Msg::ImportDone(summaries, total_rows));
        ctx.request_repaint();
    });
}

/// Completes indexing on startup if the previous run was interrupted.
pub fn spawn_index_repair(
    db_path: PathBuf,
    cancel: Arc<AtomicBool>,
    tx: Sender<Msg>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let mut db = match Db::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let _ = tx.send(Msg::Fatal(e));
                ctx.request_repaint();
                return;
            }
        };
        let _ = db.index_fts(&cancel, |done, total| {
            let _ = tx.send(Msg::Import(ImportEvent {
                file_idx: 1,
                file_count: 1,
                file_name: String::new(),
                phase: ImportPhase::Indexing,
                done,
                total,
            }));
            ctx.request_repaint();
        });
        let total_rows = db.total_rows();
        let _ = tx.send(Msg::ImportDone(Vec::new(), total_rows));
        ctx.request_repaint();
    });
}

/// Clears the database in the background because VACUUM can take minutes.
pub fn spawn_clear_db(db_path: PathBuf, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result =
            Db::open(&db_path).and_then(|mut db| db.clear_all().map_err(|e| e.to_string()));
        let _ = tx.send(Msg::DbCleared(result));
        ctx.request_repaint();
    });
}

pub fn spawn_optimize_db(db_path: PathBuf, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result = Db::open(&db_path)
            .and_then(|db| db.checkpoint_wal_truncate().map_err(|e| e.to_string()))
            .map(|info| {
                format!(
                    "Database optimized. WAL frames: {}, checkpointed: {}, busy: {}",
                    info.log_frames, info.checkpointed_frames, info.busy
                )
            });
        let _ = tx.send(Msg::MaintenanceDone(result));
        ctx.request_repaint();
    });
}

pub fn spawn_export(
    db_path: PathBuf,
    q: Query,
    dest: PathBuf,
    cancel: Arc<AtomicBool>,
    tx: Sender<Msg>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let db = match Db::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let _ = tx.send(Msg::Fatal(e));
                ctx.request_repaint();
                return;
            }
        };
        let mut last_sent = Instant::now();
        let result = export::export(&db, &q, &dest, &cancel, |done, total| {
            if last_sent.elapsed().as_millis() >= 100 {
                last_sent = Instant::now();
                let _ = tx.send(Msg::ExportProgress(done, total));
                ctx.request_repaint();
            }
        })
        .map(|written| (written, dest.clone()));
        let _ = tx.send(Msg::ExportDone(result));
        ctx.request_repaint();
    });
}
