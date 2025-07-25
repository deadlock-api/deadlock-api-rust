use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use cached::TimedCache;
use cached::proc_macro::cached;
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use tracing::debug;
use utoipa::{IntoParams, ToSchema};

use crate::context::AppState;
use crate::error::APIResult;

#[derive(Copy, Debug, Clone, Deserialize, IntoParams, Eq, PartialEq, Hash)]
pub(super) struct BadgeDistributionQuery {
    /// Filter matches based on their start time (Unix timestamp).
    min_unix_timestamp: Option<i64>,
    /// Filter matches based on their start time (Unix timestamp).
    max_unix_timestamp: Option<i64>,
    /// Filter matches based on their ID.
    min_match_id: Option<u64>,
    /// Filter matches based on their ID.
    max_match_id: Option<u64>,
}

#[derive(Debug, Clone, Row, Serialize, Deserialize, ToSchema)]
struct BadgeDistribution {
    /// The badge level. See more: <https://assets.deadlock-api.com/v2/ranks>
    badge_level: u32,
    /// The total number of matches.
    total_matches: u64,
}

fn build_query(query: &BadgeDistributionQuery) -> String {
    let mut filters = vec![];
    if let Some(min_unix_timestamp) = query.min_unix_timestamp {
        filters.push(format!("start_time >= {min_unix_timestamp}"));
    }
    if let Some(max_unix_timestamp) = query.max_unix_timestamp {
        filters.push(format!("start_time <= {max_unix_timestamp}"));
    }
    if let Some(min_match_id) = query.min_match_id {
        filters.push(format!("match_id >= {min_match_id}"));
    }
    if let Some(max_match_id) = query.max_match_id {
        filters.push(format!("match_id <= {max_match_id}"));
    }
    let filters = if filters.is_empty() {
        String::new()
    } else {
        format!(" AND {}", filters.join(" AND "))
    };
    format!(
        "
    SELECT
        coalesce(t_badge_level, 0) as badge_level,
        COUNT() as total_matches
    FROM match_info
        ARRAY JOIN [average_badge_team0, average_badge_team1] AS t_badge_level
    WHERE match_mode IN ('Ranked', 'Unranked') AND badge_level > 0 {filters}
    GROUP BY badge_level
    ORDER BY badge_level
    "
    )
}

#[cached(
    ty = "TimedCache<BadgeDistributionQuery, Vec<BadgeDistribution>>",
    create = "{ TimedCache::with_lifespan(std::time::Duration::from_secs(10 * 60)) }",
    result = true,
    convert = "{ query }",
    sync_writes = "by_key",
    key = "BadgeDistributionQuery"
)]
async fn get_badge_distribution(
    ch_client: &clickhouse::Client,
    query: BadgeDistributionQuery,
) -> APIResult<Vec<BadgeDistribution>> {
    let query = build_query(&query);
    debug!(?query);
    Ok(ch_client.query(&query).fetch_all().await?)
}

#[utoipa::path(
    get,
    path = "/badge-distribution",
    params(BadgeDistributionQuery),
    responses(
        (status = OK, description = "Badge Distribution", body = [BadgeDistribution]),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to fetch badge distribution")
    ),
    tags = ["Matches"],
    summary = "Badge Distribution",
    description = "
This endpoint returns the player badge distribution.

### Rate Limits:
| Type | Limit |
| ---- | ----- |
| IP | 100req/s |
| Key | - |
| Global | - |
    "
)]
pub(super) async fn badge_distribution(
    Query(query): Query<BadgeDistributionQuery>,
    State(state): State<AppState>,
) -> APIResult<impl IntoResponse> {
    get_badge_distribution(&state.ch_client_ro, query)
        .await
        .map(Json)
}
