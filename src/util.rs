use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `$XDG_CONFIG_HOME/figma-agent` or `~/.config/figma-agent`. Used on
/// macOS too so the user has one config location across platforms; Apple
/// HIG suggests `~/Library/Application Support/` but CLI daemons commonly
/// follow XDG (gh, claude, brew tools, etc.).
pub fn config_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_CACHE_HOME/figma-agent` or `~/.cache/figma-agent`.
pub fn cache_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

fn xdg_dir(env_var: &str, home_subdir: &str) -> Option<PathBuf> {
    let base = std::env::var_os(env_var)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(home_subdir)))?;
    Some(base.join("figma-agent"))
}
