use rusqlite::Connection;

use crate::domain::table::{ColumnStorage, SourceColumn, TableShape};
use crate::storage::meta;

pub(crate) const TABLE_SHAPE_KEY: &str = "table_shape_v1";

pub(crate) fn get(conn: &Connection) -> Option<TableShape> {
    meta::get(conn, TABLE_SHAPE_KEY).and_then(|raw| serde_json::from_str::<TableShape>(&raw).ok())
}

pub(crate) fn set(conn: &Connection, shape: &TableShape) {
    if let Ok(raw) = serde_json::to_string(shape) {
        meta::set(conn, TABLE_SHAPE_KEY, &raw);
    }
}

pub(crate) fn merge(conn: &Connection, incoming: &TableShape) -> TableShape {
    let mut merged = get(conn).unwrap_or_else(|| TableShape {
        columns: Vec::new(),
    });
    for column in &incoming.columns {
        merge_column(&mut merged.columns, column);
    }
    set(conn, &merged);
    merged
}

fn merge_column(columns: &mut Vec<SourceColumn>, incoming: &SourceColumn) {
    if let Some(existing) = columns.iter_mut().find(|column| column.id == incoming.id) {
        if existing.semantic.is_none() {
            existing.semantic = incoming.semantic;
        }
        if matches!(existing.storage, ColumnStorage::SourceJson)
            && !matches!(incoming.storage, ColumnStorage::SourceJson)
        {
            existing.storage = incoming.storage.clone();
        }
        return;
    }
    columns.push(incoming.clone());
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::{get, merge};
    use crate::domain::table::TableShape;

    #[test]
    fn shape_metadata_merges_columns_without_dropping_source_fields() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();

        merge(
            &conn,
            &TableShape::from_headers(["SKU".to_string(), "Price".to_string()]),
        );
        merge(
            &conn,
            &TableShape::from_headers(["SKU".to_string(), "Warehouse".to_string()]),
        );

        let shape = get(&conn).unwrap();
        let ids = shape
            .columns
            .iter()
            .map(|column| column.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["sku", "price", "warehouse"]);
    }
}
