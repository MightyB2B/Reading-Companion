//! The desktop application.
//!
//! A thin shell: every command here translates an IPC call into a call on
//! `reading-core`, which holds all the actual behaviour.

pub mod commands;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let state = commands::init_state(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_ollama,
            commands::get_settings,
            commands::default_settings,
            commands::save_settings,
            commands::test_ollama_host,
            commands::list_models,
            commands::list_books,
            commands::create_book,
            commands::book_stats,
            commands::delete_book,
            commands::list_pages,
            commands::set_last_page,
            commands::resume_page,
            commands::list_blocks,
            commands::edit_block,
            commands::delete_block,
            commands::set_block_kind,
            commands::import_page,
            commands::run_ocr,
            commands::critique_summary,
            commands::get_exemplar,
            commands::summary_spine,
            commands::is_multiple_sentences,
            commands::look_up_word,
            commands::word_in_context,
            commands::vocabulary,
            commands::block_sentences,
            commands::save_sentence_note,
            commands::set_page_number,
            commands::delete_page,
            commands::paragraph_context,
            commands::import_document,
            commands::book_outline,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
