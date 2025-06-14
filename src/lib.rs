#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::lossy_float_literal)]
#![deny(clippy::redundant_clone)]
#![deny(unreachable_pub)]

mod api_doc;
mod context;
mod error;
mod middleware;
pub mod routes;
mod services;
pub mod utils;

use crate::api_doc::ApiDoc;
use crate::middleware::api_key::write_api_key_to_header;
use crate::middleware::cache::CacheControlMiddleware;
use crate::middleware::feature_flags::feature_flags;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Json, Router};
use axum_prometheus::PrometheusMetricLayer;
use context::state::AppState;
use std::time::Duration;
use tower_http::compression::{CompressionLayer, DefaultPredicate};
use tower_http::cors::CorsLayer;
use tower_http::normalize_path::{NormalizePath, NormalizePathLayer};
use tower_layer::Layer;
use tracing::debug;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_scalar::{Scalar, Servable};

pub use error::*;

const DEFAULT_CACHE_TIME: u64 = 60;

async fn favicon() -> impl IntoResponse {
    let favicon = include_bytes!("../public/favicon.ico");
    let mut headers = HeaderMap::new();
    if let Ok(content_type) = "image/x-icon".parse() {
        headers.insert(header::CONTENT_TYPE, content_type);
    }
    (headers, favicon)
}

pub async fn router(port: u16) -> Result<NormalizePath<Router>, StartupError> {
    debug!("Loading application state");
    let state = AppState::from_env().await?;
    debug!("Application state loaded");

    let (mut prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();
    prometheus_layer.enable_response_body_size();

    let (router, mut api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        // Redirect root to /docs
        .route("/", get(|| async { Redirect::to("/docs") }))
        // Serve favicon
        .route("/favicon.ico", get(favicon))
        // Add application routes
        .merge(routes::router())
        // Add prometheus metrics route
        .route("/metrics", get(|| async move { metric_handle.render() }))
        .layer(prometheus_layer)
        // Add Middlewares
        .layer(from_fn(write_api_key_to_header))
        .layer(from_fn_with_state(state.clone(), feature_flags))
        .layer(CacheControlMiddleware::new(Duration::from_secs(
            DEFAULT_CACHE_TIME,
        )))
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::<DefaultPredicate>::default())
        .fallback(|uri: axum::http::Uri| async move {
            APIResult::<()>::Err(APIError::status_msg(
                StatusCode::NOT_FOUND,
                format!("No route found for {uri}"),
            ))
        })
        .split_for_parts();

    let server_url = match cfg!(debug_assertions) {
        true => &format!("http://localhost:{port}"),
        false => "https://api.deadlock-api.com",
    };
    api.servers = Some(vec![utoipa::openapi::Server::new(server_url)]);

    let router = router
        .with_state(state)
        .merge(Scalar::with_url("/docs", api.clone()))
        .route("/openapi.json", get(|| async { Json(api) }));
    Ok(NormalizePathLayer::trim_trailing_slash().layer(router))
}

#[cfg(test)]
mod tests {
    use crate::router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[ignore]
    #[tokio::test]
    async fn test_router() {
        let router = router(3000).await.expect("Router");

        {
            // Test docs redirect from root
            let response = router
                .clone()
                .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::SEE_OTHER)
        }

        {
            // Test docs route
            let response = router
                .clone()
                .oneshot(Request::builder().uri("/docs").body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK)
        }

        {
            // Test metrics route
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/metrics")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK)
        }
    }
}
