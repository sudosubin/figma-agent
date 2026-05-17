//! Discovery merges the OS font registry (CoreText / fc-list) with
//! configured `font_dirs`. Variable fonts with fvar named-instances emit
//! one `FaceInfo` per instance, matching CoreText on the upstream agent.
//!
//! `fontFiles` is a path-keyed map (one path -> many faces) to mirror
//! upstream's `/figma/font-files` response shape exactly.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceInfo {
    pub family: String,
    pub style: String,
    pub postscript: String,
    pub weight: u16,
    pub stretch: u8,
    pub italic: bool,
    #[serde(
        rename = "variationAxes",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub variation_axes: Vec<AxisInfo>,
    pub modified_at: u64,
    pub user_installed: bool,
}

/// Path -> faces parsed from that file (TTC face / fvar instance flattened).
pub type FontFiles = HashMap<String, Vec<FaceInfo>>;

static CACHE: OnceLock<Arc<FontFiles>> = OnceLock::new();

/// Disk cache is keyed by `CARGO_PKG_VERSION`; to refresh after editing
/// `font_dirs`, delete the cache file or bump the daemon version.
pub fn discover(dirs: &[(PathBuf, bool)]) -> Arc<FontFiles> {
    CACHE
        .get_or_init(|| {
            let fonts = match cache::load() {
                Some(c) => {
                    tracing::info!(paths = c.fonts.len(), "loaded font cache from disk");
                    c.fonts
                }
                None => {
                    tracing::info!("enumerating fonts");
                    let fonts = enumerate(dirs);
                    cache::save(&fonts);
                    tracing::info!(paths = fonts.len(), "enumerated fonts");
                    fonts
                }
            };
            Arc::new(fonts)
        })
        .clone()
}

fn enumerate(dirs: &[(PathBuf, bool)]) -> FontFiles {
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

    let mut out: FontFiles = HashMap::with_capacity(candidates.len());
    for (path, user_installed) in candidates {
        match parser::read_font_file(&path, user_installed) {
            Ok(faces) if !faces.is_empty() => {
                out.insert(path.to_string_lossy().into_owned(), faces);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "skip font"),
        }
    }
    out
}
