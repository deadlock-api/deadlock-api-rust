mod utils;

use deadlock_api_rust::routes::v1::info::api_info::APIInfo;
use deadlock_api_rust::routes::v1::info::health::Status;

#[tokio::test]
async fn test_info() {
    let response = utils::request_endpoint("/v1/info", []).await;
    let info: APIInfo = response.json().await.expect("Failed to parse response");

    let expected_table_sizes = [
        ("match_salts", 100),
        ("match_info", 100),
        ("match_player", 1200),
    ];
    for (table, expected_rows) in expected_table_sizes {
        let rows = info
            .table_sizes
            .get(table)
            .and_then(|t| t.rows)
            .expect("Failed to find table size for {table}");
        assert_eq!(rows, expected_rows);
    }
}

#[tokio::test]
async fn test_health() {
    let response = utils::request_endpoint("/v1/info/health", []).await;
    let status: Status = response.json().await.expect("Failed to parse response");
    assert!(status.services.all_ok());
}
