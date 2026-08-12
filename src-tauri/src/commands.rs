//! Tauri IPC surface.
//!
//! Every command is a translation of one frontend call into one HTTP call
//! against `reading-server`. There is no logic here worth testing beyond the
//! shape of the wire — the engine lives in `reading-core` and runs on the
//! server.
//!
//! Commands take and return plain JSON. The frontend's types are the
//! authority for what those shapes are; duplicating them in Rust would mean
//! two definitions to keep in step for no gain, since nothing here inspects
//! them.

use serde_json::Value;
use tauri::State;

use crate::client::{Account, Session};
use crate::error::{ClientError, Result};

pub struct AppState {
    pub session: Session,
}

// --- The server, and who we are to it ---------------------------------------

/// Where the library is. Held on this machine rather than in the library,
/// because it is the one thing that cannot be read from the library.
#[tauri::command]
pub fn server_address(state: State<'_, AppState>) -> String {
    state.session.base_url()
}

#[tauri::command]
pub fn set_server_address(state: State<'_, AppState>, url: String) -> Result<String> {
    state.session.set_base_url(&url);
    Ok(state.session.base_url())
}

/// Is the server there, and does it have any accounts yet?
///
/// Answers the sign-in screen's two questions in one call: whether to offer
/// registration, and whether to say the address is wrong.
#[tauri::command]
pub async fn server_state(state: State<'_, AppState>) -> Result<Value> {
    let setup: Value = state.session.get("/setup-state").await?;
    Ok(serde_json::json!({
        "reachable": true,
        "needs_setup": setup.get("needs_setup").and_then(Value::as_bool).unwrap_or(false),
        "signed_in": state.session.is_signed_in(),
    }))
}

#[tauri::command]
pub async fn sign_in(
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> Result<Account> {
    state.session.sign_in(&email, &password).await
}

#[tauri::command]
pub async fn register(
    state: State<'_, AppState>,
    email: String,
    display_name: String,
    password: String,
) -> Result<Account> {
    state
        .session
        .register(&email, &display_name, &password)
        .await
}

#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>) -> Result<()> {
    state.session.sign_out().await
}

/// The signed-in account, or null.
///
/// Null rather than an error when signed out: at startup "nobody is signed in"
/// is the ordinary case, not a failure.
#[tauri::command]
pub async fn current_account(state: State<'_, AppState>) -> Result<Option<Account>> {
    if !state.session.is_signed_in() {
        return Ok(None);
    }
    match state.session.get::<Account>("/me").await {
        Ok(account) => Ok(Some(account)),
        Err(e) if e.is_unauthenticated() => Ok(None),
        Err(e) => Err(e),
    }
}

// --- Books ------------------------------------------------------------------

#[tauri::command]
pub async fn list_books(state: State<'_, AppState>) -> Result<Value> {
    state.session.get("/books").await
}

#[tauri::command]
pub async fn create_book(
    state: State<'_, AppState>,
    title: String,
    author: Option<String>,
    era: String,
) -> Result<i64> {
    let created: Value = state
        .session
        .post(
            "/books",
            &serde_json::json!({ "title": title, "author": author, "era": era }),
        )
        .await?;

    created
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| ClientError::BadResponse("no book id came back".into()))
}

#[tauri::command]
pub async fn get_book(state: State<'_, AppState>, book_id: i64) -> Result<Value> {
    state.session.get(&format!("/books/{book_id}")).await
}

#[tauri::command]
pub async fn book_stats(state: State<'_, AppState>, book_id: i64) -> Result<Value> {
    state.session.get(&format!("/books/{book_id}/stats")).await
}

#[tauri::command]
pub async fn delete_book(state: State<'_, AppState>, book_id: i64) -> Result<()> {
    state.session.delete(&format!("/books/{book_id}")).await
}

#[tauri::command]
pub async fn set_last_page(
    state: State<'_, AppState>,
    book_id: i64,
    page_id: Option<i64>,
) -> Result<()> {
    state
        .session
        .put(
            &format!("/books/{book_id}/last-page"),
            &serde_json::json!({ "page_id": page_id }),
        )
        .await
}

#[tauri::command]
pub async fn resume_page(state: State<'_, AppState>, book_id: i64) -> Result<Option<i64>> {
    let response: Value = state.session.get(&format!("/books/{book_id}/resume")).await?;
    Ok(response.get("page_id").and_then(Value::as_i64))
}

#[tauri::command]
pub async fn summary_spine(state: State<'_, AppState>, book_id: i64) -> Result<Value> {
    state.session.get(&format!("/books/{book_id}/spine")).await
}

#[tauri::command]
pub async fn vocabulary(state: State<'_, AppState>, book_id: i64) -> Result<Value> {
    state
        .session
        .get(&format!("/books/{book_id}/vocabulary"))
        .await
}

// --- Pages ------------------------------------------------------------------

#[tauri::command]
pub async fn list_pages(state: State<'_, AppState>, book_id: i64) -> Result<Value> {
    state.session.get(&format!("/books/{book_id}/pages")).await
}

#[tauri::command]
pub async fn delete_page(state: State<'_, AppState>, page_id: i64) -> Result<()> {
    state.session.delete(&format!("/pages/{page_id}")).await
}

#[tauri::command]
pub async fn set_page_number(
    state: State<'_, AppState>,
    page_id: i64,
    page_no: Option<i64>,
) -> Result<()> {
    state
        .session
        .put(
            &format!("/pages/{page_id}/number"),
            &serde_json::json!({ "page_no": page_no }),
        )
        .await
}

#[tauri::command]
pub async fn adjacent_pages(state: State<'_, AppState>, page_id: i64) -> Result<Value> {
    state.session.get(&format!("/pages/{page_id}/adjacent")).await
}

/// A page image, as a data URI.
///
/// Fetched through here rather than by the webview so the session token stays
/// in Rust. A data URI rather than a local file because the image lives on the
/// server, which may not be this machine at all.
#[tauri::command]
pub async fn page_image(state: State<'_, AppState>, page_id: i64) -> Result<String> {
    let bytes = state
        .session
        .get_bytes(&format!("/pages/{page_id}/image"))
        .await?;

    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:image/jpeg;base64,{encoded}"))
}

#[tauri::command]
pub async fn book_outline(state: State<'_, AppState>, book_id: i64) -> Result<Value> {
    state.session.get(&format!("/books/{book_id}/outline")).await
}

#[tauri::command]
pub async fn paragraph_context(state: State<'_, AppState>, block_id: i64) -> Result<Value> {
    state
        .session
        .get(&format!("/blocks/{block_id}/context"))
        .await
}

// --- Import -----------------------------------------------------------------

/// Send a photograph to the server.
///
/// The file is read here rather than by the server: the reader picked it with
/// a native file dialog on *this* machine, and the server may be a different
/// one entirely with no access to that path.
#[tauri::command]
pub async fn import_page(
    state: State<'_, AppState>,
    book_id: i64,
    source_path: String,
) -> Result<Value> {
    let bytes = tokio::fs::read(&source_path).await?;

    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    state
        .session
        .post(
            &format!("/books/{book_id}/pages/photo"),
            &serde_json::json!({ "data": data }),
        )
        .await
}

/// Transcribe a page and derive its paragraphs.
#[tauri::command]
pub async fn run_ocr(state: State<'_, AppState>, page_id: i64) -> Result<Value> {
    state
        .session
        .post(&format!("/pages/{page_id}/transcribe"), &serde_json::json!({}))
        .await
}

/// Import an EPUB, a PDF, or a web page.
///
/// A local file path is only meaningful when the server is on this machine. A
/// URL always is, because the server fetches it.
#[tauri::command]
pub async fn import_document(
    state: State<'_, AppState>,
    book_id: i64,
    source: String,
) -> Result<Value> {
    state
        .session
        .post(
            &format!("/books/{book_id}/documents"),
            &serde_json::json!({ "source": source }),
        )
        .await
}

// --- Blocks -----------------------------------------------------------------

#[tauri::command]
pub async fn list_blocks(state: State<'_, AppState>, page_id: i64) -> Result<Value> {
    state.session.get(&format!("/pages/{page_id}/blocks")).await
}

#[tauri::command]
pub async fn edit_block(state: State<'_, AppState>, block_id: i64, text: String) -> Result<()> {
    state
        .session
        .put(
            &format!("/blocks/{block_id}"),
            &serde_json::json!({ "text": text }),
        )
        .await
}

#[tauri::command]
pub async fn set_block_kind(
    state: State<'_, AppState>,
    block_id: i64,
    kind: String,
) -> Result<()> {
    state
        .session
        .put(
            &format!("/blocks/{block_id}/kind"),
            &serde_json::json!({ "kind": kind }),
        )
        .await
}

#[tauri::command]
pub async fn delete_block(state: State<'_, AppState>, block_id: i64) -> Result<()> {
    state.session.delete(&format!("/blocks/{block_id}")).await
}

// --- The method -------------------------------------------------------------

#[tauri::command]
pub async fn save_sentence_note(
    state: State<'_, AppState>,
    block_id: i64,
    sentence_ordinal: i64,
    text: String,
) -> Result<i64> {
    let saved: Value = state
        .session
        .post(
            &format!("/blocks/{block_id}/summary"),
            &serde_json::json!({
                "sentence": text,
                "sentence_ordinal": sentence_ordinal,
            }),
        )
        .await?;

    saved
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| ClientError::BadResponse("no summary id came back".into()))
}

#[tauri::command]
pub async fn block_sentences(state: State<'_, AppState>, block_id: i64) -> Result<Value> {
    state
        .session
        .get(&format!("/blocks/{block_id}/sentences"))
        .await
}

#[tauri::command]
pub async fn critique_summary(
    state: State<'_, AppState>,
    block_id: i64,
    sentence: String,
    self_checked: bool,
) -> Result<Value> {
    state
        .session
        .post(
            &format!("/blocks/{block_id}/critique"),
            &serde_json::json!({ "sentence": sentence, "self_checked": self_checked }),
        )
        .await
}

#[tauri::command]
pub async fn get_exemplar(state: State<'_, AppState>, block_id: i64) -> Result<Value> {
    state
        .session
        .get(&format!("/blocks/{block_id}/exemplar"))
        .await
}

// --- Dictionary -------------------------------------------------------------

#[tauri::command]
pub async fn look_up_word(state: State<'_, AppState>, word: String) -> Result<Value> {
    state
        .session
        .post("/dictionary/look-up", &serde_json::json!({ "word": word }))
        .await
}

#[tauri::command]
pub async fn word_in_context(
    state: State<'_, AppState>,
    word: String,
    sentence: String,
    block_id: Option<i64>,
) -> Result<Value> {
    state
        .session
        .post(
            "/dictionary/in-context",
            &serde_json::json!({
                "word": word,
                "sentence": sentence,
                "block_id": block_id,
            }),
        )
        .await
}

/// The one-sentence rule, checked without a round trip.
///
/// Kept local deliberately: it runs on every keystroke, and asking a server
/// across a network whether a sentence has ended would make typing feel slow.
#[tauri::command]
pub fn is_multiple_sentences(text: String) -> bool {
    reading_core::coach::counts_as_multiple_sentences(&text)
}

// --- Configuration ----------------------------------------------------------

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Value> {
    state.session.get("/settings").await
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, settings: Value) -> Result<Value> {
    state.session.put("/settings", &settings).await
}

#[tauri::command]
pub async fn check_ollama(state: State<'_, AppState>) -> Result<Value> {
    state.session.get("/ollama/status").await
}

#[tauri::command]
pub async fn list_models(
    state: State<'_, AppState>,
    host: Option<String>,
    api_key: Option<String>,
) -> Result<Value> {
    let mut path = "/ollama/models".to_string();
    let mut query = Vec::new();
    if let Some(h) = host.filter(|h| !h.trim().is_empty()) {
        query.push(format!("host={}", urlencode(&h)));
    }
    if let Some(k) = api_key.filter(|k| !k.trim().is_empty()) {
        query.push(format!("api_key={}", urlencode(&k)));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }

    state.session.get(&path).await
}

/// Percent-encode a query value.
///
/// Small and local rather than another dependency: the only things that go
/// through it are a URL and an API key, and both are ASCII.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_escaped() {
        assert_eq!(urlencode("http://a:7878"), "http%3A%2F%2Fa%3A7878");
        assert_eq!(urlencode("sk-abc_123"), "sk-abc_123");
        // A key containing & must not become a second parameter.
        assert_eq!(urlencode("a&b=c"), "a%26b%3Dc");
    }
}
