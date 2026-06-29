use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, params};

use crate::storage::{meta, search_text};

pub(crate) fn index(
    conn: &mut Connection,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> rusqlite::Result<(u64, bool)> {
    let max_id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM records", [], |row| {
        row.get(0)
    })?;
    let start = meta::get_i64(conn, "fts_watermark");
    if start >= max_id {
        return Ok((0, false));
    }
    let span_total = (max_id - start) as u64;
    let insert_sql = format!(
        "INSERT INTO records_fts(rowid, search_text)
         SELECT id, {} FROM records WHERE id > ?1 AND id <= ?2",
        search_text::search_text_expr()
    );
    const CHUNK: i64 = 20_000;
    let mut watermark = start;
    let mut indexed: u64 = 0;
    while watermark < max_id {
        if cancel.load(Ordering::Relaxed) {
            return Ok((indexed, true));
        }
        let end = (watermark + CHUNK).min(max_id);
        let tx = conn.transaction()?;
        let n = tx.execute(&insert_sql, params![watermark, end])?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('fts_watermark', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![end.to_string()],
        )?;
        tx.commit()?;
        indexed += n as u64;
        watermark = end;
        progress((watermark - start) as u64, span_total);
    }
    Ok((indexed, false))
}

pub(crate) fn unindexed_rows(conn: &Connection) -> u64 {
    let watermark = meta::get_i64(conn, "fts_watermark");
    conn.query_row(
        "SELECT COUNT(*) FROM records WHERE id > ?1",
        [watermark],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0) as u64
}
