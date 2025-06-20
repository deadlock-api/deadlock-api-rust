use crate::context::AppState;
use crate::error::{APIError, APIResult};
use crate::routes::v1::matches::types::MatchIdQuery;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use cached::TimedCache;
use cached::proc_macro::cached;
use chrono::{DateTime, Utc};
use hex::ToHex;
use hmac::digest::InvalidLength;
use hmac::{Hmac, Mac};
use itertools::Itertools;
use reqwest::header::{HeaderValue, InvalidHeaderValue};
use serde::Serialize;
use serde_json::json;
use sha2::Sha256;
use sqlx::{Pool, Postgres};
use tracing::warn;

#[derive(Debug, Clone, Serialize)]
struct MatchCreatedWebhookPayload {
    match_id: u64,
    salts_url: String,
    metadata_url: String,
    raw_metadata_url: String,
}

impl MatchCreatedWebhookPayload {
    fn new(match_id: u64) -> Self {
        Self {
            match_id,
            salts_url: format!("https://api.deadlock-api.com/v1/matches/{match_id}/salts"),
            metadata_url: format!("https://api.deadlock-api.com/v1/matches/{match_id}/metadata"),
            raw_metadata_url: format!(
                "https://api.deadlock-api.com/v1/matches/{match_id}/metadata/raw"
            ),
        }
    }
}

#[cached(
    ty = "TimedCache<u8, Vec<(String, String)>>",
    create = "{ TimedCache::with_lifespan(60 * 60) }",
    result = true,
    convert = "{ 0 }",
    sync_writes = "default"
)]
async fn get_webhook_urls(pg_client: &Pool<Postgres>) -> sqlx::Result<Vec<(String, String)>> {
    sqlx::query!("SELECT webhook_url, secret FROM webhooks")
        .fetch_all(pg_client)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| (row.webhook_url, row.secret))
                .collect_vec()
        })
}

struct Signature {
    timestamp: i64,
    v0: String,
}

impl Signature {
    const PAYLOAD_SEPARATOR: &'static [u8] = b".";
    const SIGNATURE_PART_ASSIGNATOR: &'static str = "=";
    const SIGNATURE_PART_SEPARATOR: &'static str = ",";

    fn new(secret: &str, payload: &[u8], signed_at: DateTime<Utc>) -> Result<Self, InvalidLength> {
        let timestamp = signed_at.timestamp();
        let timestamp_str = timestamp.to_string();
        let timestamp_str_bytes = timestamp_str.as_bytes();

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
        mac.update(timestamp_str_bytes);
        mac.update(Self::PAYLOAD_SEPARATOR);
        mac.update(payload);
        let v0 = mac.finalize().into_bytes().encode_hex::<String>();

        Ok(Self { timestamp, v0 })
    }

    fn value(&self) -> String {
        let timestamp_str = self.timestamp.to_string();
        let parts = &[("t", timestamp_str.as_str()), ("v0", self.v0.as_str())];

        Itertools::intersperse(
            parts
                .iter()
                .map(|p| format!("{}{}{}", p.0, Self::SIGNATURE_PART_ASSIGNATOR, p.1)),
            Self::SIGNATURE_PART_SEPARATOR.to_owned(),
        )
        .collect::<String>()
    }

    fn to_header_value(&self) -> Result<HeaderValue, InvalidHeaderValue> {
        HeaderValue::from_str(&self.value())
    }
}

#[utoipa::path(
    post,
    path = "/{match_id}/ingest",
    params(MatchIdQuery),
    responses(
        (status = OK),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Sending event failed")
    ),
    tags = ["Internal"],
    summary = "Match Ingest Event",
    description = r#"
This endpoint is used internally to send a match ingest event to webhook subcribers.

### Rate Limits:
| Type | Limit |
| ---- | ----- |
| IP | 100req/s |
| Key | - |
| Global | - |
    "#
)]
pub(super) async fn ingest_event(
    Path(MatchIdQuery { match_id }): Path<MatchIdQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> APIResult<impl IntoResponse> {
    headers
        .get("X-API-Key")
        .and_then(|key| key.to_str().ok().map(|key| key.to_string()))
        .map(|key| key == state.config.internal_api_key)
        .ok_or_else(|| APIError::status_msg(reqwest::StatusCode::UNAUTHORIZED, "Unauthorized"))?;

    let payload = MatchCreatedWebhookPayload::new(match_id);
    let webhook_urls: Vec<(String, String)> = get_webhook_urls(&state.pg_client).await?;
    for (webhook_url, secret) in webhook_urls {
        let payload = serde_json::to_vec(&payload)?;
        let sig = Signature::new(&secret, &payload, Utc::now())
            .ok()
            .and_then(|m| m.to_header_value().ok())
            .ok_or(APIError::internal(
                "Failed to serialize payload".to_string(),
            ))?;
        if let Err(e) = state
            .steam_client
            .send_webhook(&webhook_url, payload, &sig)
            .await
        {
            warn!("Failed to send webhook to {webhook_url}: {e}");
        }
    }
    Ok(Json(json!({ "status": "success" })))
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::prelude::*;

    #[test]
    fn create_signature() {
        let signed_at = Utc.with_ymd_and_hms(2021, 11, 15, 0, 30, 0).unwrap();
        let payload = "hello !";
        let secret = "secret";

        let sig = Signature::new(secret, payload.as_bytes(), signed_at).unwrap();
        assert_eq!(
            sig.value(),
            "t=1636936200,v0=1b3d69df55f1e52f05224ba94a5162abeb17ef52cd7f4948c390f810d6a87e98"
        );
    }
}
