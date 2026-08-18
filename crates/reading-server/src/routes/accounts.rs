//! Registering, signing in, and signing out.

use axum::extract::{ConnectInfo, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use crate::auth_layer::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::throttle::{keys_for, Verdict};

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
///
/// Open in exactly two situations: the server has no accounts at all, so
/// somebody has to be able to make the first one; or an administrator has
/// deliberately turned registration on. Otherwise a server reachable from a
/// network would accept signups from anyone who found the port.
pub async fn register(
    State(state): State<AppState>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    Json(body): Json<RegisterRequest>,
) -> ApiResult<Json<SessionResponse>> {
    // Throttled on the source alone: there is no account yet to key on, and
    // this is the other endpoint an anonymous request can reach. Registration
    // is closed on a settled server, but while it is open — which is exactly
    // when a server is most exposed — it hashes a password for anyone asking.
    let source = vec![format!("from:{}", from.ip())];
    if let Verdict::Wait(delay) = state.throttle.check(&source) {
        return Err(too_many(delay));
    }

    let bootstrapping = !state.db.has_any_user().await?;
    if !bootstrapping && !state.settings().open_registration {
        return Err(reading_core::AppError::Invalid(
            "this library is not accepting new accounts. Ask its administrator to turn registration on."
                .into(),
        )
        .into());
    }

    let display_name = if body.display_name.trim().is_empty() {
        body.email.split('@').next().unwrap_or("reader").to_string()
    } else {
        body.display_name.clone()
    };

    if let Err(e) =
        reading_core::auth::register(&state.db, &body.email, &display_name, &body.password).await
    {
        // A rejected registration counts: an address already taken, or a
        // password too short, is a request that cost the server work.
        state.throttle.record_failure(&source);
        return Err(e.into());
    }

    let session =
        reading_core::auth::sign_in(&state.db, &body.email, &body.password, "registration").await?;
    state.throttle.record_success(&source);

    Ok(Json(SessionResponse {
        token: session.token,
        expires_at: session.expires_at.to_rfc3339(),
        user: session.user.into(),
    }))
}

pub async fn sign_in(
    State(state): State<AppState>,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    Json(body): Json<SignInRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let keys = keys_for(&body.email, Some(from.ip()));

    // Checked before the password is verified. A throttled request must not
    // reach argon2, or the throttle would cost the server exactly the work it
    // exists to avoid — which would turn it into the attack.
    if let Verdict::Wait(delay) = state.throttle.check(&keys) {
        return Err(too_many(delay));
    }

    match reading_core::auth::sign_in(&state.db, &body.email, &body.password, &body.label).await {
        Ok(session) => {
            state.throttle.record_success(&keys);
            Ok(Json(SessionResponse {
                token: session.token,
                expires_at: session.expires_at.to_rfc3339(),
                user: session.user.into(),
            }))
        }
        Err(e) => {
            if let Some(delay) = state.throttle.record_failure(&keys) {
                tracing::warn!(
                    from = %from.ip(),
                    seconds = delay.as_secs(),
                    "repeated sign-in failures; throttling"
                );
                // The wait is reported rather than the password being called
                // wrong again: it is now true, and hiding it would leave a
                // legitimate reader retyping a correct password into a
                // request that was never going to be checked.
                return Err(too_many(delay));
            }
            Err(e.into())
        }
    }
}

/// 429, with how long to wait.
///
/// `Retry-After` is the header a client is meant to read, and this one is
/// honest: the same number the server is actually enforcing.
fn too_many(delay: std::time::Duration) -> ApiError {
    let seconds = delay.as_secs().max(1);
    ApiError::retry_after(
        seconds,
        format!("too many attempts. Try again in {}.", describe(seconds)),
    )
}

fn describe(seconds: u64) -> String {
    match seconds {
        0..=1 => "a second".to_string(),
        2..=59 => format!("{seconds} seconds"),
        60..=119 => "a minute".to_string(),
        _ => format!("{} minutes", (seconds + 59) / 60),
    }
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

/// What the sign-in screen needs to know before it can draw itself.
///
/// Unauthenticated, and it says only whether an account can be created — which
/// is discoverable anyway by trying. It does not say how many accounts exist
/// or who they belong to.
pub async fn needs_setup(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let any = state.db.has_any_user().await?;
    Ok(Json(serde_json::json!({
        "needs_setup": !any,
        // Registration is possible on an empty server whatever the setting
        // says, or the first person could never get in.
        "registration_open": !any || state.settings().open_registration,
    })))
}
