//! Auth endpoints: register / login / me.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use bookshelf_core::model::User;

use crate::error::ApiError;
use crate::routes::{current_user, St};
use crate::rows::UserRow;

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

pub async fn register(
    State(st): State<St>,
    Json(req): Json<Credentials>,
) -> Result<impl IntoResponse, ApiError> {
    if !st.auth.enabled() {
        return Err(ApiError::bad_request("authentication is disabled"));
    }
    if !st.auth.allow_register() {
        return Err(ApiError::Forbidden);
    }
    let username = validate_username(&req.username)?;
    if req.password.chars().count() < 8 {
        return Err(ApiError::bad_request("password must be at least 8 characters"));
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let hash = st.auth.hash_password(&req.password)?;

    let result = sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, 'user')")
        .bind(&id)
        .bind(&username)
        .bind(&hash)
        .execute(&st.db)
        .await;
    if let Err(sqlx::Error::Database(e)) = &result {
        if e.is_unique_violation() {
            return Err(ApiError::Conflict("username already taken".into()));
        }
    }
    result?;

    let user = User {
        id,
        username,
        role: bookshelf_core::model::Role::User,
        created_at: String::new(),
    };
    let token = st.auth.issue_token(&user)?;
    Ok(Json(AuthResponse { token, user }))
}

pub async fn login(
    State(st): State<St>,
    Json(req): Json<Credentials>,
) -> Result<impl IntoResponse, ApiError> {
    if !st.auth.enabled() {
        return Err(ApiError::bad_request("authentication is disabled"));
    }
    let row: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, username, role, created_at, password_hash FROM users WHERE username = ?",
    )
    .bind(&req.username)
    .fetch_optional(&st.db)
    .await?;
    let Some((id, username, role, created_at, password_hash)) = row else {
        return Err(ApiError::Unauthorized);
    };
    if !st.auth.verify_password(&req.password, &password_hash) {
        return Err(ApiError::Unauthorized);
    }
    let user = UserRow {
        id,
        username,
        role,
        created_at,
    }
    .into_model()?;
    let token = st.auth.issue_token(&user)?;
    Ok(Json(AuthResponse { token, user }))
}

pub async fn me(
    State(st): State<St>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    Ok(Json(user))
}

fn validate_username(username: &str) -> Result<String, ApiError> {
    let len = username.chars().count();
    if !(3..=32).contains(&len) {
        return Err(ApiError::bad_request("username must be 3-32 characters"));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "username may only contain letters, digits, '_' and '-'",
        ));
    }
    Ok(username.to_string())
}