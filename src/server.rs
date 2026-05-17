//! PNA: Chrome 94+ blocks public-origin to 127.0.0.1 preflight without
//! `Access-Control-Allow-Private-Network: true`.

use crate::config::Config;
use crate::routes;
use anyhow::Result;
use axum::{
    http::{header, HeaderName, HeaderValue, Method},
    routing::get,
    Router,
    ServiceExt,
};
use std::sync::Arc;
use tower::Layer;
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    normalize_path::NormalizePathLayer,
    set_header::SetResponseHeaderLayer,
};

pub async fn serve(config: Config) -> Result<()> {
    let http_addr = format!("{}:{}", config.host, config.port);
    #[cfg(feature = "tls")]
    let tls_addr = config.tls_port.map(|p| format!("{}:{}", config.host, p));
    let state = Arc::new(config);

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
        .route("/font-files", get(routes::font_files))
        .with_state(state.clone())
        .layer(CompressionLayer::new())
        .layer(server_header)
        .layer(cors);

    // Must wrap Router from outside, rewrites path before route matching.
    let app = NormalizePathLayer::trim_trailing_slash().layer(router);

    let http_listener = tokio::net::TcpListener::bind(&http_addr).await?;
    tracing::info!(addr = %http_addr, "listening (http)");

    let http_task = tokio::spawn({
        let app = app.clone();
        async move {
            let svc = ServiceExt::<axum::extract::Request>::into_make_service(app);
            axum::serve(http_listener, svc).await
        }
    });

    #[cfg(feature = "tls")]
    let tls_task = tls_addr.map(|addr| tokio::spawn(spawn_tls(addr, app.clone(), state.clone())));

    http_task.await??;
    #[cfg(feature = "tls")]
    if let Some(t) = tls_task {
        t.await??;
    }
    Ok(())
}

#[cfg(feature = "tls")]
async fn spawn_tls(
    addr: String,
    app: tower_http::normalize_path::NormalizePath<Router>,
    state: Arc<Config>,
) -> anyhow::Result<()> {
    use std::net::SocketAddr;
    let cfg = crate::tls::build_config(state.tls_cert.as_deref(), state.tls_key.as_deref()).await?;
    let socket: SocketAddr = addr.parse()?;
    tracing::info!(%addr, "listening (https)");
    let svc = ServiceExt::<axum::extract::Request>::into_make_service(app);
    axum_server::bind_rustls(socket, cfg).serve(svc).await?;
    Ok(())
}
