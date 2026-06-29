use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;

use crate::storage::extra::{extra_value_for_header, parse_extra};
use crate::storage::migrations;
use crate::storage::normalize::{
    clean_label_value, month_key, normalize_country_key, normalize_text_key, parse_number,
};
use crate::storage::search_text::contains_ci;

pub(crate) fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    let conn = Connection::open(path).map_err(|err| err.to_string())?;
    initialize(&conn).map_err(|err| err.to_string())?;
    Ok(conn)
}

fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    configure_pragmas(conn)?;
    register_scalar_functions(conn)?;
    register_aggregate_functions(conn)?;
    migrations::ensure_schema(conn)?;
    Ok(())
}

fn configure_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -131072)?;
    conn.pragma_update(None, "mmap_size", 268435456i64)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

fn register_scalar_functions(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("cyr_contains", 2, flags, |ctx| {
        let hay = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        let needle = ctx
            .get_raw(1)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        Ok(match (hay, needle) {
            (Some(hay), Some(needle)) => contains_ci(hay, needle),
            _ => false,
        })
    })?;
    conn.create_scalar_function("num_value", 1, flags, |ctx| {
        let raw = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        Ok(raw.and_then(parse_number))
    })?;
    conn.create_scalar_function("country_key", 1, flags, |ctx| {
        let raw = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        Ok(raw.map(normalize_country_key).unwrap_or_default())
    })?;
    conn.create_scalar_function("text_key", 1, flags, |ctx| {
        let raw = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        Ok(raw.map(normalize_text_key).unwrap_or_default())
    })?;
    conn.create_scalar_function("label_value", 1, flags, |ctx| {
        let raw = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        Ok(raw.map(clean_label_value).unwrap_or_default())
    })?;
    conn.create_scalar_function("month_key", 1, flags, |ctx| {
        let raw = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        Ok(raw.map(month_key).unwrap_or_default())
    })?;
    conn.create_scalar_function("extra_values_text", 1, flags, |ctx| {
        let raw = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        let values = parse_extra(raw)
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>()
            .join(" ");
        Ok(values)
    })?;
    conn.create_scalar_function("extra_value", 2, flags, |ctx| {
        let raw = ctx
            .get_raw(0)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        let header = ctx
            .get_raw(1)
            .as_str_or_null()
            .map_err(|err| rusqlite::Error::UserFunctionError(Box::new(err)))?;
        Ok(extra_value_for_header(raw, header))
    })?;
    Ok(())
}

fn register_aggregate_functions(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_aggregate_function("pctl_text", 1, flags, PercentilesAggregate)?;
    conn.create_aggregate_function("median_num", 1, flags, MedianAggregate)?;
    Ok(())
}

struct PercentilesAggregate;

impl rusqlite::functions::Aggregate<Vec<f64>, Option<String>> for PercentilesAggregate {
    fn init(&self, _ctx: &mut rusqlite::functions::Context<'_>) -> rusqlite::Result<Vec<f64>> {
        Ok(Vec::new())
    }

    fn step(
        &self,
        ctx: &mut rusqlite::functions::Context<'_>,
        acc: &mut Vec<f64>,
    ) -> rusqlite::Result<()> {
        if let Some(value) = ctx.get::<Option<f64>>(0)?
            && value.is_finite()
        {
            acc.push(value);
        }
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut rusqlite::functions::Context<'_>,
        acc: Option<Vec<f64>>,
    ) -> rusqlite::Result<Option<String>> {
        let mut values = acc.unwrap_or_default();
        if values.is_empty() {
            return Ok(None);
        }
        values.sort_unstable_by(f64::total_cmp);
        let pick = |p: f64| {
            let idx = ((values.len() - 1) as f64 * p).round() as usize;
            values[idx.min(values.len() - 1)]
        };
        Ok(Some(format!("{}|{}|{}", pick(0.25), pick(0.5), pick(0.75))))
    }
}

struct MedianAggregate;

impl rusqlite::functions::Aggregate<Vec<f64>, Option<f64>> for MedianAggregate {
    fn init(&self, _ctx: &mut rusqlite::functions::Context<'_>) -> rusqlite::Result<Vec<f64>> {
        Ok(Vec::new())
    }

    fn step(
        &self,
        ctx: &mut rusqlite::functions::Context<'_>,
        acc: &mut Vec<f64>,
    ) -> rusqlite::Result<()> {
        if let Some(value) = ctx.get::<Option<f64>>(0)?
            && value.is_finite()
        {
            acc.push(value);
        }
        Ok(())
    }

    fn finalize(
        &self,
        _ctx: &mut rusqlite::functions::Context<'_>,
        acc: Option<Vec<f64>>,
    ) -> rusqlite::Result<Option<f64>> {
        let mut values = acc.unwrap_or_default();
        if values.is_empty() {
            return Ok(None);
        }
        values.sort_unstable_by(f64::total_cmp);
        Ok(Some(values[values.len() / 2]))
    }
}
