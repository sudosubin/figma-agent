use std::env;

/// Upstream figma_agent 126.x is linked with the macOS 15.5 SDK. On macOS 26,
/// CoreText's `CTFontManagerCopyAvailableFontURLs` keys off the caller's
/// Mach-O `LC_BUILD_VERSION` sdk field: binaries stamped < 15 receive a
/// legacy list that includes document-support fonts (Athelas, STIX*,
/// NotoSans* minority scripts, SFNS, ...) that upstream never serves.
/// Stamping upstream's exact sdk version keeps enumeration identical no
/// matter which SDK the build environment provides (nix ships 14.4).
const UPSTREAM_SDK_VERSION: &str = "15.5";

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let minos = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| {
        // rustc's own defaults per target; must agree so the linker does not
        // see two different minimum versions.
        match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "11.0".to_string(),
            _ => "10.12".to_string(),
        }
    });
    // Appended after rustc's own -platform_version; ld keeps the last one.
    println!("cargo:rustc-link-arg=-Wl,-platform_version,macos,{minos},{UPSTREAM_SDK_VERSION}");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
}
