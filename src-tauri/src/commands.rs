//! Tauri commands: the only way the React frontend talks to the Rust core.
//!
//! On failure React receives only a `CommandError` with a short message it
//! can show to the user; technical detail (which may include file paths, SQL,
//! or FSRS internals) is printed to the developer console instead.
//!
//! Tauri maps snake_case argument names to camelCase for JS, so `deck_id`
//! is passed as `deckId`.

use chrono::Utc;
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, State};

use tauri_plugin_dialog::DialogExt;

use crate::authoring::{self, ArchivedDeck, AuthoringError, DeckDetail, InvalidInput};
use crate::db::Database;
use crate::export::{self, ExportError};
use crate::import::{self, ImportError, ImportState, RestoreError};
use crate::notes::{self, DeletedNote, InvalidNote, Note, NoteError, NoteSummary};
use crate::scheduler::Rating;
use crate::study::{self, DeckSummary, SessionCard, StartedSession, StudyError};

/// Error sent to React: `{ "kind": "failed" | "stale" | "invalid", "message": "..." }`,
/// plus `"field": "name" | "description" | "front" | "back" | "title" | "body"`
/// when the user's input failed validation.
#[derive(Debug, serde::Serialize)]
pub struct CommandError {
    kind: ErrorKind,
    message: &'static str,
    /// The form field to fix. Only present for invalid user input.
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<Field>,
}

/// The form fields a validation error can point at.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum Field {
    Name,
    Description,
    Front,
    Back,
    Title,
    Body,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum ErrorKind {
    /// Something went wrong; trying again may help.
    Failed,
    /// The data changed since it was shown; reload instead of retrying.
    Stale,
    /// The request refers to something that doesn't exist or is malformed.
    Invalid,
}

impl CommandError {
    fn failed(message: &'static str) -> Self {
        CommandError {
            kind: ErrorKind::Failed,
            message,
            field: None,
        }
    }

    fn stale(message: &'static str) -> Self {
        CommandError {
            kind: ErrorKind::Stale,
            message,
            field: None,
        }
    }

    fn invalid(message: &'static str) -> Self {
        CommandError {
            kind: ErrorKind::Invalid,
            message,
            field: None,
        }
    }

    /// A validation error that names the field to fix. The length limits in
    /// these messages must match `authoring.rs` (checked by a test below).
    fn invalid_input(problem: InvalidInput) -> Self {
        let (field, message) = match problem {
            InvalidInput::NameBlank => (Field::Name, "Enter a deck name."),
            InvalidInput::NameTooLong => (Field::Name, "Deck names can be at most 100 characters."),
            InvalidInput::NameTaken => (Field::Name, "You already have a deck with that name."),
            InvalidInput::DescriptionTooLong => (
                Field::Description,
                "Descriptions can be at most 500 characters.",
            ),
            InvalidInput::FrontBlank => (Field::Front, "Enter the front of the card."),
            InvalidInput::FrontTooLong => {
                (Field::Front, "The front can be at most 2,000 characters.")
            }
            InvalidInput::BackBlank => (Field::Back, "Enter the back of the card."),
            InvalidInput::BackTooLong => (Field::Back, "The back can be at most 2,000 characters."),
        };
        CommandError {
            kind: ErrorKind::Invalid,
            message,
            field: Some(field),
        }
    }

    /// As [`CommandError::invalid_input`], for a note. The limits must match
    /// `notes.rs` (checked by a test below).
    fn invalid_note(problem: InvalidNote) -> Self {
        let (field, message) = match problem {
            InvalidNote::TitleBlank => (Field::Title, "Give the note a title."),
            InvalidNote::TitleTooLong => (Field::Title, "Titles can be at most 200 characters."),
            InvalidNote::BodyBlank => (Field::Body, "Write something in the note."),
            InvalidNote::BodyTooLong => (Field::Body, "A note can be at most 100,000 characters."),
        };
        CommandError {
            kind: ErrorKind::Invalid,
            message,
            field: Some(field),
        }
    }
}

const DECK_NOT_FOUND: &str = "That deck doesn't exist.";
const NOTE_NOT_FOUND: &str = "That note doesn't exist.";
const NOTE_DELETED: &str = "This note was already deleted.";
const NOTE_NOT_DELETED: &str = "This note isn't deleted.";
const DECK_ARCHIVED: &str = "This deck has been archived.";
const DECK_NOT_ARCHIVED: &str = "This deck isn't archived.";
const CARD_NOT_FOUND: &str = "That card doesn't exist.";
const CARD_NOT_DELETED: &str = "This card isn't deleted.";
const SESSION_NOT_FOUND: &str = "That review session doesn't exist.";

async fn pool(app: &AppHandle, database: &Database) -> Result<SqlitePool, CommandError> {
    database.pool(app).await.map_err(|err| {
        eprintln!("[synapse] opening database failed: {err}");
        CommandError::failed("Synapse couldn't open its local database.")
    })
}

/// Turns a study error into a user-safe error, logging internal detail.
fn study_error(
    err: StudyError,
    context: &str,
    not_found: &'static str,
    failed: &'static str,
) -> CommandError {
    match err {
        StudyError::NotFound => CommandError::invalid(not_found),
        StudyError::DeckArchived => CommandError::stale(DECK_ARCHIVED),
        StudyError::Stale => CommandError::stale("This card was already reviewed."),
        StudyError::Internal(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::failed(failed)
        }
    }
}

/// Turns an authoring error into a user-safe error, logging internal detail.
/// User-typed text is never logged.
fn authoring_error(err: AuthoringError, context: &str, failed: &'static str) -> CommandError {
    match err {
        AuthoringError::Invalid(problem) => CommandError::invalid_input(problem),
        AuthoringError::DeckNotFound => CommandError::invalid(DECK_NOT_FOUND),
        AuthoringError::SampleDeck => CommandError::invalid("The sample deck can't be edited."),
        // The screen is out of date: the deck was archived since it was shown.
        AuthoringError::DeckArchived => CommandError::stale(DECK_ARCHIVED),
        // Likewise, the other way round: the archived list this came from no
        // longer holds this deck. Nothing was written, so React reloads
        // instead of retrying, as it does for a note that isn't deleted.
        AuthoringError::DeckNotArchived => CommandError::stale(DECK_NOT_ARCHIVED),
        AuthoringError::CardNotFound => CommandError::invalid(CARD_NOT_FOUND),
        AuthoringError::CardDeleted => CommandError::stale("This card was already deleted."),
        // The other way round: the *Deleted cards* list this came from no
        // longer holds this card. Nothing was written, so React reloads
        // instead of retrying, as it does for a note that isn't deleted.
        AuthoringError::CardNotDeleted => CommandError::stale(CARD_NOT_DELETED),
        AuthoringError::Internal(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::failed(failed)
        }
    }
}

/// Turns a note error into a user-safe error, logging internal detail.
/// The note's text is never logged.
fn note_error(err: NoteError, context: &str, failed: &'static str) -> CommandError {
    match err {
        NoteError::Invalid(problem) => CommandError::invalid_note(problem),
        NoteError::NotFound => CommandError::invalid(NOTE_NOT_FOUND),
        // Both mean the screen is out of date rather than the request being
        // wrong, so React reloads instead of retrying. Nothing was written.
        NoteError::Deleted => CommandError::stale(NOTE_DELETED),
        NoteError::NotDeleted => CommandError::stale(NOTE_NOT_DELETED),
        NoteError::Internal(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::failed(failed)
        }
    }
}

/// The dashboard: every active (not archived) deck with its due-card count.
/// Returns `[{ id, name, description, isSample, dueCount }]`.
#[tauri::command]
pub async fn get_decks(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<DeckSummary>, CommandError> {
    let pool = pool(&app, &database).await?;
    study::decks(&pool, Utc::now()).await.map_err(|err| {
        eprintln!("[synapse] loading decks failed: {err}");
        CommandError::failed("Synapse couldn't load your decks.")
    })
}

/// The dashboard's history: every archived deck, most recently archived first.
/// Returns `[{ id, name, description, cardCount, archivedAt }]`.
#[tauri::command]
pub async fn get_archived_decks(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<ArchivedDeck>, CommandError> {
    let pool = pool(&app, &database).await?;
    authoring::archived_decks(&pool).await.map_err(|err| {
        eprintln!("[synapse] loading archived decks failed: {err}");
        CommandError::failed("Synapse couldn't load your archived decks.")
    })
}

/// Starts (or resumes) a review session for `{ deckId }`.
/// Returns `{ status: "started", sessionId }` or `{ status: "noneDue" }`.
#[tauri::command]
pub async fn start_session(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
) -> Result<StartedSession, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    study::start_session(&pool, deck_id, Utc::now())
        .await
        .map_err(|err| {
            study_error(
                err,
                "starting a session",
                DECK_NOT_FOUND,
                "Synapse couldn't start the review.",
            )
        })
}

/// The next card for `{ sessionId }`, or its completed state.
/// Returns `{ status: "due", card: { id, front, back, reps },
/// progress: { reviewed, remaining } }` or `{ status: "completed", cardsReviewed }`.
///
/// `progress.remaining` includes the card being returned, so the review screen
/// shows card `reviewed + 1` of `reviewed + remaining`.
#[tauri::command]
pub async fn get_session_card(
    app: AppHandle,
    database: State<'_, Database>,
    session_id: i64,
) -> Result<SessionCard, CommandError> {
    if session_id < 1 {
        return Err(CommandError::invalid(SESSION_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    study::session_card(&pool, session_id, Utc::now())
        .await
        .map_err(|err| {
            study_error(
                err,
                "loading a session card",
                SESSION_NOT_FOUND,
                "Synapse couldn't load a flashcard.",
            )
        })
}

/// Saves a rating for the card currently shown in a session.
///
/// Arguments: `{ sessionId, cardId, expectedReps, rating: 1 | 2 | 3 | 4 }`,
/// where `expectedReps` is the `reps` value of the card that was displayed.
/// Returns `null` on success.
#[tauri::command]
pub async fn review_card(
    app: AppHandle,
    database: State<'_, Database>,
    session_id: i64,
    card_id: i64,
    expected_reps: i64,
    rating: i64,
) -> Result<(), CommandError> {
    let Some(rating) = Rating::from_number(rating) else {
        eprintln!("[synapse] rejected review with invalid rating {rating}");
        return Err(CommandError::invalid("That rating isn't valid."));
    };
    if session_id < 1 || card_id < 1 || expected_reps < 0 {
        eprintln!(
            "[synapse] rejected review with invalid ids: session {session_id}, \
             card {card_id}, expected reps {expected_reps}"
        );
        return Err(CommandError::invalid("That review request isn't valid."));
    }

    let pool = pool(&app, &database).await?;
    study::record_review(
        &pool,
        session_id,
        card_id,
        expected_reps,
        rating,
        Utc::now(),
    )
    .await
    .map_err(|err| {
        study_error(
            err,
            "saving a review",
            SESSION_NOT_FOUND,
            "Your rating couldn't be saved.",
        )
    })
}

/// Creates a normal deck (never the sample deck).
///
/// Arguments: `{ name, description }`, where `description` may be `null` or
/// blank for none. Returns the new deck as
/// `{ id, name, description, cardCount, dueCount }`.
#[tauri::command]
pub async fn create_deck(
    app: AppHandle,
    database: State<'_, Database>,
    name: String,
    description: Option<String>,
) -> Result<DeckDetail, CommandError> {
    let pool = pool(&app, &database).await?;
    authoring::create_deck(&pool, &name, description.as_deref(), Utc::now())
        .await
        .map_err(|err| authoring_error(err, "creating a deck", "Synapse couldn't save the deck."))
}

/// One normal deck with its card counts and active cards, for the deck screen.
/// Arguments: `{ deckId }`. Returns
/// `{ id, name, description, cardCount, dueCount, cards: [{ id, front, back }] }`.
#[tauri::command]
pub async fn get_deck_detail(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
) -> Result<DeckDetail, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::deck_detail(&pool, deck_id, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "loading a deck", "Synapse couldn't load this deck."))
}

/// Adds a front/back card to a normal deck. The card is new and due immediately.
///
/// Arguments: `{ deckId, front, back }`. Returns the updated deck (same shape
/// as `get_deck_detail`).
#[tauri::command]
pub async fn create_flashcard(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    front: String,
    back: String,
) -> Result<DeckDetail, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::create_flashcard(&pool, deck_id, &front, &back, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "adding a card", "Synapse couldn't save the card."))
}

/// Replaces a card's front and back, keeping its schedule and review history.
///
/// Arguments: `{ cardId, front, back }`. Returns the card's deck (same shape as
/// `get_deck_detail`).
#[tauri::command]
pub async fn update_flashcard(
    app: AppHandle,
    database: State<'_, Database>,
    card_id: i64,
    front: String,
    back: String,
) -> Result<DeckDetail, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::update_flashcard(&pool, card_id, &front, &back, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "editing a card", "Synapse couldn't save the card."))
}

/// Soft-deletes a card: it leaves lists, counts, and reviews, but its row and
/// review history are kept, and it can be restored.
///
/// Arguments: `{ cardId }`. Returns the card's deck without it, now listing it
/// under `deletedCards` (same shape as `get_deck_detail`).
#[tauri::command]
pub async fn delete_flashcard(
    app: AppHandle,
    database: State<'_, Database>,
    card_id: i64,
) -> Result<DeckDetail, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::delete_flashcard(&pool, card_id, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "deleting a card", "Synapse couldn't delete the card."))
}

/// Restores a soft-deleted card: it returns to its deck with its FSRS state,
/// due date, counts, and review history exactly as they were. No review
/// session is started, and the card is due again only if the due date it
/// already had has passed.
///
/// Arguments: `{ cardId }`. Returns the card's deck with it back (same shape
/// as `get_deck_detail`); rejects with kind "stale" if the card isn't deleted
/// or its deck has since been archived, writing nothing either way.
#[tauri::command]
pub async fn restore_flashcard(
    app: AppHandle,
    database: State<'_, Database>,
    card_id: i64,
) -> Result<DeckDetail, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::restore_flashcard(&pool, card_id, Utc::now())
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "restoring a card",
                "Synapse couldn't restore the card.",
            )
        })
}

/// Renames an active normal deck; its cards, schedule, and history are kept.
///
/// Arguments: `{ deckId, name }`. Returns the renamed deck (same shape as
/// `get_deck_detail`).
#[tauri::command]
pub async fn rename_deck(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    name: String,
) -> Result<DeckDetail, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::rename_deck(&pool, deck_id, &name, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "renaming a deck", "Synapse couldn't rename the deck."))
}

/// Archives an active normal deck: it leaves the dashboard and reviews, and its
/// unfinished session (if any) ends. Nothing is removed, and *Unarchive deck*
/// brings it back.
///
/// Arguments: `{ deckId }`. Returns `null` on success; rejects with kind
/// "stale" if the deck was already archived.
#[tauri::command]
pub async fn archive_deck(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
) -> Result<(), CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::archive_deck(&pool, deck_id, Utc::now())
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "archiving a deck",
                "Synapse couldn't archive the deck.",
            )
        })
}

/// Unarchives an archived normal deck: it returns to the dashboard exactly as
/// it was, with its cards, their schedules, and its review history. No review
/// session is started or resumed, and deleted cards stay deleted.
///
/// Arguments: `{ deckId }`. Returns `null` on success; rejects with kind
/// "stale" if the deck isn't archived, writing nothing either way.
#[tauri::command]
pub async fn unarchive_deck(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
) -> Result<(), CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::unarchive_deck(&pool, deck_id)
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "unarchiving a deck",
                "Synapse couldn't unarchive the deck.",
            )
        })
}

/// The note library: every active (not deleted) note, most recently saved first.
/// Returns `[{ id, title, createdAt, updatedAt }]`.
#[tauri::command]
pub async fn get_notes(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<NoteSummary>, CommandError> {
    let pool = pool(&app, &database).await?;
    notes::notes(&pool).await.map_err(|err| {
        eprintln!("[synapse] loading notes failed: {err}");
        CommandError::failed("Synapse couldn't load your notes.")
    })
}

/// The library's history: every deleted note, most recently deleted first.
/// Returns `[{ id, title, createdAt, updatedAt, deletedAt }]`.
#[tauri::command]
pub async fn get_deleted_notes(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<DeletedNote>, CommandError> {
    let pool = pool(&app, &database).await?;
    notes::deleted_notes(&pool).await.map_err(|err| {
        eprintln!("[synapse] loading deleted notes failed: {err}");
        CommandError::failed("Synapse couldn't load your deleted notes.")
    })
}

/// One whole active note. Arguments: `{ noteId }`. Returns
/// `{ id, title, body, createdAt, updatedAt }`; rejects with kind "stale" if
/// the note has been deleted.
#[tauri::command]
pub async fn get_note(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
) -> Result<Note, CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    notes::note(&pool, note_id)
        .await
        .map_err(|err| note_error(err, "loading a note", "Synapse couldn't load this note."))
}

/// Saves a new plain-text note.
///
/// Arguments: `{ title, body }`. Returns the saved note (same shape as
/// `get_note`); rejects with kind "invalid" and a `field` if either is blank
/// or too long.
#[tauri::command]
pub async fn create_note(
    app: AppHandle,
    database: State<'_, Database>,
    title: String,
    body: String,
) -> Result<Note, CommandError> {
    let pool = pool(&app, &database).await?;
    notes::create_note(&pool, &title, &body, Utc::now())
        .await
        .map_err(|err| note_error(err, "saving a note", "Synapse couldn't save the note."))
}

/// Replaces a note's title and body.
///
/// Arguments: `{ noteId, title, body }`. Returns the saved note (same shape as
/// `get_note`).
#[tauri::command]
pub async fn update_note(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
    title: String,
    body: String,
) -> Result<Note, CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    notes::update_note(&pool, note_id, &title, &body, Utc::now())
        .await
        .map_err(|err| note_error(err, "editing a note", "Synapse couldn't save the note."))
}

/// Soft-deletes a note: it leaves the library, but its text is kept and it can
/// be restored. No card is changed, because no card is linked to a note.
///
/// Arguments: `{ noteId }`. Returns `null` on success; rejects with kind
/// "stale" if the note was already deleted, writing nothing either way.
#[tauri::command]
pub async fn delete_note(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
) -> Result<(), CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    notes::delete_note(&pool, note_id, Utc::now())
        .await
        .map_err(|err| note_error(err, "deleting a note", "Synapse couldn't delete the note."))
}

/// Restores a deleted note: it returns to the library exactly as it was.
///
/// Arguments: `{ noteId }`. Returns `null` on success; rejects with kind
/// "stale" if the note isn't deleted, writing nothing either way.
#[tauri::command]
pub async fn restore_note(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
) -> Result<(), CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    notes::restore_note(&pool, note_id).await.map_err(|err| {
        note_error(
            err,
            "restoring a note",
            "Synapse couldn't restore the note.",
        )
    })
}

/// What an export attempt did: `{ "status": "saved", "fileName": "..." }` or
/// `{ "status": "cancelled" }`.
///
/// Only the file's name is returned, never the folder the user picked.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ExportOutcome {
    Saved {
        file_name: String,
    },
    /// The user closed the save dialog without choosing a file.
    Cancelled,
}

/// Saves a backup of all Synapse study data as a `.zip` the user chooses.
///
/// Opens the OS "save as" dialog, so nothing is written anywhere the user
/// didn't pick, and the OS asks before replacing an existing file. Closing
/// the dialog returns `{ status: "cancelled" }` and writes nothing.
///
/// Takes no arguments. Returns [`ExportOutcome`].
#[tauri::command]
pub async fn export_data(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<ExportOutcome, CommandError> {
    let pool = pool(&app, &database).await?;

    // The dialog reports the chosen path through a callback, so hand it back
    // to this async command over a one-message channel.
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .add_filter("Synapse export", &["zip"])
        .set_file_name(export::suggested_file_name(Utc::now()));
    // Owned by the main window, so it stays in front of Synapse instead of
    // being lost behind it while the UI waits for an answer.
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.save_file(move |path| {
        // Capacity 1 and one message, so this can't block or overflow.
        let _ = sender.try_send(path);
    });

    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the export save dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the save window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(ExportOutcome::Cancelled);
    };
    let destination = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen export path was unusable: {err}");
        CommandError::invalid("That save location can't be used.")
    })?;

    // Saving the archive onto Synapse's own database file would destroy it.
    // The dialog makes this take real effort, but the cost is losing every
    // card, so refuse it outright.
    if database.is_database_file(&app, &destination) {
        return Err(CommandError::invalid(
            "That's Synapse's own data file. Choose another name.",
        ));
    }

    // Timed when the package is written, not when the dialog opened, so
    // `exported_at` says when the backup was actually taken.
    export::write_package(
        &pool,
        &destination,
        Utc::now(),
        app.package_info().version.to_string().as_str(),
    )
    .await
    .map_err(|err| match err {
        ExportError::Destination(detail) => {
            eprintln!("[synapse] writing the export failed: {detail}");
            CommandError::failed("Synapse couldn't save the export there. Try another folder.")
        }
        ExportError::Internal(detail) => {
            eprintln!("[synapse] building the export failed: {detail}");
            CommandError::failed("Synapse couldn't build the export.")
        }
    })?;

    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "your export".to_string());
    Ok(ExportOutcome::Saved { file_name })
}

/// What choosing a backup to restore did:
/// `{ "status": "ready", "token": 1, "fileName": "...", "exportedAt": "..." }`
/// or `{ "status": "cancelled" }`.
///
/// Only the file's name is returned, never the folder it came from.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ImportCheck {
    /// The backup passed every check and waits for the user to confirm.
    /// `token` identifies it to `confirm_import` and `cancel_import`.
    Ready {
        token: u64,
        file_name: String,
        /// When the backup was made (ISO-8601 UTC), from its manifest.
        exported_at: String,
    },
    /// The user closed the file window without choosing a file.
    Cancelled,
}

const IMPORT_NOT_READY: &str = "Synapse couldn't prepare the restore. Nothing was changed.";

/// Turns a refused or failed backup check into a user-safe error, logging
/// internal detail. Every one of these leaves the current data untouched.
fn import_error(err: ImportError) -> CommandError {
    match err {
        ImportError::NotAnExport(detail) => {
            eprintln!("[synapse] refused an import: {detail}");
            CommandError::invalid("That file isn't a Synapse export. Nothing was changed.")
        }
        ImportError::TooNew(detail) => {
            eprintln!("[synapse] refused an import from a newer Synapse: {detail}");
            CommandError::invalid(
                "That backup was made by a newer version of Synapse, so this version can't \
                 restore it. Nothing was changed.",
            )
        }
        ImportError::Damaged(detail) => {
            eprintln!("[synapse] refused a damaged import: {detail}");
            CommandError::invalid(
                "That backup is damaged or incomplete, so it can't be restored. Nothing was \
                 changed.",
            )
        }
        ImportError::TooLarge => {
            eprintln!("[synapse] refused an import over the size limit");
            CommandError::invalid("That backup is too large to restore. Nothing was changed.")
        }
        ImportError::Unreadable(detail) => {
            eprintln!("[synapse] reading the chosen import failed: {detail}");
            CommandError::failed("Synapse couldn't read that file. Nothing was changed.")
        }
        ImportError::Internal(detail) => {
            eprintln!("[synapse] preparing an import failed: {detail}");
            CommandError::failed(IMPORT_NOT_READY)
        }
    }
}

/// Turns a restore that didn't complete into a user-safe error, logging
/// internal detail.
fn restore_error(err: RestoreError) -> CommandError {
    match err {
        RestoreError::NotRestored(detail) => {
            eprintln!("[synapse] restoring a backup failed; nothing changed: {detail}");
            CommandError::failed(
                "Synapse couldn't restore the backup. Your current data hasn't changed.",
            )
        }
        RestoreError::RollbackFailed(detail) => {
            eprintln!("[synapse] restoring a backup failed, and so did rolling back: {detail}");
            // True either way: the previous data was put back in place, or
            // its rollback copy stays in the import workspace.
            CommandError::failed(
                "Synapse couldn't finish restoring the backup, but a copy of your previous data \
                 was kept. Close and reopen Synapse before doing anything else.",
            )
        }
    }
}

/// Where the database and the import workspace are.
fn import_paths(app: &AppHandle) -> Result<(std::path::PathBuf, std::path::PathBuf), CommandError> {
    Database::file_path(app)
        .and_then(|db_path| Ok((db_path, Database::import_workspace(app)?)))
        .map_err(|err| {
            eprintln!("[synapse] locating the database for an import failed: {err}");
            CommandError::failed(IMPORT_NOT_READY)
        })
}

/// Chooses a backup to restore and checks it, without replacing anything.
///
/// Opens the OS "open file" dialog, then checks the chosen `.zip` completely
/// (see `import::stage_package`) and prepares a private copy of its database.
/// The current data is not touched until `confirm_import`. Closing the dialog
/// returns `{ status: "cancelled" }`.
///
/// Takes no arguments. Returns [`ImportCheck`]; a refused file rejects with
/// kind "invalid".
#[tauri::command]
pub async fn prepare_import(
    app: AppHandle,
    imports: State<'_, ImportState>,
) -> Result<ImportCheck, CommandError> {
    let (_, workspace) = import_paths(&app)?;
    let mut pending = imports.pending().lock().await;
    // Choosing again replaces a backup that was still waiting.
    pending.clear(&workspace);

    // As in `export_data`: the dialog answers through a callback.
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose a Synapse export to restore")
        .add_filter("Synapse export", &["zip"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |path| {
        let _ = sender.try_send(path);
    });

    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the import file dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the file window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(ImportCheck::Cancelled);
    };
    let archive = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen import path was unusable: {err}");
        CommandError::invalid("That file can't be used. Nothing was changed.")
    })?;

    let staged = import::stage_package(&archive, &workspace)
        .await
        .map_err(import_error)?;
    let token = pending.hold(staged.database);
    let file_name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "your backup".to_string());
    Ok(ImportCheck::Ready {
        token,
        file_name,
        exported_at: staged.manifest.exported_at,
    })
}

/// Replaces all current study data with the checked backup held under
/// `{ token }`. This is the irreversible step; the frontend asks first.
///
/// Returns `null` on success, after which every screen must reload. Rejects
/// with kind "stale" if that backup is no longer waiting (choose it again),
/// or "failed" if the restore couldn't complete — in which case the previous
/// data is still in place, unless the message says to restart.
#[tauri::command]
pub async fn confirm_import(
    app: AppHandle,
    database: State<'_, Database>,
    imports: State<'_, ImportState>,
    token: u64,
) -> Result<(), CommandError> {
    let (db_path, workspace) = import_paths(&app)?;
    let mut pending = imports.pending().lock().await;
    let Some(staged) = pending.take(token) else {
        return Err(CommandError::stale(
            "That restore is no longer ready. Choose the backup again.",
        ));
    };

    let result = async {
        // The database is open whenever the dashboard is shown, but make
        // sure: replacing needs the open pool to take its rollback copy.
        pool(&app, &database).await?;
        import::replace_database(&database, &db_path, &staged, &workspace)
            .await
            .map_err(restore_error)
    }
    .await;

    // Whatever happened, the checked copy is now the database or not needed.
    import::discard_staged(&workspace);
    result
}

/// Forgets the checked backup held under `{ token }` and removes its private
/// copy. Nothing else changes. Returns `null`, even if it was already gone.
#[tauri::command]
pub async fn cancel_import(
    app: AppHandle,
    imports: State<'_, ImportState>,
    token: u64,
) -> Result<(), CommandError> {
    let (_, workspace) = import_paths(&app)?;
    let mut pending = imports.pending().lock().await;
    if pending.take(token).is_some() {
        import::discard_staged(&workspace);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{CARD_TEXT_MAX_CHARS, DECK_DESCRIPTION_MAX_CHARS, DECK_NAME_MAX_CHARS};
    use crate::notes::{NOTE_BODY_MAX_CHARS, NOTE_TITLE_MAX_CHARS};
    use serde_json::json;

    #[test]
    fn errors_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(CommandError::invalid_input(InvalidInput::NameTaken)).unwrap(),
            json!({
                "kind": "invalid",
                "message": "You already have a deck with that name.",
                "field": "name"
            })
        );
        // Errors that aren't about one field have no `field` key at all.
        assert_eq!(
            serde_json::to_value(CommandError::failed("Synapse couldn't save the card.")).unwrap(),
            json!({ "kind": "failed", "message": "Synapse couldn't save the card." })
        );
    }

    #[test]
    fn card_errors_tell_react_whether_to_reload() {
        let to_json = |err| serde_json::to_value(authoring_error(err, "test", "failed")).unwrap();
        // Already deleted: the screen is out of date, so React reloads it.
        assert_eq!(
            to_json(AuthoringError::CardDeleted),
            json!({ "kind": "stale", "message": "This card was already deleted." })
        );
        // The other way round: the *Deleted cards* list is out of date, so
        // React reloads that instead of retrying the restore.
        assert_eq!(
            to_json(AuthoringError::CardNotDeleted),
            json!({ "kind": "stale", "message": "This card isn't deleted." })
        );
        assert_eq!(
            to_json(AuthoringError::CardNotFound),
            json!({ "kind": "invalid", "message": "That card doesn't exist." })
        );
        assert_eq!(
            to_json(AuthoringError::SampleDeck),
            json!({ "kind": "invalid", "message": "The sample deck can't be edited." })
        );
        // Deck refusals carry no `field`: writing a card from a note relies on
        // that to show them under its deck list.
        assert_eq!(
            to_json(AuthoringError::DeckNotFound),
            json!({ "kind": "invalid", "message": "That deck doesn't exist." })
        );
    }

    #[test]
    fn archived_deck_errors_tell_react_to_reload() {
        let archived = json!({ "kind": "stale", "message": "This deck has been archived." });
        assert_eq!(
            serde_json::to_value(authoring_error(
                AuthoringError::DeckArchived,
                "test",
                "failed"
            ))
            .unwrap(),
            archived
        );
        assert_eq!(
            serde_json::to_value(study_error(
                StudyError::DeckArchived,
                "test",
                DECK_NOT_FOUND,
                "failed"
            ))
            .unwrap(),
            archived
        );
        // The mirror image: unarchiving a deck that isn't archived means the
        // archived list on screen is out of date, so React reloads that too
        // rather than retrying. Nothing was written either way.
        assert_eq!(
            serde_json::to_value(authoring_error(
                AuthoringError::DeckNotArchived,
                "test",
                "failed"
            ))
            .unwrap(),
            json!({ "kind": "stale", "message": "This deck isn't archived." })
        );
    }

    #[test]
    fn export_outcomes_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(ExportOutcome::Saved {
                file_name: "synapse-export-2026-09-16.zip".to_string()
            })
            .unwrap(),
            json!({ "status": "saved", "fileName": "synapse-export-2026-09-16.zip" })
        );
        assert_eq!(
            serde_json::to_value(ExportOutcome::Cancelled).unwrap(),
            json!({ "status": "cancelled" })
        );
    }

    #[test]
    fn import_checks_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(ImportCheck::Ready {
                token: 3,
                file_name: "synapse-export-2026-09-16.zip".to_string(),
                exported_at: "2026-09-16T09:30:00.000Z".to_string(),
            })
            .unwrap(),
            json!({
                "status": "ready",
                "token": 3,
                "fileName": "synapse-export-2026-09-16.zip",
                "exportedAt": "2026-09-16T09:30:00.000Z"
            })
        );
        assert_eq!(
            serde_json::to_value(ImportCheck::Cancelled).unwrap(),
            json!({ "status": "cancelled" })
        );
    }

    #[test]
    fn refused_backups_are_reported_without_detail() {
        let to_json = |err| serde_json::to_value(import_error(err)).unwrap();
        // Internal detail (here, a path) never reaches React.
        let detail = || -> crate::db::DbError { "C:\\Users\\someone\\secret.zip: bad".into() };
        for (err, kind) in [
            (ImportError::NotAnExport(detail()), "invalid"),
            (ImportError::TooNew(detail()), "invalid"),
            (ImportError::Damaged(detail()), "invalid"),
            (ImportError::TooLarge, "invalid"),
            (ImportError::Unreadable(detail()), "failed"),
            (ImportError::Internal(detail()), "failed"),
        ] {
            let value = to_json(err);
            assert_eq!(value["kind"], kind);
            let message = value["message"].as_str().unwrap();
            assert!(
                !message.contains("secret") && !message.contains('\\'),
                "{message}"
            );
            assert!(message.ends_with("Nothing was changed."), "{message}");
            assert!(value.get("field").is_none());
        }
    }

    #[test]
    fn failed_restores_are_reported_without_detail() {
        let detail = || -> crate::db::DbError { "C:\\data\\synapse.sqlite: disk I/O error".into() };
        assert_eq!(
            serde_json::to_value(restore_error(RestoreError::NotRestored(detail()))).unwrap(),
            json!({
                "kind": "failed",
                "message": "Synapse couldn't restore the backup. Your current data hasn't changed."
            })
        );
        assert_eq!(
            serde_json::to_value(restore_error(RestoreError::RollbackFailed(detail()))).unwrap(),
            json!({
                "kind": "failed",
                "message": "Synapse couldn't finish restoring the backup, but a copy of your \
                            previous data was kept. Close and reopen Synapse before doing \
                            anything else."
            })
        );
    }

    #[test]
    fn length_messages_state_the_real_limits() {
        let cases = [
            (InvalidInput::NameTooLong, DECK_NAME_MAX_CHARS),
            (InvalidInput::DescriptionTooLong, DECK_DESCRIPTION_MAX_CHARS),
            (InvalidInput::FrontTooLong, CARD_TEXT_MAX_CHARS),
            (InvalidInput::BackTooLong, CARD_TEXT_MAX_CHARS),
        ];
        for (problem, limit) in cases {
            let message = CommandError::invalid_input(problem)
                .message
                .replace(',', "");
            assert!(
                message.contains(&format!(" {limit} characters")),
                "{problem:?}: {message}"
            );
        }
        for (problem, limit) in [
            (InvalidNote::TitleTooLong, NOTE_TITLE_MAX_CHARS),
            (InvalidNote::BodyTooLong, NOTE_BODY_MAX_CHARS),
        ] {
            let message = CommandError::invalid_note(problem).message.replace(',', "");
            assert!(
                message.contains(&format!(" {limit} characters")),
                "{problem:?}: {message}"
            );
        }
    }

    #[test]
    fn notes_serialize_to_the_shape_react_expects() {
        let time = "2026-09-16T12:00:00.000Z".to_string();
        assert_eq!(
            serde_json::to_value(Note {
                id: 1,
                title: "Cells".to_string(),
                body: "one\ntwo".to_string(),
                created_at: time.clone(),
                updated_at: time.clone(),
            })
            .unwrap(),
            json!({
                "id": 1,
                "title": "Cells",
                "body": "one\ntwo",
                "createdAt": "2026-09-16T12:00:00.000Z",
                "updatedAt": "2026-09-16T12:00:00.000Z"
            })
        );
        assert_eq!(
            serde_json::to_value(NoteSummary {
                id: 1,
                title: "Cells".to_string(),
                created_at: time.clone(),
                updated_at: time,
            })
            .unwrap(),
            json!({
                "id": 1,
                "title": "Cells",
                "createdAt": "2026-09-16T12:00:00.000Z",
                "updatedAt": "2026-09-16T12:00:00.000Z"
            })
        );
    }

    #[test]
    fn note_errors_point_at_the_field_to_fix() {
        let to_json = |err| serde_json::to_value(note_error(err, "test", "failed")).unwrap();
        assert_eq!(
            to_json(NoteError::Invalid(InvalidNote::TitleBlank)),
            json!({ "kind": "invalid", "message": "Give the note a title.", "field": "title" })
        );
        assert_eq!(
            to_json(NoteError::Invalid(InvalidNote::BodyBlank)),
            json!({ "kind": "invalid", "message": "Write something in the note.", "field": "body" })
        );
        assert_eq!(
            to_json(NoteError::Invalid(InvalidNote::TitleTooLong))["field"],
            "title"
        );
        assert_eq!(
            to_json(NoteError::Invalid(InvalidNote::BodyTooLong))["field"],
            "body"
        );
        assert_eq!(
            to_json(NoteError::NotFound),
            json!({ "kind": "invalid", "message": "That note doesn't exist." })
        );
        assert_eq!(
            to_json(NoteError::Internal("no such table: notes".into())),
            json!({ "kind": "failed", "message": "failed" })
        );
    }

    #[test]
    fn deleted_note_errors_tell_react_to_reload() {
        let to_json = |err| serde_json::to_value(note_error(err, "test", "failed")).unwrap();
        // Both mean the screen is out of date, not that the request was wrong,
        // so React reloads the library instead of retrying. Neither carries a
        // `field`: nothing the user typed is at fault.
        assert_eq!(
            to_json(NoteError::Deleted),
            json!({ "kind": "stale", "message": "This note was already deleted." })
        );
        assert_eq!(
            to_json(NoteError::NotDeleted),
            json!({ "kind": "stale", "message": "This note isn't deleted." })
        );
    }

    #[test]
    fn deleted_notes_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(DeletedNote {
                id: 1,
                title: "Cells".to_string(),
                created_at: "2026-09-16T12:00:00.000Z".to_string(),
                updated_at: "2026-09-16T13:00:00.000Z".to_string(),
                deleted_at: "2026-09-16T14:00:00.000Z".to_string(),
            })
            .unwrap(),
            json!({
                "id": 1,
                "title": "Cells",
                "createdAt": "2026-09-16T12:00:00.000Z",
                "updatedAt": "2026-09-16T13:00:00.000Z",
                "deletedAt": "2026-09-16T14:00:00.000Z"
            })
        );
    }
}
