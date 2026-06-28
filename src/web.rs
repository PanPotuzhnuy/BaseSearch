//! Local browser interface for Base Search.
//!
//! A small dependency-free HTTP/1.1 server that exposes the same local database
//! the desktop app uses, so the data can be searched and analysed from a regular
//! browser. It is deliberately minimal and safe by default:
//!
//! - binds to loopback only (never a public interface);
//! - every `/api/*` route requires a per-session token sent in the
//!   `Authorization: Bearer` header (never in the URL), compared in constant time;
//! - requests are bounded in size and time, and served by a fixed pool of worker
//!   threads that each reuse one SQLite connection.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::db::{
    Analytics, AnalyticsFilterAction, AnalyticsFilterField, AnalyticsGroupRow, AnalyticsOverview,
    AnalyticsPriceMetric, AnalyticsScope, AnalyticsSection, AnalyticsSectionKind, CompanyProfile,
    Db, Filters, PivotDim, PivotLimits, PivotMetric, PriceMetricKind, Query, analytics_should_run,
};
use crate::export::csv_safe_cell;
use crate::i18n::{Lang, tr};
use crate::schema::{RESULT_COLUMNS, column_glossary, header_for};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 7832;

/// Hard ceiling on a request head; bodies are never read (only GET is served).
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const REQUEST_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_SEARCH_LIMIT: u64 = 100;
const MAX_SEARCH_LIMIT: u64 = 500;
const MAX_ANALYTICS_LIMIT: u64 = 50;
const MAX_SECTION_LIMIT: u64 = 20_000;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const APP_JS: &str = include_str!("../web/app.js");

/// Runtime configuration for the local web server.
#[derive(Clone, Debug)]
pub struct WebConfig {
    pub db_path: PathBuf,
    pub host: String,
    pub port: u16,
    pub open_browser: bool,
    /// Fixed session token; when `None` a fresh random one is generated per run.
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

/// Shared, immutable state handed to every worker thread.
struct WebState {
    db_path: PathBuf,
    token: String,
}

// ---------------------------------------------------------------------------
// Server lifecycle
// ---------------------------------------------------------------------------

pub fn run(config: WebConfig) -> Result<(), String> {
    let (listener, url) = bind_local(&config.host, config.port)?;
    ensure_indexed(&config.db_path)?;

    let token = match config.token {
        Some(token) if !token.trim().is_empty() => token,
        _ => make_token()?,
    };

    if config.open_browser {
        let _ = open_browser(&url);
    }

    eprintln!("Base Search web interface: {url}");
    eprintln!("Database file: {}", db_file_name(&config.db_path));

    let state = Arc::new(WebState {
        db_path: config.db_path,
        token,
    });

    // Bounded worker pool: a fixed number of long-lived threads, each caching its
    // own SQLite connection (see `with_db`). This caps concurrency and memory and
    // applies backpressure to the accept loop through the bounded queue.
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
                let stream = match rx.lock() {
                    Ok(guard) => guard.recv(),
                    Err(_) => break,
                };
                match stream {
                    Ok(stream) => {
                        let _ = serve_connection(stream, &state);
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
    validate_loopback_host(host)?;
    for offset in 0..50u16 {
        let Some(port) = preferred_port.checked_add(offset) else {
            break;
        };
        let addr = format!("{}:{port}", bracketed(host));
        if let Ok(listener) = TcpListener::bind(&addr) {
            return Ok((listener, format!("http://{}:{port}", bracketed(host))));
        }
    }
    Err(format!(
        "Could not bind the local web server on {host}:{preferred_port}..{}",
        preferred_port.saturating_add(49)
    ))
}

/// Refuses to bind anywhere that is not loopback, so the local database is never
/// exposed to the network by accident.
fn validate_loopback_host(host: &str) -> Result<(), String> {
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    let ip: IpAddr = host
        .parse()
        .map_err(|_| format!("Web host must be localhost or a loopback IP, got {host:?}"))?;
    if ip.is_loopback() {
        Ok(())
    } else {
        Err(format!(
            "Refusing to bind the local web interface to non-loopback host {host:?}"
        ))
    }
}

/// Wraps a bare IPv6 address in brackets for use in a socket address / URL.
fn bracketed(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn make_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|err| format!("Could not create session token: {err}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn db_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "base_search.db".to_string())
}

/// Finishes the full-text index up front so the first browser search is fast.
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

// ---------------------------------------------------------------------------
// One SQLite connection per worker thread
// ---------------------------------------------------------------------------

thread_local! {
    /// Opened lazily on first use and reused for the life of the worker, so the
    /// schema init does not run on every request.
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

// ---------------------------------------------------------------------------
// HTTP request / response
// ---------------------------------------------------------------------------

struct HttpRequest {
    method: String,
    path: String,
    params: HashMap<String, String>,
    /// Bearer token from the `Authorization` header, if any.
    auth_token: Option<String>,
}

struct HttpResponse {
    status: u16,
    status_text: &'static str,
    content_type: &'static str,
    extra_headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(
        status: u16,
        status_text: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            status_text,
            content_type,
            extra_headers: Vec::new(),
            body,
        }
    }

    fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.extra_headers.push((name, value.into()));
        self
    }

    fn into_bytes(self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n\
             Content-Type: {}\r\n\
             Content-Length: {}\r\n\
             Cache-Control: no-store\r\n\
             Connection: close\r\n\
             X-Content-Type-Options: nosniff\r\n\
             Referrer-Policy: no-referrer\r\n",
            self.status,
            self.status_text,
            self.content_type,
            self.body.len()
        );
        for (name, value) in self.extra_headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(&value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        let mut out = Vec::with_capacity(head.len() + self.body.len());
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

fn serve_connection(mut stream: TcpStream, state: &WebState) -> std::io::Result<()> {
    let response = match read_head(&mut stream) {
        Ok(raw) => match parse_request(&raw) {
            Ok(request) => route(state, &request),
            Err(message) => error_json(400, "Bad Request", &message),
        },
        Err(_) => error_json(400, "Bad Request", "Could not read the request."),
    };
    stream.write_all(&response.into_bytes())
}

/// Reads the request head (request line + headers) up to the blank line. Only
/// newly read bytes are scanned for the terminator, so a large body or a flood
/// cannot trigger a quadratic rescan. The body is intentionally not read.
fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = [0u8; 4096];
    let mut bytes: Vec<u8> = Vec::new();
    let started = Instant::now();
    let mut complete = false;
    loop {
        let elapsed = started.elapsed();
        if elapsed >= REQUEST_HEADER_TIMEOUT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "HTTP request header timeout",
            ));
        }
        stream.set_read_timeout(Some(REQUEST_HEADER_TIMEOUT - elapsed))?;
        let read = stream.read(&mut buf)?;
        if read == 0 {
            break;
        }
        let scan_from = bytes.len().saturating_sub(3);
        bytes.extend_from_slice(&buf[..read]);
        if bytes[scan_from..].windows(4).any(|w| w == b"\r\n\r\n") {
            complete = true;
            break;
        }
        if bytes.len() >= MAX_REQUEST_BYTES {
            break;
        }
    }
    if !complete {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Incomplete HTTP request head",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_request(raw: &str) -> Result<HttpRequest, String> {
    let mut lines = raw.lines();
    let request_line = lines.next().ok_or("Empty HTTP request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("Missing HTTP method")?.to_string();
    let target = parts.next().ok_or("Missing HTTP target")?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

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
        params: parse_query(query),
        auth_token,
    })
}

// ---------------------------------------------------------------------------
// Routing + auth
// ---------------------------------------------------------------------------

fn route(state: &WebState, request: &HttpRequest) -> HttpResponse {
    if request.method != "GET" {
        return error_json(
            405,
            "Method Not Allowed",
            "Only GET requests are supported.",
        )
        .header("Allow", "GET");
    }

    // Every data route is gated by the per-session token in the Authorization
    // header. Static assets are public (they contain no data).
    if request.path.starts_with("/api/") {
        let ok = request
            .auth_token
            .as_deref()
            .is_some_and(|token| constant_time_eq(token, &state.token));
        if !ok {
            return error_json(403, "Forbidden", "Invalid local session token.");
        }
    }

    match request.path.as_str() {
        "/" | "/index.html" => serve_index(&state.token),
        "/assets/app.css" => serve_static("text/css; charset=utf-8", APP_CSS),
        "/assets/app.js" => serve_static("text/javascript; charset=utf-8", APP_JS),
        "/api/stats" => api_stats(state),
        "/api/schema" => api_schema(),
        "/api/i18n" => api_i18n(&request.params),
        "/api/search" => api_search(state, &request.params),
        "/api/count" => api_count(state, &request.params),
        "/api/card" => api_card(state, &request.params),
        "/api/analytics" => api_analytics(state, &request.params),
        "/api/section" => api_section(state, &request.params),
        "/api/company" => api_company(state, &request.params),
        "/api/pivot" => api_pivot(state, &request.params),
        "/api/export-page.csv" => api_export_csv(state, &request.params),
        _ => error_json(404, "Not Found", "Unknown route."),
    }
}

/// Length-checked, content-constant-time comparison so a network observer cannot
/// recover the token byte by byte through timing.
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

// ---------------------------------------------------------------------------
// Static responses
// ---------------------------------------------------------------------------

fn serve_index(token: &str) -> HttpResponse {
    // Inject the session token as a small bootstrap script so the client can send
    // it in the Authorization header. It is never placed in a URL or a cookie.
    let token_json = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".to_string());
    let bootstrap = format!("<script>window.__BASE_SEARCH_TOKEN = {token_json};</script>");
    let html = INDEX_HTML.replace("</head>", &format!("    {bootstrap}\n  </head>"));
    serve_static("text/html; charset=utf-8", &html)
}

fn serve_static(content_type: &'static str, text: &str) -> HttpResponse {
    HttpResponse::new(200, "OK", content_type, text.as_bytes().to_vec())
}

fn json_ok(value: Value) -> HttpResponse {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    HttpResponse::new(200, "OK", "application/json; charset=utf-8", body)
}

fn error_json(status: u16, status_text: &'static str, message: &str) -> HttpResponse {
    let body = serde_json::to_vec(&json!({ "ok": false, "error": message })).unwrap_or_default();
    HttpResponse::new(status, status_text, "application/json; charset=utf-8", body)
}

/// Wraps a handler result, turning an error string into a 500 JSON response.
fn respond(result: Result<Value, String>) -> HttpResponse {
    match result {
        Ok(value) => json_ok(value),
        Err(err) => error_json(500, "Internal Server Error", &err),
    }
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

fn api_stats(state: &WebState) -> HttpResponse {
    respond(with_db(state, |db| {
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
            "total_rows": db.total_rows(),
            "unindexed_rows": db.unindexed_rows(),
            "imports": imports,
        }))
    }))
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
    json_ok(json!({ "ok": true, "columns": columns }))
}

fn api_search(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let query = query_from_params(params);
    let limit = parse_u64(params, "limit", DEFAULT_SEARCH_LIMIT).clamp(1, MAX_SEARCH_LIMIT);
    let page = parse_u64(params, "page", 0);
    let offset = page.saturating_mul(limit);
    let started = Instant::now();

    respond(with_db(state, |db| {
        // Fetch one extra row to learn whether a next page exists.
        let (mut ids, mut rows, mut dups) = db
            .search_page(&query, limit.saturating_add(1), offset)
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
            .map(|(i, cells)| {
                json!({
                    "id": ids.get(i).copied().unwrap_or_default(),
                    "duplicate_of": dups.get(i).cloned().unwrap_or(None),
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
    }))
}

/// Total number of rows matching the query. Called separately from `/api/search`
/// so paging a result page stays fast while the exact total fills in afterwards.
fn api_count(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let query = query_from_params(params);
    respond(with_db(state, |db| {
        let total = db.count(&query).map_err(|err| err.to_string())?;
        Ok(json!({ "ok": true, "total": total }))
    }))
}

fn api_card(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let Some(id) = params.get("id").and_then(|value| value.parse::<i64>().ok()) else {
        return error_json(400, "Bad Request", "Missing or invalid row id.");
    };
    respond(with_db(state, |db| {
        let card = db.record_card(id).map_err(|err| err.to_string())?;
        let mut fields: Vec<Value> = card
            .fields
            .iter()
            .map(|(label, value)| json!({ "label": label, "value": value }))
            .collect();
        // Extra (unmapped) columns from differently shaped files, flagged so the
        // UI can mark them.
        for (label, value) in &card.extra {
            fields.push(json!({ "label": label, "value": value, "extra": true }));
        }
        Ok(json!({
            "ok": true,
            "id": id,
            "source_file": card.source_file,
            "fields": fields,
        }))
    }))
}

fn api_analytics(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let query = query_from_params(params);
    if !analytics_should_run(&query) {
        return json_ok(needs_query("Enter a query or filter to build analytics."));
    }
    let limit = parse_u64(params, "limit", 10).clamp(1, MAX_ANALYTICS_LIMIT);
    let hs_level = parse_u64(params, "hs_level", 10).clamp(2, 10) as u8;
    let scope_param = params.get("scope").map(String::as_str);
    let started = Instant::now();
    respond(with_db(state, |db| {
        // `scope=overview` computes only the overview + monthly dynamics (cheap);
        // a concrete scope computes that one category; no scope computes them all.
        let analytics = match scope_param {
            Some("overview") => db.analytics_scoped(&query, limit, None, hs_level),
            Some(other) => match parse_scope(other) {
                Some(scope) => db.analytics_scoped(&query, limit, Some(scope), hs_level),
                None => db.analytics(&query, limit),
            },
            None => db.analytics(&query, limit),
        }
        .map_err(|err| err.to_string())?;
        Ok(json!({
            "ok": true,
            "needs_query": false,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "analytics": analytics_json(&analytics),
        }))
    }))
}

fn api_section(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let query = query_from_params(params);
    if !analytics_should_run(&query) {
        return json_ok(needs_query("Enter a query or filter to build the list."));
    }
    let Some(kind) = params.get("kind").and_then(|v| parse_section_kind(v)) else {
        return error_json(400, "Bad Request", "Unknown section kind.");
    };
    let hs_level = parse_u64(params, "hs_level", 10).clamp(2, 10) as u8;
    let limit = parse_u64(params, "limit", MAX_SECTION_LIMIT).clamp(1, MAX_SECTION_LIMIT);
    let started = Instant::now();
    respond(with_db(state, |db| {
        let section = db
            .analytics_section(&query, kind, hs_level, limit)
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
    }))
}

fn api_company(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let edrpou = params
        .get("edrpou")
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    if edrpou.is_empty() {
        return error_json(400, "Bad Request", "Missing EDRPOU.");
    }
    let limit = parse_u64(params, "limit", 10).clamp(1, MAX_ANALYTICS_LIMIT);
    respond(with_db(state, |db| {
        let profile = db
            .company_profile(edrpou, limit)
            .map_err(|err| err.to_string())?;
        Ok(json!({ "ok": true, "profile": company_profile_json(&profile) }))
    }))
}

fn api_pivot(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let query = query_from_params(params);
    if !analytics_should_run(&query) {
        return json_ok(needs_query(
            "Enter a query or filter to build the pivot table.",
        ));
    }
    let row_dim = params
        .get("row")
        .and_then(|v| parse_pivot_dim(v))
        .unwrap_or(PivotDim::Recipient);
    let col_dim = params
        .get("col")
        .and_then(|v| parse_pivot_dim(v))
        .unwrap_or(PivotDim::Month);
    let metric = params
        .get("metric")
        .and_then(|v| parse_pivot_metric(v))
        .unwrap_or(PivotMetric::Value);
    respond(with_db(state, |db| {
        let pivot = db
            .pivot(
                &query,
                row_dim,
                col_dim,
                metric,
                PivotLimits { rows: 25, cols: 18 },
                "Other",
            )
            .map_err(|err| err.to_string())?;
        Ok(json!({
            "ok": true,
            "needs_query": false,
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
    }))
}

fn api_export_csv(state: &WebState, params: &HashMap<String, String>) -> HttpResponse {
    let query = query_from_params(params);
    let limit = parse_u64(params, "limit", DEFAULT_SEARCH_LIMIT).clamp(1, MAX_SEARCH_LIMIT);
    let page = parse_u64(params, "page", 0);
    let offset = page.saturating_mul(limit);
    match with_db(state, |db| {
        let (_ids, rows, _dups) = db
            .search_page(&query, limit, offset)
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
                    .map(|v| csv_cell(v))
                    .collect::<Vec<_>>()
                    .join(";"),
            );
            csv.push_str("\r\n");
        }
        Ok(csv)
    }) {
        Ok(csv) => HttpResponse::new(200, "OK", "text/csv; charset=utf-8", csv.into_bytes())
            .header(
                "Content-Disposition",
                "attachment; filename=\"base-search-page.csv\"",
            ),
        Err(err) => error_json(500, "Internal Server Error", &err),
    }
}

/// Translated UI strings and the language list, so the browser interface uses the
/// same translations as the desktop app.
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
    json_ok(json!({
        "ok": true,
        "lang": lang.code(),
        "languages": languages,
        "language_label": t.language,
        "strings": strings,
    }))
}

// ---------------------------------------------------------------------------
// JSON serialization of analytics structures
// ---------------------------------------------------------------------------

fn needs_query(message: &str) -> Value {
    json!({ "ok": true, "needs_query": true, "message": message })
}

fn analytics_json(analytics: &Analytics) -> Value {
    json!({
        "overview": overview_json(&analytics.overview),
        "months": analytics.months.iter().map(month_json).collect::<Vec<_>>(),
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
        "months": profile.months.iter().map(month_json).collect::<Vec<_>>(),
        "product_sections": sections_json(&profile.product_sections),
        "country_sections": sections_json(&profile.country_sections),
        "price_sections": price_metrics_json(&profile.price_sections),
        "top_senders": profile.top_senders.iter().map(group_row_json).collect::<Vec<_>>(),
    })
}

fn month_json(row: &crate::db::AnalyticsMonthRow) -> Value {
    json!({
        "month": &row.month,
        "rows": row.rows,
        "declarations": row.declarations,
        "total_value_usd": row.total_value_usd,
        "total_net_kg": row.total_net_kg,
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
    json!({ "field": filter_field_id(action.field), "value": &action.value })
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

// ---------------------------------------------------------------------------
// Query + parameter parsing
// ---------------------------------------------------------------------------

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
        advanced: None,
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

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), percent_decode(value));
    }
    params
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                    (Some(a), Some(b)) => {
                        out.push((a << 4) | b);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Escapes a cell for `;`-separated CSV, neutralizing spreadsheet formula cells.
fn csv_cell(value: &str) -> String {
    let safe = csv_safe_cell(value);
    let value = safe.as_ref();
    if value.contains(';') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

// ---------------------------------------------------------------------------
// Enum <-> string mapping for query parameters and JSON
// ---------------------------------------------------------------------------

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
        let params = parse_query("q=Apple&year=2024&code=8517&origin=CN&recipient=Demo");
        let query = query_from_params(&params);
        assert_eq!(query.text, "Apple");
        assert_eq!(query.filters.year, "2024");
        assert_eq!(query.filters.product_code, "8517");
        assert_eq!(query.filters.origin_country, "CN");
        assert_eq!(query.filters.recipient, "Demo");
    }

    #[test]
    fn csv_cell_quotes_only_when_needed() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a;b"), "\"a;b\"");
        assert_eq!(csv_cell("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn csv_export_neutralizes_formula_cells() {
        assert_eq!(
            csv_cell("=WEBSERVICE(\"x\")"),
            "\"'=WEBSERVICE(\"\"x\"\")\""
        );
        assert_eq!(csv_cell("+SUM(1,1)"), "'+SUM(1,1)");
    }

    #[test]
    fn loopback_host_validation_rejects_public_binds() {
        assert!(validate_loopback_host("127.0.0.1").is_ok());
        assert!(validate_loopback_host("::1").is_ok());
        assert!(validate_loopback_host("localhost").is_ok());
        assert!(validate_loopback_host("0.0.0.0").is_err());
        assert!(validate_loopback_host("192.168.1.10").is_err());
        assert!(validate_loopback_host("example.com").is_err());
    }

    #[test]
    fn api_routes_require_the_session_token() {
        let state = WebState {
            db_path: PathBuf::from("unused.db"),
            token: "secret".to_string(),
        };
        // Token in the query string is not accepted.
        let request = parse_request("GET /api/schema?token=secret HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(route(&state, &request).status, 403);
        // Token in the Authorization header is accepted.
        let request =
            parse_request("GET /api/schema HTTP/1.1\r\nAuthorization: Bearer secret\r\n\r\n")
                .unwrap();
        assert_eq!(route(&state, &request).status, 200);
    }

    #[test]
    fn generated_tokens_are_random_hex() {
        let first = make_token().unwrap();
        let second = make_token().unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
