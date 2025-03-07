use crate::config::Config;
use crate::error::{APIError, APIResult};
use crate::routes::v1::matches::salts::fetch_match_salts;
use crate::routes::v1::matches::types::MatchIdQuery;
use crate::state::AppState;
use crate::utils::limiter::{RateLimitQuota, apply_limits};
use async_compression::tokio::bufread::BzDecoder;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use cached::TimedCache;
use cached::proc_macro::cached;
use futures::future::join;
use object_store::ObjectStore;
use prost::Message;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tracing::debug;
use valveprotos::deadlock::{CMsgMatchMetaData, CMsgMatchMetaDataContents};

async fn fetch_from_s3(s3: &impl ObjectStore, file: &str) -> object_store::Result<Vec<u8>> {
    s3.get(&object_store::path::Path::from(file))
        .await?
        .bytes()
        .await
        .map(|r| r.to_vec())
}

#[cached(
    ty = "TimedCache<u64, Vec<u8>>",
    create = "{ TimedCache::with_lifespan(5 * 60) }",
    result = true,
    convert = "{ match_id }",
    sync_writes = "by_key",
    key = "u64"
)]
async fn fetch_match_metadata_raw(
    config: &Config,
    http_client: &reqwest::Client,
    ch_client: &clickhouse::Client,
    s3: &impl ObjectStore,
    s3_cache: &impl ObjectStore,
    match_id: u64,
) -> APIResult<Vec<u8>> {
    // Try to fetch from cache first
    let results = join(
        fetch_from_s3(s3_cache, &format!("{match_id}.meta.bz2")),
        fetch_from_s3(s3_cache, &format!("{match_id}.meta_hltv.bz2")),
    )
    .await;
    if let Ok(data) = results.0 {
        debug!("Match metadata found in cache");
        return Ok(data);
    }
    if let Ok(data) = results.1 {
        debug!("Match metadata found in cache, hltv");
        return Ok(data);
    }

    // If not in cache, fetch from S3
    let results = join(
        fetch_from_s3(s3, &format!("processed/metadata/{match_id}.meta.bz2")),
        fetch_from_s3(s3, &format!("processed/metadata/{match_id}.meta_hltv.bz2")),
    )
    .await;
    if let Ok(data) = results.0 {
        debug!("Match metadata found on s3");
        return Ok(data);
    }
    if let Ok(data) = results.1 {
        debug!("Match metadata found on s3, hltv");
        return Ok(data);
    }

    // If not in S3, fetch from Steam
    let salts = fetch_match_salts(config, http_client, ch_client, match_id, false).await?;
    http_client
        .get(format!(
            "http://replay{}.valve.net/1422450/{}_{}.meta.bz2",
            salts.replay_group_id.unwrap_or_default(),
            match_id,
            salts.metadata_salt.unwrap_or_default()
        ))
        .send()
        .await
        .map_err(|e| APIError::InternalError {
            message: format!("Failed to fetch match metadata: {e}"),
        })?
        .bytes()
        .await
        .map(|r| r.to_vec())
        .map_err(|e| APIError::InternalError {
            message: format!("Failed to fetch match metadata: {e}"),
        })
}

async fn parse_match_metadata_raw(raw_data: &[u8]) -> APIResult<CMsgMatchMetaDataContents> {
    let mut decompressor = BzDecoder::new(raw_data);
    let mut buf = Vec::with_capacity(decompressor.get_ref().len());
    decompressor
        .read_to_end(&mut buf)
        .await
        .map_err(|e| APIError::InternalError {
            message: format!("Failed to decompress match metadata: {e}"),
        })?;
    let match_data = CMsgMatchMetaData::decode(buf.as_slice())
        .map_err(|e| APIError::InternalError {
            message: format!("Failed to parse match metadata: {e}"),
        })?
        .match_details
        .ok_or_else(|| APIError::InternalError {
            message: "Failed to parse match metadata: No data".to_string(),
        })?;
    CMsgMatchMetaDataContents::decode(match_data.as_slice()).map_err(|e| APIError::InternalError {
        message: format!("Failed to parse match metadata contents: {e}"),
    })
}

#[utoipa::path(
    get,
    path = "/{match_id}/metadata/raw",
    params(MatchIdQuery),
    responses(
        (status = OK, body = [u8]),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = TOO_MANY_REQUESTS, description = "Rate limit exceeded"),
        (status = NOT_FOUND, description = "Match metadata not found"),
        (status = INTERNAL_SERVER_ERROR, description = "Fetching match metadata failed")
    ),
    tags = ["Matches"],
    summary = "Match Metadata as Protobuf",
    description = r#"
This endpoints returns the raw .meta.bz2 file for the given `match_id`.

You have to decompress it and decode the protobuf message.

Protobuf definitions can be found here: [https://github.com/SteamDatabase/Protobufs](https://github.com/SteamDatabase/Protobufs)

Relevant Protobuf Messages:
- CMsgMatchMetaData
- CMsgMatchMetaDataContents
    "#
)]
pub async fn metadata_raw(
    Path(MatchIdQuery { match_id }): Path<MatchIdQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> APIResult<impl IntoResponse> {
    apply_limits(
        &headers,
        &state,
        "match_metadata",
        &[RateLimitQuota::ip_limit(100, Duration::from_secs(1))],
    )
    .await?;
    tryhard::retry_fn(|| {
        fetch_match_metadata_raw(
            &state.config,
            &state.http_client,
            &state.clickhouse_client,
            &state.s3_client,
            &state.s3_cache_client,
            match_id,
        )
    })
    .retries(3)
    .fixed_backoff(Duration::from_millis(10))
    .await
}

#[utoipa::path(
    get,
    path = "/{match_id}/metadata",
    params(MatchIdQuery),
    responses(
        (status = OK, description = "Match metadata, see protobuf type: CMsgMatchMetaDataContents"),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = TOO_MANY_REQUESTS, description = "Rate limit exceeded"),
        (status = NOT_FOUND, description = "Match metadata not found"),
        (status = INTERNAL_SERVER_ERROR, description = "Fetching or parsing match metadata failed")
    ),
    tags = ["Matches"],
    summary = "Matches Metadata",
    description = r#"
This endpoint returns the match metadata for the given `match_id` parsed into JSON.

Protobuf definitions can be found here: [https://github.com/SteamDatabase/Protobufs](https://github.com/SteamDatabase/Protobufs)

Relevant Protobuf Messages:
- CMsgMatchMetaData
- CMsgMatchMetaDataContents
    "#
)]
pub async fn metadata(
    Path(MatchIdQuery { match_id }): Path<MatchIdQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> APIResult<impl IntoResponse> {
    apply_limits(
        &headers,
        &state,
        "match_metadata",
        &[RateLimitQuota::ip_limit(100, Duration::from_secs(1))],
    )
    .await?;
    let raw_data = tryhard::retry_fn(|| {
        fetch_match_metadata_raw(
            &state.config,
            &state.http_client,
            &state.clickhouse_client,
            &state.s3_client,
            &state.s3_cache_client,
            match_id,
        )
    })
    .retries(3)
    .fixed_backoff(Duration::from_millis(10))
    .await?;
    parse_match_metadata_raw(&raw_data).await.map(Json)
}
