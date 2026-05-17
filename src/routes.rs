//! HTTP route handlers, the two endpoints figma.com calls.
//!
//! Beyond orig's validation chain we additionally restrict the served path
//! to the configured `font_dirs`. orig trusts any absolute path because its
//! client only ever sends paths it received from `/font-files`; CORS=*
//! plus an unrestricted client surface would let a malicious page on
//! figma.com read arbitrary files from disk, so we tighten this.

use crate::config::Config;
use crate::fonts::{self, FontInfo};
use crate::util::{now_secs, VERSION};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

const MAX_PATH_LEN: usize = 4 * 1024;
// orig macOS Figma Agent hardcodes 32 MB, but many shipped macOS fonts
// exceed that (AppleSDGothicNeo.ttc, PingFang.ttc, Apple Color Emoji).
// We raise to 256 MB so large CJK collections aren't unreachable.
const MAX_FONT_SIZE: u64 = 256 * 1024 * 1024;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Serialize)]
struct FontFilesResponse<'a> {
    version: &'static str,
    modified_at: u64,
    modified_fonts: Vec<String>,
    fonts: &'a [FontInfo],
    request_id: u64,
    elapsed_ms: u64,
}

#[derive(Deserialize)]
pub struct FileQuery {
    file: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    detail: String,
    version: &'static str,
    request_id: u64,
}

// request_id is stamped at handler boundary, not on construction.
struct RouteError {
    status: StatusCode,
    error: &'static str,
    detail: String,
}

impl RouteError {
    fn bad_request(error: &'static str, detail: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, error, detail: detail.into() }
    }
    fn not_found(detail: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, error: "Not found", detail: detail.into() }
    }
}

fn error_response(e: RouteError, request_id: u64) -> Response {
    (
        e.status,
        Json(ErrorBody {
            error: e.error,
            detail: e.detail,
            version: VERSION,
            request_id,
        }),
    )
        .into_response()
}

pub async fn font_files(State(cfg): State<Arc<Config>>) -> Response {
    let started = Instant::now();
    let request_id = next_request_id();
    let fonts = fonts::discover(&cfg.font_dirs);
    Json(FontFilesResponse {
        version: VERSION,
        modified_at: now_secs(),
        modified_fonts: Vec::new(),
        fonts: &fonts,
        request_id,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
    .into_response()
}

pub async fn font_file(
    State(cfg): State<Arc<Config>>,
    Query(q): Query<FileQuery>,
) -> Response {
    let request_id = next_request_id();
    match process_font_file(&cfg, &q).await {
        Ok(resp) => resp,
        Err(e) => error_response(e, request_id),
    }
}

async fn process_font_file(cfg: &Config, q: &FileQuery) -> Result<Response, RouteError> {
    validate_query(cfg, q)?;

    let path = PathBuf::from(&q.file);
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| RouteError::not_found(e.to_string()))?;
    if !meta.is_file() {
        return Err(RouteError::not_found("not a regular file"));
    }
    if meta.len() > MAX_FONT_SIZE {
        return Err(RouteError::bad_request(
            "Invalid path",
            format!("font file exceeds {MAX_FONT_SIZE} bytes"),
        ));
    }

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| RouteError::not_found(e.to_string()))?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(bytes),
    )
        .into_response())
}

fn validate_query(cfg: &Config, q: &FileQuery) -> Result<(), RouteError> {
    if q.file.is_empty() {
        return Err(RouteError::bad_request(
            "resource name should not be empty",
            "file param missing or empty",
        ));
    }
    if q.file.len() > MAX_PATH_LEN {
        return Err(RouteError::bad_request(
            "Invalid path",
            format!("file path is {} bytes (max {MAX_PATH_LEN})", q.file.len()),
        ));
    }
    if !q.file.starts_with('/') {
        return Err(RouteError::bad_request(
            "Invalid path",
            "file path must be absolute",
        ));
    }
    // Broader than Components::ParentDir; also catches `..namedfork/rsrc`.
    if q.file.contains("..") {
        return Err(RouteError::bad_request(
            "Invalid path",
            "file path contains `..` segment",
        ));
    }

    let path = Path::new(&q.file);
    if !cfg.font_dirs.iter().any(|(dir, _)| path.starts_with(dir)) {
        return Err(RouteError::bad_request(
            "Invalid path",
            "file path is outside configured font_dirs",
        ));
    }
    Ok(())
}
