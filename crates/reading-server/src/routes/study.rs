//! The parts that involve a model: the coach and the dictionary.

use axum::extract::{Path, State};
use axum::Json;
use reading_core::coach;
use reading_core::dict::{ContextualSense, Lookup};
use reading_core::models::Era;
use serde::{Deserialize, Serialize};

use crate::auth_layer::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CritiqueRequest {
    pub sentence: String,
    #[serde(default)]
    pub self_checked: bool,
}

#[derive(Serialize)]
pub struct CritiqueResponse {
    pub summary_id: i64,
    pub feedback: coach::Feedback,
}

/// Grade a summary the reader has written.
pub async fn critique(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
    Json(body): Json<CritiqueRequest>,
) -> ApiResult<Json<CritiqueResponse>> {
    // Fetching the block first is the ownership check for everything below.
    let block = state.db.get_block(caller.id(), block_id).await?;
    let era = era_for_block(&state, caller.clone(), block.page_id).await?;

    let summary_id = state
        .db
        .save_summary(
            caller.id(),
            block_id,
            None,
            &body.sentence,
            body.self_checked,
        )
        .await?;

    let settings = state.settings();
    let assessment = coach::critique(
        &state.ollama(),
        &settings.text_model,
        &block.text_norm,
        &body.sentence,
        era,
    )
    .await?;

    // The rubric is what gets stored: categorical judgements, as the model
    // emitted them. The prose the reader sees is composed from it in Rust and
    // can be recomposed later if the wording changes.
    let r = &assessment.rubric;
    state
        .db
        .save_critique(
            caller.id(),
            summary_id,
            &tag(r.verdict, "partial"),
            (
                r.covers.subject,
                r.covers.main_action,
                r.covers.reason_or_result,
            ),
            &tag(r.problem, "none"),
            &r.steering_question,
            &settings.text_model,
        )
        .await?;

    Ok(Json(CritiqueResponse {
        summary_id,
        feedback: assessment.feedback,
    }))
}

/// A model-written example, only ever on an explicit request.
///
/// Kept a separate endpoint rather than a field on the critique response: the
/// Socratic contract is that an answer cannot arrive as a side effect of
/// asking for feedback, and a separate route is the structural form of that.
pub async fn exemplar(
    State(state): State<AppState>,
    caller: Caller,
    Path(block_id): Path<i64>,
) -> ApiResult<Json<coach::Exemplar>> {
    let block = state.db.get_block(caller.id(), block_id).await?;
    let era = era_for_block(&state, caller.clone(), block.page_id).await?;
    let settings = state.settings();

    Ok(Json(
        coach::exemplar(
            &state.ollama(),
            &settings.text_model,
            &block.text_norm,
            era,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct WordQuery {
    pub word: String,
}

/// Layer one: instant, offline, and unable to invent.
pub async fn look_up_word(
    State(state): State<AppState>,
    _caller: Caller,
    Json(body): Json<WordQuery>,
) -> ApiResult<Json<Option<Lookup>>> {
    let Some(dict) = state.dict.as_deref() else {
        return Ok(Json(None));
    };
    Ok(Json(dict.lookup(&body.word)?))
}

#[derive(Deserialize)]
pub struct ContextQuery {
    pub word: String,
    pub sentence: String,
    #[serde(default)]
    pub block_id: Option<i64>,
}

#[derive(Serialize)]
pub struct ContextResponse {
    pub lookup: Lookup,
    pub in_context: ContextualSense,
}

/// Layer two: which of the retrieved senses applies here.
///
/// The model selects from senses already found in the dictionary rather than
/// defining from memory, which is what makes a hallucinated definition
/// structurally unlikely rather than merely discouraged.
pub async fn word_in_context(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ContextQuery>,
) -> ApiResult<Json<ContextResponse>> {
    let dict = state
        .dict
        .as_deref()
        .ok_or_else(|| ApiError(reading_core::AppError::Invalid(
            "the dictionary is not installed on this server".into(),
        )))?;

    let lookup = dict.lookup(&body.word)?.ok_or_else(|| {
        ApiError(reading_core::AppError::NotFound(format!(
            "{} is not in the dictionary",
            body.word
        )))
    })?;

    let settings = state.settings();
    // The model is given the senses already retrieved from the dictionary and
    // asked to choose, rather than to define. That is what makes an invented
    // meaning structurally unlikely rather than merely discouraged.
    let in_context = reading_core::dict::sense_in_context(
        &state.ollama(),
        &settings.text_model,
        &lookup.word,
        &body.sentence,
        &lookup.senses,
    )
    .await?;

    // Recorded only when we know which book it belongs to, so a stray lookup
    // does not land in the wrong vocabulary list.
    if let Some(block_id) = body.block_id {
        if let Ok(block) = state.db.get_block(caller.id(), block_id).await {
            if let Ok(book_id) = state.db.book_id_for_page(caller.id(), block.page_id).await {
                let _ = state
                    .db
                    .record_lookup(
                        caller.id(),
                        book_id,
                        block_id,
                        &body.word,
                        &lookup.lemma,
                        &body.sentence,
                        &in_context.plain_meaning,
                    )
                    .await;
            }
        }
    }

    Ok(Json(ContextResponse { lookup, in_context }))
}

/// The snake_case name serde gives an enum variant, which is exactly the
/// string the database column holds.
///
/// Serialising a fieldless enum cannot fail, but going through `serde_json`
/// means the stored value tracks the `#[serde(rename_all)]` attribute rather
/// than a second hand-written mapping that could drift from it.
fn tag<T: serde::Serialize>(value: T, fallback: &str) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| fallback.to_string())
}

async fn era_for_block(state: &AppState, caller: Caller, page_id: i64) -> ApiResult<Era> {
    let book_id = state.db.book_id_for_page(caller.id(), page_id).await?;
    let book = state.db.get_book(caller.id(), book_id).await?;
    Ok(Era::from_str_lossy(&book.era))
}
