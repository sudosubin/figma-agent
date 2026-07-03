use std::env;

/// macOS 26 CoreText returns a larger legacy font list to binaries whose
/// LC_BUILD_VERSION sdk field is below 15. Stamp upstream figma_agent's SDK
/// (ld keeps the last -platform_version) so font enumeration matches
/// upstream regardless of the build environment's SDK.
const UPSTREAM_SDK_VERSION: &str = "15.5";

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // Fall back to rustc's per-target minos defaults.
    let minos = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| {
        match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "11.0".to_string(),
            _ => "10.12".to_string(),
        }
    });
    println!("cargo:rustc-link-arg=-Wl,-platform_version,macos,{minos},{UPSTREAM_SDK_VERSION}");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
}
