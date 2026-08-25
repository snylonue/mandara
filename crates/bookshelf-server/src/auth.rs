//! Authentication: optional JWT-based auth with role checks.
//!
//! When `auth_enabled` is false every request acts as the local admin user
//! (the whole permission layer can be switched off: single-user mode).

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{Role, User};

use crate::db::DieselDb;
use crate::error::ApiError;
use crate::rows::UserRow;
#[allow(unused_imports)]
use crate::schema::users;
use diesel::QueryDsl as _;
use diesel::prelude::SelectableHelper as _;
use diesel_async::RunQueryDsl as _;

#[derive(Debug, Clone)]
pub struct AuthService {
    enabled: bool,
    allow_register: bool,
    secret: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    username: String,
    role: String,
    iat: usize,
    exp: usize,
}

const TOKEN_TTL_SECS: usize = 7 * 24 * 3600;

impl AuthService {
    pub fn new(enabled: bool, allow_register: bool, secret: &str) -> Self {
        AuthService {
            enabled,
            allow_register,
            secret: secret.into(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn allow_register(&self) -> bool {
        self.allow_register
    }

    /// The user every request acts as when auth is disabled.
    pub fn local_user(&self) -> User {
        User {
            id: "local".into(),
            username: "local".into(),
            role: Role::Admin,
            created_at: String::new(),
        }
    }

    pub fn hash_password(&self, password: &str) -> Result<String, ApiError> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))
    }

    pub fn verify_password(&self, password: &str, hash: &str) -> bool {
        PasswordHash::new(hash)
            .ok()
            .map(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
            .unwrap_or(false)
    }

    pub fn issue_token(&self, user: &User) -> Result<String, ApiError> {
        let now = Utc::now().timestamp() as usize;
        let claims = Claims {
            sub: user.id.clone(),
            username: user.username.clone(),
            role: user.role.as_str().into(),
            iat: now,
            exp: now + TOKEN_TTL_SECS,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))
    }

    /// Resolve the current user from the Authorization header.
    pub async fn require_user(&self, headers: &HeaderMap, db: &DieselDb) -> Result<User, ApiError> {
        if !self.enabled {
            return Ok(self.local_user());
        }
        let token = bearer(headers).ok_or(ApiError::Unauthorized)?;
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| ApiError::Unauthorized)?;
        use diesel::OptionalExtension as _;
        let mut conn = db.get().await?;
        let row = users::table
            .find(&data.claims.sub)
            .select(UserRow::as_select())
            .first::<UserRow>(&mut conn)
            .await
            .optional()?;
        row.map(UserRow::into_model).ok_or(ApiError::Unauthorized)?
    }

    pub fn require_admin(user: &User) -> Result<(), ApiError> {
        if user.role == Role::Admin {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
