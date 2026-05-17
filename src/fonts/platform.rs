//! Linux uses `fc-list` (subprocess) rather than libfontconfig so we don't
//! pull a native link dependency. macOS enumerates via the same CoreText
//! family-name query upstream uses: for each visible family, ask CT for
//! its descriptors; CT applies the user-shadows-system precedence rule.

use std::path::PathBuf;

pub(super) fn system_font_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos_font_paths()
    }
    #[cfg(target_os = "linux")]
    {
        linux_font_paths()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn macos_font_paths() -> Vec<PathBuf> {
    use std::collections::HashSet;
    use std::os::raw::c_void;
    type CFTypeRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFURLRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFSetRef = *const c_void;
    type CTFontDescriptorRef = *const c_void;

    const KCFURL_POSIX_PATH_STYLE: u32 = 0;
    const KCFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const RSRC_SUFFIX: &str = "/..namedfork/rsrc";

    #[link(name = "CoreText", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CTFontManagerCopyAvailableFontFamilyNames() -> CFArrayRef;
        fn CTFontDescriptorCreateWithAttributes(attributes: CFDictionaryRef) -> CTFontDescriptorRef;
        fn CTFontDescriptorCreateMatchingFontDescriptors(
            descriptor: CTFontDescriptorRef,
            mandatoryAttributes: CFSetRef,
        ) -> CFArrayRef;
        fn CTFontDescriptorCopyAttribute(
            descriptor: CTFontDescriptorRef,
            attribute: CFStringRef,
        ) -> CFTypeRef;

        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(arr: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: isize) -> *const c_void;
        fn CFURLCopyFileSystemPath(url: CFURLRef, style: u32) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, buf_size: isize, enc: u32) -> bool;
        fn CFDictionaryCreate(
            allocator: CFTypeRef,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;

        static kCTFontFamilyNameAttribute: CFStringRef;
        static kCTFontNameAttribute: CFStringRef;
        static kCTFontURLAttribute: CFStringRef;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
    }

    unsafe fn cfstring_to_string(s: CFStringRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let len = CFStringGetLength(s);
        let max = CFStringGetMaximumSizeForEncoding(len, KCFSTRING_ENCODING_UTF8) + 1;
        let mut buf = vec![0u8; max as usize];
        let ok = CFStringGetCString(s, buf.as_mut_ptr(), max, KCFSTRING_ENCODING_UTF8);
        if !ok {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0)?;
        std::str::from_utf8(&buf[..end]).ok().map(|s| s.to_string())
    }

    unsafe fn cfurl_to_path(url: CFURLRef) -> Option<PathBuf> {
        if url.is_null() {
            return None;
        }
        let path_cf = CFURLCopyFileSystemPath(url, KCFURL_POSIX_PATH_STYLE);
        if path_cf.is_null() {
            return None;
        }
        let len = CFStringGetLength(path_cf);
        let max = CFStringGetMaximumSizeForEncoding(len, KCFSTRING_ENCODING_UTF8) + 1;
        let mut buf = vec![0u8; max as usize];
        let ok = CFStringGetCString(path_cf, buf.as_mut_ptr(), max, KCFSTRING_ENCODING_UTF8);
        CFRelease(path_cf as CFTypeRef);
        if !ok {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0)?;
        let s = std::str::from_utf8(&buf[..end]).ok()?;
        let trimmed = s.strip_suffix(RSRC_SUFFIX).unwrap_or(s);
        Some(PathBuf::from(trimmed))
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let is_user_path = |p: &PathBuf| -> bool {
        home.as_ref().is_some_and(|h| p.starts_with(h))
    };

    unsafe {
        let families = CTFontManagerCopyAvailableFontFamilyNames();
        if families.is_null() {
            return Vec::new();
        }
        let family_count = CFArrayGetCount(families);
        // postscript -> (path, is_user). User-installed wins ties so that
        // /Users/.../NotoSansAdlam.ttf shadows /System/.../NotoSansAdlam-Regular.ttf
        // when both expose the same `postscript` (CT returns both descriptors).
        let mut by_ps: std::collections::HashMap<String, (PathBuf, bool)> = std::collections::HashMap::new();

        for i in 0..family_count {
            let family = CFArrayGetValueAtIndex(families, i) as CFStringRef;
            if family.is_null() {
                continue;
            }
            let keys = [kCTFontFamilyNameAttribute as *const c_void];
            let values = [family as *const c_void];
            let attrs = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks as *const _ as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const _ as *const c_void,
            );
            if attrs.is_null() {
                continue;
            }
            let descriptor = CTFontDescriptorCreateWithAttributes(attrs);
            CFRelease(attrs);
            if descriptor.is_null() {
                continue;
            }
            let matching = CTFontDescriptorCreateMatchingFontDescriptors(descriptor, std::ptr::null());
            CFRelease(descriptor as CFTypeRef);
            if matching.is_null() {
                continue;
            }
            let mcount = CFArrayGetCount(matching);
            for j in 0..mcount {
                let desc = CFArrayGetValueAtIndex(matching, j) as CTFontDescriptorRef;
                let url = CTFontDescriptorCopyAttribute(desc, kCTFontURLAttribute) as CFURLRef;
                let name = CTFontDescriptorCopyAttribute(desc, kCTFontNameAttribute) as CFStringRef;
                if let (Some(path), Some(ps)) = (cfurl_to_path(url), cfstring_to_string(name)) {
                    let user = is_user_path(&path);
                    by_ps
                        .entry(ps)
                        .and_modify(|cur| {
                            if !cur.1 && user {
                                *cur = (path.clone(), true);
                            }
                        })
                        .or_insert((path, user));
                }
                if !url.is_null() {
                    CFRelease(url as CFTypeRef);
                }
                if !name.is_null() {
                    CFRelease(name as CFTypeRef);
                }
            }
            CFRelease(matching as CFTypeRef);
        }
        CFRelease(families as CFTypeRef);

        let unique: HashSet<PathBuf> = by_ps.into_values().map(|(p, _)| p).collect();
        unique.into_iter().collect()
    }
}

#[cfg(target_os = "linux")]
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
