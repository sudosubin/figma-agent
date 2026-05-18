//! Disk-side persistence. The on-disk shape is our own (keyed by daemon
//! version) and is independent from the wire response.

use super::FontFiles;
use crate::util::{cache_dir, now_secs, VERSION};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
pub(super) struct DiskCache {
    version: String,
    fonts_cached_at: u64,
    pub(super) fonts: FontFiles,
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

pub(super) fn save(fonts: &FontFiles) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, "failed to create cache dir");
            return;
        }
    }
    let cache = DiskCache {
        version: VERSION.to_string(),
        fonts_cached_at: now_secs(),
        fonts: fonts.clone(),
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
