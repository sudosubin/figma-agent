//! `/figma/font-files` returns the enumerated catalogue in upstream Figma
//! agent's exact shape. `/figma/font-file` streams the bytes of a single
//! font path.
//!
//! Origin filtering and OPTIONS handling live in `server::cors_middleware`.
//! By the time these handlers run, the request has already been verified
//! to carry an allowed `Origin` header.

use crate::config::Config;
use crate::fonts;
use crate::util::{machine_id, UPSTREAM_API_VERSION, UPSTREAM_PACKAGE};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const MAX_PATH_LEN: usize = 4 * 1024;
// Upstream caps at 32 MB; we lift to 256 MB so large CJK collections
// (AppleSDGothicNeo.ttc, PingFang.ttc, Apple Color Emoji) remain reachable.
const MAX_FONT_SIZE: u64 = 256 * 1024 * 1024;

fn cached_machine_id() -> &'static str {
    static MID: OnceLock<String> = OnceLock::new();
    MID.get_or_init(machine_id)
}

#[derive(Serialize)]
struct FontFilesResponse<'a> {
    version: u32,
    package: &'static str,
    modified_at: Option<u64>,
    modified_fonts: Option<Vec<String>>,
    machine_id: &'static str,
    launch_source: &'static str,
    #[serde(rename = "fontFiles")]
    font_files: &'a fonts::FontFiles,
}

pub async fn font_files(State(cfg): State<Arc<Config>>) -> Response {
    let font_files = fonts::discover(&cfg.font_dirs);
    Json(FontFilesResponse {
        version: UPSTREAM_API_VERSION,
        package: UPSTREAM_PACKAGE,
        modified_at: None,
        modified_fonts: None,
        machine_id: cached_machine_id(),
        launch_source: "other",
        font_files: &font_files,
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct FileQuery {
    file: String,
}

pub async fn font_file(
    State(cfg): State<Arc<Config>>,
    Query(q): Query<FileQuery>,
) -> Response {
    match serve_file(cfg.as_ref(), &q).await {
        Ok(resp) => resp,
        Err(status) => status.into_response(),
    }
}

async fn serve_file(cfg: &Config, q: &FileQuery) -> Result<Response, StatusCode> {
    if q.file.is_empty() || q.file.len() > MAX_PATH_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !q.file.starts_with('/') || q.file.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = PathBuf::from(&q.file);
    if !cfg.font_dirs.iter().any(|(dir, _)| path.starts_with(dir)) {
        return Err(StatusCode::NOT_FOUND);
    }
    let meta = tokio::fs::metadata(&path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    if !meta.is_file() || meta.len() > MAX_FONT_SIZE {
        return Err(StatusCode::NOT_FOUND);
    }
    let bytes = tokio::fs::read(&path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(bytes),
    )
        .into_response())
}
