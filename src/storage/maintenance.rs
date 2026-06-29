use std::path::{Path, PathBuf};

use rusqlite::Connection;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DatabaseStorageInfo {
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub shm_bytes: u64,
    pub page_count: u64,
    pub page_size: u64,
    pub freelist_pages: u64,
    pub freelist_bytes: u64,
}

impl DatabaseStorageInfo {
    pub fn total_file_bytes(&self) -> u64 {
        self.database_bytes + self.wal_bytes + self.shm_bytes
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalCheckpointInfo {
    pub busy: u64,
    pub log_frames: u64,
    pub checkpointed_frames: u64,
}

pub(crate) fn storage_info(
    conn: &Connection,
    db_path: &Path,
) -> rusqlite::Result<DatabaseStorageInfo> {
    let page_count = pragma_u64(conn, "page_count")?;
    let page_size = pragma_u64(conn, "page_size")?;
    let freelist_pages = pragma_u64(conn, "freelist_count")?;
    Ok(DatabaseStorageInfo {
        database_bytes: file_len(db_path),
        wal_bytes: file_len(&sidecar_path(db_path, "-wal")),
        shm_bytes: file_len(&sidecar_path(db_path, "-shm")),
        page_count,
        page_size,
        freelist_pages,
        freelist_bytes: freelist_pages.saturating_mul(page_size),
    })
}

pub(crate) fn checkpoint_wal_truncate(conn: &Connection) -> rusqlite::Result<WalCheckpointInfo> {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok(WalCheckpointInfo {
            busy: row.get::<_, i64>(0)?.max(0) as u64,
            log_frames: row.get::<_, i64>(1)?.max(0) as u64,
            checkpointed_frames: row.get::<_, i64>(2)?.max(0) as u64,
        })
    })
}

pub(crate) fn vacuum_database(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("VACUUM;")?;
    checkpoint_wal_truncate(conn)?;
    Ok(())
}

fn pragma_u64(conn: &Connection, name: &str) -> rusqlite::Result<u64> {
    let sql = format!("PRAGMA {name}");
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
        .map(|value| value.max(0) as u64)
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}
