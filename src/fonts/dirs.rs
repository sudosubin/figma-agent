use std::path::{Path, PathBuf};

pub fn default_font_dirs() -> Vec<(PathBuf, bool)> {
    // The system registry (CoreText / fc-list) is authoritative; we only
    // walk extra `font_dirs` entries when the user opts in via config.
    Vec::new()
}

pub(super) fn classify_user_installed(path: &Path) -> bool {
    let Some(home) = home_dir() else { return false };

    #[cfg(target_os = "macos")]
    {
        path.starts_with(home.join("Library/Fonts"))
            || path.starts_with(home.join("Library/Application Support/Fonts"))
            || path.starts_with(home.join("Library/Fonts Disabled"))
    }
    #[cfg(target_os = "linux")]
    {
        path.starts_with(&home)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = home;
        false
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}
