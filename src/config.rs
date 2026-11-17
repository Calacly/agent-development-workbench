use anyhow::Result;
use serde::Deserialize;
use std::env;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    pub jwt_secret: String,
    pub jwt_expiration_hours: i64,
    pub encryption_key: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Config {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:./mcp_client.db".to_string()),
            port: env::var("PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()?,
            jwt_secret: env::var("JWT_SECRET")
                .unwrap_or_else(|_| {
                    tracing::warn!("JWT_SECRET not set, using default (INSECURE for production!)");
                    "dev_secret_change_in_production".to_string()
                }),
            jwt_expiration_hours: env::var("JWT_EXPIRATION_HOURS")
                .unwrap_or_else(|_| "24".to_string())
                .parse()?,
            encryption_key: env::var("ENCRYPTION_KEY")
                .unwrap_or_else(|_| {
                    tracing::warn!("ENCRYPTION_KEY not set, using default (INSECURE for production!)");
                    "dev_encryption_key_32_bytes_long".to_string()
                }),
        })
    }
}
