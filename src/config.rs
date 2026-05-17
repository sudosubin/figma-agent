//! `font_dirs` accepts either a raw path (system, `user_installed=false`)
//! or `{ "path": "...", "user_installed": true }` for per-directory override.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_tls_port")]
    pub tls_port: Option<u16>,
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
    #[serde(default = "default_font_dirs", deserialize_with = "deser_font_dirs")]
    pub font_dirs: Vec<(PathBuf, bool)>,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    44950
}

fn default_tls_port() -> Option<u16> {
    Some(44951)
}

fn default_font_dirs() -> Vec<(PathBuf, bool)> {
    crate::fonts::default_font_dirs()
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FontDirEntry {
    Plain(PathBuf),
    Detailed {
        path: PathBuf,
        #[serde(default)]
        user_installed: bool,
    },
}

fn deser_font_dirs<'de, D>(d: D) -> Result<Vec<(PathBuf, bool)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let entries = Vec::<FontDirEntry>::deserialize(d)?;
    Ok(entries
        .into_iter()
        .map(|e| match e {
            FontDirEntry::Plain(p) => (p, false),
            FontDirEntry::Detailed { path, user_installed } => (path, user_installed),
        })
        .collect())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            tls_port: default_tls_port(),
            tls_cert: None,
            tls_key: None,
            font_dirs: default_font_dirs(),
        }
    }
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let path = explicit
            .map(|p| p.to_path_buf())
            .or_else(default_config_path);
        match path {
            Some(p) if p.exists() => {
                let bytes = std::fs::read(&p)
                    .with_context(|| format!("reading config {}", p.display()))?;
                let cfg: Self = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing config {}", p.display()))?;
                tracing::info!(path = %p.display(), "loaded config");
                Ok(cfg)
            }
            _ => {
                tracing::info!("no config file; using defaults");
                Ok(Self::default())
            }
        }
    }
}

fn default_config_path() -> Option<PathBuf> {
    crate::util::config_dir().map(|d| d.join("config.json"))
}
