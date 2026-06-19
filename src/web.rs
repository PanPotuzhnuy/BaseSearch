//! Local browser interface for Base Search.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::db::{
    Analytics, AnalyticsFilterAction, AnalyticsFilterField, AnalyticsGroupRow, AnalyticsOverview,
    AnalyticsPriceMetric, AnalyticsScope, AnalyticsSection, AnalyticsSectionKind, CompanyProfile,
    Db, Filters, PivotDim, PivotLimits, PivotMetric, PriceMetricKind, Query, analytics_should_run,
};
use crate::i18n::{Lang, tr};
use crate::schema::{RESULT_COLUMNS, column_glossary, header_for};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 7832;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_SEARCH_LIMIT: u64 = 500;
const DEFAULT_SEARCH_LIMIT: u64 = 100;
const MAX_ANALYTICS_LIMIT: u64 = 50;
const MAX_SECTION_LIMIT: u64 = 20_000;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");

#[derive(Clone, Debug)]
pub struct WebConfig {
    pub db_path: PathBuf,
    pub host: String,
    pub port: u16,
    pub open_browser: bool,
    /// Fixed session token; when None a random one is generated per run.
    pub token: Option<String>,
}

impl WebConfig {
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            open_browser: true,
            token: None,
        }
    }
}

#[derive(Clone)]
struct WebState {
    db_path: PathBuf,
    token: String,
}

struct HttpRequest {
    method: String,
    path: String,
    params: HashMap<String, String>,
    /// Bearer token from the `Authorization` header, if present.
    auth_token: Option<String>,
}

struct HttpResponse {
    status_code: u16,
    status_text: &'static str,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(
        status_code: u16,
        status_text: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status_code,
            status_text,
            content_type,
            headers: Vec::new(),
            body,
        }
    }

    fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn into_bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + 512);
        let mut headers = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n",
            self.status_code,
            self.status_text,
            self.content_type,
            self.body.len()
        );
        for (name, value) in self.headers {
            headers.push_str(name);
            headers.push_str(": ");
            headers.push_str(&value);
            headers.push_str("\r\n");
        }
        headers.push_str("\r\n");
        out.extend_from_slice(headers.as_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

pub fn run(config: WebConfig) -> Result<(), String> {
    let (listener, url) = bind_local(&config.host, config.port)?;
    ensure_indexed(&config.db_path)?;
    let token = config
        .token
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| make_token(&config.db_path));
    let browser_url = format!("{url}/?token={token}");

    if config.open_browser {
        let _ = open_browser(&browser_url);
    }

    eprintln!("Base Search web interface: {browser_url}");
    eprintln!("Database: {}", config.db_path.display());
    let state = Arc::new(WebState {
        db_path: config.db_path,
        token,
    });

    // Bounded worker pool: a fixed number of long-lived threads, each caching
    // its own SQLite connection (see `with_db`). This caps concurrency and
    // memory, and avoids spawning an unbounded number of threads/connections
    // under load. The bounded queue applies backpressure to the accept loop.
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8);
    let (tx, rx) = sync_channel::<TcpStream>(64);
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..worker_count {
        let rx = Arc::clone(&rx);
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            loop {
                let stream = {
                    let guard = match rx.lock() {
                        Ok(guard) => guard,
                        Err(_) => break,
                    };
                    guard.recv()
                };
                match stream {
                    Ok(stream) => {
                        let _ = handle_connection(stream, &state);
                    }
                    Err(_) => break, // listener stopped, channel closed
                }
            }
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if tx.send(stream).is_err() {
                    break; // all workers gone
                }
            }
            Err(err) => eprintln!("HTTP accept error: {err}"),
        }
    }
    Ok(())
}

fn bind_local(host: &str, preferred_port: u16) -> Result<(TcpListener, String), String> {
    for offset in 0..50u16 {
        let Some(port) = preferred_port.checked_add(offset) else {
            break;
        };
        let addr = format!("{host}:{port}");
        if let Ok(listener) = TcpListener::bind(&addr) {
            let url = format!("http://{host}:{port}");
            return Ok((listener, url));
        }
    }
    Err(format!(
        "Could not bind local web server on {host}:{preferred_port}..{}",
        preferred_port.saturating_add(49)
    ))
}

fn make_token(db_path: &Path) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    db_path.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    now.hash(&mut hasher);
    format!("{now:x}{:x}", hasher.finish())
}

fn ensure_indexed(db_path: &Path) -> Result<(), String> {
    let mut db = Db::open(db_path)?;
    if db.unindexed_rows() == 0 {
        return Ok(());
    }
    let cancel = AtomicBool::new(false);
    db.index_fts(&cancel, |_, _| {})
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream, state: &WebState) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let raw = read_request(&mut stream)?;
    let response = match parse_request(&raw) {
        Ok(request) => route_request(state, &request),
        Err(message) => json_error(400, "Bad Request", &message),
    };
    stream.write_all(&response.into_bytes())
}

/// Reads the request head (request line + headers) up to the blank line.
/// Only newly read bytes are scanned for the terminator, so a large body or a
/// flood cannot trigger a quadratic rescan. The body is intentionally not read:
/// only GET is served and the connection is closed after the response.
fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = [0u8; 4096];
    let mut bytes = Vec::new();
    loop {
        let read = stream.read(&mut buf)?;
        if read == 0 {
            break;
        }
        // Scan a small window straddling the previous tail and the new bytes,
        // not the whole accumulated buffer.
        let scan_from = bytes.len().saturating_sub(3);
        bytes.extend_from_slice(&buf[..read]);
        if bytes.len() >= MAX_REQUEST_BYTES
            || bytes[scan_from..].windows(4).any(|w| w == b"\r\n\r\n")
        {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn parse_request(raw: &str) -> Result<HttpRequest, String> {
    let mut lines = raw.lines();
    let first_line = lines.next().ok_or_else(|| "Empty HTTP request".to_string())?;
    let mut parts = first_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| "Missing HTTP method".to_string())?
        .to_string();
    let target = parts
        .next()
        .ok_or_else(|| "Missing HTTP target".to_string())?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    // Header lines until the blank line; we only need the bearer token.
    let mut auth_token = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("authorization")
            && let Some(token) = value.trim().strip_prefix("Bearer ")
        {
            auth_token = Some(token.trim().to_string());
        }
    }

    Ok(HttpRequest {
        method,
        path: percent_decode(path),
        params: parse_query_string(query),
        auth_token,
    })
}

fn route_request(state: &WebState, request: &HttpRequest) -> HttpResponse {
    if request.method != "GET" {
        return json_error(405, "Method Not Allowed", "Only GET requests are supported.")
            .with_header("Allow", "GET");
    }
    // Prefer the Authorization header (kept out of URLs and logs); fall back to
    // a query token for plain navigations such as the CSV download link.
    if request.path.starts_with("/api/") {
        let provided = request
            .auth_token
            .as_deref()
            .or_else(|| request.params.get("token").map(String::as_str));
        if !provided.is_some_and(|token| constant_time_eq(token, &state.token)) {
            return json_error(403, "Forbidden", "Invalid local session token.");
        }
    }

    match request.path.as_str() {
        "/" | "/index.html" => static_text("text/html; charset=utf-8", INDEX_HTML),
        "/assets/app.css" => static_text("text/css; charset=utf-8", APP_CSS),
        "/assets/app.js" => static_text("text/javascript; charset=utf-8", APP_JS),
        "/api/stats" => api_stats(state),
        "/api/schema" => api_schema(),
        "/api/search" => api_search(state, &request.params),
        "/api/card" => api_card(state, &request.params),
        "/api/analytics" => api_analytics(state, &request.params),
        "/api/section" => api_section(state, &request.params),
        "/api/company" => api_company(state, &request.params),
        "/api/pivot" => api_pivot(state, &request.params),
        "/api/i18n" => api_i18n(&request.params),
        "/api/export-page.csv" => api_export_page_csv(state, &request.params),
        _ => json_error(404, "Not Found", "Unknown route."),
    }
}

fn static_text(content_type: &'static str, text: &str) -> HttpResponse {
    HttpResponse::new(200, "OK", content_type, text.as_bytes().to_vec())
}

fn json_response(value: Value) -> HttpResponse {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    HttpResponse::new(200, "OK", "application/json; charset=utf-8", body)
}

fn json_error(status_code: u16, status_text: &'static str, message: &str) -> HttpResponse {
    let body = serde_json::to_vec(&json!({
        "ok": false,
        "error": message,
    }))
    .unwrap_or_default();
    HttpResponse::new(
        status_code,
        status_text,
        "application/json; charset=utf-8",
        body,
    )
}

thread_local! {
    /// One SQLite connection per worker thread, opened lazily on first use and
    /// reused for the life of the worker — instead of opening (and running the
    /// full schema init) on every request.
    static WORKER_DB: RefCell<Option<Db>> = const { RefCell::new(None) };
}

fn with_db<T>(state: &WebState, f: impl FnOnce(&Db) -> Result<T, String>) -> Result<T, String> {
    WORKER_DB.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(Db::open(&state.db_path)?);
        }
        f(slot.as_ref().expect("worker db initialized"))
    })
}

/// Length-checked, content-constant-time string comparison for the session
/// token, so a network observer cannot recover it byte by byte via timing.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn api_stats(state: &WebState) -> HttpResponse {
    match with_db(state, |db| {
        let imports: Vec<Value> = db
            .import_log(12)
            .into_iter()
            .map(|entry| {
                json!({
                    "file_name": entry.file_name,
                    "total_rows": entry.total_rows,
                    "imported": entry.imported,
                    "duplicates": entry.duplicates,
                    "seconds": entry.seconds,
                    "imported_at": entry.imported_at,
                })
            })
            .collect();
        Ok(json!({
            "ok": true,
            "db_path": state.db_path.display().to_string(),
            "total_rows": db.total_rows(),
            "unindexed_rows": db.unindexed_rows(),
            "imports": imports,
        }))
    }) {
        Ok(value) => json_response(value),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

fn api_schema() -> HttpResponse {
    let columns: Vec<Value> = RESULT_COLUMNS
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "label": header_for(name),
                "glossary": column_glossary(name).unwrap_or(""),
            })
        })
        .collect();
    json_response(json!({
        "ok": true,
        "columns": columns,
    }))
}

fn api_search(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let q = query_from_params(params);
    let limit = parse_u64(params, "limit", DEFAULT_SEARCH_LIMIT).clamp(1, MAX_SEARCH_LIMIT);
    let page = parse_u64(params, "page", 0);
    let offset = page.saturating_mul(limit);
    let started = Instant::now();

    match with_db(state, |db| {
        let (mut ids, mut rows, mut dups) = db
            .search_page(&q, limit.saturating_add(1), offset)
            .map_err(|err| err.to_string())?;
        let has_next = rows.len() as u64 > limit;
        if has_next {
            ids.truncate(limit as usize);
            rows.truncate(limit as usize);
            dups.truncate(limit as usize);
        }
        let row_values: Vec<Value> = rows
            .into_iter()
            .enumerate()
            .map(|(idx, cells)| {
                json!({
                    "id": ids.get(idx).copied().unwrap_or_default(),
                    "duplicate_of": dups.get(idx).cloned().unwrap_or(None),
                    "cells": cells,
                })
            })
            .collect();
        Ok(json!({
            "ok": true,
            "page": page,
            "limit": limit,
            "has_next": has_next,
            "rows": row_values,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }))
    }) {
        Ok(value) => json_response(value),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

fn api_card(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let Some(id) = params.get("id").and_then(|value| value.parse::<i64>().ok()) else {
        return json_error(400, "Bad Request", "Missing or invalid row id.");
    };
    match with_db(state, |db| {
        let card = db.record_card(id).map_err(|err| err.to_string())?;
        let mut fields: Vec<Value> = card
            .fields
            .iter()
            .map(|(label, value)| json!({ "label": label, "value": value }))
            .collect();
        // Extra (unmapped) columns from differently shaped files, shown after the
        // known fields and flagged so the UI can mark them.
        for (label, value) in &card.extra {
            fields.push(json!({ "label": label, "value": value, "extra": true }));
        }
        Ok(json!({
            "ok": true,
            "id": id,
            "source_file": card.source_file,
            "fields": fields,
        }))
    }) {
        Ok(value) => json_response(value),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

fn api_analytics(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let q = query_from_params(params);
    if !analytics_should_run(&q) {
        return json_response(json!({
            "ok": true,
            "needs_query": true,
            "message": "Enter a query or filter to build analytics.",
        }));
    }

    let limit = parse_u64(params, "limit", 10).clamp(1, MAX_ANALYTICS_LIMIT);
    let hs_level = parse_u64(params, "hs_level", 10).clamp(2, 10) as u8;
    let scope = params.get("scope").and_then(|value| parse_scope(value));
    let started = Instant::now();
    match with_db(state, |db| {
        let analytics = match scope {
            Some(scope) => db.analytics_scoped(&q, limit, Some(scope), hs_level),
            None => db.analytics(&q, limit),
        }
        .map_err(|err| err.to_string())?;
        Ok(json!({
            "ok": true,
            "needs_query": false,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "analytics": analytics_json(&analytics),
        }))
    }) {
        Ok(value) => json_response(value),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

fn api_company(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let edrpou = params
        .get("edrpou")
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    if edrpou.is_empty() {
        return json_error(400, "Bad Request", "Missing EDRPOU.");
    }
    let limit = parse_u64(params, "limit", 10).clamp(1, MAX_ANALYTICS_LIMIT);
    match with_db(state, |db| {
        let profile = db
            .company_profile(edrpou, limit)
            .map_err(|err| err.to_string())?;
        Ok(json!({
            "ok": true,
            "profile": company_profile_json(&profile),
        }))
    }) {
        Ok(value) => json_response(value),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

fn api_pivot(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let q = query_from_params(params);
    if !analytics_should_run(&q) {
        return json_response(json!({
            "ok": true,
            "needs_query": true,
            "message": "Enter a query or filter to build the pivot table.",
        }));
    }
    let row_dim = params
        .get("row")
        .and_then(|value| parse_pivot_dim(value))
        .unwrap_or(PivotDim::Recipient);
    let col_dim = params
        .get("col")
        .and_then(|value| parse_pivot_dim(value))
        .unwrap_or(PivotDim::Month);
    let metric = params
        .get("metric")
        .and_then(|value| parse_pivot_metric(value))
        .unwrap_or(PivotMetric::Value);
    match with_db(state, |db| {
        let pivot = db
            .pivot(
                &q,
                row_dim,
                col_dim,
                metric,
                PivotLimits { rows: 25, cols: 18 },
                "Other",
            )
            .map_err(|err| err.to_string())?;
        Ok(json!({
            "ok": true,
            "pivot": {
                "row_labels": pivot.row_labels,
                "col_labels": pivot.col_labels,
                "cells": pivot.cells,
                "row_totals": pivot.row_totals,
                "col_totals": pivot.col_totals,
                "grand_total": pivot.grand_total,
                "rows_truncated": pivot.rows_truncated,
                "cols_truncated": pivot.cols_truncated,
            }
        }))
    }) {
        Ok(value) => json_response(value),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

/// Full grouped list for one analytics section — the "show all" drill-down,
/// not just the top N. Sorting/filtering happens client-side.
fn api_section(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let q = query_from_params(params);
    if !analytics_should_run(&q) {
        return json_response(json!({
            "ok": true,
            "needs_query": true,
            "message": "Enter a query or filter to build the list.",
        }));
    }
    let Some(kind) = params
        .get("kind")
        .and_then(|value| parse_section_kind(value))
    else {
        return json_error(400, "Bad Request", "Unknown section kind.");
    };
    let hs_level = parse_u64(params, "hs_level", 10).clamp(2, 10) as u8;
    let limit = parse_u64(params, "limit", MAX_SECTION_LIMIT).clamp(1, MAX_SECTION_LIMIT);
    let started = Instant::now();
    match with_db(state, |db| {
        let section = db
            .analytics_section(&q, kind, hs_level, limit)
            .map_err(|err| err.to_string())?;
        let limited = section.rows.len() as u64 >= limit;
        Ok(json!({
            "ok": true,
            "needs_query": false,
            "kind": section_kind_id(section.kind),
            "title": section_kind_title(section.kind),
            "count": section.rows.len(),
            "limited": limited,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "rows": section.rows.iter().map(group_row_json).collect::<Vec<_>>(),
        }))
    }) {
        Ok(value) => json_response(value),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

/// Translated UI strings and the language list, so the browser interface can
/// switch language using the same translations as the desktop app.
fn api_i18n(params: &HashMap<String, String>) -> HttpResponse {
    let lang = Lang::from_code(params.get("lang").map(String::as_str).unwrap_or("en"));
    let t = tr(lang);
    let languages: Vec<Value> = Lang::ALL
        .iter()
        .map(|l| json!({ "code": l.code(), "label": l.label() }))
        .collect();
    let pairs: &[(&str, &str)] = &[
        ("app_title", t.app_title),
        ("find", t.find),
        ("search_hint", t.search_hint),
        ("filters", t.filters),
        ("clear_filters", t.clear_filters),
        ("year", t.year),
        ("product_code", t.product_code),
        ("edrpou", t.edrpou),
        ("trademark", t.trademark),
        ("recipient", t.recipient),
        ("sender", t.sender),
        ("description", t.description),
        ("origin_country", t.origin_country),
        ("dispatch_country", t.dispatch_country),
        ("trade_country", t.trade_country),
        ("results_tab", t.results_tab),
        ("analytics", t.analytics),
        ("export", t.export),
        ("tab_overview", t.tab_overview),
        ("companies_section", t.companies_section),
        ("products_section", t.products_section),
        ("countries_section", t.countries_section),
        ("prices_section", t.prices_section),
        ("months_section", t.months_section),
        ("all_label", t.all_label),
        ("close", t.close),
        ("details", t.details),
        ("analytics_hint", t.analytics_hint),
        ("company_profile", t.company_profile),
        ("copy_visible", t.copy_visible),
        ("dup_first_seen", t.dup_first_seen),
        ("searching", t.searching),
        ("nothing_found", t.nothing_found),
        ("db_rows", t.db_rows),
        ("search_ms", t.search_ms),
        ("show_results", t.show_results),
        ("rows_label", t.rows_label),
        ("declarations_label", t.declarations_label),
        ("recipients_label", t.recipients_label),
        ("unique_senders", t.unique_senders),
        ("net_weight", t.net_weight),
        ("gross_weight", t.gross_weight),
        ("total_value", t.total_value),
        ("avg_value_kg", t.avg_value_kg),
        ("quantity", t.quantity),
        ("median", t.median),
        ("weighted_avg", t.weighted_avg),
        ("col_share", t.col_share),
        ("col_companies", t.col_companies),
        ("col_label", t.col_label),
        ("group_search_hint", t.group_search_hint),
        ("showing", t.group_showing),
        ("sec_recipients", t.sec_recipients),
        ("sec_senders", t.sec_senders),
        ("sec_edrpou", t.sec_edrpou),
        ("sec_product_codes", t.sec_product_codes),
        ("sec_trademarks", t.sec_trademarks),
        ("sec_product_groups", t.sec_product_groups),
        ("sec_origin_countries", t.sec_origin_countries),
        ("sec_dispatch_countries", t.sec_dispatch_countries),
        ("sec_trade_countries", t.sec_trade_countries),
        ("pm_value_per_net_kg", t.pm_value_per_net_kg),
        ("pm_rfv", t.pm_rfv),
        ("pm_rmv_net", t.pm_rmv_net),
        ("pm_rmv_extra_unit", t.pm_rmv_extra_unit),
        ("pm_rmv_gross", t.pm_rmv_gross),
        ("pm_min_base", t.pm_min_base),
    ];
    let strings: serde_json::Map<String, Value> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::from(*v)))
        .collect();
    json_response(json!({
        "ok": true,
        "lang": lang.code(),
        "languages": languages,
        "language_label": t.language,
        "strings": strings,
    }))
}

fn api_export_page_csv(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let q = query_from_params(params);
    let limit = parse_u64(params, "limit", DEFAULT_SEARCH_LIMIT).clamp(1, MAX_SEARCH_LIMIT);
    let page = parse_u64(params, "page", 0);
    let offset = page.saturating_mul(limit);
    match with_db(state, |db| {
        let (_ids, rows, _dups) = db
            .search_page(&q, limit, offset)
            .map_err(|err| err.to_string())?;
        let mut csv = String::from('\u{feff}');
        csv.push_str(
            &RESULT_COLUMNS
                .iter()
                .map(|name| csv_cell(header_for(name)))
                .collect::<Vec<_>>()
                .join(";"),
        );
        csv.push_str("\r\n");
        for row in rows {
            csv.push_str(
                &row.iter()
                    .map(|value| csv_cell(value))
                    .collect::<Vec<_>>()
                    .join(";"),
            );
            csv.push_str("\r\n");
        }
        Ok(csv)
    }) {
        Ok(csv) => HttpResponse::new(200, "OK", "text/csv; charset=utf-8", csv.into_bytes())
            .with_header(
                "Content-Disposition",
                "attachment; filename=\"base-search-page.csv\"",
            ),
        Err(err) => json_error(500, "Internal Server Error", &err),
    }
}

fn analytics_json(analytics: &Analytics) -> Value {
    json!({
        "overview": overview_json(&analytics.overview),
        "months": analytics.months.iter().map(|row| json!({
            "month": &row.month,
            "rows": row.rows,
            "declarations": row.declarations,
            "total_value_usd": row.total_value_usd,
            "total_net_kg": row.total_net_kg,
        })).collect::<Vec<_>>(),
        "company_sections": sections_json(&analytics.company_sections),
        "product_sections": sections_json(&analytics.product_sections),
        "country_sections": sections_json(&analytics.country_sections),
        "price_sections": price_metrics_json(&analytics.price_sections),
    })
}

fn company_profile_json(profile: &CompanyProfile) -> Value {
    json!({
        "edrpou": &profile.edrpou,
        "names": &profile.names,
        "overview": overview_json(&profile.overview),
        "months": profile.months.iter().map(|row| json!({
            "month": &row.month,
            "rows": row.rows,
            "declarations": row.declarations,
            "total_value_usd": row.total_value_usd,
            "total_net_kg": row.total_net_kg,
        })).collect::<Vec<_>>(),
        "product_sections": sections_json(&profile.product_sections),
        "country_sections": sections_json(&profile.country_sections),
        "price_sections": price_metrics_json(&profile.price_sections),
        "top_senders": profile.top_senders.iter().map(group_row_json).collect::<Vec<_>>(),
    })
}

fn overview_json(overview: &AnalyticsOverview) -> Value {
    json!({
        "row_count": overview.row_count,
        "declaration_count": overview.declaration_count,
        "distinct_senders": overview.distinct_senders,
        "distinct_recipients": overview.distinct_recipients,
        "distinct_edrpou": overview.distinct_edrpou,
        "distinct_trademarks": overview.distinct_trademarks,
        "distinct_product_codes": overview.distinct_product_codes,
        "distinct_origin_countries": overview.distinct_origin_countries,
        "distinct_dispatch_countries": overview.distinct_dispatch_countries,
        "distinct_trade_countries": overview.distinct_trade_countries,
        "total_value_usd": overview.total_value_usd,
        "total_gross_kg": overview.total_gross_kg,
        "total_net_kg": overview.total_net_kg,
        "total_quantity": overview.total_quantity,
        "avg_value_per_net_kg": overview.avg_value_per_net_kg,
    })
}

fn sections_json(sections: &[AnalyticsSection]) -> Vec<Value> {
    sections
        .iter()
        .map(|section| {
            json!({
                "kind": section_kind_id(section.kind),
                "title": section_kind_title(section.kind),
                "rows": section.rows.iter().map(group_row_json).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn group_row_json(row: &AnalyticsGroupRow) -> Value {
    json!({
        "label": &row.label,
        "rows": row.rows,
        "declarations": row.declarations,
        "companies": row.companies,
        "total_value_usd": row.total_value_usd,
        "total_net_kg": row.total_net_kg,
        "total_gross_kg": row.total_gross_kg,
        "total_quantity": row.total_quantity,
        "share_percent": row.share_percent,
        "avg_value_per_net_kg": row.avg_value_per_net_kg,
        "filter_action": row.filter_action.as_ref().map(filter_action_json),
    })
}

fn filter_action_json(action: &AnalyticsFilterAction) -> Value {
    json!({
        "field": filter_field_id(action.field),
        "value": &action.value,
    })
}

fn price_metrics_json(metrics: &[AnalyticsPriceMetric]) -> Vec<Value> {
    metrics
        .iter()
        .map(|metric| {
            json!({
                "kind": price_kind_id(metric.kind),
                "title": price_kind_title(metric.kind),
                "count": metric.count,
                "average": metric.average,
                "minimum": metric.minimum,
                "maximum": metric.maximum,
                "weighted_average": metric.weighted_average,
                "median": metric.median,
                "p25": metric.p25,
                "p75": metric.p75,
            })
        })
        .collect()
}

fn query_from_params(params: &HashMap<String, String>) -> Query {
    Query {
        text: first_param(params, &["text", "q"]),
        filters: Filters {
            year: first_param(params, &["year"]),
            product_code: first_param(params, &["product_code", "code"]),
            trademark: first_param(params, &["trademark"]),
            description: first_param(params, &["description"]),
            sender: first_param(params, &["sender"]),
            recipient: first_param(params, &["recipient"]),
            edrpou: first_param(params, &["edrpou"]),
            trade_country: first_param(params, &["trade_country", "trade"]),
            dispatch_country: first_param(params, &["dispatch_country", "dispatch"]),
            origin_country: first_param(params, &["origin_country", "origin"]),
        },
    }
}

fn first_param(params: &HashMap<String, String>, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| params.get(*key))
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

fn parse_u64(params: &HashMap<String, String>, key: &str, default: u64) -> u64 {
    params
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), percent_decode(value));
    }
    params
}

fn percent_decode(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(a), Some(b)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                    out.push((a << 4) | b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn csv_cell(value: &str) -> String {
    if value.contains(';') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn parse_scope(value: &str) -> Option<AnalyticsScope> {
    match value {
        "companies" => Some(AnalyticsScope::Companies),
        "products" => Some(AnalyticsScope::Products),
        "countries" => Some(AnalyticsScope::Countries),
        "prices" => Some(AnalyticsScope::Prices),
        _ => None,
    }
}

fn parse_pivot_dim(value: &str) -> Option<PivotDim> {
    match value {
        "recipient" => Some(PivotDim::Recipient),
        "sender" => Some(PivotDim::Sender),
        "edrpou" => Some(PivotDim::Edrpou),
        "product_code" => Some(PivotDim::ProductCode),
        "trademark" => Some(PivotDim::Trademark),
        "origin_country" => Some(PivotDim::OriginCountry),
        "dispatch_country" => Some(PivotDim::DispatchCountry),
        "trade_country" => Some(PivotDim::TradeCountry),
        "month" => Some(PivotDim::Month),
        "year" => Some(PivotDim::Year),
        _ => None,
    }
}

fn parse_pivot_metric(value: &str) -> Option<PivotMetric> {
    match value {
        "value" => Some(PivotMetric::Value),
        "rows" => Some(PivotMetric::Rows),
        "net_kg" => Some(PivotMetric::NetKg),
        _ => None,
    }
}

fn section_kind_id(kind: AnalyticsSectionKind) -> &'static str {
    match kind {
        AnalyticsSectionKind::Recipients => "recipients",
        AnalyticsSectionKind::Senders => "senders",
        AnalyticsSectionKind::Edrpou => "edrpou",
        AnalyticsSectionKind::ProductCodes => "product_codes",
        AnalyticsSectionKind::Trademarks => "trademarks",
        AnalyticsSectionKind::ProductGroups => "product_groups",
        AnalyticsSectionKind::OriginCountries => "origin_countries",
        AnalyticsSectionKind::DispatchCountries => "dispatch_countries",
        AnalyticsSectionKind::TradeCountries => "trade_countries",
    }
}

fn parse_section_kind(value: &str) -> Option<AnalyticsSectionKind> {
    match value {
        "recipients" => Some(AnalyticsSectionKind::Recipients),
        "senders" => Some(AnalyticsSectionKind::Senders),
        "edrpou" => Some(AnalyticsSectionKind::Edrpou),
        "product_codes" => Some(AnalyticsSectionKind::ProductCodes),
        "trademarks" => Some(AnalyticsSectionKind::Trademarks),
        "product_groups" => Some(AnalyticsSectionKind::ProductGroups),
        "origin_countries" => Some(AnalyticsSectionKind::OriginCountries),
        "dispatch_countries" => Some(AnalyticsSectionKind::DispatchCountries),
        "trade_countries" => Some(AnalyticsSectionKind::TradeCountries),
        _ => None,
    }
}

fn section_kind_title(kind: AnalyticsSectionKind) -> &'static str {
    match kind {
        AnalyticsSectionKind::Recipients => "Recipients",
        AnalyticsSectionKind::Senders => "Senders",
        AnalyticsSectionKind::Edrpou => "EDRPOU",
        AnalyticsSectionKind::ProductCodes => "Product codes",
        AnalyticsSectionKind::Trademarks => "Trademarks",
        AnalyticsSectionKind::ProductGroups => "Description groups",
        AnalyticsSectionKind::OriginCountries => "Origin countries",
        AnalyticsSectionKind::DispatchCountries => "Dispatch countries",
        AnalyticsSectionKind::TradeCountries => "Trade countries",
    }
}

fn filter_field_id(field: AnalyticsFilterField) -> &'static str {
    match field {
        AnalyticsFilterField::Recipient => "recipient",
        AnalyticsFilterField::Sender => "sender",
        AnalyticsFilterField::Edrpou => "edrpou",
        AnalyticsFilterField::ProductCode => "product_code",
        AnalyticsFilterField::Trademark => "trademark",
        AnalyticsFilterField::OriginCountry => "origin_country",
        AnalyticsFilterField::DispatchCountry => "dispatch_country",
        AnalyticsFilterField::TradeCountry => "trade_country",
        AnalyticsFilterField::Description => "description",
    }
}

fn price_kind_id(kind: PriceMetricKind) -> &'static str {
    match kind {
        PriceMetricKind::ValuePerNetKg => "value_per_net_kg",
        PriceMetricKind::RfvUsdKg => "rfv_usd_kg",
        PriceMetricKind::RmvNetUsdKg => "rmv_net_usd_kg",
        PriceMetricKind::RmvUsdExtraUnit => "rmv_usd_extra_unit",
        PriceMetricKind::RmvGrossUsdKg => "rmv_gross_usd_kg",
        PriceMetricKind::MinBaseUsdKg => "min_base_usd_kg",
    }
}

fn price_kind_title(kind: PriceMetricKind) -> &'static str {
    match kind {
        PriceMetricKind::ValuePerNetKg => "Value / net kg",
        PriceMetricKind::RfvUsdKg => "RFV $/kg",
        PriceMetricKind::RmvNetUsdKg => "RMV net $/kg",
        PriceMetricKind::RmvUsdExtraUnit => "RMV per extra unit",
        PriceMetricKind::RmvGrossUsdKg => "RMV gross $/kg",
        PriceMetricKind::MinBaseUsdKg => "Min. base $/kg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_utf8_and_spaces() {
        assert_eq!(percent_decode("Apple+iPhone"), "Apple iPhone");
        assert_eq!(percent_decode("%D0%A2%D0%95%D0%A1%D0%A2"), "ТЕСТ");
        assert_eq!(percent_decode("bad%zz"), "bad%zz");
    }

    #[test]
    fn query_params_map_to_search_query() {
        let params = parse_query_string("q=Apple&year=2024&code=8517&origin=CN&recipient=Demo");
        let q = query_from_params(&params);
        assert_eq!(q.text, "Apple");
        assert_eq!(q.filters.year, "2024");
        assert_eq!(q.filters.product_code, "8517");
        assert_eq!(q.filters.origin_country, "CN");
        assert_eq!(q.filters.recipient, "Demo");
    }

    #[test]
    fn csv_cell_quotes_only_when_needed() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a;b"), "\"a;b\"");
        assert_eq!(csv_cell("a\"b"), "\"a\"\"b\"");
    }
}
