use crate::error::{APIError, APIResult};
use crate::state::AppState;
use crate::utils::parse::{
    default_last_month_timestamp, default_true_option, parse_steam_id_option,
};
use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use cached::TimedCache;
use cached::proc_macro::cached;
use clickhouse::Row;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use utoipa::{IntoParams, ToSchema};

#[derive(Copy, Debug, Clone, Serialize, Deserialize, IntoParams, Eq, PartialEq, Hash)]
pub struct HeroSynergyStatsQuery {
    /// Filter matches based on their start time (Unix timestamp). **Default:** 30 days ago.
    #[serde(default = "default_last_month_timestamp")]
    #[param(default = default_last_month_timestamp)]
    pub min_unix_timestamp: Option<u64>,
    /// Filter matches based on their start time (Unix timestamp).
    pub max_unix_timestamp: Option<u64>,
    /// Filter matches based on their duration in seconds (up to 7000s).
    #[param(maximum = 7000)]
    pub min_duration_s: Option<u64>,
    /// Filter matches based on their duration in seconds (up to 7000s).
    #[param(maximum = 7000)]
    pub max_duration_s: Option<u64>,
    /// Filter matches based on the average badge level (0-116) of *both* teams involved.
    #[param(minimum = 0, maximum = 116)]
    pub min_average_badge: Option<u8>,
    /// Filter matches based on the average badge level (0-116) of *both* teams involved.
    #[param(minimum = 0, maximum = 116)]
    pub max_average_badge: Option<u8>,
    /// Filter matches based on their ID.
    pub min_match_id: Option<u64>,
    /// Filter matches based on their ID.
    pub max_match_id: Option<u64>,
    /// When `true`, only considers matchups where both `hero_id` and `enemy_hero_id` were assigned to the same lane (e.g., both Mid Lane). When `false`, considers all matchups regardless of assigned lane.
    #[serde(default = "default_true_option")]
    #[param(default = true)]
    pub same_lane_filter: Option<bool>,
    /// Filter for matches with a specific player account ID.
    #[serde(default, deserialize_with = "parse_steam_id_option")]
    pub account_id: Option<u32>,
}

#[derive(Debug, Clone, Row, Serialize, Deserialize, ToSchema)]
pub struct HeroSynergyStats {
    /// The ID of the first hero in the pair.
    pub hero_id1: u32,
    /// The ID of the second hero in the pair.
    pub hero_id2: u32,
    /// The number of times the team won when both `hero_id1` and `hero_id2` were on the same team.
    pub wins: u64,
    /// The total number of matches played where `hero_id1` and `hero_id2` were on the same team, meeting the filter criteria.
    pub matches_played: u64,
}

fn build_hero_synergy_stats_query(query: &HeroSynergyStatsQuery) -> String {
    let mut info_filters = vec![];
    if let Some(min_unix_timestamp) = query.min_unix_timestamp {
        info_filters.push(format!("start_time >= {}", min_unix_timestamp));
    }
    if let Some(max_unix_timestamp) = query.max_unix_timestamp {
        info_filters.push(format!("start_time <= {}", max_unix_timestamp));
    }
    if let Some(min_match_id) = query.min_match_id {
        info_filters.push(format!("match_id >= {}", min_match_id));
    }
    if let Some(max_match_id) = query.max_match_id {
        info_filters.push(format!("match_id <= {}", max_match_id));
    }
    if let Some(min_badge_level) = query.min_average_badge {
        info_filters.push(format!(
            "average_badge_team0 >= {} AND average_badge_team1 >= {}",
            min_badge_level, min_badge_level
        ));
    }
    if let Some(max_badge_level) = query.max_average_badge {
        info_filters.push(format!(
            "average_badge_team0 <= {} AND average_badge_team1 <= {}",
            max_badge_level, max_badge_level
        ));
    }
    if let Some(min_duration_s) = query.min_duration_s {
        info_filters.push(format!("duration_s >= {}", min_duration_s));
    }
    if let Some(max_duration_s) = query.max_duration_s {
        info_filters.push(format!("duration_s <= {}", max_duration_s));
    }
    let info_filters = if info_filters.is_empty() {
        "".to_string()
    } else {
        format!(" AND {}", info_filters.join(" AND "))
    };
    let mut player_filters = vec![];
    if query.same_lane_filter.unwrap_or(true) {
        player_filters.push("p1.assigned_lane = p2.assigned_lane".to_string());
    }
    if let Some(account_id) = query.account_id {
        player_filters.push(format!("p1.account_id = {}", account_id));
    }
    let player_filters = if player_filters.is_empty() {
        "".to_string()
    } else {
        format!(" AND {}", player_filters.join(" AND "))
    };
    format!(
        r#"
    WITH matches AS (SELECT match_id
                 FROM match_info
                 WHERE match_outcome = 'TeamWin'
                   AND match_mode IN ('Ranked', 'Unranked')
                   AND game_mode = 'Normal' {})
    SELECT p1.hero_id  AS hero_id1,
           p2.hero_id  AS hero_id2,
           SUM(p1.won) AS wins,
           COUNT()     AS matches_played
    FROM match_player p1
             INNER JOIN match_player p2 USING (match_id)
    WHERE match_id IN matches
      AND p1.team = p2.team
      AND p1.hero_id < p2.hero_id
      {}
    GROUP BY p1.hero_id, p2.hero_id
    HAVING matches_played > 1
    ORDER BY p1.hero_id, p2.hero_id
    "#,
        info_filters, player_filters
    )
}

#[cached(
    ty = "TimedCache<HeroSynergyStatsQuery, Vec<HeroSynergyStats>>",
    create = "{ TimedCache::with_lifespan(60 * 60) }",
    result = true,
    convert = "{ query }",
    sync_writes = "by_key",
    key = "HeroSynergyStatsQuery"
)]
pub async fn get_hero_synergy_stats(
    ch_client: &clickhouse::Client,
    query: HeroSynergyStatsQuery,
) -> APIResult<Vec<HeroSynergyStats>> {
    let query = build_hero_synergy_stats_query(&query);
    debug!(?query);
    ch_client.query(&query).fetch_all().await.map_err(|e| {
        warn!("Failed to fetch hero synergy stats: {}", e);
        APIError::InternalError {
            message: format!("Failed to fetch hero synergy stats: {}", e),
        }
    })
}

#[utoipa::path(
    get,
    path = "/hero-synergy-stats",
    params(HeroSynergyStatsQuery),
    responses(
        // Update the response body description
        (status = OK, description = "Hero Synergy Stats", body = [HeroSynergyStats]),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to fetch hero synergy stats")
    ),
    tags = ["Analytics"],
    summary = "Hero Synergy Stats",
    description = r#"
Retrieves hero pair synergy statistics based on historical match data.

This endpoint analyzes completed matches to calculate how often a specific pair of heroes (`hero_id1` and `hero_id2`) won when playing *together on the same team*, and the total number of times they have played together under the specified filter conditions.

Results are cached for **1 hour** based on the combination of query parameters provided. Subsequent identical requests within this timeframe will receive the cached response.
    "#
)]
pub async fn hero_synergies_stats(
    Query(query): Query<HeroSynergyStatsQuery>,
    State(state): State<AppState>,
) -> APIResult<impl IntoResponse> {
    get_hero_synergy_stats(&state.ch_client, query)
        .await
        .map(Json)
}

#[cfg(test)]
mod test {
    #![allow(clippy::too_many_arguments)]
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn test_build_hero_synergy_stats_query(
        #[values(None, Some(1672531200))] min_unix_timestamp: Option<u64>,
        #[values(None, Some(1675209599))] max_unix_timestamp: Option<u64>,
        #[values(None, Some(600))] min_duration_s: Option<u64>,
        #[values(None, Some(1800))] max_duration_s: Option<u64>,
        #[values(None, Some(1))] min_average_badge: Option<u8>,
        #[values(None, Some(116))] max_average_badge: Option<u8>,
        #[values(None, Some(10000))] min_match_id: Option<u64>,
        #[values(None, Some(1000000))] max_match_id: Option<u64>,
        #[values(None, Some(true), Some(false))] same_lane_filter: Option<bool>,
        #[values(None, Some(18373975))] account_id: Option<u32>,
    ) {
        let query = HeroSynergyStatsQuery {
            min_unix_timestamp,
            max_unix_timestamp,
            min_duration_s,
            max_duration_s,
            min_average_badge,
            max_average_badge,
            min_match_id,
            max_match_id,
            same_lane_filter,
            account_id,
        };
        let query = build_hero_synergy_stats_query(&query);

        if let Some(min_unix_timestamp) = min_unix_timestamp {
            assert!(query.contains(&format!("start_time >= {}", min_unix_timestamp)));
        }
        if let Some(max_unix_timestamp) = max_unix_timestamp {
            assert!(query.contains(&format!("start_time <= {}", max_unix_timestamp)));
        }
        if let Some(min_duration_s) = min_duration_s {
            assert!(query.contains(&format!("duration_s >= {}", min_duration_s)));
        }
        if let Some(max_duration_s) = max_duration_s {
            assert!(query.contains(&format!("duration_s <= {}", max_duration_s)));
        }
        if let Some(min_average_badge) = min_average_badge {
            assert!(query.contains(&format!(
                "average_badge_team0 >= {} AND average_badge_team1 >= {}",
                min_average_badge, min_average_badge
            )));
        }
        if let Some(max_average_badge) = max_average_badge {
            assert!(query.contains(&format!(
                "average_badge_team0 <= {} AND average_badge_team1 <= {}",
                max_average_badge, max_average_badge
            )));
        }
        if let Some(min_match_id) = min_match_id {
            assert!(query.contains(&format!("match_id >= {}", min_match_id)));
        }
        if let Some(max_match_id) = max_match_id {
            assert!(query.contains(&format!("match_id <= {}", max_match_id)));
        }
        if let Some(same_lane_filter) = same_lane_filter {
            if same_lane_filter {
                assert!(query.contains("p1.assigned_lane = p2.assigned_lane"));
            } else {
                assert!(!query.contains("p1.assigned_lane = p2.assigned_lane"));
            }
        }
        if let Some(account_id) = account_id {
            assert!(query.contains(&format!("account_id = {}", account_id)));
        }
    }
}
