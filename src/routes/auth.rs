use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};

use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::user::{AuthResponse, CreateUserRequest, LoginRequest, User, UserResponse},
    services::auth::AuthService,
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
}

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/me", get(me))
}

pub(crate) async fn register(
    State(state): State<AppState>,
    Json(payload): Json<CreateUserRequest>,
) -> AppResult<(StatusCode, Json<AuthResponse>)> {
    // Validate input
    if payload.username.trim().is_empty() {
        return Err(AppError::BadRequest("Username cannot be empty".to_string()));
    }
    if payload.email.trim().is_empty() {
        return Err(AppError::BadRequest("Email cannot be empty".to_string()));
    }
    if payload.password.len() < 8 {
        return Err(AppError::BadRequest(
            "Password must be at least 8 characters".to_string(),
        ));
    }

    // Check if user already exists
    let existing_user = sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE username = ? OR email = ?",
    )
    .bind(&payload.username)
    .bind(&payload.email)
    .fetch_optional(&state.db)
    .await?;

    if existing_user.is_some() {
        return Err(AppError::BadRequest(
            "Username or email already exists".to_string(),
        ));
    }

    // Hash password
    let auth_service = AuthService::new(
        state.config.jwt_secret.clone(),
        state.config.jwt_expiration_hours,
    );
    let password_hash = auth_service.hash_password(&payload.password)?;

    // Create user
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, email, password_hash) VALUES (?, ?, ?) RETURNING *",
    )
    .bind(&payload.username)
    .bind(&payload.email)
    .bind(&password_hash)
    .fetch_one(&state.db)
    .await?;

    // Generate token
    let token = auth_service.generate_token(user.id, &user.username)?;

    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            token,
            user: user.into(),
        }),
    ))
}

pub(crate) async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    // Find user
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(&payload.username)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::Authentication("Invalid credentials".to_string()))?;

    // Verify password
    let auth_service = AuthService::new(
        state.config.jwt_secret.clone(),
        state.config.jwt_expiration_hours,
    );

    let is_valid = auth_service.verify_password(&payload.password, &user.password_hash)?;
    if !is_valid {
        return Err(AppError::Authentication("Invalid credentials".to_string()));
    }

    // Generate token
    let token = auth_service.generate_token(user.id, &user.username)?;

    Ok(Json(AuthResponse {
        token,
        user: user.into(),
    }))
}

async fn me(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<UserResponse>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(auth_user.user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    Ok(Json(user.into()))
}
