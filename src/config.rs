use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::net::IpAddr;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub database_url: String,
    /// Address to listen on. Local only by default; `HOST=0.0.0.0` opens the app to the network.
    pub host: IpAddr,
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
            host: parse_host(env::var("HOST").ok())?,
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

/// The `HOST` setting: an IP address, 127.0.0.1 when it is not set.
fn parse_host(value: Option<String>) -> Result<IpAddr> {
    let value = value.unwrap_or_else(|| "127.0.0.1".to_string());
    value
        .trim()
        .parse()
        .with_context(|| format!("HOST must be an IP address such as 127.0.0.1 or 0.0.0.0, got \"{value}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_is_local_only_by_default() {
        assert_eq!(parse_host(None).unwrap(), IpAddr::from([127, 0, 0, 1]));
    }

    #[test]
    fn host_can_be_overridden() {
        assert_eq!(parse_host(Some("0.0.0.0".to_string())).unwrap(), IpAddr::from([0, 0, 0, 0]));
        assert!(parse_host(Some("::1".to_string())).unwrap().is_loopback());
    }

    #[test]
    fn host_must_be_an_ip_address() {
        let error = parse_host(Some("localhost".to_string())).unwrap_err();
        assert!(error.to_string().contains("HOST must be an IP address"));
    }
}
