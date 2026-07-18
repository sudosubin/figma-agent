//! Origin-restricted CORS matching upstream Figma agent 126.x exactly:
//! requests without `Origin: https://www.figma.com` get 403, OPTIONS
//! preflight gets 204 with the upstream-shaped header set, and the
//! `Access-Control-Allow-Private-Network: true` header is required so
//! Chrome 94+ lets figma.com reach 127.0.0.1.

use crate::config::Config;
use crate::routes;
use anyhow::Result;
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router, ServiceExt,
};
use std::sync::Arc;
use tower::Layer;
use tower_http::normalize_path::NormalizePathLayer;

const ALLOWED_ORIGIN: &str = "https://www.figma.com";

pub async fn serve(config: Config) -> Result<()> {
    let http_addr = format!("{}:{}", config.host, config.port);
    #[cfg(feature = "tls")]
    let tls_addr = config.tls_port.map(|p| format!("{}:{}", config.host, p));
    let state = Arc::new(config);

    let router = Router::new()
        .route("/figma/font-files", get(routes::font_files))
        .route("/figma/font-file", get(routes::font_file))
        .with_state(state.clone())
        .layer(middleware::from_fn(cors_middleware));

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

async fn cors_middleware(req: Request, next: Next) -> Response {
    let origin = req
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if origin != ALLOWED_ORIGIN {
        return StatusCode::FORBIDDEN.into_response();
    }
    let is_options = req.method() == Method::OPTIONS;
    let mut resp = if is_options {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NO_CONTENT;
        r
    } else {
        next.run(req).await
    };
    let h = resp.headers_mut();
    // axum's auto-OPTIONS handler can sneak an `allow` in; strip for parity.
    h.remove("allow");
    h.insert(
        "access-control-allow-origin",
        HeaderValue::from_static(ALLOWED_ORIGIN),
    );
    h.insert("vary", HeaderValue::from_static("Origin"));
    if is_options {
        h.insert(
            "access-control-allow-headers",
            HeaderValue::from_static("Content-Type, Accept"),
        );
        h.insert("access-control-max-age", HeaderValue::from_static("600"));
        h.insert(
            "access-control-allow-private-network",
            HeaderValue::from_static("true"),
        );
        h.insert(
            "access-control-allow-methods",
            HeaderValue::from_static("POST, GET, OPTIONS"),
        );
    }
    resp
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
