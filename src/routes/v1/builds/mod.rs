pub mod query;
pub mod route;
pub mod structs;

use crate::context::AppState;
use crate::middleware::cache::CacheControlMiddleware;
use std::time::Duration;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

#[derive(OpenApi)]
#[openapi(tags((
    name = "Builds",
    description = "Endpoints to get all data about ingame hero builds."
)))]
pub struct ApiDoc;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(route::search_builds))
        .layer(CacheControlMiddleware::new(Duration::from_secs(60 * 60)))
}
