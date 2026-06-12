//! Export search results to CSV (UTF-8 BOM, ';') and streaming XLSX.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::db::{Db, Query};
use crate::schema::COLUMNS;

/// Excel worksheet row limit minus the header row.
pub const XLSX_MAX_ROWS: u64 = 1_048_575;
const BATCH: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportFormat {
    Csv,
    Xlsx,
}

impl ExportFormat {
    pub fn from_path(path: &Path) -> ExportFormat {
        match path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref()
        {
            Some("xlsx") => ExportFormat::Xlsx,
            _ => ExportFormat::Csv,
        }
    }
}

#[derive(Debug)]
pub enum ExportError {
    /// More rows than a single Excel worksheet can store; CSV is required.
    TooManyRowsForXlsx(u64),
    Cancelled,
    Other(String),
}

fn headers() -> Vec<&'static str> {
    COLUMNS
        .iter()
        .map(|c| c.header)
        .chain(std::iter::once("Файл"))
        .collect()
}

/// Exports all rows matching the query and returns the row count.
pub fn export(
    db: &Db,
    q: &Query,
    dest: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64, ExportError> {
    let total = db.count(q).map_err(|e| ExportError::Other(e.to_string()))?;
    let format = ExportFormat::from_path(dest);
    if format == ExportFormat::Xlsx && total > XLSX_MAX_ROWS {
        return Err(ExportError::TooManyRowsForXlsx(total));
    }
    let result = match format {
        ExportFormat::Csv => export_csv(db, q, dest, total, cancel, &mut progress),
        ExportFormat::Xlsx => export_xlsx(db, q, dest, total, cancel, &mut progress),
    };
    if matches!(result, Err(ExportError::Cancelled)) {
        let _ = std::fs::remove_file(dest);
    }
    result
}

fn export_csv(
    db: &Db,
    q: &Query,
    dest: &Path,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
) -> Result<u64, ExportError> {
    let mut file = std::fs::File::create(dest).map_err(|e| ExportError::Other(e.to_string()))?;
    // BOM makes Excel open Cyrillic text as UTF-8; ';' is friendlier for
    // locales that use a decimal comma.
    file.write_all(b"\xEF\xBB\xBF")
        .map_err(|e| ExportError::Other(e.to_string()))?;
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b';')
        .terminator(csv::Terminator::CRLF)
        .from_writer(std::io::BufWriter::new(file));
    writer
        .write_record(headers())
        .map_err(|e| ExportError::Other(e.to_string()))?;

    let mut written: u64 = 0;
    let mut last_id: i64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let (max_id, rows) = db
            .export_batch(q, last_id, BATCH)
            .map_err(|e| ExportError::Other(e.to_string()))?;
        if rows.is_empty() {
            break;
        }
        last_id = max_id;
        for row in &rows {
            writer
                .write_record(row)
                .map_err(|e| ExportError::Other(e.to_string()))?;
        }
        written += rows.len() as u64;
        progress(written, total);
    }
    writer
        .flush()
        .map_err(|e| ExportError::Other(e.to_string()))?;
    Ok(written)
}

fn export_xlsx(
    db: &Db,
    q: &Query,
    dest: &Path,
    total: u64,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
) -> Result<u64, ExportError> {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let worksheet = workbook.add_worksheet_with_constant_memory();
    for (col, header) in headers().iter().enumerate() {
        worksheet
            .write_string(0, col as u16, *header)
            .map_err(|e| ExportError::Other(e.to_string()))?;
    }
    let mut written: u64 = 0;
    let mut last_id: i64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let (max_id, rows) = db
            .export_batch(q, last_id, BATCH)
            .map_err(|e| ExportError::Other(e.to_string()))?;
        if rows.is_empty() {
            break;
        }
        last_id = max_id;
        for row in &rows {
            written += 1;
            for (col, value) in row.iter().enumerate() {
                if !value.is_empty() {
                    worksheet
                        .write_string(written as u32, col as u16, value)
                        .map_err(|e| ExportError::Other(e.to_string()))?;
                }
            }
        }
        progress(written, total);
    }
    workbook
        .save(dest)
        .map_err(|e| ExportError::Other(e.to_string()))?;
    Ok(written)
}
