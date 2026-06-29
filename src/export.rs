//! Export search results to CSV (UTF-8 BOM, ';') and streaming XLSX.

use std::borrow::Cow;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::{Db, Query};

/// Excel worksheet row limit minus the header row.
pub const XLSX_MAX_ROWS: u64 = 1_048_575;
const BATCH: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportFormat {
    Csv,
    Xlsx,
}

impl ExportFormat {
    pub fn from_path(path: &Path) -> Result<ExportFormat, ExportError> {
        match path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref()
        {
            Some("csv") => Ok(ExportFormat::Csv),
            Some("xlsx") => Ok(ExportFormat::Xlsx),
            other => Err(ExportError::UnsupportedExtension(
                other.unwrap_or("").to_string(),
            )),
        }
    }
}

#[derive(Debug)]
pub enum ExportError {
    /// More rows than a single Excel worksheet can store; CSV is required.
    TooManyRowsForXlsx(u64),
    UnsupportedExtension(String),
    Cancelled,
    Other(String),
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
    let format = ExportFormat::from_path(dest)?;
    if format == ExportFormat::Xlsx && total > XLSX_MAX_ROWS {
        return Err(ExportError::TooManyRowsForXlsx(total));
    }
    let temp_dest = temp_export_path(dest);
    let result = match format {
        ExportFormat::Csv => export_csv(db, q, &temp_dest, total, cancel, &mut progress),
        ExportFormat::Xlsx => export_xlsx(db, q, &temp_dest, total, cancel, &mut progress),
    };
    match result {
        Ok(written) => {
            if dest.exists() {
                std::fs::remove_file(dest).map_err(|e| ExportError::Other(e.to_string()))?;
            }
            std::fs::rename(&temp_dest, dest).map_err(|e| ExportError::Other(e.to_string()))?;
            Ok(written)
        }
        Err(err) => {
            let _ = std::fs::remove_file(&temp_dest);
            Err(err)
        }
    }
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
    let fields = db.result_fields_cached();
    let headers: Vec<&str> = fields.iter().map(|field| field.label.as_str()).collect();
    writer
        .write_record(headers)
        .map_err(|e| ExportError::Other(e.to_string()))?;

    let mut written: u64 = 0;
    let mut last_id: i64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let (max_id, rows) = db
            .export_batch_fields(q, last_id, BATCH, &fields)
            .map_err(|e| ExportError::Other(e.to_string()))?;
        if rows.is_empty() {
            break;
        }
        last_id = max_id;
        for row in &rows {
            let safe_row: Vec<String> = row
                .iter()
                .map(|value| csv_safe_cell(value).into_owned())
                .collect();
            writer
                .write_record(&safe_row)
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

pub fn csv_safe_cell(value: &str) -> Cow<'_, str> {
    let trimmed = value.trim_start_matches([' ', '\t', '\r', '\n']);
    if trimmed
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(*byte, b'=' | b'+' | b'-' | b'@'))
    {
        Cow::Owned(format!("'{value}"))
    } else {
        Cow::Borrowed(value)
    }
}

fn temp_export_path(dest: &Path) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    let file_name = dest
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "base-search-export".into());
    dest.with_file_name(format!("{file_name}.{pid}.{stamp}.tmp"))
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
    let fields = db.result_fields_cached();
    for (col, field) in fields.iter().enumerate() {
        worksheet
            .write_string(0, col as u16, &field.label)
            .map_err(|e| ExportError::Other(e.to_string()))?;
    }
    let mut written: u64 = 0;
    let mut last_id: i64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let (max_id, rows) = db
            .export_batch_fields(q, last_id, BATCH, &fields)
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
