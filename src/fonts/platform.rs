//! macOS sources its candidate font paths from
//! `CTFontManagerCopyAvailableFontURLs` (excludes hidden families like
//! Athelas / Iowan / STIXGeneral) and parses each file with ttf-parser.
//! Linux uses `fc-list` for the same role.

use super::FontFiles;
use std::path::PathBuf;

pub(super) fn enumerate(dirs: &[(PathBuf, bool)]) -> FontFiles {
    #[cfg(target_os = "macos")]
    {
        let _ = dirs;
        macos::enumerate()
    }
    #[cfg(not(target_os = "macos"))]
    {
        linux::enumerate(dirs)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::FontFiles;
    use crate::fonts::{dirs, parser, FaceInfo};
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_foundation::url::CFURL;
    use core_text::font_descriptor::{
        kCTFontFamilyNameAttribute, kCTFontNameAttribute, kCTFontStyleNameAttribute,
        CTFontDescriptor, CTFontDescriptorCopyAttribute,
    };
    use core_text::font_manager::{
        CTFontManagerCopyAvailableFontURLs, CTFontManagerCreateFontDescriptorsFromURL,
    };
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Mach-O resource-fork suffix CT bakes into URLs for `.dfont`-style
    /// resource files; the on-disk file lives at the path without it.
    const RSRC_SUFFIX: &str = "/..namedfork/rsrc";

    fn ct_font_paths() -> Vec<PathBuf> {
        let urls: CFArray<CFURL> =
            unsafe { TCFType::wrap_under_create_rule(CTFontManagerCopyAvailableFontURLs()) };
        let mut paths = Vec::with_capacity(urls.len() as usize);
        let mut seen = HashSet::new();
        for url in urls.iter() {
            let Some(p) = url.to_path() else { continue };
            let p = match p.to_string_lossy().strip_suffix(RSRC_SUFFIX) {
                Some(stripped) => PathBuf::from(stripped),
                None => p,
            };
            if seen.insert(p.clone()) {
                paths.push(p);
            }
        }
        paths
    }

    /// Read a CFString-valued descriptor attribute. Returns the empty
    /// string when the attribute is missing or non-string, matching the
    /// behaviour of the pre-refactor manual FFI helper.
    fn copy_string_attr(
        desc: &CTFontDescriptor,
        key: core_foundation::string::CFStringRef,
    ) -> String {
        use core_foundation::base::CFType;
        unsafe {
            let value = CTFontDescriptorCopyAttribute(desc.as_concrete_TypeRef(), key);
            if value.is_null() {
                return String::new();
            }
            let any = CFType::wrap_under_create_rule(value);
            if !any.instance_of::<CFString>() {
                return String::new();
            }
            CFString::wrap_under_get_rule(value as _).to_string()
        }
    }

    /// Last-resort face info for files ttf-parser can't open (e.g.
    /// `NISC18030.ttf`, an Apple bitmap font with no `head` table). CT
    /// happily parses these; copy the three name attributes and use
    /// generic regular-weight defaults.
    fn ct_fallback_faces(path: &Path, user_installed: bool) -> Vec<FaceInfo> {
        let Some(url) = CFURL::from_path(path, false) else {
            return Vec::new();
        };
        let descs: CFArray<CTFontDescriptor> = unsafe {
            let raw = CTFontManagerCreateFontDescriptorsFromURL(url.as_concrete_TypeRef());
            if raw.is_null() {
                return Vec::new();
            }
            TCFType::wrap_under_create_rule(raw)
        };
        let modified_at = parser::file_ctime(path);
        descs
            .iter()
            .filter_map(|desc| {
                let family = copy_string_attr(&desc, unsafe { kCTFontFamilyNameAttribute });
                if family.is_empty() || family.starts_with('.') {
                    return None;
                }
                let postscript = copy_string_attr(&desc, unsafe { kCTFontNameAttribute });
                let mut style = copy_string_attr(&desc, unsafe { kCTFontStyleNameAttribute });
                if style.is_empty() {
                    style = "Regular".to_string();
                }
                Some(FaceInfo {
                    family,
                    style,
                    postscript,
                    weight: 400,
                    stretch: 5,
                    italic: false,
                    variation_axes: Vec::new(),
                    modified_at,
                    user_installed,
                })
            })
            .collect()
    }

    pub(super) fn enumerate() -> FontFiles {
        let mut out = FontFiles::new();
        for path in ct_font_paths() {
            let user_installed = dirs::classify_user_installed(&path);
            let faces = match parser::read_font_file(&path, user_installed) {
                Ok(f) if !f.is_empty() => f,
                _ => ct_fallback_faces(&path, user_installed),
            };
            if !faces.is_empty() {
                out.insert(path.to_string_lossy().into_owned(), faces);
            }
        }
        out
    }
}

#[cfg(not(target_os = "macos"))]
mod linux {
    use super::FontFiles;
    use crate::fonts::{dirs, parser};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use walkdir::WalkDir;

    pub(super) fn enumerate(dirs_cfg: &[(PathBuf, bool)]) -> FontFiles {
        let mut candidates: HashMap<PathBuf, bool> = HashMap::new();
        for path in linux_font_paths() {
            if !parser::is_font_file(&path) || !path.exists() {
                continue;
            }
            let user_installed = dirs::classify_user_installed(&path);
            candidates.entry(path).or_insert(user_installed);
        }
        for (dir, user_installed) in dirs_cfg {
            for entry in WalkDir::new(dir).follow_links(true).into_iter().filter_map(|e| e.ok()) {
                let path = entry.path();
                if !parser::is_font_file(path) {
                    continue;
                }
                candidates.insert(path.to_path_buf(), *user_installed);
            }
        }
        let mut out = FontFiles::new();
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

    fn linux_font_paths() -> Vec<PathBuf> {
        let output = std::process::Command::new("fc-list")
            .arg("-f")
            .arg("%{file}\n")
            .output();
        let stdout = match output {
            Ok(o) if o.status.success() => o.stdout,
            Ok(_) => {
                tracing::warn!("fc-list exited non-zero; relying on font_dirs only");
                return Vec::new();
            }
            Err(e) => {
                tracing::warn!(error = %e, "fc-list not available; relying on font_dirs only");
                return Vec::new();
            }
        };
        String::from_utf8_lossy(&stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect()
    }
}
