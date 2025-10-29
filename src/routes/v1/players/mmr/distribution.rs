use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use tracing::debug;
use utoipa::{IntoParams, ToSchema};

use crate::context::AppState;
use crate::error::APIResult;
use crate::routes::v1::players::mmr::batch::HeroMMRPath;
use crate::routes::v1::players::mmr::mmr_history::{SMOOTHING_FACTOR, WINDOW_SIZE};
use crate::utils::parse::default_last_month_timestamp;

#[derive(Copy, Debug, Clone, Deserialize, IntoParams, Eq, PartialEq, Hash)]
pub(crate) struct MMRDistributionQuery {
    /// Filter matches based on their start time (Unix timestamp). **Default:** 30 days ago.
    #[serde(default = "default_last_month_timestamp")]
    #[param(default = default_last_month_timestamp)]
    min_unix_timestamp: Option<i64>,
    /// Filter matches based on their start time (Unix timestamp).
    max_unix_timestamp: Option<i64>,
    /// Filter matches based on their duration in seconds (up to 7000s).
    #[param(maximum = 7000)]
    min_duration_s: Option<u64>,
    /// Filter matches based on their duration in seconds (up to 7000s).
    #[param(maximum = 7000)]
    max_duration_s: Option<u64>,
    /// Filter matches based on whether they are in the high skill range.
    is_high_skill_range_parties: Option<bool>,
    /// Filter matches based on whether they are in the low priority pool.
    is_low_pri_pool: Option<bool>,
    /// Filter matches based on whether they are in the new player pool.
    is_new_player_pool: Option<bool>,
    /// Filter matches based on their ID.
    min_match_id: Option<u64>,
    /// Filter matches based on their ID.
    max_match_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, Row, Serialize, Deserialize, ToSchema)]
pub(super) struct DistributionEntry {
    rank: u8,
    players: u64,
}

fn build_info_filters(query: &MMRDistributionQuery) -> String {
    let mut info_filters = vec![];
    info_filters.push("match_mode IN ('Ranked', 'Unranked')".to_owned());
    if let Some(min_unix_timestamp) = query.min_unix_timestamp {
        info_filters.push(format!("start_time >= {min_unix_timestamp}"));
    }
    if let Some(max_unix_timestamp) = query.max_unix_timestamp {
        info_filters.push(format!("start_time <= {max_unix_timestamp}"));
    }
    if let Some(min_match_id) = query.min_match_id {
        info_filters.push(format!("match_id >= {min_match_id}"));
    }
    if let Some(max_match_id) = query.max_match_id {
        info_filters.push(format!("match_id <= {max_match_id}"));
    }
    if let Some(max_duration_s) = query.max_duration_s {
        info_filters.push(format!("duration_s <= {max_duration_s}"));
    }
    if let Some(is_high_skill_range_parties) = query.is_high_skill_range_parties {
        info_filters.push(format!(
            "is_high_skill_range_parties = {is_high_skill_range_parties}"
        ));
    }
    if let Some(is_low_pri_pool) = query.is_low_pri_pool {
        info_filters.push(format!("low_pri_pool = {is_low_pri_pool}"));
    }
    if let Some(is_new_player_pool) = query.is_new_player_pool {
        info_filters.push(format!("new_player_pool = {is_new_player_pool}"));
    }
    if info_filters.is_empty() {
        String::new()
    } else {
        format!(" AND {}", info_filters.join(" AND "))
    }
}

fn build_mmr_query(query: &MMRDistributionQuery) -> String {
    let info_filters = build_info_filters(query);
    format!(
        "
    WITH
        {WINDOW_SIZE} as window_size,
        {SMOOTHING_FACTOR} as k,
        arrayMap(x -> pow(x, -k), range(1, window_size + 1)) AS exp_weights,
        t_matches AS (SELECT account_id,
                             match_id,
                             assumeNotNull(if(player_team = 'Team1', average_badge_team1, average_badge_team0)) AS current_match_badge,
                             (intDiv(current_match_badge, 10) - 1) * 6 + current_match_badge % 10               AS mmr
                      FROM player_match_history
                               INNER JOIN match_info USING (match_id)
                      WHERE current_match_badge > 0
                        AND (not_scored is NULL OR not_scored != true)
                        {info_filters}
                      ORDER BY account_id, match_id),
        mmr_data AS (SELECT account_id,
                            groupArray(mmr)
                                       OVER (PARTITION BY account_id ORDER BY match_id ROWS BETWEEN window_size - 1 PRECEDING AND CURRENT ROW) AS mmr_window,
                            arraySlice(exp_weights, 1, length(mmr_window))                                                                     AS weights
                     FROM t_matches
                     ORDER BY match_id DESC
                     LIMIT 1 BY account_id),
        distribution AS (SELECT toUInt32(clamp(dotProduct(mmr_window, weights) / arraySum(weights), 0, 66)) AS player_score,
                                uniq(account_id)                                                            as players
                         FROM mmr_data
                         GROUP BY player_score)
    SELECT toUInt8(if(player_score <= 0, 0, 10 * intDiv(player_score - 1, 6) + 11 + modulo(player_score - 1, 6))) AS rank,
           players
    FROM distribution
    WHERE rank BETWEEN 11 AND 116
    ORDER BY rank
    "
    )
}

fn build_hero_mmr_distribution_query(hero_id: u8, query: &MMRDistributionQuery) -> String {
    let info_filters = build_info_filters(query);
    format!(
        "
    WITH
        {WINDOW_SIZE} as window_size,
        {SMOOTHING_FACTOR} as k,
        arrayMap(x -> pow(x, -k), range(1, window_size + 1)) AS exp_weights,
        t_matches AS (
            SELECT
                account_id,
                match_id,
                start_time,
                assumeNotNull(if(player_team = 'Team1', average_badge_team1, average_badge_team0)) AS current_match_badge,
                (intDiv(current_match_badge, 10) - 1) * 6 + current_match_badge % 10 AS mmr
            FROM player_match_history
                INNER JOIN match_info USING (match_id)
            WHERE current_match_badge > 0
            AND (not_scored is NULL OR not_scored != true)
            AND hero_id = {hero_id}
            {info_filters}
            ORDER BY account_id, match_id
        ),
        mmr_data AS (SELECT account_id,
                            groupArray(mmr)
                                       OVER (PARTITION BY account_id ORDER BY match_id ROWS BETWEEN window_size - 1 PRECEDING AND CURRENT ROW) AS mmr_window,
                            arraySlice(exp_weights, 1, length(mmr_window))                                                                     AS weights
                     FROM t_matches
                     ORDER BY match_id DESC
                     LIMIT 1 BY account_id),
        distribution AS (SELECT toUInt32(clamp(dotProduct(mmr_window, weights) / arraySum(weights), 0, 66)) AS player_score,
                                uniq(account_id)                                                            as players
                         FROM mmr_data
                         GROUP BY player_score)
    SELECT toUInt8(if(player_score <= 0, 0, 10 * intDiv(player_score - 1, 6) + 11 + modulo(player_score - 1, 6))) AS rank,
           players
    FROM distribution
    ORDER BY rank
    "
    )
}

#[utoipa::path(
    get,
    path = "/mmr/distribution",
    params(MMRDistributionQuery),
    responses(
        (status = OK, description = "MMR", body = [DistributionEntry]),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to fetch mmr")
    ),
    tags = ["MMR"],
    summary = "MMR Distribution",
    description = "
Player MMR Distribution
",
)]
pub(super) async fn mmr(
    State(state): State<AppState>,
    Query(query): Query<MMRDistributionQuery>,
) -> APIResult<impl IntoResponse> {
    let query = build_mmr_query(&query);
    debug!(?query);
    Ok(state
        .ch_client_ro
        .query(&query)
        .fetch_all::<DistributionEntry>()
        .await
        .map(Json)?)
}

#[utoipa::path(
    get,
    path = "/mmr/distribution/{hero_id}",
    params(MMRDistributionQuery, HeroMMRPath),
    responses(
        (status = OK, description = "Hero MMR", body = [DistributionEntry]),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to fetch hero mmr")
    ),
    tags = ["MMR"],
    summary = "Hero MMR Distribution",
    description = "
Player Hero MMR Distribution
",
)]
pub(super) async fn hero_mmr(
    Path(HeroMMRPath { hero_id }): Path<HeroMMRPath>,
    Query(query): Query<MMRDistributionQuery>,
    State(state): State<AppState>,
) -> APIResult<impl IntoResponse> {
    let query = build_hero_mmr_distribution_query(hero_id, &query);
    debug!(?query);
    Ok(state
        .ch_client_ro
        .query(&query)
        .fetch_all::<DistributionEntry>()
        .await
        .map(Json)?)
}
