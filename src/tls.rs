//! HTTPS is technically optional; browsers treat loopback as a secure
//! context, so figma.com (HTTPS) can fetch HTTP from 127.0.0.1. We listen
//! on 44951 anyway for parity with orig macOS Figma Agent.

use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;
use std::path::Path;

pub async fn build_config(
    cert_path: Option<&Path>,
    key_path: Option<&Path>,
) -> Result<RustlsConfig> {
    match (cert_path, key_path) {
        (Some(cert), Some(key)) => RustlsConfig::from_pem_file(cert, key)
            .await
            .with_context(|| format!("loading cert {} / key {}", cert.display(), key.display())),
        _ => generate_self_signed().await,
    }
}

async fn generate_self_signed() -> Result<RustlsConfig> {
    let cert = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    let pem = cert.cert.pem();
    let key = cert.key_pair.serialize_pem();
    tracing::warn!(
        "generated a self-signed TLS cert; trust it (macOS: `security add-trusted-cert`, \
         Linux: NSS DB) to avoid browser warnings, or set tls_cert/tls_key in config"
    );
    RustlsConfig::from_pem(pem.into_bytes(), key.into_bytes())
        .await
        .context("loading generated self-signed cert")
}
