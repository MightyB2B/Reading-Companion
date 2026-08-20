pub mod analyst;
pub mod coach;
pub mod commands;
pub mod db;
pub mod dict;
pub mod error;
pub mod export;
pub mod ingest;
pub mod models;
pub mod ocr;
pub mod ollama;
pub mod scripture;
pub mod study;

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
            commands::recrop_page,
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
            // ---- the study layer ----
            study::commands::move_catalogue,
            study::commands::book_blocks,
            study::commands::page_gaps,
            study::commands::block_annotations,
            study::commands::add_note,
            study::commands::update_note,
            study::commands::delete_note,
            study::commands::attach_anchors,
            study::commands::note_anchors,
            study::commands::delete_note_anchor,
            study::commands::list_notes,
            study::commands::list_notebooks,
            study::commands::create_notebook,
            study::commands::list_terms,
            study::commands::save_term,
            study::commands::delete_term,
            study::commands::suggest_terms,
            study::commands::suggest_moves,
            study::commands::find_interlocutor,
            study::commands::list_arguments,
            study::commands::create_argument,
            study::commands::save_argument,
            study::commands::delete_argument,
            study::commands::link_arguments,
            study::commands::unlink_arguments,
            study::commands::check_support,
            study::commands::check_charity,
            study::commands::set_thesis,
            study::commands::get_thesis,
            study::commands::scan_references,
            study::commands::citations_of,
            study::commands::verse_text,
            study::commands::locate_reference,
            study::commands::correct_reference,
            study::commands::delete_reference,
            study::commands::canon,
            study::commands::export_book,
            study::commands::search_library,
            study::commands::concordance,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
