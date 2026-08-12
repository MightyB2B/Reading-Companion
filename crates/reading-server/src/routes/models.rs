//! Server-wide configuration and the inference server behind it.
//!
//! Reading these is open to any signed-in user, because the client needs to
//! know which models are configured to show sensible status. Changing them
//! takes an [`Admin`]: `ollama_host` is a URL that *this server* then makes
//! requests to, so an ordinary account able to set it would have server-side
//! request forgery.

use axum::extract::{Query, State};
use axum::Json;
use reading_core::ollama::{ModelInfo, OllamaClient};
use serde::{Deserialize, Serialize};

use crate::auth_layer::{Admin, Caller};
use crate::error::ApiResult;
use crate::state::{AppState, Settings};

/// Settings as sent to a client.
///
/// The API key is never included. A reader who can read settings could
/// otherwise read the credential to somebody's paid inference account.
#[derive(Serialize)]
pub struct PublicSettings {
    pub ollama_host: String,
    /// Whether a key is set, which is all a client needs to render the field.
    pub has_api_key: bool,
    pub ocr_model: String,
    pub text_model: String,
    pub vision_model: String,
    pub keep_alive: String,
    pub open_registration: bool,
}

impl From<Settings> for PublicSettings {
    fn from(s: Settings) -> Self {
        Self {
            ollama_host: s.ollama_host,
            has_api_key: !s.ollama_api_key.trim().is_empty(),
            ocr_model: s.ocr_model,
            text_model: s.text_model,
            vision_model: s.vision_model,
            keep_alive: s.keep_alive,
            open_registration: s.open_registration,
        }
    }
}

#[derive(Deserialize)]
pub struct SettingsUpdate {
    pub ollama_host: String,
    /// Absent leaves the stored key alone; empty string clears it. Without
    /// this distinction a client that never sees the key cannot save any
    /// other setting without wiping it.
    #[serde(default)]
    pub ollama_api_key: Option<String>,
    pub ocr_model: String,
    pub text_model: String,
    pub vision_model: String,
    pub keep_alive: String,
    /// Absent leaves it as it was, so a client that does not know about this
    /// setting cannot turn it on by omission.
    #[serde(default)]
    pub open_registration: Option<bool>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    _caller: Caller,
) -> ApiResult<Json<PublicSettings>> {
    Ok(Json(state.settings().into()))
}

pub async fn save_settings(
    State(state): State<AppState>,
    _admin: Admin,
    Json(body): Json<SettingsUpdate>,
) -> ApiResult<Json<PublicSettings>> {
    let existing = state.settings();

    let cleaned = Settings {
        ollama_host: Settings::normalise_host(&body.ollama_host),
        ollama_api_key: match body.ollama_api_key {
            Some(k) => k.trim().to_string(),
            None => existing.ollama_api_key.clone(),
        },
        ocr_model: non_empty(&body.ocr_model, &existing.ocr_model),
        text_model: non_empty(&body.text_model, &existing.text_model),
        vision_model: non_empty(&body.vision_model, &existing.vision_model),
        keep_alive: non_empty(&body.keep_alive, &existing.keep_alive),
        open_registration: body.open_registration.unwrap_or(existing.open_registration),
    };

    if cleaned.open_registration && !existing.open_registration {
        tracing::warn!("registration opened: anyone who can reach this server can now create an account");
    }

    cleaned.save(&state.db).await?;
    *state.settings.lock().unwrap_or_else(|e| e.into_inner()) = cleaned.clone();
    state.rebuild_ollama(&cleaned);

    Ok(Json(cleaned.into()))
}

#[derive(Serialize)]
pub struct OllamaStatus {
    pub reachable: bool,
    pub models: Vec<String>,
    pub ocr_model_ready: bool,
    pub text_model_ready: bool,
    pub message: String,
}

pub async fn check_ollama(
    State(state): State<AppState>,
    _caller: Caller,
) -> ApiResult<Json<OllamaStatus>> {
    let settings = state.settings();

    Ok(Json(match state.ollama().health().await {
        Ok(models) => {
            let has = |want: &str| {
                models
                    .iter()
                    .any(|m| m == want || m.split(':').next() == Some(want))
            };
            let ocr_ready = has(&settings.ocr_model);
            let text_ready = has(&settings.text_model);

            let mut missing = Vec::new();
            if !ocr_ready {
                missing.push(settings.ocr_model.clone());
            }
            if !text_ready {
                missing.push(settings.text_model.clone());
            }

            let message = if missing.is_empty() {
                "Ollama is running and both models are installed.".to_string()
            } else {
                format!(
                    "Ollama is running, but these models are missing: {}. Install with: ollama pull {}",
                    missing.join(", "),
                    missing.join(" && ollama pull ")
                )
            };

            OllamaStatus {
                reachable: true,
                models,
                ocr_model_ready: ocr_ready,
                text_model_ready: text_ready,
                message,
            }
        }
        Err(e) => OllamaStatus {
            reachable: false,
            models: Vec::new(),
            ocr_model_ready: false,
            text_model_ready: false,
            message: auth_hint(&e).unwrap_or_else(|| e.to_string()),
        },
    }))
}

#[derive(Deserialize)]
pub struct HostQuery {
    /// Try this address instead of the configured one, so an administrator can
    /// see what a machine has before committing to it.
    pub host: Option<String>,
    pub api_key: Option<String>,
}

/// Models installed on the inference server.
///
/// Any signed-in user may read the configured server's list — the client needs
/// it to show which model is in use. Probing an *arbitrary* address is a
/// different act: it makes this server fetch a URL of the caller's choosing,
/// so it takes an administrator.
pub async fn list_models(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<HostQuery>,
) -> ApiResult<Json<Vec<ModelInfo>>> {
    let client = match q.host.as_deref() {
        Some(host) if !host.trim().is_empty() => {
            if !caller.0.is_admin {
                return Err(reading_core::AppError::NotFound("route".into()).into());
            }
            OllamaClient::new(Settings::normalise_host(host), q.api_key.as_deref())
        }
        _ => state.ollama(),
    };

    Ok(Json(client.installed_models().await?))
}

fn non_empty(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Say what a rejected request actually means. The server plainly answered
/// when it returns 401, so reporting it as unreachable would send an
/// administrator to check the network when the problem is the key.
fn auth_hint(e: &reading_core::AppError) -> Option<String> {
    match e {
        reading_core::AppError::OllamaStatus { status: 401, .. } => Some(
            "The inference server answered, but rejected the API key.".to_string(),
        ),
        reading_core::AppError::OllamaStatus { status: 403, .. } => Some(
            "The inference server answered, but refused this key.".to_string(),
        ),
        _ => None,
    }
}
