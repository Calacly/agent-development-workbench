use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

pub async fn init_db(database_url: &str) -> anyhow::Result<SqlitePool> {
    tracing::info!("Connecting to database: {}", database_url);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    tracing::info!("Database connection established");

    Ok(pool)
}
