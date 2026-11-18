use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::str::FromStr;

pub async fn init_db(database_url: &str) -> anyhow::Result<SqlitePool> {
    tracing::info!("Connecting to database: {}", database_url);

    // SQLite does not create a missing database file unless asked to
    let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    tracing::info!("Database connection established");

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_a_missing_database_file() {
        // Prefix "mcp-client-test-": tests/common/mod.rs removes old files with it
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mcp-client-test-create-{}-{nanos}.db",
            std::process::id()
        ));
        assert!(!path.exists());

        // Same URL form as the default, without "?mode=rwc"
        let pool = init_db(&format!("sqlite:{}", path.display())).await.unwrap();
        sqlx::query("SELECT 1").execute(&pool).await.unwrap();

        assert!(path.exists());
    }
}
