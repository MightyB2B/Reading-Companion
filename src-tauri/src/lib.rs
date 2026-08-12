//! The desktop application.
//!
//! A window and a proxy. Every command translates one IPC call into one HTTP
//! call against `reading-server`, which holds the library and does the work.
//!
//! The session token lives in `client::Session` and never crosses into the
//! webview — see that module for why.

pub mod client;
pub mod commands;
pub mod error;

use tauri::Manager;

/// Where the library server is, remembered between launches.
///
/// Stored on this machine rather than in the library, because it is the one
/// piece of configuration that cannot be read from the library.
const SERVER_KEY: &str = "server_address";

/// The server's certificate, when it is one no public authority signed.
///
/// Stored rather than re-imported each launch: a reader should trust their own
/// server once, not every time they open the application.
const CERT_KEY: &str = "server_certificate";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let address = remembered_server(app.handle());
            let certificate = remembered_certificate(app.handle());
            app.manage(commands::AppState {
                session: client::Session::new(address, certificate),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // The server, and who we are to it
            commands::server_address,
            commands::set_server_address,
            commands::server_state,
            commands::trust_server_certificate,
            commands::forget_server_certificate,
            commands::sign_in,
            commands::register,
            commands::sign_out,
            commands::current_account,
            // Books
            commands::list_books,
            commands::create_book,
            commands::get_book,
            commands::book_stats,
            commands::delete_book,
            commands::set_last_page,
            commands::resume_page,
            commands::summary_spine,
            commands::vocabulary,
            commands::book_outline,
            commands::paragraph_context,
            // Import
            commands::import_page,
            commands::run_ocr,
            commands::import_document,
            // Pages
            commands::list_pages,
            commands::delete_page,
            commands::set_page_number,
            commands::adjacent_pages,
            commands::page_image,
            // Blocks
            commands::list_blocks,
            commands::edit_block,
            commands::set_block_kind,
            commands::delete_block,
            // The method
            commands::save_sentence_note,
            commands::block_sentences,
            commands::critique_summary,
            commands::get_exemplar,
            commands::is_multiple_sentences,
            // Dictionary
            commands::look_up_word,
            commands::word_in_context,
            // Configuration
            commands::get_settings,
            commands::save_settings,
            commands::check_ollama,
            commands::list_models,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// The address last used, or the loopback default for someone running the
/// server on the same machine.
fn remembered_server(app: &tauri::AppHandle) -> String {
    remembered(app, SERVER_KEY).unwrap_or_else(|| client::DEFAULT_SERVER.to_string())
}

fn remembered_certificate(app: &tauri::AppHandle) -> Option<String> {
    remembered(app, CERT_KEY)
}

fn remembered(app: &tauri::AppHandle, key: &str) -> Option<String> {
    use tauri_plugin_store::StoreExt;

    app.store("settings.json")
        .ok()
        .and_then(|store| store.get(key))
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.trim().is_empty())
}

/// Remember, or forget, the certificate this reader trusts for their server.
pub fn remember_certificate(app: &tauri::AppHandle, pem: Option<&str>) {
    use tauri_plugin_store::StoreExt;

    if let Ok(store) = app.store("settings.json") {
        match pem {
            Some(p) => store.set(CERT_KEY, serde_json::Value::String(p.to_string())),
            None => {
                store.delete(CERT_KEY);
            }
        }
        let _ = store.save();
    }
}
