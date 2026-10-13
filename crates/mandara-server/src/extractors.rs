//! Request guards for authentication and role-based authorization.
//!
//! Put these before path, query, and body extractors so authentication is
//! checked before request data is parsed. Resource permissions remain the
//! responsibility of the service or resource-specific guard.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use mandara_core::model::User;

use crate::auth::AuthService;
use crate::error::ApiError;
use crate::state::St;

/// Extracts a database-backed user after verifying request credentials.
/// The public field supports destructuring directly in handler parameters.
#[derive(Debug)]
pub struct AuthenticatedUser(pub User);

/// Extracts a user after verifying request credentials and the admin role.
#[derive(Debug)]
pub struct AdminUser(pub User);

impl FromRequestParts<St> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &St) -> Result<Self, Self::Rejection> {
        let user = state
            .auth
            .require_user(&parts.headers, &state.diesel_db)
            .await?;
        Ok(Self(user))
    }
}

impl FromRequestParts<St> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &St) -> Result<Self, Self::Rejection> {
        let AuthenticatedUser(user) = AuthenticatedUser::from_request_parts(parts, state).await?;
        AuthService::require_admin(&user)?;
        Ok(Self(user))
    }
}

#[cfg(test)]
mod tests;
