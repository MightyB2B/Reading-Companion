//! Turning a bearer token into a user, before any handler runs.
//!
//! Handlers take [`Caller`] as an argument. There is no way to write a handler
//! that touches a library without one, and no way to obtain one except by
//! presenting a valid token — so "did this endpoint check authentication?" is
//! answered by its signature rather than by reading its body.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use reading_core::db::pg::{User, UserId};
use reading_core::AppError;

use crate::error::ApiError;
use crate::state::AppState;

/// An authenticated user. Its presence in a handler's arguments is the proof
/// that the request was authenticated.
#[derive(Debug, Clone)]
pub struct Caller(pub User);

impl Caller {
    pub fn id(&self) -> UserId {
        self.0.id
    }
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let token = bearer_token(parts).ok_or_else(|| unauthenticated())?;
        let user = reading_core::auth::authenticate(&state.db, &token).await?;
        Ok(Caller(user))
    }
}

/// A user who may change server-wide settings.
///
/// A separate extractor rather than a flag checked inside the handler: an
/// endpoint either takes an `Admin` or it does not, and forgetting the check
/// is then a thing that cannot be expressed.
#[derive(Debug, Clone)]
pub struct Admin(pub User);

impl Admin {
    pub fn id(&self) -> UserId {
        self.0.id
    }
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Caller(user) = Caller::from_request_parts(parts, state).await?;
        if !user.is_admin {
            // Not "you are not an admin": that tells someone probing the API
            // that the endpoint exists and what it would take to reach it.
            return Err(ApiError(AppError::NotFound("route".into())));
        }
        Ok(Admin(user))
    }
}

fn bearer_token(parts: &Parts) -> Option<String> {
    let header = parts.headers.get(axum::http::header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;

    // Case-insensitive on the scheme, because RFC 7235 says it is and some
    // clients send "bearer".
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

fn unauthenticated() -> ApiError {
    ApiError(AppError::Invalid("not signed in".into()))
}
