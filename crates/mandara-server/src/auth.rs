//! Authentication: JWT-based auth with role checks.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderValue};
use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use mandara_core::model::{Role, User};

use crate::db::DieselDb;
use crate::error::ApiError;
use crate::rows::UserRow;
#[allow(unused_imports)]
use crate::schema::users;
use diesel::QueryDsl as _;
use diesel::prelude::SelectableHelper as _;
use diesel_async::RunQueryDsl as _;

/// Name of the session cookie carrying the JWT (HttpOnly — the browser
/// sends it automatically, so a page refresh never drops the login).
pub const TOKEN_COOKIE: &str = "mandara_token";

/// Lifetime of the session cookie; matches the JWT TTL.
pub const TOKEN_TTL_SECS: usize = 7 * 24 * 3600;

#[derive(Debug, Clone)]
pub struct AuthService {
    allow_register: bool,
    secret: String,
    /// Mark the session cookie `Secure` (only sent over https).
    cookie_secure: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    username: String,
    role: String,
    iat: usize,
    exp: usize,
}

impl AuthService {
    pub fn new(allow_register: bool, secret: &str, cookie_secure: bool) -> Self {
        AuthService {
            allow_register,
            secret: secret.into(),
            cookie_secure,
        }
    }

    /// `Set-Cookie` header value that persists `token` in the session
    /// cookie (HttpOnly + SameSite=Lax; the token is a 7-day JWT).
    pub fn set_token_cookie(&self, token: &str) -> HeaderValue {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{TOKEN_COOKIE}={token}; Path=/; Max-Age={TOKEN_TTL_SECS}; HttpOnly; SameSite=Lax{secure}"
        ))
        .expect("JWT is a valid cookie value")
    }

    /// `Set-Cookie` header value that clears the session cookie (logout).
    pub fn clear_token_cookie(&self) -> HeaderValue {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{TOKEN_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure}"
        ))
        .expect("static cookie value")
    }

    pub fn allow_register(&self) -> bool {
        self.allow_register
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
            role: user.role.to_string(),
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

    /// Resolve the current user from the Authorization header, falling
    /// back to the session cookie (so a page refresh keeps the login).
    ///
    /// Both failure modes — an invalid token and a valid token whose user
    /// was deleted — map to 401: a deleted account voids the session just
    /// like an expired one (there is no delete-user endpoint, so this state
    /// only arises from manual DB edits).
    pub async fn require_user(&self, headers: &HeaderMap, db: &DieselDb) -> Result<User, ApiError> {
        let token = bearer(headers)
            .map(|t| t.to_string())
            .or_else(|| cookie_token(headers))
            .ok_or(ApiError::Unauthorized)?;
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

/// The session cookie value (`mandara_token=…`) from the `Cookie`
/// header, if present.
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for pair in cookie.split(';') {
        let (k, v) = pair.trim().split_once('=')?;
        if k == TOKEN_COOKIE && !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}
