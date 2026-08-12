//! Registering, signing in, and signing out.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth_layer::Caller;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct SignInRequest {
    pub email: String,
    pub password: String,
    /// Free text naming the machine, so a reader can recognise their own
    /// sessions later.
    #[serde(default)]
    pub label: String,
}

#[derive(Serialize)]
pub struct SessionResponse {
    /// Shown once. Not recoverable afterwards.
    pub token: String,
    pub expires_at: String,
    pub user: UserResponse,
}

#[derive(Serialize)]
pub struct UserResponse {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
}

impl From<reading_core::db::pg::User> for UserResponse {
    fn from(u: reading_core::db::pg::User) -> Self {
        Self {
            id: u.id.get(),
            email: u.email,
            display_name: u.display_name,
            is_admin: u.is_admin,
        }
    }
}

/// Create an account and sign straight in.
///
/// Signing in as part of registering saves a round trip and avoids the
/// possibility of an account that exists but was never entered.
pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let display_name = if body.display_name.trim().is_empty() {
        body.email.split('@').next().unwrap_or("reader").to_string()
    } else {
        body.display_name.clone()
    };

    reading_core::auth::register(&state.db, &body.email, &display_name, &body.password).await?;

    let session =
        reading_core::auth::sign_in(&state.db, &body.email, &body.password, "registration").await?;

    Ok(Json(SessionResponse {
        token: session.token,
        expires_at: session.expires_at.to_rfc3339(),
        user: session.user.into(),
    }))
}

pub async fn sign_in(
    State(state): State<AppState>,
    Json(body): Json<SignInRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let session =
        reading_core::auth::sign_in(&state.db, &body.email, &body.password, &body.label).await?;

    Ok(Json(SessionResponse {
        token: session.token,
        expires_at: session.expires_at.to_rfc3339(),
        user: session.user.into(),
    }))
}

/// Who am I? Used by a client holding a stored token to find out whether it
/// is still good without making a change.
pub async fn me(caller: Caller) -> ApiResult<Json<UserResponse>> {
    Ok(Json(caller.0.into()))
}

/// Sign out, revoking the presented token.
///
/// Takes the token from the header rather than the body so that signing out
/// cannot revoke somebody else's session by naming it.
pub async fn sign_out(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    _caller: Caller,
) -> ApiResult<axum::http::StatusCode> {
    if let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .map(|(_, t)| t.trim().to_string())
    {
        reading_core::auth::sign_out(&state.db, &token).await?;
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Whether this server has any accounts yet.
///
/// The sign-in screen uses it to offer registration on a fresh server without
/// making the reader guess. It reveals only that the server is unconfigured,
/// which is apparent from being able to register at all.
pub async fn needs_setup(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let any = state.db.has_any_user().await?;
    Ok(Json(serde_json::json!({ "needs_setup": !any })))
}
