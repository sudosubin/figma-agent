//! Discovery merges the OS font registry (CoreText / fc-list) with
//! configured `font_dirs`. Variable fonts with fvar named-instances emit
//! one `FontInfo` per instance, matching CoreText on the orig macOS agent.

mod cache;
mod dirs;
mod parser;
mod platform;

pub use dirs::default_font_dirs;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisInfo {
    pub tag: String,
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub hidden: bool,
}

/// `path` is our addition; orig's client knows paths out-of-band, but a
/// browser client needs it for the `/font-file?file=...` follow-up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontInfo {
    pub family: String,
    pub style: String,
    pub postscript: String,
    pub weight: f64,
    pub stretch: f64,
    pub italic: bool,
    #[serde(rename = "variationAxes")]
    pub variation_axes: Vec<AxisInfo>,
    pub user_installed: bool,
    pub name: String,
    pub path: String,
}

static CACHE: OnceLock<Arc<Vec<FontInfo>>> = OnceLock::new();

/// Disk cache is keyed by `CARGO_PKG_VERSION`; to refresh after editing
/// `font_dirs`, delete the cache file or bump the daemon version.
pub fn discover(dirs: &[(PathBuf, bool)]) -> Arc<Vec<FontInfo>> {
    CACHE
        .get_or_init(|| {
            let fonts = match cache::load() {
                Some(c) => {
                    tracing::info!(count = c.fonts.len(), "loaded font cache from disk");
                    c.fonts
                }
                None => {
                    tracing::info!("enumerating fonts");
                    let fonts = enumerate(dirs);
                    cache::save(&fonts);
                    tracing::info!(count = fonts.len(), "enumerated fonts");
                    fonts
                }
            };
            Arc::new(fonts)
        })
        .clone()
}

fn enumerate(dirs: &[(PathBuf, bool)]) -> Vec<FontInfo> {
    let mut candidates: HashMap<PathBuf, bool> = HashMap::new();

    for path in platform::system_font_paths() {
        if !parser::is_font_file(&path) || !path.exists() {
            continue;
        }
        let user_installed = dirs::classify_user_installed(&path);
        candidates.entry(path).or_insert(user_installed);
    }

    for (dir, user_installed) in dirs {
        for entry in WalkDir::new(dir).follow_links(true).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !parser::is_font_file(path) {
                continue;
            }
            // `insert` overrides; explicit config beats $HOME heuristic.
            candidates.insert(path.to_path_buf(), *user_installed);
        }
    }

    let mut out = Vec::new();
    for (path, user_installed) in candidates {
        match parser::read_font_file(&path, user_installed) {
            Ok(mut entries) => out.append(&mut entries),
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "skip font"),
        }
    }
    out
}
