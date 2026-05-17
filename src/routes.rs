use crate::config::Config;
use crate::fonts::{self, FontInfo};
use crate::util::{now_secs, VERSION};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

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
