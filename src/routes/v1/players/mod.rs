pub mod card;
pub mod enemy_stats;
pub mod hero_stats;
pub mod item_stats;
pub mod match_history;
pub mod mate_stats;
pub mod mmr_history;
pub mod party_stats;

use crate::context::AppState;
use crate::middleware::cache::CacheControlMiddleware;
use crate::utils::parse::parse_steam_id;
use serde::Deserialize;
use std::time::Duration;
use utoipa::{IntoParams, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

#[derive(Deserialize, IntoParams, Default)]
pub struct AccountIdQuery {
    /// The players SteamID3
    #[serde(default)]
    #[serde(deserialize_with = "parse_steam_id")]
    pub account_id: u32,
}

#[derive(OpenApi)]
#[openapi(tags((name = "Players", description = "Player related endpoints")))]
pub struct ApiDoc;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(
            OpenApiRouter::new()
                .routes(routes!(match_history::match_history))
                .routes(routes!(mmr_history::mmr_history))
                .routes(routes!(mmr_history::hero_mmr_history))
                .routes(routes!(card::card_raw))
                .routes(routes!(card::card))
                .layer(CacheControlMiddleware::new(Duration::from_secs(5 * 60))),
        )
        .merge(
            OpenApiRouter::new()
                .routes(routes!(mate_stats::mate_stats))
                .routes(routes!(enemy_stats::enemy_stats))
                .routes(routes!(party_stats::party_stats))
                .routes(routes!(item_stats::item_stats))
                .routes(routes!(hero_stats::hero_stats))
                .layer(CacheControlMiddleware::new(Duration::from_secs(60 * 60))),
        )
}
