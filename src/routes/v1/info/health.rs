use crate::error::{APIError, APIResult};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use cached::TimedCache;
use cached::proc_macro::cached;
use redis::AsyncCommands;
use serde::Serialize;
use serde_json::json;
use sqlx::{Pool, Postgres};
use utoipa::ToSchema;

#[derive(Debug, Copy, Clone, Serialize, Default, ToSchema)]
pub struct StatusServices {
    pub clickhouse: bool,
    pub postgres: bool,
    pub redis: bool,
}

impl StatusServices {
    pub fn all_ok(&self) -> bool {
        self.clickhouse && self.postgres && self.redis
    }
}

#[derive(Debug, Copy, Clone, Serialize, Default, ToSchema)]
pub struct Status {
    pub services: StatusServices,
}

#[cached(
    ty = "TimedCache<String, Status>",
    create = "{ TimedCache::with_lifespan(60) }",
    result = true,
    convert = r#"{ format!("") }"#,
    sync_writes = "by_key",
    key = "String"
)]
async fn check_health(
    clickhouse_client: clickhouse::Client,
    postgres_client: Pool<Postgres>,
    redis_client: &mut redis::aio::MultiplexedConnection,
) -> APIResult<Status> {
    let mut status = Status::default();

    // Check Clickhouse connection
    status.services.clickhouse = clickhouse_client.query("SELECT 1").execute().await.is_ok();

    // Check Postgres connection
    status.services.postgres = !postgres_client.is_closed();

    // Check Redis connection
    status.services.redis = redis_client
        .exists::<&str, bool>("health_check")
        .await
        .is_ok();

    match status.services.all_ok() {
        true => Ok(status),
        false => Err(APIError::StatusMsgJson {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: json!(status),
        }),
    }
}

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = OK, body = Status),
        (status = INTERNAL_SERVER_ERROR, body = String)
    ),
    tags = ["Info"],
)]
pub async fn health_check(State(mut state): State<AppState>) -> APIResult<Json<Status>> {
    check_health(
        state.clickhouse_client,
        state.postgres_client,
        &mut state.redis_client,
    )
    .await
    .map(Json)
}
