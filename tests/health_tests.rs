mod common;
use common::base_url;

#[tokio::test]
async fn health_reports_uptime_since_start() {
    let body: serde_json::Value = reqwest::get(format!("{}/health", base_url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["status"], "healthy");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    let uptime = body["uptime_seconds"].as_u64().unwrap();
    // Seconds since the server started, not since 1970
    assert!(uptime < 365 * 24 * 3600, "uptime_seconds = {uptime}");
}
