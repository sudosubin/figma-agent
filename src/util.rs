use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Upstream Figma desktop version we shape our responses after.
pub const UPSTREAM_PACKAGE: &str = "126.4.11";

/// HTTP response schema version (the integer `version` field on /figma/font-files).
pub const UPSTREAM_API_VERSION: u32 = 24;

/// Salt used in the upstream agent to derive `machine_id` from the platform UUID.
const MACHINE_ID_SALT: &str = "figma_agent_machine_id_salt_v1";

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

/// SHA-256(IOPlatformUUID + salt) hex string; matches upstream exactly on
/// macOS. On Linux there is no IOPlatformUUID; we fall back to
/// `/etc/machine-id` with the same salt, which upstream does not generate
/// the same way, so machine_id parity is macOS-only.
pub fn machine_id() -> String {
    let uuid = platform_uuid().unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(uuid.as_bytes());
    hasher.update(MACHINE_ID_SALT.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(target_os = "macos")]
fn platform_uuid() -> Option<String> {
    let out = std::process::Command::new("ioreg")
        .args(["-d2", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if !line.contains("IOPlatformUUID") {
            continue;
        }
        // Format: '    "IOPlatformUUID" = "XXXX-XXXX-..."'
        let mut quotes = line.match_indices('"');
        let (_, _) = quotes.next()?;
        let (_, _) = quotes.next()?;
        let (start, _) = quotes.next()?;
        let (end, _) = quotes.next()?;
        return Some(line[start + 1..end].to_string());
    }
    None
}

#[cfg(target_os = "linux")]
fn platform_uuid() -> Option<String> {
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_uuid() -> Option<String> {
    None
}
