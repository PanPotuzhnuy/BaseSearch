use rusqlite::{Connection, params};

use crate::schema::COLUMNS;
use crate::storage::{derived, import_log, meta, records};

const FTS_SCHEMA_VERSION: &str = "5";
const RECORDS_SCHEMA_VERSION: &str = "5";

pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    ensure_meta_schema(conn)?;
    ensure_fts_schema(conn)?;
    migrate_records_schema(conn)?;
    conn.execute_batch(&format!(
        "{records};
        CREATE VIRTUAL TABLE IF NOT EXISTS records_fts USING fts5(
            search_text,
            content='',
            detail=none,
            columnsize=0,
            tokenize='unicode61 remove_diacritics 2'
        );
        CREATE INDEX IF NOT EXISTS idx_records_year ON records(year);
        CREATE INDEX IF NOT EXISTS idx_records_product_code ON records(product_code);
        CREATE INDEX IF NOT EXISTS idx_records_edrpou ON records(edrpou);
        CREATE INDEX IF NOT EXISTS idx_records_hash ON records(row_hash);",
        records = records_ddl()
    ))?;
    import_log::ensure_schema(conn)?;
    Ok(())
}

fn ensure_meta_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT
        );",
    )
}

fn ensure_fts_schema(conn: &Connection) -> rusqlite::Result<()> {
    if meta::get(conn, "fts_schema").as_deref() != Some(FTS_SCHEMA_VERSION) {
        if existing_rows_may_need_fts_rebuild(conn)? {
            conn.execute_batch("DROP TABLE IF EXISTS records_fts;")?;
            meta::set(conn, "fts_watermark", "0");
        }
        meta::set(conn, "fts_schema", FTS_SCHEMA_VERSION);
    }
    Ok(())
}

fn records_ddl_for(table_name: &str) -> String {
    let mut fields: Vec<String> = COLUMNS
        .iter()
        .map(|column| format!("{} TEXT", column.name))
        .collect();
    fields.extend(derived::ddl_definitions());
    format!(
        "CREATE TABLE IF NOT EXISTS {table_name} (
            id INTEGER PRIMARY KEY,
            row_hash BLOB NOT NULL,
            source_file TEXT NOT NULL,
            year INTEGER,
            dup_first_file TEXT,
            extra TEXT,
            imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            {}
        )",
        fields.join(",\n            ")
    )
}

fn records_ddl() -> String {
    records_ddl_for("records")
}

fn migrate_records_schema(conn: &Connection) -> rusqlite::Result<()> {
    let current_schema = meta::get(conn, "records_schema");
    if current_schema.as_deref() == Some(RECORDS_SCHEMA_VERSION) {
        return Ok(());
    }

    if table_exists(conn, "records_v2")? {
        if table_exists(conn, "records")? {
            conn.execute_batch("DROP TABLE records_v2;")?;
        } else {
            conn.execute_batch("ALTER TABLE records_v2 RENAME TO records;")?;
            meta::set(conn, "fts_watermark", "0");
        }
    }

    if table_exists(conn, "records")? {
        let has_dup_first = table_has_column(conn, "records", "dup_first_file")?;
        let has_extra = table_has_column(conn, "records", "extra")?;
        if records_have_known_columns(conn)? {
            if !has_dup_first {
                conn.execute_batch("ALTER TABLE records ADD COLUMN dup_first_file TEXT;")?;
            }
            if !has_extra {
                conn.execute_batch("ALTER TABLE records ADD COLUMN extra TEXT;")?;
            }
            if table_exists(conn, "import_log")? {
                import_log::reset_file_hashes(conn)?;
            }
            let schema_version = current_schema
                .as_deref()
                .and_then(|version| version.parse::<u32>().ok())
                .unwrap_or(0);
            if schema_version < 2 {
                meta::set(conn, "fts_watermark", "0");
            }
            backfill_derived_columns(conn)?;
            meta::set(conn, "records_schema", RECORDS_SCHEMA_VERSION);
            return Ok(());
        }

        let column_names = COLUMNS.iter().map(|column| column.name).collect::<Vec<_>>();
        let columns_sql = column_names.join(", ");
        let dup_expr = if has_dup_first {
            "dup_first_file"
        } else {
            "NULL AS dup_first_file"
        };
        let extra_expr = if has_extra { "extra" } else { "NULL AS extra" };

        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let migration_result = (|| -> rusqlite::Result<()> {
            conn.execute_batch(
                "DROP TABLE IF EXISTS records_fts; DROP TABLE IF EXISTS records_v2;",
            )?;
            conn.execute_batch(&records_ddl_for("records_v2"))?;
            conn.execute_batch(&format!(
                "INSERT INTO records_v2 (
                    id, row_hash, source_file, year, dup_first_file, extra, imported_at, {columns_sql}
                 )
                 SELECT
                    id, row_hash, source_file, year, {dup_expr}, {extra_expr}, imported_at, {columns_sql}
                 FROM records;
                 DROP TABLE records;
                 ALTER TABLE records_v2 RENAME TO records;"
            ))?;
            Ok(())
        })();
        match migration_result {
            Ok(()) => conn.execute_batch("COMMIT;")?,
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err);
            }
        }
        rebuild_record_hashes(conn)?;
        if table_exists(conn, "import_log")? {
            import_log::reset_file_hashes(conn)?;
        }
        meta::set(conn, "fts_watermark", "0");
    }

    if table_exists(conn, "records")? {
        backfill_derived_columns(conn)?;
    }
    meta::set(conn, "records_schema", RECORDS_SCHEMA_VERSION);
    Ok(())
}

fn backfill_derived_columns(conn: &Connection) -> rusqlite::Result<()> {
    for column in derived::DERIVED {
        if !table_has_column(conn, "records", column.name)? {
            conn.execute_batch(&format!(
                "ALTER TABLE records ADD COLUMN {} {};",
                column.name, column.sql_type
            ))?;
        }
    }
    conn.execute_batch(&format!(
        "UPDATE records SET {};",
        derived::backfill_assignments()
    ))?;
    Ok(())
}

fn existing_rows_may_need_fts_rebuild(conn: &Connection) -> rusqlite::Result<bool> {
    if !table_exists(conn, "records")? || !table_has_column(conn, "records", "extra")? {
        return Ok(false);
    }
    let has_extra_payload = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM records
            WHERE extra IS NOT NULL AND TRIM(extra) <> ''
            LIMIT 1
        )",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(has_extra_payload != 0)
}

fn records_have_known_columns(conn: &Connection) -> rusqlite::Result<bool> {
    for name in ["id", "row_hash", "source_file", "year", "imported_at"] {
        if !table_has_column(conn, "records", name)? {
            return Ok(false);
        }
    }
    for column in COLUMNS {
        if !table_has_column(conn, "records", column.name)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn rebuild_record_hashes(conn: &Connection) -> rusqlite::Result<()> {
    let select: Vec<String> = COLUMNS
        .iter()
        .map(|column| column.name.to_string())
        .collect();
    let sql = format!("SELECT id, {}, extra FROM records", select.join(", "));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        let mut values = Vec::with_capacity(COLUMNS.len());
        for i in 0..COLUMNS.len() {
            values.push(row.get::<_, Option<String>>(i + 1)?.unwrap_or_default());
        }
        let extra: Option<String> = row.get(COLUMNS.len() + 1)?;
        Ok((
            id,
            records::canonical_record_hash(&values, extra.as_deref()),
        ))
    })?;
    let updates: Vec<(i64, [u8; 16])> = rows.collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let update_result = (|| -> rusqlite::Result<()> {
        let mut stmt = conn.prepare_cached("UPDATE records SET row_hash = ?1 WHERE id = ?2")?;
        for (id, hash) in updates {
            stmt.execute(params![&hash[..], id])?;
        }
        Ok(())
    })();
    match update_result {
        Ok(()) => conn.execute_batch("COMMIT;"),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(err)
        }
    }
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type IN ('table', 'virtual table') AND name = ?1
        )",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}
