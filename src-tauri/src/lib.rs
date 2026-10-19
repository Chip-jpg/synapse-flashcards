mod authoring;
mod commands;
mod db;
mod export;
// Public for its test of the PDF helper process (`tests/pdf_helper.rs`).
pub mod extract;
mod import;
mod notes;
mod scheduler;
mod study;
mod suggest;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // The native "save as" and "open" dialogs used by export and import.
        // Only Rust opens them, so the frontend needs no dialog permission.
        .plugin(tauri_plugin_dialog::init())
        // The database opens lazily on the first command call; see `db::Database`.
        .manage(db::Database::default())
        // A checked backup waiting for the user to confirm a restore.
        .manage(import::ImportState::default())
        // The card suggestion Ollama is working on, so it can be stopped.
        .manage(suggest::Suggesting::default())
        .setup(|app| {
            // A backup checked in an earlier run but never confirmed or
            // cancelled (the window closed while Synapse asked) can't be
            // confirmed any more, so its private copy goes now. Rollback
            // copies are left alone.
            if let Ok(workspace) = db::Database::import_workspace(app.handle()) {
                import::discard_staged(&workspace);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_decks,
            commands::get_today,
            commands::get_archived_decks,
            commands::start_session,
            commands::get_session_card,
            commands::review_card,
            commands::create_deck,
            commands::get_deck_detail,
            commands::create_flashcard,
            commands::create_flashcards,
            commands::update_flashcard,
            commands::delete_flashcard,
            commands::restore_flashcard,
            commands::rename_deck,
            commands::archive_deck,
            commands::unarchive_deck,
            commands::export_data,
            commands::prepare_import,
            commands::confirm_import,
            commands::cancel_import,
            commands::get_notes,
            commands::get_deleted_notes,
            commands::get_note,
            commands::create_note,
            commands::update_note,
            commands::delete_note,
            commands::restore_note,
            commands::import_file,
            commands::get_models,
            commands::suggest_cards,
            commands::stop_suggesting
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
