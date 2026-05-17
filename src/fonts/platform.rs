//! Linux uses `fc-list` (subprocess) rather than libfontconfig so we don't
//! pull a native link dependency.

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
    // Direct FFI; `core-foundation` / `core-text` crates are far heavier
    // than these ~30 lines for a single call.
    use std::os::raw::c_void;
    type CFTypeRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFURLRef = *const c_void;

    const KCFURL_POSIX_PATH_STYLE: u32 = 0;
    const KCFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreText", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CTFontManagerCopyAvailableFontURLs() -> CFArrayRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(arr: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(arr: CFArrayRef, idx: isize) -> *const c_void;
        fn CFURLCopyFileSystemPath(url: CFURLRef, style: u32) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(len: isize, enc: u32) -> isize;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, buf_size: isize, enc: u32) -> bool;
    }

    // Legacy resource-fork URLs from CoreText (rare on modern macOS); strip
    // the suffix to get the regular file path. Matches orig's `correct_path`.
    const RSRC_SUFFIX: &str = "/..namedfork/rsrc";

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

    unsafe {
        let arr = CTFontManagerCopyAvailableFontURLs();
        if arr.is_null() {
            return Vec::new();
        }
        let count = CFArrayGetCount(arr);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            if let Some(p) = cfurl_to_path(CFArrayGetValueAtIndex(arr, i) as CFURLRef) {
                out.push(p);
            }
        }
        CFRelease(arr as CFTypeRef);
        out
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
