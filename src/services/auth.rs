use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,      // user_id
    pub username: String,
    pub exp: i64,         // expiration timestamp
}

pub struct AuthService {
    jwt_secret: String,
    jwt_expiration_hours: i64,
}

impl AuthService {
    pub fn new(jwt_secret: String, jwt_expiration_hours: i64) -> Self {
        Self {
            jwt_secret,
            jwt_expiration_hours,
        }
    }

    /// Hash a password using Argon2
    pub fn hash_password(&self, password: &str) -> AppResult<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();

        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| AppError::Internal(format!("Failed to hash password: {e}")))?
            .to_string();

        Ok(password_hash)
    }

    /// Verify a password against a hash
    pub fn verify_password(&self, password: &str, password_hash: &str) -> AppResult<bool> {
        let parsed_hash = PasswordHash::new(password_hash)
            .map_err(|e| AppError::Internal(format!("Invalid password hash: {e}")))?;

        let argon2 = Argon2::default();

        match argon2.verify_password(password.as_bytes(), &parsed_hash) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Generate a JWT token for a user
    pub fn generate_token(&self, user_id: i64, username: &str) -> AppResult<String> {
        let expiration = Utc::now()
            .checked_add_signed(Duration::hours(self.jwt_expiration_hours))
            .ok_or_else(|| AppError::Internal("Failed to calculate expiration".to_string()))?
            .timestamp();

        let claims = Claims {
            sub: user_id.to_string(),
            username: username.to_string(),
            exp: expiration,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )
        .map_err(|e| AppError::Internal(format!("Failed to generate token: {e}")))?;

        Ok(token)
    }

    /// Validate a JWT token and extract claims
    pub fn validate_token(&self, token: &str) -> AppResult<Claims> {
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|e| AppError::Authentication(format!("Invalid token: {e}")))?;

        Ok(token_data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing() {
        let auth_service = AuthService::new("test_secret".to_string(), 24);
        let password = "test_password_123";

        let hash = auth_service.hash_password(password).unwrap();
        assert!(auth_service.verify_password(password, &hash).unwrap());
        assert!(!auth_service.verify_password("wrong_password", &hash).unwrap());
    }

    #[test]
    fn test_jwt_generation_and_validation() {
        let auth_service = AuthService::new("test_secret".to_string(), 24);

        let token = auth_service.generate_token(1, "testuser").unwrap();
        let claims = auth_service.validate_token(&token).unwrap();

        assert_eq!(claims.sub, "1");
        assert_eq!(claims.username, "testuser");
    }

    #[test]
    fn test_invalid_token() {
        let auth_service = AuthService::new("test_secret".to_string(), 24);
        let result = auth_service.validate_token("invalid.token.here");

        assert!(result.is_err());
    }
}
