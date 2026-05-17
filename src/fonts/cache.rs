//! Same JSON shape as orig macOS's `font_cache.json` plus the `path` field
//! we add on each `FontInfo`.

use super::FontInfo;
use crate::util::{cache_dir, now_secs, VERSION};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
pub(super) struct DiskCache {
    version: String,
    fonts_cached_at: u64,
    modified_at: u64,
    modified_fonts: Vec<String>,
    pub(super) fonts: Vec<FontInfo>,
}

fn cache_path() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("font_cache.json"))
}

pub(super) fn load() -> Option<DiskCache> {
    let path = cache_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let cache: DiskCache = serde_json::from_slice(&bytes).ok()?;
    if cache.version != VERSION {
        tracing::info!(
            cached = %cache.version,
            current = %VERSION,
            "font cache version mismatch, re-enumerating"
        );
        return None;
    }
    Some(cache)
}

pub(super) fn save(fonts: &[FontInfo]) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, "failed to create cache dir");
            return;
        }
    }
    let now = now_secs();
    let cache = DiskCache {
        version: VERSION.to_string(),
        fonts_cached_at: now,
        modified_at: now,
        modified_fonts: Vec::new(),
        fonts: fonts.to_vec(),
    };
    let data = match serde_json::to_vec(&cache) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize font cache");
            return;
        }
    };
    match std::fs::write(&path, data) {
        Ok(()) => tracing::info!(path = %path.display(), "wrote font cache"),
        Err(e) => tracing::warn!(error = %e, "failed to write font cache"),
    }
}
