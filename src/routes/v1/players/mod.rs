mod card;
mod hero_stats;
mod item_stats;
pub mod match_history;
mod mate_stats;
mod party_stats;
mod scoreboard;
pub mod types;

use crate::state::AppState;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

#[derive(OpenApi)]
#[openapi(tags((name = "Players", description = "Player related endpoints")))]
pub struct ApiDoc;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(match_history::match_history))
        .routes(routes!(card::card_raw))
        .routes(routes!(card::card))
        .routes(routes!(mate_stats::mate_stats))
        .routes(routes!(party_stats::party_stats))
        .routes(routes!(item_stats::item_stats))
        .routes(routes!(hero_stats::hero_stats))
        .routes(routes!(scoreboard::scoreboard))
}
