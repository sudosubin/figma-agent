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
    use std::collections::HashSet;
    use std::os::raw::c_void;
    use std::path::{Path, PathBuf};

    type CFTypeRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFURLRef = *const c_void;
    type CTFontDescriptorRef = *const c_void;

    const KCFURL_POSIX_PATH_STYLE: u32 = 0;
    const KCFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;
    /// Mach-O resource-fork suffix CT bakes into URLs for `.dfont`-style
    /// resource files; the on-disk file lives at the path without it.
    const RSRC_SUFFIX: &str = "/..namedfork/rsrc";

    #[link(name = "CoreText", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CTFontManagerCopyAvailableFontURLs() -> CFArrayRef;
        fn CTFontManagerCreateFontDescriptorsFromURL(url: CFURLRef) -> CFArrayRef;
        fn CTFontDescriptorCopyAttribute(
            descriptor: CTFontDescriptorRef,
            attribute: CFStringRef,
        ) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(arr: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: isize) -> *const c_void;
        fn CFURLCopyFileSystemPath(url: CFURLRef, style: u32) -> CFStringRef;
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: *const c_void,
            buf: *const u8,
            len: isize,
            is_directory: bool,
        ) -> CFURLRef;
        fn CFStringGetLength(s: CFStringRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, buf_size: isize, enc: u32) -> bool;

        static kCTFontFamilyNameAttribute: CFStringRef;
        static kCTFontNameAttribute: CFStringRef;
        static kCTFontStyleNameAttribute: CFStringRef;
    }

    unsafe fn cfstr_to_string(s: CFStringRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let len = CFStringGetLength(s);
        let max = CFStringGetMaximumSizeForEncoding(len, KCFSTRING_ENCODING_UTF8) + 1;
        let mut buf = vec![0u8; max as usize];
        if !CFStringGetCString(s, buf.as_mut_ptr(), max, KCFSTRING_ENCODING_UTF8) {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0)?;
        std::str::from_utf8(&buf[..end]).ok().map(|s| s.to_string())
    }

    unsafe fn cfurl_to_path(url: CFURLRef) -> Option<PathBuf> {
        if url.is_null() {
            return None;
        }
        let s = CFURLCopyFileSystemPath(url, KCFURL_POSIX_PATH_STYLE);
        if s.is_null() {
            return None;
        }
        let out = cfstr_to_string(s);
        CFRelease(s);
        let s = out?;
        let trimmed = s.strip_suffix(RSRC_SUFFIX).unwrap_or(&s);
        Some(PathBuf::from(trimmed))
    }

    fn ct_font_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        unsafe {
            let urls = CTFontManagerCopyAvailableFontURLs();
            if urls.is_null() {
                return paths;
            }
            for i in 0..CFArrayGetCount(urls) {
                let url = CFArrayGetValueAtIndex(urls, i) as CFURLRef;
                if let Some(p) = cfurl_to_path(url) {
                    if seen.insert(p.clone()) {
                        paths.push(p);
                    }
                }
            }
            CFRelease(urls);
        }
        paths
    }

    /// Last-resort face info for files ttf-parser can't open (e.g.
    /// `NISC18030.ttf`, an Apple bitmap font with no `head` table). CT
    /// happily parses these; we copy the three name attributes and use
    /// generic regular-weight defaults.
    unsafe fn ct_fallback_faces(path: &Path, user_installed: bool) -> Vec<FaceInfo> {
        let bytes = path.as_os_str().as_encoded_bytes();
        let cf_url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            false,
        );
        if cf_url.is_null() {
            return Vec::new();
        }
        let descs = CTFontManagerCreateFontDescriptorsFromURL(cf_url);
        CFRelease(cf_url);
        if descs.is_null() {
            return Vec::new();
        }
        let modified_at = parser::file_ctime(path);
        let mut out = Vec::new();
        for j in 0..CFArrayGetCount(descs) {
            let desc = CFArrayGetValueAtIndex(descs, j) as CTFontDescriptorRef;
            if desc.is_null() {
                continue;
            }
            let family = copy_string_attr(desc, kCTFontFamilyNameAttribute);
            if family.is_empty() || family.starts_with('.') {
                continue;
            }
            let postscript = copy_string_attr(desc, kCTFontNameAttribute);
            let style = {
                let s = copy_string_attr(desc, kCTFontStyleNameAttribute);
                if s.is_empty() { "Regular".to_string() } else { s }
            };
            out.push(FaceInfo {
                family,
                style,
                postscript,
                weight: 400,
                stretch: 5,
                italic: false,
                variation_axes: Vec::new(),
                modified_at,
                user_installed,
            });
        }
        CFRelease(descs);
        out
    }

    unsafe fn copy_string_attr(desc: CTFontDescriptorRef, key: CFStringRef) -> String {
        let v = CTFontDescriptorCopyAttribute(desc, key);
        if v.is_null() {
            return String::new();
        }
        let out = cfstr_to_string(v as CFStringRef).unwrap_or_default();
        CFRelease(v);
        out
    }

    pub(super) fn enumerate() -> FontFiles {
        let mut out = FontFiles::new();
        for path in ct_font_paths() {
            let user_installed = dirs::classify_user_installed(&path);
            let faces = match parser::read_font_file(&path, user_installed) {
                Ok(f) if !f.is_empty() => f,
                _ => unsafe { ct_fallback_faces(&path, user_installed) },
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
