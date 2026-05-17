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

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            tls_port: default_tls_port(),
            tls_cert: None,
            tls_key: None,
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
