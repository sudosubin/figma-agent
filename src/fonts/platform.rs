//! macOS uses CoreText for every face attribute (family/style/postscript,
//! weight/stretch/italic from `kCTFontTraitsAttribute`, axes from
//! `CTFontCopyVariationAxes`) so the output matches upstream byte-for-byte.
//! Linux still relies on `fc-list` + `ttf-parser` since no CoreText is
//! available.

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
    use crate::fonts::{AxisInfo, FaceInfo, FontFiles};
    use std::os::raw::{c_double, c_void};
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    type CFTypeRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFURLRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFNumberRef = *const c_void;
    type CFBooleanRef = *const c_void;
    type CTFontDescriptorRef = *const c_void;
    type CTFontRef = *const c_void;

    const KCFURL_POSIX_PATH_STYLE: u32 = 0;
    const KCFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const KCF_NUMBER_FLOAT64_TYPE: i32 = 13;
    const KCF_NUMBER_SINT32_TYPE: i32 = 3;
    const RSRC_SUFFIX: &str = "/..namedfork/rsrc";
    const KCT_FONT_TRAIT_ITALIC: u32 = 1;

    #[link(name = "CoreText", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CTFontManagerCopyAvailableFontURLs() -> CFArrayRef;
        fn CTFontManagerCreateFontDescriptorsFromURL(url: CFURLRef) -> CFArrayRef;
        fn CTFontDescriptorCopyAttribute(
            descriptor: CTFontDescriptorRef,
            attribute: CFStringRef,
        ) -> CFTypeRef;
        fn CTFontCreateWithFontDescriptor(
            descriptor: CTFontDescriptorRef,
            size: c_double,
            matrix: *const c_void,
        ) -> CTFontRef;
        fn CTFontCopyVariationAxes(font: CTFontRef) -> CFArrayRef;

        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(arr: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: isize) -> *const c_void;
        fn CFURLCopyFileSystemPath(url: CFURLRef, style: u32) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, buf_size: isize, enc: u32) -> bool;
        fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(num: CFNumberRef, ty: i32, val: *mut c_void) -> bool;
        fn CFBooleanGetValue(b: CFBooleanRef) -> bool;

        static kCTFontFamilyNameAttribute: CFStringRef;
        static kCTFontNameAttribute: CFStringRef;
        static kCTFontStyleNameAttribute: CFStringRef;
        static kCTFontTraitsAttribute: CFStringRef;
        static kCTFontVariationAttribute: CFStringRef;
        static kCTFontVariationAxisIdentifierKey: CFStringRef;
        static kCTFontVariationAxisNameKey: CFStringRef;
        static kCTFontVariationAxisMinimumValueKey: CFStringRef;
        static kCTFontVariationAxisDefaultValueKey: CFStringRef;
        static kCTFontVariationAxisMaximumValueKey: CFStringRef;
        static kCTFontVariationAxisHiddenKey: CFStringRef;
        static kCTFontWeightTrait: CFStringRef;
        static kCTFontWidthTrait: CFStringRef;
        static kCTFontSymbolicTrait: CFStringRef;
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

    unsafe fn copy_string_attr(desc: CTFontDescriptorRef, key: CFStringRef) -> String {
        let v = CTFontDescriptorCopyAttribute(desc, key);
        if v.is_null() {
            return String::new();
        }
        let out = cfstr_to_string(v as CFStringRef).unwrap_or_default();
        CFRelease(v);
        out
    }

    unsafe fn cfnum_to_f64(n: CFTypeRef) -> Option<f64> {
        if n.is_null() {
            return None;
        }
        let mut v = 0.0_f64;
        if CFNumberGetValue(
            n as CFNumberRef,
            KCF_NUMBER_FLOAT64_TYPE,
            &mut v as *mut f64 as *mut c_void,
        ) {
            Some(v)
        } else {
            None
        }
    }

    unsafe fn cfnum_to_i32(n: CFTypeRef) -> Option<i32> {
        if n.is_null() {
            return None;
        }
        let mut v = 0i32;
        if CFNumberGetValue(
            n as CFNumberRef,
            KCF_NUMBER_SINT32_TYPE,
            &mut v as *mut i32 as *mut c_void,
        ) {
            Some(v)
        } else {
            None
        }
    }

    unsafe fn dict_get(dict: CFTypeRef, key: CFStringRef) -> CFTypeRef {
        if dict.is_null() {
            return std::ptr::null();
        }
        CFDictionaryGetValue(dict as CFDictionaryRef, key as *const c_void)
    }

    /// Apple's CT weight floats nominally cover [-1.0, 1.0]; the bucket
    /// centres below are the values Apple ships in its docs / SDK.
    fn ct_weight_to_int(w: f64) -> u16 {
        const BUCKETS: &[(f64, u16)] = &[
            (-0.8, 100),
            (-0.6, 200),
            (-0.4, 300),
            (0.0, 400),
            (0.23, 500),
            (0.3, 600),
            (0.4, 700),
            (0.56, 800),
            (0.62, 900),
        ];
        let mut best = 400u16;
        let mut best_d = f64::INFINITY;
        for &(t, v) in BUCKETS {
            let d = (w - t).abs();
            if d < best_d {
                best_d = d;
                best = v;
            }
        }
        best
    }

    /// CT width float -> OS/2 usWidthClass bucket.
    fn ct_width_to_int(w: f64) -> u8 {
        const BUCKETS: &[(f64, u8)] = &[
            (-1.0, 1),
            (-0.75, 2),
            (-0.5, 3),
            (-0.25, 4),
            (0.0, 5),
            (0.25, 6),
            (0.5, 7),
            (0.75, 8),
            (1.0, 9),
        ];
        let mut best = 5u8;
        let mut best_d = f64::INFINITY;
        for &(t, v) in BUCKETS {
            let d = (w - t).abs();
            if d < best_d {
                best_d = d;
                best = v;
            }
        }
        best
    }

    fn ctime_secs(path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.ctime() as u64).unwrap_or(0)
    }

    fn tag_to_str(tag: u32) -> String {
        let b = tag.to_be_bytes();
        String::from_utf8_lossy(&b).into_owned()
    }

    unsafe fn extract_axes(desc: CTFontDescriptorRef) -> Vec<AxisInfo> {
        let font = CTFontCreateWithFontDescriptor(desc, 12.0, std::ptr::null());
        if font.is_null() {
            return Vec::new();
        }
        let arr = CTFontCopyVariationAxes(font);
        CFRelease(font);
        if arr.is_null() {
            return Vec::new();
        }
        // Per-instance axis values (only set on named-instance descriptors).
        let variation = CTFontDescriptorCopyAttribute(desc, kCTFontVariationAttribute);

        let n = CFArrayGetCount(arr);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let d = CFArrayGetValueAtIndex(arr, i);
            if d.is_null() {
                continue;
            }
            let id_ref = dict_get(d, kCTFontVariationAxisIdentifierKey);
            let tag_num = cfnum_to_i32(id_ref).unwrap_or(0) as u32;
            let name = cfstr_to_string(dict_get(d, kCTFontVariationAxisNameKey) as CFStringRef)
                .unwrap_or_default();
            let min = cfnum_to_f64(dict_get(d, kCTFontVariationAxisMinimumValueKey)).unwrap_or(0.0);
            let default = cfnum_to_f64(dict_get(d, kCTFontVariationAxisDefaultValueKey)).unwrap_or(0.0);
            let max = cfnum_to_f64(dict_get(d, kCTFontVariationAxisMaximumValueKey)).unwrap_or(0.0);
            let hidden_ref = dict_get(d, kCTFontVariationAxisHiddenKey);
            let hidden = !hidden_ref.is_null() && CFBooleanGetValue(hidden_ref as CFBooleanRef);

            let value = if !variation.is_null() && !id_ref.is_null() {
                let v = CFDictionaryGetValue(variation as CFDictionaryRef, id_ref);
                cfnum_to_f64(v).unwrap_or(default)
            } else {
                default
            };

            out.push(AxisInfo {
                tag: tag_to_str(tag_num),
                name,
                value,
                min,
                max,
                default,
                hidden,
            });
        }
        if !variation.is_null() {
            CFRelease(variation);
        }
        CFRelease(arr);
        out
    }

    unsafe fn build_face(
        desc: CTFontDescriptorRef,
        modified_at: u64,
        user_installed: bool,
    ) -> Option<FaceInfo> {
        let family = copy_string_attr(desc, kCTFontFamilyNameAttribute);
        if family.is_empty() || family.starts_with('.') {
            return None;
        }
        let postscript = copy_string_attr(desc, kCTFontNameAttribute);
        let style_raw = copy_string_attr(desc, kCTFontStyleNameAttribute);
        let style = if style_raw.is_empty() {
            "Regular".to_string()
        } else {
            style_raw
        };

        let traits = CTFontDescriptorCopyAttribute(desc, kCTFontTraitsAttribute);
        let (weight, stretch, italic) = if !traits.is_null() {
            let w = cfnum_to_f64(dict_get(traits, kCTFontWeightTrait)).unwrap_or(0.0);
            let s = cfnum_to_f64(dict_get(traits, kCTFontWidthTrait)).unwrap_or(0.0);
            let sym = cfnum_to_i32(dict_get(traits, kCTFontSymbolicTrait)).unwrap_or(0) as u32;
            CFRelease(traits);
            (
                ct_weight_to_int(w),
                ct_width_to_int(s),
                (sym & KCT_FONT_TRAIT_ITALIC) != 0,
            )
        } else {
            (400u16, 5u8, false)
        };

        let variation_axes = extract_axes(desc);

        Some(FaceInfo {
            family,
            style,
            postscript,
            weight,
            stretch,
            italic,
            variation_axes,
            modified_at,
            user_installed,
        })
    }

    pub(super) fn enumerate() -> FontFiles {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let is_user_path = |p: &Path| -> bool {
            home.as_ref().is_some_and(|h| p.starts_with(h))
        };
        let mut out = FontFiles::new();
        unsafe {
            let urls = CTFontManagerCopyAvailableFontURLs();
            if urls.is_null() {
                return out;
            }
            let n = CFArrayGetCount(urls);
            for i in 0..n {
                let url = CFArrayGetValueAtIndex(urls, i) as CFURLRef;
                if url.is_null() {
                    continue;
                }
                let Some(path) = cfurl_to_path(url) else { continue };
                let modified_at = ctime_secs(&path);
                let user_installed = is_user_path(&path);

                let descs = CTFontManagerCreateFontDescriptorsFromURL(url);
                if descs.is_null() {
                    continue;
                }
                let n_d = CFArrayGetCount(descs);
                let key = path.to_string_lossy().into_owned();
                let bucket = out.entry(key).or_default();
                for j in 0..n_d {
                    let desc = CFArrayGetValueAtIndex(descs, j) as CTFontDescriptorRef;
                    if desc.is_null() {
                        continue;
                    }
                    if let Some(face) = build_face(desc, modified_at, user_installed) {
                        // CT lists each face as its own URL, so the same
                        // descriptor may surface multiple times across the
                        // outer loop; dedup by postscript.
                        if !bucket.iter().any(|f| f.postscript == face.postscript) {
                            bucket.push(face);
                        }
                    }
                }
                CFRelease(descs);
            }
            CFRelease(urls);
        }
        // CoreText's user-shadows-system rule: if a postscript also appears
        // at a user-installed path, drop the system-installed face.
        let user_postscripts: std::collections::HashSet<String> = out
            .iter()
            .filter(|(_, faces)| faces.first().is_some_and(|f| f.user_installed))
            .flat_map(|(_, faces)| faces.iter().map(|f| f.postscript.clone()))
            .collect();
        out.retain(|_, faces| {
            faces.retain(|f| f.user_installed || !user_postscripts.contains(&f.postscript));
            !faces.is_empty()
        });
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
