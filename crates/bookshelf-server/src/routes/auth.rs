//! Auth endpoints: register / login / me.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use diesel::ExpressionMethods as _;
use diesel::QueryDsl as _;
use diesel_async::RunQueryDsl as _;

use bookshelf_core::model::{Role, User};

use diesel::OptionalExtension as _;

use crate::error::ApiError;
use crate::routes::{St, current_user};
use crate::rows::UserRow;
#[allow(unused_imports)]
use crate::schema::users;

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
    if !st.auth.allow_register() {
        return Err(ApiError::Forbidden);
    }
    let username = validate_username(&req.username)?;
    if req.password.chars().count() < 8 {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters",
        ));
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let hash = st.auth.hash_password(&req.password)?;

    let mut conn = st.diesel_db.get().await?;
    // The very first account bootstraps the admin role — there is no other
    // way to create one.
    let first = users::table.count().get_result::<i64>(&mut conn).await? == 0;
    let role = if first {
        Role::Admin.to_string()
    } else {
        Role::User.to_string()
    };
    let result = diesel::insert_into(users::table)
        .values((
            users::id.eq(&id),
            users::username.eq(&username),
            users::password_hash.eq(&hash),
            users::role.eq(&role),
        ))
        .execute(&mut conn)
        .await;
    if matches!(
        &result,
        Err(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::UniqueViolation,
            _
        ))
    ) {
        return Err(ApiError::Conflict("username already taken".into()));
    }
    result?;

    let user = User {
        id,
        username,
        created_at: String::new(),
        role: if first {
            bookshelf_core::model::Role::Admin
        } else {
            bookshelf_core::model::Role::User
        },
    };
    let token = st.auth.issue_token(&user)?;
    Ok(Json(AuthResponse { token, user }))
}

pub async fn login(
    State(st): State<St>,
    Json(req): Json<Credentials>,
) -> Result<impl IntoResponse, ApiError> {
    let mut conn = st.diesel_db.get().await?;
    let row: Option<(String, String, String, String, String)> = users::table
        .filter(users::username.eq(&req.username))
        .select((
            users::id,
            users::username,
            users::role,
            users::created_at,
            users::password_hash,
        ))
        .first::<(String, String, String, String, String)>(&mut conn)
        .await
        .optional()?;
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

pub async fn me(State(st): State<St>, headers: HeaderMap) -> Result<impl IntoResponse, ApiError> {
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
