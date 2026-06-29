use rusqlite::{Connection, OptionalExtension};

use crate::db::ImportRecord;
use crate::schema::{self, COLUMNS};
use crate::storage::derived;

pub(crate) fn begin_import_file(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")
}

pub(crate) fn commit_import_file(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("COMMIT")
}

pub(crate) fn rollback_import_file(conn: &Connection) {
    let _ = conn.execute_batch("ROLLBACK");
}

pub(crate) fn insert_batch(
    conn: &Connection,
    source_file: &str,
    records: &[ImportRecord],
) -> rusqlite::Result<(u64, u64)> {
    if records.is_empty() {
        return Ok((0, 0));
    }
    let col_names: Vec<&str> = COLUMNS.iter().map(|column| column.name).collect();
    let derived_src: Vec<usize> = derived::DERIVED
        .iter()
        .map(|column| schema::col_index(column.source).expect("derived source is a schema column"))
        .collect();
    let derived_count = derived::DERIVED.len();
    let sql = format!(
        "INSERT INTO records (row_hash, source_file, year, dup_first_file, extra, {}, {}) VALUES ({})",
        col_names.join(", "),
        derived::insert_column_list(),
        std::iter::repeat_n("?", 5 + col_names.len() + derived_count)
            .collect::<Vec<_>>()
            .join(", ")
    );

    conn.execute_batch("SAVEPOINT insert_batch")?;
    let result = (|| -> rusqlite::Result<(u64, u64)> {
        let mut first_seen: u64 = 0;
        let mut duplicates: u64 = 0;
        let mut lookup = conn.prepare_cached(
            "SELECT source_file
             FROM records
             WHERE row_hash = ?1 AND dup_first_file IS NULL
             ORDER BY id ASC
             LIMIT 1",
        )?;
        let mut stmt = conn.prepare_cached(&sql)?;
        for rec in records {
            let prior: Option<String> = lookup
                .query_row([&rec.hash[..]], |row| row.get(0))
                .optional()?;
            stmt.raw_bind_parameter(1, &rec.hash[..])?;
            stmt.raw_bind_parameter(2, source_file)?;
            stmt.raw_bind_parameter(3, rec.year)?;
            match prior {
                Some(ref first_file) => {
                    stmt.raw_bind_parameter(4, first_file.as_str())?;
                    duplicates += 1;
                }
                None => {
                    stmt.raw_bind_parameter(4, rusqlite::types::Null)?;
                    first_seen += 1;
                }
            }
            stmt.raw_bind_parameter(5, rec.extra.as_deref())?;
            for (i, value) in rec.values.iter().enumerate() {
                stmt.raw_bind_parameter(6 + i, value.as_str())?;
            }
            let derived_base = 6 + rec.values.len();
            for (j, column) in derived::DERIVED.iter().enumerate() {
                let source_value = rec.values[derived_src[j]].as_str();
                let value = derived::compute(column.derivation, source_value);
                stmt.raw_bind_parameter(derived_base + j, value)?;
            }
            stmt.raw_execute()?;
        }
        Ok((first_seen + duplicates, duplicates))
    })();
    match result {
        Ok(counts) => {
            conn.execute_batch("RELEASE insert_batch")?;
            Ok(counts)
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK TO insert_batch");
            let _ = conn.execute_batch("RELEASE insert_batch");
            Err(err)
        }
    }
}

pub(crate) fn total_rows(conn: &Connection) -> u64 {
    conn.query_row("SELECT COUNT(*) FROM records", [], |row| {
        row.get::<_, i64>(0)
    })
    .unwrap_or(0) as u64
}
