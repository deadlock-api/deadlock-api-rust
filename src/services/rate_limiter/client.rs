use core::time::Duration;

use axum::http::StatusCode;
use cached::TimedCache;
use cached::proc_macro::cached;
use chrono::{DateTime, Utc};
use redis::aio::MultiplexedConnection;
use redis::{AsyncCommands, RedisResult};
use sqlx::{Pool, Postgres};
use tracing::error;
use uuid::Uuid;

use crate::error::{APIError, APIResult};
use crate::services::rate_limiter::extractor::RateLimitKey;
use crate::services::rate_limiter::types::QuotaType;
use crate::services::rate_limiter::{Quota, Status};

const MAX_TTL_MICROS: i64 = 60 * 60 * 1000 * 1000;

#[derive(Clone)]
pub(crate) struct RateLimitClient {
    redis_client: MultiplexedConnection,
    pg_client: Pool<Postgres>,
    emergency_mode: bool,
}

impl RateLimitClient {
    pub(crate) fn new(
        redis_client: MultiplexedConnection,
        pg_client: Pool<Postgres>,
        emergency_mode: bool,
    ) -> Self {
        Self {
            redis_client,
            pg_client,
            emergency_mode,
        }
    }

    pub(crate) async fn apply_limits(
        &self,
        rate_limit_key: &RateLimitKey,
        key: &str,
        quotas: &[Quota],
    ) -> APIResult<Option<Status>> {
        if quotas.is_empty() {
            return Ok(None);
        }

        if let Some(api_key) = rate_limit_key.api_key {
            // If API key is present, check if it is valid
            if !is_api_key_valid(&self.pg_client, api_key).await {
                return Err(APIError::status_msg(
                    StatusCode::FORBIDDEN,
                    "Invalid API key",
                ));
            }
        } else if quotas.iter().any(|q| q.r#type.is_key())
            && !quotas.iter().any(|q| q.r#type.is_ip())
        {
            // If API key is not present check if this route requires an API key
            // Routes that have a key limit, but no IP limit, require an API key
            // This way we can make routes key-only, by only assigning a key limit
            return Err(APIError::status_msg(
                StatusCode::FORBIDDEN,
                "API key is required for this endpoint",
            ));
        } else if self.emergency_mode {
            // If API key is not present check if the service is in emergency mode
            // If the service is in emergency mode, only requests with an API key are allowed
            return Err(APIError::status_msg(
                StatusCode::SERVICE_UNAVAILABLE,
                "Service is in emergency mode",
            ));
        }

        let prefix = rate_limit_key
            .api_key
            .map_or_else(|| rate_limit_key.ip.to_string(), |k| k.to_string());
        let prefixed_key = format!("{prefix}:{key}");
        // If incrementing the key fails, we don't apply any limits
        if let Err(e) = self.increment_key(&prefixed_key).await {
            error!("Failed to increment rate limit key: {e}, will not apply limits");
            return Ok(None);
        }

        // Check for custom quotas
        let quotas = match rate_limit_key.api_key {
            None => quotas.to_vec(),
            Some(api_key) => {
                let custom_quotas = get_custom_quotas(&self.pg_client, api_key, key).await;
                if custom_quotas.is_empty() {
                    let has_api_key_limits = quotas.iter().any(|q| q.r#type.is_key());
                    // Remove IP quotas if there are key quotas and api_key is present
                    quotas
                        .iter()
                        .filter(|q| !has_api_key_limits || !q.r#type.is_ip())
                        .copied()
                        .collect()
                } else {
                    custom_quotas
                }
            }
        };

        // Check all quotas
        let mut all_statuses = Vec::new();
        for quota in quotas {
            let prefixed_key = if quota.r#type.is_global() {
                key
            } else {
                &format!("{prefix}:{key}")
            };
            let Ok((requests, oldest_request)) =
                self.check_requests(prefixed_key, quota.period).await
            else {
                error!("Failed to check rate limit key: {prefixed_key}, will not apply limits");
                continue;
            };
            let status = Status {
                quota,
                requests,
                oldest_request,
            };
            status.raise_if_exceeded()?;
            all_statuses.push(status);
        }

        // If incrementing the key fails, we don't apply any limits
        if let Err(e) = self.increment_key(key).await {
            error!("Failed to increment rate limit key: {e}, will not apply limits");
            return Ok(None);
        }

        // Return the status with the lowest remaining requests (most critical)
        Ok(all_statuses.into_iter().min_by_key(Status::remaining))
    }

    async fn check_requests(
        &self,
        key: &str,
        period: Duration,
    ) -> RedisResult<(usize, DateTime<Utc>)> {
        let current_time = Utc::now();
        let period_start = current_time - period;
        let timestamps: Vec<i64> = self
            .redis_client
            .clone()
            .zrangebyscore(
                key,
                period_start.timestamp_micros(),
                current_time.timestamp_micros(),
            )
            .await?;
        let num_requests = timestamps.len();
        if num_requests == 0 {
            return Ok((0, current_time));
        }
        let oldest_timestamp = timestamps
            .into_iter()
            .min()
            .and_then(DateTime::from_timestamp_micros)
            .unwrap_or_else(Utc::now);
        Ok((num_requests - 1, oldest_timestamp))
    }

    async fn increment_key(&self, key: &str) -> RedisResult<()> {
        let current_time = Utc::now().timestamp_micros();
        redis::pipe()
            .zrembyscore(key, 0, current_time - MAX_TTL_MICROS) // Remove old entries for the key
            .zadd(key, current_time, current_time) // Add current timestamp for the key
            .expire(key, MAX_TTL_MICROS / 1000 / 1000) // Set expiration time for the key
            .exec_async(&mut self.redis_client.clone()) // Execute the pipeline
            .await
    }
}

// Helper functions outside the impl block since cached macros cannot be used directly on methods
#[cached(
    ty = "TimedCache<Uuid, bool>",
    create = "{ TimedCache::with_lifespan(std::time::Duration::from_secs(60 * 60)) }",
    convert = "{ api_key }",
    sync_writes = "by_key",
    key = "Uuid"
)]
async fn is_api_key_valid(pg_client: &Pool<Postgres>, api_key: Uuid) -> bool {
    sqlx::query!(
        "SELECT COUNT(*) FROM api_keys WHERE key = $1 AND disabled IS false",
        api_key
    )
    .fetch_one(pg_client)
    .await
    .ok()
    .and_then(|row| row.count.map(|c| c > 0))
    .unwrap_or(false)
}

#[cached(
    ty = "TimedCache<String, Vec<Quota>>",
    create = "{ TimedCache::with_lifespan(std::time::Duration::from_secs(10 * 60)) }",
    convert = r#"{ format!("{api_key}-{path}") }"#,
    sync_writes = "by_key",
    key = "String"
)]
async fn get_custom_quotas(pg_client: &Pool<Postgres>, api_key: Uuid, path: &str) -> Vec<Quota> {
    sqlx::query!(
        "SELECT rate_limit, rate_period FROM api_key_limits WHERE key = $1 AND path = $2",
        api_key,
        path
    )
    .fetch_all(pg_client)
    .await
    .ok()
    .map(|rows| {
        rows.iter()
            .map(|row| Quota {
                #[allow(clippy::cast_sign_loss)]
                limit: row.rate_limit as usize,
                #[allow(clippy::cast_sign_loss)]
                period: Duration::from_micros(row.rate_period.microseconds as u64),
                r#type: QuotaType::Key,
            })
            .collect()
    })
    .unwrap_or_default()
}
