use std::path::{Path, PathBuf};

pub fn default_font_dirs() -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = Vec::new();

    #[cfg(target_os = "linux")]
    {
        out.push((PathBuf::from("/usr/share/fonts"), false));
        out.push((PathBuf::from("/usr/local/share/fonts"), false));
        if let Some(home) = home_dir() {
            out.push((home.join(".fonts"), true));
            out.push((home.join(".local/share/fonts"), true));
        }
    }

    #[cfg(target_os = "macos")]
    {
        out.push((PathBuf::from("/System/Library/Fonts"), false));
        out.push((PathBuf::from("/System/Library/Fonts/Supplemental"), false));
        out.push((PathBuf::from("/Library/Fonts"), false));
        if let Some(home) = home_dir() {
            out.push((home.join("Library/Fonts"), true));
            out.push((home.join("Library/Application Support/Fonts"), true));
            // Font Book parks disabled faces here; some apps still surface them.
            out.push((home.join("Library/Fonts Disabled"), true));
        }
    }

    out.retain(|(p, _)| p.exists());
    out
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
