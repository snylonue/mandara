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

/// A database-backed user whose request credentials have been verified.
/// The private field prevents handlers from constructing unchecked users.
#[derive(Debug)]
pub struct AuthenticatedUser(User);

impl AuthenticatedUser {
    pub fn user(&self) -> &User {
        &self.0
    }

    pub fn into_inner(self) -> User {
        self.0
    }
}

/// A guard proving that the authenticated user has the admin role.
#[derive(Debug)]
pub struct AdminUser {
    _user: AuthenticatedUser,
}

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
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;
        AuthService::require_admin(user.user())?;
        Ok(Self { _user: user })
    }
}

#[cfg(test)]
mod tests;
