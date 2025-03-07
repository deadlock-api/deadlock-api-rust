use crate::error::{APIError, APIResult};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use clickhouse::Row;
use futures::future::join;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

const TABLE_SIZES_QUERY: &str = r#"
SELECT
    name                     AS table,
    toBool(parts IS NULL)    AS is_view,
    total_rows               AS rows,
    total_bytes              AS data_compressed_bytes,
    total_bytes_uncompressed AS data_uncompressed_bytes
FROM system.tables
WHERE database = 'default'
    AND name NOT LIKE 'system.%'
    AND name NOT LIKE '%inner%'
    AND total_rows IS NOT NULL
    AND total_bytes IS NOT NULL
    AND total_bytes_uncompressed IS NOT NULL
ORDER BY table
"#;

const FETCHED_MATCHES_LAST_24H_QUERY: &str = r#"
WITH fetched_matches AS (
    SELECT match_id
    FROM match_info
    WHERE created_at > now() - INTERVAL 1 DAY
    UNION
    DISTINCT
    SELECT match_id
    FROM match_salts
    WHERE created_at > now() - INTERVAL 1 DAY
)
SELECT COUNT() as fetched_matches_per_day
FROM fetched_matches
"#;

#[derive(Deserialize, Row)]
struct TableSizeRow {
    pub table: String,
    pub is_view: bool,
    pub rows: Option<u64>,
    pub data_compressed_bytes: Option<u64>,
    pub data_uncompressed_bytes: Option<u64>,
}
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TableSize {
    pub is_view: bool,
    pub rows: Option<u64>,
    pub data_compressed_bytes: Option<u64>,
    pub data_uncompressed_bytes: Option<u64>,
}

impl From<TableSizeRow> for TableSize {
    fn from(row: TableSizeRow) -> Self {
        TableSize {
            is_view: row.is_view,
            rows: row.rows,
            data_compressed_bytes: row.data_compressed_bytes,
            data_uncompressed_bytes: row.data_uncompressed_bytes,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct APIInfo {
    pub fetched_matches_per_day: u64,
    pub table_sizes: HashMap<String, TableSize>,
}

#[utoipa::path(
    get,
    path = "/",
    responses(
        (status = OK, body = APIInfo),
        (status = INTERNAL_SERVER_ERROR, body = String)
    ),
    tags = ["Info"],
)]
pub async fn info(State(state): State<AppState>) -> APIResult<Json<APIInfo>> {
    let (table_sizes, fetched_matches_per_day) = join(
        state
            .clickhouse_client
            .query(TABLE_SIZES_QUERY)
            .fetch_all::<TableSizeRow>(),
        state
            .clickhouse_client
            .query(FETCHED_MATCHES_LAST_24H_QUERY)
            .fetch_one::<u64>(),
    )
    .await;

    let table_sizes = table_sizes
        .map_err(|e| APIError::InternalError {
            message: format!("Failed to fetch table sizes: {e}"),
        })?
        .into_iter()
        .map(|row| (row.table.clone(), row.into()))
        .sorted_by_key(|(table, _)| table.clone())
        .collect();

    let fetched_matches_per_day = fetched_matches_per_day.map_err(|e| APIError::InternalError {
        message: format!("Failed to fetch fetched matches: {e}"),
    })?;

    Ok(Json(APIInfo {
        fetched_matches_per_day,
        table_sizes,
    }))
}
