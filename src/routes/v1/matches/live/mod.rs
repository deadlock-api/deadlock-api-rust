mod demo;
mod events;
mod parser;
mod url;

use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::context::AppState;

#[derive(OpenApi)]
#[openapi(tags((name = "Live Matches", description = "Live Match related endpoints")))]
struct ApiDoc;

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(url::url))
        .routes(routes!(demo::demo))
        .routes(routes!(events::events))
}
