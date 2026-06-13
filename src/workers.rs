//! Background threads for search, import, and export. The GUI never blocks.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use crate::db::{
    Analytics, AnalyticsScope, CompanyProfile, Db, PivotDim, PivotMetric, PivotResult, Query,
    analytics_should_run,
};
use crate::export::{self, ExportError};
use crate::import::{self, FileSummary, ImportPhase};

pub enum WorkerReq {
    Search {
        q: Box<Query>,
        page: u64,
        generation: u64,
    },
    /// One analytics category for the current query; cheap enough to
    /// request lazily as the user switches tabs. `scope = None` loads
    /// only the overview and the monthly dynamics.
    Analytics {
        q: Box<Query>,
        limit: u64,
        scope: Option<AnalyticsScope>,
        hs_level: u8,
        generation: u64,
    },
    /// Company dossier for one EDRPOU.
    Profile { edrpou: String, generation: u64 },
    /// Cross-tab of the current query.
    Pivot {
        q: Box<Query>,
        row_dim: PivotDim,
        col_dim: PivotDim,
        metric: PivotMetric,
        others_label: String,
        generation: u64,
    },
    Stats,
}

#[derive(Clone)]
pub struct ImportEvent {
    pub file_idx: usize,
    pub file_count: usize,
    pub file_name: String,
    pub phase: ImportPhase,
    pub done: u64,
    pub total: u64,
}

pub enum Msg {
    SearchDone {
        generation: u64,
        ids: Vec<i64>,
        rows: Vec<Vec<String>>,
        total: u64,
        ms: u64,
    },
    SearchError {
        generation: u64,
        message: String,
    },
    AnalyticsDone {
        generation: u64,
        scope: Option<AnalyticsScope>,
        analytics: Box<Analytics>,
    },
    ProfileDone {
        generation: u64,
        profile: Box<CompanyProfile>,
    },
    PivotDone {
        generation: u64,
        pivot: Box<PivotResult>,
    },
    Stats(u64),
    Import(ImportEvent),
    ImportDone(Vec<FileSummary>, u64),
    ExportProgress(u64, u64),
    ExportDone(Result<(u64, PathBuf), ExportError>),
    DbCleared(Result<(), String>),
    Fatal(String),
}

pub const PAGE_SIZE: u64 = 100;

/// Persistent search thread with its own connection and COUNT cache.
pub fn spawn_search_worker(
    db_path: PathBuf,
    rx: Receiver<WorkerReq>,
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
        let mut count_cache: Option<(Query, u64)> = None;
        while let Ok(req) = rx.recv() {
            match req {
                WorkerReq::Search {
                    q,
                    page,
                    generation,
                } => {
                    let started = Instant::now();
                    let total = match count_cache.as_ref().filter(|(cq, _)| cq == q.as_ref()) {
                        Some((_, n)) => Ok(*n),
                        None => db.count(&q),
                    };
                    let result = total.and_then(|total| {
                        count_cache = Some(((*q).clone(), total));
                        let (ids, rows) = db.search_page(&q, PAGE_SIZE, page * PAGE_SIZE)?;
                        Ok((ids, rows, total))
                    });
                    let msg = match result {
                        Ok((ids, rows, total)) => Msg::SearchDone {
                            generation,
                            ids,
                            rows,
                            total,
                            ms: started.elapsed().as_millis() as u64,
                        },
                        Err(e) => Msg::SearchError {
                            generation,
                            message: e.to_string(),
                        },
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                }
                WorkerReq::Analytics {
                    q,
                    limit,
                    scope,
                    hs_level,
                    generation,
                } => {
                    if !analytics_should_run(&q) {
                        continue;
                    }
                    let msg = match db.analytics_scoped(&q, limit, scope, hs_level) {
                        Ok(analytics) => Msg::AnalyticsDone {
                            generation,
                            scope,
                            analytics: Box::new(analytics),
                        },
                        Err(e) => Msg::SearchError {
                            generation,
                            message: e.to_string(),
                        },
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                }
                WorkerReq::Profile { edrpou, generation } => {
                    let msg = match db.company_profile(&edrpou, 10) {
                        Ok(profile) => Msg::ProfileDone {
                            generation,
                            profile: Box::new(profile),
                        },
                        Err(e) => Msg::SearchError {
                            generation,
                            message: e.to_string(),
                        },
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                }
                WorkerReq::Pivot {
                    q,
                    row_dim,
                    col_dim,
                    metric,
                    others_label,
                    generation,
                } => {
                    if !analytics_should_run(&q) {
                        continue;
                    }
                    let msg = match db.pivot(&q, row_dim, col_dim, metric, 25, 18, &others_label) {
                        Ok(pivot) => Msg::PivotDone {
                            generation,
                            pivot: Box::new(pivot),
                        },
                        Err(e) => Msg::SearchError {
                            generation,
                            message: e.to_string(),
                        },
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                }
                WorkerReq::Stats => {
                    count_cache = None;
                    let _ = tx.send(Msg::Stats(db.total_rows()));
                    ctx.request_repaint();
                }
            }
        }
    });
}

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
