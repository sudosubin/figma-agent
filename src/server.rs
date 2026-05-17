//! PNA: Chrome 94+ blocks public-origin to 127.0.0.1 preflight without
//! `Access-Control-Allow-Private-Network: true`.

use anyhow::Result;
use axum::{
    http::{header, HeaderName, HeaderValue, Method},
    Router,
    ServiceExt,
};
use tower::Layer;
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    normalize_path::NormalizePathLayer,
    set_header::SetResponseHeaderLayer,
};

pub async fn serve() -> Result<()> {
    let http_addr = "127.0.0.1:44950";

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([HeaderName::from_static("content-type")])
        .allow_private_network(true);

    let server_header = SetResponseHeaderLayer::overriding(
        header::SERVER,
        HeaderValue::from_static(concat!("FigmaAgent/", env!("CARGO_PKG_VERSION"))),
    );

    let router = Router::new()
        .layer(CompressionLayer::new())
        .layer(server_header)
        .layer(cors);

    // Must wrap Router from outside, rewrites path before route matching.
    let app = NormalizePathLayer::trim_trailing_slash().layer(router);

    let http_listener = tokio::net::TcpListener::bind(http_addr).await?;
    tracing::info!(addr = %http_addr, "listening (http)");

    let svc = ServiceExt::<axum::extract::Request>::into_make_service(app);
    axum::serve(http_listener, svc).await?;
    Ok(())
}
