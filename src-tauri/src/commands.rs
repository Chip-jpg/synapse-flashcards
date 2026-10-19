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

use crate::authoring::{self, ArchivedDeck, AuthoringError, CardText, DeckDetail, InvalidInput};
use crate::cards_csv::{self, CardsImported, CsvError, Delimiter, Duplicate, RowProblem};
use crate::db::Database;
use crate::export::{self, ExportError};
use crate::extract::{self, ExtractError, ExtractedText, FileKind};
use crate::import::{self, ImportError, ImportState, RestoreError};
use crate::notes::{self, DeletedNote, InvalidNote, Note, NoteError, NoteSummary};
use crate::optimizer::{self, IntervalChange, Optimized, OptimizerError, Optimizing, Settings};
use crate::scheduler::Rating;
use crate::stats::{self, Statistics};
use crate::study::{self, DeckSummary, SessionCard, StartedSession, StudyError, TodaySummary};
use crate::suggest::{self, NotePart, Ollama, SuggestError, SuggestedCard, Suggesting};

/// Error sent to React: `{ "kind": "failed" | "stale" | "invalid", "message": "..." }`,
/// plus `"field": "name" | "description" | "front" | "back" | "title" | "body"`
/// when the user's input failed validation, and `"index"` when that input is
/// one of several cards.
#[derive(Debug, serde::Serialize)]
pub struct CommandError {
    kind: ErrorKind,
    message: &'static str,
    /// The form field to fix. Only present for invalid user input.
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<Field>,
    /// Which of several cards `field` belongs to, counting from 0. Only
    /// present when adding several cards at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
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
            index: None,
        }
    }

    fn stale(message: &'static str) -> Self {
        CommandError {
            kind: ErrorKind::Stale,
            message,
            field: None,
            index: None,
        }
    }

    fn invalid(message: &'static str) -> Self {
        CommandError {
            kind: ErrorKind::Invalid,
            message,
            field: None,
            index: None,
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
            index: None,
        }
    }

    /// As [`CommandError::invalid_input`], for one of several cards.
    fn invalid_card(index: usize, problem: InvalidInput) -> Self {
        CommandError {
            index: Some(index),
            ..Self::invalid_input(problem)
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
            index: None,
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
        AuthoringError::InvalidCard(index, problem) => CommandError::invalid_card(index, problem),
        AuthoringError::NoCards => CommandError::invalid("There are no cards to save."),
        AuthoringError::TooManyCards => {
            CommandError::invalid("At most 100 cards can be saved at once.")
        }
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
        // Likewise for *Restore all*: the deleted cards on screen aren't the
        // ones the deck has now (none, or a different number).
        AuthoringError::DeletedCardsChanged => {
            CommandError::stale("This deck's deleted cards have changed.")
        }
        AuthoringError::NoneSelected => CommandError::invalid("Select at least one card."),
        AuthoringError::TooManySelected => {
            CommandError::invalid("At most 10,000 cards can be changed at once.")
        }
        AuthoringError::DuplicateSelected => {
            CommandError::invalid("The same card was selected twice.")
        }
        // The cards on screen aren't the ones the deck has now: reload rather
        // than retry. Nothing was written.
        AuthoringError::SelectionChanged => {
            CommandError::stale("Some of the selected cards have changed.")
        }
        AuthoringError::SameDeck => CommandError::invalid("The cards are already in that deck."),
        // The deck list the user chose from is out of date (the deck was
        // archived since), so React reloads it. The sample deck and a missing
        // deck are never offered, so they only arrive here from a stale list.
        AuthoringError::MoveTargetUnavailable => {
            CommandError::stale("That deck can't take cards any more.")
        }
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

/// The Study Desk's "today": reviews recorded since local midnight on this
/// computer's clock, and how many different cards they were for.
/// Returns `{ reviews, cards }`.
#[tauri::command]
pub async fn get_today(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<TodaySummary, CommandError> {
    let pool = pool(&app, &database).await?;
    study::today(&pool, Utc::now(), &chrono::Local)
        .await
        .map_err(|err| {
            eprintln!("[synapse] loading today's reviews failed: {err}");
            CommandError::failed("Synapse couldn't count today's reviews.")
        })
}

/// The Statistics screen: review history over the last 30 days and the
/// current cards' states and due dates, on this computer's calendar, for
/// every deck or only `deckId` (an active deck). Reads only.
/// Returns `{ reviewsByDay, retention, cardStates, forecast }`.
#[tauri::command]
pub async fn get_statistics(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: Option<i64>,
) -> Result<Statistics, CommandError> {
    if deck_id.is_some_and(|id| id < 1) {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    stats::statistics(&pool, deck_id, Utc::now(), &chrono::Local)
        .await
        .map_err(|err| {
            study_error(
                err,
                "loading statistics",
                DECK_NOT_FOUND,
                "Synapse couldn't load your statistics.",
            )
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

/// Adds several front/back cards to a normal deck at once (saving suggested
/// cards), each checked as `create_flashcard` checks one. All or nothing: if
/// any card or the deck is refused, no card is added.
///
/// Arguments: `{ deckId, cards: [{ front, back }] }`. Returns the updated deck
/// (same shape as `get_deck_detail`). A refused card rejects with kind
/// "invalid", its `field`, and its `index` in `cards`.
#[tauri::command]
pub async fn create_flashcards(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    cards: Vec<CardText>,
) -> Result<DeckDetail, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::create_flashcards(&pool, deck_id, &cards, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "adding cards", "Synapse couldn't save the cards."))
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

/// Restores every soft-deleted card of an active normal deck in one go, each
/// exactly as `restore_flashcard` restores one.
///
/// Arguments: `{ deckId, expected }`, where `expected` is how many deleted
/// cards the user confirmed. Returns `{ restored, deck }`: how many came back,
/// and the deck with them (same shape as `get_deck_detail`). Rejects with kind
/// "stale" if the deck has since been archived or no longer has `expected`
/// deleted cards, writing nothing either way.
#[tauri::command]
pub async fn restore_deleted_cards(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    expected: usize,
) -> Result<authoring::RestoredCards, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::restore_deleted_cards(&pool, deck_id, expected, Utc::now())
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "restoring deleted cards",
                "Synapse couldn't restore the cards.",
            )
        })
}

/// Soft-deletes several cards of one deck at once, each exactly as
/// `delete_flashcard` deletes one.
///
/// Arguments: `{ deckId, cardIds }`, the cards the user selected and
/// confirmed. Returns `{ changed, deck }`: how many were deleted, and the deck
/// now listing them under `deletedCards` (same shape as `get_deck_detail`).
/// Rejects with kind "stale" if any of them is no longer an active card of the
/// deck, or the deck has been archived, writing nothing either way.
#[tauri::command]
pub async fn delete_flashcards(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    card_ids: Vec<i64>,
) -> Result<authoring::ChangedCards, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::delete_flashcards(&pool, deck_id, &card_ids, Utc::now())
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "deleting selected cards",
                "Synapse couldn't delete the cards.",
            )
        })
}

/// Moves several cards of one deck into another active normal deck. Each card
/// keeps its id, text, FSRS state, due date, counts, and review history.
///
/// Arguments: `{ deckId, cardIds, toDeckId }`. Returns `{ changed, deck }`:
/// how many moved, and the deck they came from, without them. Rejects with
/// kind "stale" if any card is no longer an active card of `deckId`, or either
/// deck has been archived, writing nothing either way.
#[tauri::command]
pub async fn move_flashcards(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    card_ids: Vec<i64>,
    to_deck_id: i64,
) -> Result<authoring::ChangedCards, CommandError> {
    if deck_id < 1 || to_deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::move_flashcards(&pool, deck_id, &card_ids, to_deck_id, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "moving cards", "Synapse couldn't move the cards."))
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

/// The note library narrowed to the active notes whose title or body contains
/// `query`, ignoring case, most recently saved first. Arguments: `{ query }`;
/// a blank query lists every active note. Returns the same shape as
/// `get_notes`. Reads only.
#[tauri::command]
pub async fn search_notes(
    app: AppHandle,
    database: State<'_, Database>,
    query: String,
) -> Result<Vec<NoteSummary>, CommandError> {
    let pool = pool(&app, &database).await?;
    notes::search_notes(&pool, &query).await.map_err(|err| {
        eprintln!("[synapse] searching notes failed: {err}");
        CommandError::failed("Synapse couldn't search your notes.")
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

const OLLAMA_UNREACHABLE: &str =
    "Synapse couldn't reach Ollama on this computer. Check that Ollama is installed and running, \
     then try again.";
const OLLAMA_UNEXPECTED: &str =
    "Ollama didn't answer the way Synapse expected. Restarting or updating Ollama may help.";

/// Turns a failed suggestion into a user-safe error, logging internal detail.
/// Neither the note nor the model's answer is ever logged.
fn suggest_error(err: SuggestError) -> CommandError {
    match err {
        SuggestError::Unreachable(detail) => {
            eprintln!("[synapse] reaching Ollama failed: {detail}");
            CommandError::failed(OLLAMA_UNREACHABLE)
        }
        SuggestError::Unexpected(detail) => {
            eprintln!("[synapse] Ollama answered unexpectedly: {detail}");
            CommandError::failed(OLLAMA_UNEXPECTED)
        }
        // The model list on screen is out of date, so React reloads it.
        SuggestError::ModelMissing => {
            CommandError::stale("That model isn't in Ollama any more. Choose another.")
        }
        SuggestError::NoteTooLong => CommandError::invalid(
            "This is too long to suggest cards from: up to 12,000 characters of a note can be \
             used at a time.",
        ),
        SuggestError::NoteOverflowed => CommandError::failed(
            "This note was too long for the model to read all at once, so no cards were \
             suggested. Try a shorter note.",
        ),
        SuggestError::TimedOut => CommandError::failed(
            "The model took too long to answer, so Synapse stopped waiting. Try again, or choose \
             a smaller model.",
        ),
        SuggestError::ModelFailed(detail) => {
            eprintln!("[synapse] Ollama couldn't run the model: {detail}");
            CommandError::failed("Ollama couldn't run that model. Try again, or choose another.")
        }
        SuggestError::Unusable(detail) => {
            eprintln!("[synapse] the model's answer was unusable: {detail}");
            CommandError::failed(
                "The model's answer didn't include any usable cards. Try again, or choose \
                 another model.",
            )
        }
        SuggestError::Ungrounded => CommandError::failed(
            "None of the cards the model suggested could be traced back to your note, so none \
             are shown. Try again, or choose a larger model.",
        ),
        SuggestError::Internal(detail) => {
            eprintln!("[synapse] setting up a request to Ollama failed: {detail}");
            CommandError::failed("Synapse couldn't ask Ollama for suggestions.")
        }
    }
}

/// The models cards can be suggested with: Ollama's local models that write
/// text, by name, sorted. Cloud models are never listed. Takes no arguments.
/// Returns `["llama3.2:3b", ...]`, which is empty if Ollama has no models;
/// rejects with kind "failed" if Ollama can't be reached.
#[tauri::command]
pub async fn get_models() -> Result<Vec<String>, CommandError> {
    let ollama = Ollama::local().map_err(suggest_error)?;
    ollama.models().await.map_err(suggest_error)
}

/// What a suggestion did: `{ "status": "suggested", "cards": [{ front, back,
/// source, unfamiliar }], "leftOut": 2, "unsupported": 1 }` or
/// `{ "status": "stopped" }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SuggestOutcome {
    Suggested {
        /// Drafts for the user to check: nothing has been saved.
        cards: Vec<SuggestedCard>,
        /// Suggestions left out as unusable, so the user can be told.
        left_out: usize,
        /// Suggestions left out as not coming from the note.
        unsupported: usize,
    },
    /// `stop_suggesting` was called first.
    Stopped,
}

/// Asks a local model in Ollama to suggest cards from a note. Only Ollama on
/// this computer is asked, and nothing is saved: the cards are drafts, which
/// the user may save with `create_flashcards`. Starting another suggestion,
/// or `stop_suggesting`, stops this one.
///
/// Arguments: `{ noteId, model, part? }`, where `model` is a name from
/// `get_models`, and `part`, `{ start, end }` in characters, is the part of a
/// note too long to use whole that cards are suggested from. Returns
/// [`SuggestOutcome`]; rejects with kind "stale" if the note was deleted, no
/// longer has that part, or the model is no longer in Ollama.
#[tauri::command]
pub async fn suggest_cards(
    app: AppHandle,
    database: State<'_, Database>,
    suggesting: State<'_, Suggesting>,
    note_id: i64,
    model: String,
    part: Option<NotePart>,
) -> Result<SuggestOutcome, CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    // Tracked from the start, so a stop while the note loads counts too.
    let suggestion = suggesting.begin();
    let work = async {
        let pool = pool(&app, &database).await?;
        let note = notes::note(&pool, note_id).await.map_err(|err| {
            note_error(
                err,
                "loading a note to suggest cards from",
                "Synapse couldn't load this note.",
            )
        })?;
        let body = match part {
            None => note.body.as_str(),
            Some(part) => suggest::part_of(&note.body, part).ok_or_else(|| {
                CommandError::stale(
                    "That part of the note has changed. Go back to the note and try again.",
                )
            })?,
        };
        let ollama = Ollama::local().map_err(suggest_error)?;
        ollama
            .suggest(&model, &note.title, body)
            .await
            .map_err(suggest_error)
    };
    match suggestion.run(work).await {
        None => Ok(SuggestOutcome::Stopped),
        Some(result) => result.map(|suggestions| SuggestOutcome::Suggested {
            cards: suggestions.cards,
            left_out: suggestions.left_out,
            unsupported: suggestions.unsupported,
        }),
    }
}

/// Stops the suggestion being worked on, if any; its `suggest_cards` call
/// then returns `{ status: "stopped" }`. Takes no arguments; returns `null`.
#[tauri::command]
pub fn stop_suggesting(suggesting: State<'_, Suggesting>) {
    suggesting.stop();
}

/// What choosing a file to import as a note did: `{ "status": "read", "fileName",
/// "kind", "title", "body", "pages", "pagesWithoutText" }` or
/// `{ "status": "cancelled" }`. Only the file's name is returned, never its
/// folder.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FileImport {
    /// The file's text, for the user to check and save as a note.
    Read {
        file_name: String,
        #[serde(flatten)]
        text: ExtractedText,
    },
    /// The user closed the file window without choosing a file.
    Cancelled,
}

/// Turns a file that couldn't be imported into a user-safe error, logging
/// internal detail (never the file's text).
fn extract_error(err: ExtractError) -> CommandError {
    match err {
        ExtractError::Unsupported => CommandError::invalid(
            "Synapse can import PDF, text (.txt), Markdown (.md), Word (.docx), and PowerPoint \
             (.pptx) files. Save older .doc and .ppt files in the newer format first.",
        ),
        ExtractError::TooLarge => {
            CommandError::invalid("That file is too large to import: the limit is 50 MB.")
        }
        ExtractError::Unreadable(detail) => {
            eprintln!("[synapse] opening a file to import failed: {detail}");
            CommandError::failed("Synapse couldn't open that file.")
        }
        ExtractError::Damaged(detail) => {
            eprintln!("[synapse] refused a file to import: {detail}");
            CommandError::invalid(
                "Synapse couldn't read that file. It may be damaged, password-protected, or not \
                 the kind of file its name says.",
            )
        }
        ExtractError::NoText => CommandError::invalid(
            "That file has no text Synapse can import. If it's scanned, its pages are pictures \
             of text, which Synapse can't read yet.",
        ),
        ExtractError::HelperFailed(detail) => {
            eprintln!("[synapse] reading a PDF failed: {detail}");
            CommandError::failed(
                "Synapse couldn't read that PDF. It may be damaged, or made in a way Synapse \
                 can't read.",
            )
        }
    }
}

/// Chooses a file and reads its text, to import as a new note. Nothing is
/// saved: the text goes to the new-note form, where the user checks it and
/// saves it with `create_note` like a note they typed. The file isn't kept.
///
/// Opens the OS "open file" dialog for PDF, text, Markdown, Word, and
/// PowerPoint files; closing it returns `{ status: "cancelled" }`. Takes no
/// arguments. Returns [`FileImport`]; a file that can't be imported rejects
/// with kind "invalid" or "failed".
#[tauri::command]
pub async fn import_file(app: AppHandle) -> Result<FileImport, CommandError> {
    // As in `export_data`: the dialog answers through a callback.
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose a file to import as a note")
        .add_filter("Notes and documents", &FileKind::EXTENSIONS);
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
        return Ok(FileImport::Cancelled);
    };
    let path = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen file's path was unusable: {err}");
        CommandError::invalid("That file can't be used.")
    })?;
    // PDFs are read by Synapse itself, started as a helper (see `extract.rs`).
    let helper = std::env::current_exe().map_err(|err| {
        eprintln!("[synapse] finding Synapse's own program failed: {err}");
        CommandError::failed("Synapse couldn't read that file.")
    })?;

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "your file".to_string());
    let text = tauri::async_runtime::spawn_blocking(move || extract::read_file(&path, &helper))
        .await
        .map_err(|err| {
            eprintln!("[synapse] reading a file to import stopped: {err}");
            CommandError::failed("Synapse couldn't read that file.")
        })?
        .map_err(extract_error)?;
    Ok(FileImport::Read { file_name, text })
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
    optimizing: State<'_, Optimizing>,
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
    // Parameters fitted to the history that was just replaced don't belong
    // to the restored one.
    optimizing.forget();
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

/// Turns a failed optimization, apply, or reset into a user-safe error,
/// logging internal detail.
fn optimizer_error(err: OptimizerError, context: &str, failed: &'static str) -> CommandError {
    match err {
        OptimizerError::NotEnoughReviews => CommandError::invalid(
            "There aren't enough reviews yet to fit FSRS to your history. Keep reviewing and \
             try again later.",
        ),
        OptimizerError::Stale => CommandError::stale(
            "The scheduling parameters changed since this was shown. Synapse has reloaded them.",
        ),
        OptimizerError::NotBetter => CommandError::invalid(
            "These parameters don't predict your reviews better than the ones in use, so they \
             weren't applied.",
        ),
        OptimizerError::TimedOut => CommandError::failed(
            "Optimizing took too long, so Synapse stopped it. Nothing was changed.",
        ),
        OptimizerError::Internal(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::failed(failed)
        }
    }
}

/// The scheduling settings: which FSRS parameters are in use, and how many
/// reviews an optimization could learn from. Returns [`Settings`].
#[tauri::command]
pub async fn get_scheduling(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Settings, CommandError> {
    let pool = pool(&app, &database).await?;
    optimizer::settings(&pool).await.map_err(|err| {
        eprintln!("[synapse] loading scheduling settings failed: {err}");
        CommandError::failed("Synapse couldn't load the scheduling settings.")
    })
}

/// What an optimization did: `{ "status": "optimized", token, reviewsUsed,
/// current, proposed, currentError, proposedError, better, intervals }` or
/// `{ "status": "stopped" }`. `current` and `proposed` are FSRS's 21
/// parameters; the errors are how far predicted recall is from actual
/// recall on average (0 to 1, lower is better); `intervals` is
/// `[{ case, current, proposed }]` in days.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OptimizeOutcome {
    Optimized {
        /// Pass to `apply_parameters` to use the proposed parameters.
        token: u64,
        reviews_used: usize,
        current: Vec<f32>,
        proposed: Vec<f32>,
        current_error: f32,
        proposed_error: f32,
        /// Whether the proposed parameters predict the history better. Only
        /// then can they be applied.
        better: bool,
        intervals: Vec<IntervalChange>,
    },
    /// `stop_optimizing` was called, or another optimization started.
    Stopped,
}

/// Fits FSRS's parameters to the whole review history, off the UI thread,
/// and compares them with the ones in use. Nothing is saved: the result
/// waits for `apply_parameters`. Takes no arguments. Returns
/// [`OptimizeOutcome`]; rejects with kind "invalid" below the minimum number
/// of reviews, or "failed" if it took too long.
#[tauri::command]
pub async fn optimize_parameters(
    app: AppHandle,
    database: State<'_, Database>,
    optimizing: State<'_, Optimizing>,
) -> Result<OptimizeOutcome, CommandError> {
    let pool = pool(&app, &database).await?;
    let outcome = optimizer::optimize(&pool, &optimizing, optimizer::TIME_LIMIT)
        .await
        .map_err(|err| {
            optimizer_error(
                err,
                "optimizing FSRS parameters",
                "Synapse couldn't optimize the scheduling parameters. Nothing was changed.",
            )
        })?;
    Ok(match outcome {
        Optimized::Stopped => OptimizeOutcome::Stopped,
        Optimized::Proposed { token, proposal } => OptimizeOutcome::Optimized {
            token,
            reviews_used: proposal.reviews_used,
            current: proposal.current,
            proposed: proposal.proposed,
            current_error: proposal.current_fit.error,
            proposed_error: proposal.proposed_fit.error,
            better: proposal.better,
            intervals: proposal.intervals,
        },
    })
}

/// Stops the optimization running now, if any. Returns `null`.
#[tauri::command]
pub fn stop_optimizing(optimizing: State<'_, Optimizing>) {
    optimizing.stop();
}

/// Uses the parameters from the optimization held under `{ token }` for
/// every rating from now on. Past schedules and reviews don't change.
/// Returns the new [`Settings`]; rejects with kind "stale" if that result is
/// no longer waiting or the parameters in use changed since.
#[tauri::command]
pub async fn apply_parameters(
    app: AppHandle,
    database: State<'_, Database>,
    optimizing: State<'_, Optimizing>,
    token: u64,
) -> Result<Settings, CommandError> {
    let pool = pool(&app, &database).await?;
    let Some(proposal) = optimizing.take(token) else {
        return Err(CommandError::stale(
            "That optimization is no longer ready to apply. Optimize again.",
        ));
    };
    let failed = "Synapse couldn't apply the new parameters. Nothing was changed.";
    optimizer::apply(&pool, &proposal, Utc::now())
        .await
        .map_err(|err| optimizer_error(err, "applying FSRS parameters", failed))?;
    get_scheduling(app, database).await
}

/// Goes back to FSRS's default parameters for every rating from now on.
/// `{ version }` is the `version` of the settings shown. Returns the new
/// [`Settings`]; rejects with kind "stale" if they changed since, or the
/// defaults are already in use.
#[tauri::command]
pub async fn reset_parameters(
    app: AppHandle,
    database: State<'_, Database>,
    version: i64,
) -> Result<Settings, CommandError> {
    let pool = pool(&app, &database).await?;
    let failed = "Synapse couldn't reset the parameters. Nothing was changed.";
    optimizer::reset(&pool, version, Utc::now())
        .await
        .map_err(|err| optimizer_error(err, "resetting FSRS parameters", failed))?;
    get_scheduling(app, database).await
}

/// One row of a cards file, checked: `{ line, front, back, problem, duplicate }`.
/// `problem` is `{ field, message }` when the row can't become a card, and
/// `duplicate` says why its card may already be there (see `cards_csv.rs`).
#[derive(Debug, serde::Serialize)]
pub struct CardsFileRow {
    line: usize,
    front: String,
    back: String,
    problem: Option<RowProblemMessage>,
    duplicate: Option<Duplicate>,
}

/// Why a row of a cards file can't become a card, as a typed card's error.
#[derive(Debug, serde::Serialize)]
pub struct RowProblemMessage {
    field: Field,
    message: &'static str,
}

impl RowProblemMessage {
    fn of(problem: RowProblem) -> Self {
        let (field, message) = match problem {
            RowProblem::NoBack => (
                Field::Back,
                "This row has no back: put the back in a second column.",
            ),
            RowProblem::Card(problem) => {
                let error = CommandError::invalid_input(problem);
                (error.field.unwrap_or(Field::Front), error.message)
            }
        };
        RowProblemMessage { field, message }
    }
}

/// What choosing a cards file did:
/// `{ "status": "read", "fileName", "delimiter", "header", "rows", "rowsWithExtraColumns" }`
/// or `{ "status": "cancelled" }`. Nothing has been saved.
#[derive(Debug, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CardsFile {
    Read {
        file_name: String,
        delimiter: Delimiter,
        /// The first row was column names, so it isn't among `rows`.
        header: bool,
        rows: Vec<CardsFileRow>,
        /// Rows with something in a third or later column, which is ignored.
        rows_with_extra_columns: usize,
    },
    /// The user closed the file window without choosing a file.
    Cancelled,
}

/// Turns a cards file or import that was refused into a user-safe error,
/// logging internal detail (never a card's text). The limits in these
/// messages must match `cards_csv.rs` (checked by a test below).
fn csv_error(err: CsvError, context: &str, failed: &'static str) -> CommandError {
    match err {
        CsvError::TooLarge => {
            CommandError::invalid("That file is too large to import: the limit is 10 MB.")
        }
        CsvError::TooManyRows => CommandError::invalid(
            "A file can hold at most 5,000 cards. Split it into smaller files.",
        ),
        CsvError::NoRows => CommandError::invalid("That file has no cards in it."),
        CsvError::UnclosedQuote { line } => {
            eprintln!("[synapse] a cards file has an unclosed quote from line {line}");
            CommandError::invalid(
                "A quoted cell in that file never ends, so Synapse can't tell where the cards \
                 are. Check the file's quote marks.",
            )
        }
        CsvError::OnlyDuplicates => CommandError::stale(
            "Every one of these cards is already in the deck, so nothing was added.",
        ),
        CsvError::Authoring(err) => authoring_error(err, context, failed),
    }
}

/// Chooses a CSV or TSV file of cards for an active normal deck and checks
/// every row, without saving anything: the rows go to the screen, where the
/// user sees which can be added and saves them with `import_cards_csv`.
///
/// Arguments: `{ deckId }`. The deck is checked before the file window opens.
/// Returns [`CardsFile`]; a file that can't be used rejects with kind
/// "invalid" or "failed".
#[tauri::command]
pub async fn read_cards_csv(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
) -> Result<CardsFile, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    // Refuse a deck that can't take cards before asking for a file.
    authoring::deck_detail(&pool, deck_id, Utc::now())
        .await
        .map_err(|err| {
            authoring_error(err, "loading a deck", "Synapse couldn't load this deck.")
        })?;

    // As in `export_data`: the dialog answers through a callback.
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose a file of cards to import")
        .add_filter("Spreadsheet files (CSV, TSV)", &["csv", "tsv", "txt"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |path| {
        let _ = sender.try_send(path);
    });

    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the cards file dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the file window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(CardsFile::Cancelled);
    };
    let path = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen cards file's path was unusable: {err}");
        CommandError::invalid("That file can't be used.")
    })?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "your file".to_string());
    let tsv = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tsv"));

    let bytes = tauri::async_runtime::spawn_blocking(move || {
        // The size first, so a huge file is refused without reading it.
        let size = std::fs::metadata(&path)?.len();
        if size > cards_csv::CSV_FILE_MAX_BYTES {
            return Ok(None);
        }
        std::fs::read(&path).map(Some)
    })
    .await
    .map_err(|err| {
        eprintln!("[synapse] reading a cards file stopped: {err}");
        CommandError::failed("Synapse couldn't read that file.")
    })?
    .map_err(|err: std::io::Error| {
        eprintln!("[synapse] reading a cards file failed: {err}");
        CommandError::failed("Synapse couldn't open that file.")
    })?
    .ok_or_else(|| csv_error(CsvError::TooLarge, "", ""))?;

    let preview = cards_csv::read_cards(&pool, deck_id, &bytes, tsv, Utc::now())
        .await
        .map_err(|err| {
            csv_error(
                err,
                "reading a cards file",
                "Synapse couldn't read that file.",
            )
        })?;
    Ok(CardsFile::Read {
        file_name,
        delimiter: preview.delimiter,
        header: preview.header,
        rows: preview
            .rows
            .into_iter()
            .map(|row| CardsFileRow {
                line: row.line,
                front: row.front,
                back: row.back,
                problem: row.problem.map(RowProblemMessage::of),
                duplicate: row.duplicate,
            })
            .collect(),
        rows_with_extra_columns: preview.rows_with_extra_columns,
    })
}

/// Adds the cards the user kept from a cards file to an active normal deck,
/// each checked as `create_flashcard` checks one. All or nothing. With
/// `skipDuplicates`, cards whose front is already in the deck (or on an
/// earlier card sent) are left out, judged afresh at saving.
///
/// Arguments: `{ deckId, cards: [{ front, back }], skipDuplicates }`. Returns
/// `{ added, skipped, cardCount }`. A refused card rejects with kind
/// "invalid", its `field`, and its `index` in `cards`; nothing new to add
/// rejects with kind "stale".
#[tauri::command]
pub async fn import_cards_csv(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    cards: Vec<CardText>,
    skip_duplicates: bool,
) -> Result<CardsImported, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    cards_csv::import_cards(&pool, deck_id, &cards, skip_duplicates, Utc::now())
        .await
        .map_err(|err| csv_error(err, "importing cards", "Synapse couldn't save the cards."))
}

/// Saves an active deck's cards (front and back only) as a CSV file the user
/// chooses. The sample deck can be exported too. Opens the OS "save as"
/// dialog, as `export_data` does; closing it returns `{ status: "cancelled" }`
/// and writes nothing.
///
/// Arguments: `{ deckId }`. Returns [`ExportOutcome`]. A deck with no cards
/// rejects with kind "invalid" before the dialog opens.
#[tauri::command]
pub async fn export_cards_csv(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
) -> Result<ExportOutcome, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    let (deck_name, cards) =
        cards_csv::deck_cards(&pool, deck_id)
            .await
            .map_err(|err| match err {
                AuthoringError::NoCards => {
                    CommandError::invalid("This deck has no cards to export.")
                }
                err => {
                    authoring_error(err, "exporting cards", "Synapse couldn't export the cards.")
                }
            })?;

    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .add_filter("CSV file", &["csv"])
        .set_file_name(cards_csv::suggested_file_name(&deck_name));
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.save_file(move |path| {
        let _ = sender.try_send(path);
    });

    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the cards export save dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the save window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(ExportOutcome::Cancelled);
    };
    let destination = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen cards export path was unusable: {err}");
        CommandError::invalid("That save location can't be used.")
    })?;
    // As in `export_data`: never write over Synapse's own database.
    if database.is_database_file(&app, &destination) {
        return Err(CommandError::invalid(
            "That's Synapse's own data file. Choose another name.",
        ));
    }

    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "your file".to_string());
    let csv = cards_csv::write_csv(&cards);
    // Written beside the destination first and then moved into place, so a
    // failed write never leaves half a file where the user's file was.
    let partial = destination.with_file_name(format!(".{file_name}.synapse-partial"));
    tauri::async_runtime::spawn_blocking(move || {
        let written =
            std::fs::write(&partial, csv).and_then(|()| std::fs::rename(&partial, &destination));
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        written
    })
    .await
    .map_err(|err| {
        eprintln!("[synapse] writing a cards export stopped: {err}");
        CommandError::failed("Synapse couldn't save the file there. Try another folder.")
    })?
    .map_err(|err| {
        eprintln!("[synapse] writing a cards export failed: {err}");
        CommandError::failed("Synapse couldn't save the file there. Try another folder.")
    })?;
    Ok(ExportOutcome::Saved { file_name })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{
        CARD_BATCH_MAX, CARD_SELECTION_MAX, CARD_TEXT_MAX_CHARS, DECK_DESCRIPTION_MAX_CHARS,
        DECK_NAME_MAX_CHARS,
    };
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
        // *Restore all* on an out-of-date list: reload rather than retry.
        assert_eq!(
            to_json(AuthoringError::DeletedCardsChanged),
            json!({ "kind": "stale", "message": "This deck's deleted cards have changed." })
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
    fn bulk_card_errors_tell_react_whether_to_reload() {
        let to_json = |err| serde_json::to_value(authoring_error(err, "test", "failed")).unwrap();
        assert_eq!(
            to_json(AuthoringError::SelectionChanged),
            json!({ "kind": "stale", "message": "Some of the selected cards have changed." })
        );
        assert_eq!(
            to_json(AuthoringError::MoveTargetUnavailable),
            json!({ "kind": "stale", "message": "That deck can't take cards any more." })
        );
        assert_eq!(
            to_json(AuthoringError::NoneSelected),
            json!({ "kind": "invalid", "message": "Select at least one card." })
        );
        assert_eq!(
            to_json(AuthoringError::SameDeck),
            json!({ "kind": "invalid", "message": "The cards are already in that deck." })
        );
        // The limit in the message must match `authoring.rs`.
        assert_eq!(CARD_SELECTION_MAX, 10_000);
        assert_eq!(
            to_json(AuthoringError::TooManySelected),
            json!({ "kind": "invalid", "message": "At most 10,000 cards can be changed at once." })
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
    fn a_refused_card_among_several_names_its_index_and_field() {
        let to_json = |err| serde_json::to_value(authoring_error(err, "test", "failed")).unwrap();
        assert_eq!(
            to_json(AuthoringError::InvalidCard(2, InvalidInput::BackBlank)),
            json!({
                "kind": "invalid",
                "message": "Enter the back of the card.",
                "field": "back",
                "index": 2
            })
        );
        // A single card's error has no `index`, as before.
        assert_eq!(
            to_json(AuthoringError::Invalid(InvalidInput::FrontTooLong)),
            json!({
                "kind": "invalid",
                "message": "The front can be at most 2,000 characters.",
                "field": "front"
            })
        );
        assert_eq!(
            to_json(AuthoringError::NoCards),
            json!({ "kind": "invalid", "message": "There are no cards to save." })
        );
        let too_many = to_json(AuthoringError::TooManyCards);
        assert_eq!(too_many["kind"], "invalid");
        assert!(too_many["message"]
            .as_str()
            .unwrap()
            .contains(&format!("At most {CARD_BATCH_MAX} cards")));
    }

    #[test]
    fn cards_files_serialize_to_the_shape_react_expects() {
        let file = CardsFile::Read {
            file_name: "biology.csv".to_string(),
            delimiter: Delimiter::Semicolon,
            header: true,
            rows: vec![
                CardsFileRow {
                    line: 2,
                    front: "What is DNA?".to_string(),
                    back: "A molecule.".to_string(),
                    problem: None,
                    duplicate: Some(Duplicate::InFile { line: 1 }),
                },
                CardsFileRow {
                    line: 3,
                    front: "Alone".to_string(),
                    back: String::new(),
                    problem: Some(RowProblemMessage::of(RowProblem::NoBack)),
                    duplicate: Some(Duplicate::InDeck),
                },
                CardsFileRow {
                    line: 4,
                    front: String::new(),
                    back: "b".to_string(),
                    problem: Some(RowProblemMessage::of(RowProblem::Card(
                        InvalidInput::FrontBlank,
                    ))),
                    duplicate: None,
                },
            ],
            rows_with_extra_columns: 0,
        };
        assert_eq!(
            serde_json::to_value(file).unwrap(),
            json!({
                "status": "read",
                "fileName": "biology.csv",
                "delimiter": "semicolon",
                "header": true,
                "rows": [
                    {
                        "line": 2, "front": "What is DNA?", "back": "A molecule.",
                        "problem": null, "duplicate": { "kind": "inFile", "line": 1 }
                    },
                    {
                        "line": 3, "front": "Alone", "back": "",
                        "problem": {
                            "field": "back",
                            "message": "This row has no back: put the back in a second column."
                        },
                        "duplicate": { "kind": "inDeck" }
                    },
                    {
                        "line": 4, "front": "", "back": "b",
                        "problem": { "field": "front", "message": "Enter the front of the card." },
                        "duplicate": null
                    }
                ],
                "rowsWithExtraColumns": 0
            })
        );
        assert_eq!(
            serde_json::to_value(CardsFile::Cancelled).unwrap(),
            json!({ "status": "cancelled" })
        );
        assert_eq!(
            serde_json::to_value(CardsImported {
                added: 2,
                skipped: 1,
                card_count: 5
            })
            .unwrap(),
            json!({ "added": 2, "skipped": 1, "cardCount": 5 })
        );
    }

    #[test]
    fn cards_file_errors_state_the_real_limits() {
        use crate::cards_csv::{CSV_FILE_MAX_BYTES, CSV_ROWS_MAX};
        let message = |err| csv_error(err, "test", "failed").message;
        assert!(message(CsvError::TooLarge)
            .contains(&format!("{} MB", CSV_FILE_MAX_BYTES / 1024 / 1024)));
        assert!(message(CsvError::TooManyRows).contains("5,000"));
        assert_eq!(CSV_ROWS_MAX, 5_000);
        // Nothing new to add: the deck changed, so React says so and reloads.
        let only = serde_json::to_value(csv_error(CsvError::OnlyDuplicates, "test", "failed"));
        assert_eq!(only.unwrap()["kind"], "stale");
        // A deck refusal is the same as when adding typed cards.
        assert_eq!(
            message(CsvError::Authoring(AuthoringError::DeckArchived)),
            DECK_ARCHIVED
        );
    }

    #[test]
    fn suggestions_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(SuggestOutcome::Suggested {
                cards: vec![SuggestedCard {
                    front: "What does mitosis produce?".to_string(),
                    back: "Two identical cells, unlike meiosis.".to_string(),
                    source: "It produces two identical cells.".to_string(),
                    unfamiliar: vec!["unlike".to_string(), "meiosis".to_string()],
                }],
                left_out: 2,
                unsupported: 1,
            })
            .unwrap(),
            json!({
                "status": "suggested",
                "cards": [{
                    "front": "What does mitosis produce?",
                    "back": "Two identical cells, unlike meiosis.",
                    "source": "It produces two identical cells.",
                    "unfamiliar": ["unlike", "meiosis"]
                }],
                "leftOut": 2,
                "unsupported": 1
            })
        );
        assert_eq!(
            serde_json::to_value(SuggestOutcome::Stopped).unwrap(),
            json!({ "status": "stopped" })
        );
    }

    #[test]
    fn failed_suggestions_are_reported_without_detail() {
        let detail = || -> crate::db::DbError { "http://127.0.0.1:11434/api/chat: secret".into() };
        for (err, kind) in [
            (SuggestError::Unreachable(detail()), "failed"),
            (SuggestError::Unexpected(detail()), "failed"),
            (SuggestError::ModelMissing, "stale"),
            (SuggestError::NoteTooLong, "invalid"),
            (SuggestError::NoteOverflowed, "failed"),
            (SuggestError::TimedOut, "failed"),
            (SuggestError::ModelFailed(detail()), "failed"),
            (SuggestError::Unusable(detail()), "failed"),
            (SuggestError::Ungrounded, "failed"),
            (SuggestError::Internal(detail()), "failed"),
        ] {
            let value = serde_json::to_value(suggest_error(err)).unwrap();
            assert_eq!(value["kind"], kind, "{value}");
            let message = value["message"].as_str().unwrap();
            assert!(
                !message.contains("secret") && !message.contains("11434"),
                "{message}"
            );
            assert!(value.get("field").is_none() && value.get("index").is_none());
        }
        let too_long = serde_json::to_value(suggest_error(SuggestError::NoteTooLong)).unwrap();
        assert!(too_long["message"]
            .as_str()
            .unwrap()
            .replace(',', "")
            .contains(&format!(
                " {} characters",
                crate::suggest::SUGGESTION_NOTE_MAX_CHARS
            )));
    }

    #[test]
    fn file_imports_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(FileImport::Read {
                file_name: "Lecture 3.pdf".to_string(),
                text: ExtractedText {
                    kind: FileKind::PowerPoint,
                    title: "Lecture 3".to_string(),
                    body: "Slide 1\nMitosis".to_string(),
                    pages: Some(12),
                    pages_without_text: vec![4, 7],
                },
            })
            .unwrap(),
            json!({
                "status": "read",
                "fileName": "Lecture 3.pdf",
                "kind": "powerPoint",
                "title": "Lecture 3",
                "body": "Slide 1\nMitosis",
                "pages": 12,
                "pagesWithoutText": [4, 7]
            })
        );
        assert_eq!(
            serde_json::to_value(FileImport::Cancelled).unwrap(),
            json!({ "status": "cancelled" })
        );
    }

    #[test]
    fn files_that_cannot_be_imported_are_reported_without_detail() {
        let detail = || -> crate::db::DbError { "C:\\Users\\someone\\secret.pdf: bad".into() };
        for (err, kind) in [
            (ExtractError::Unsupported, "invalid"),
            (ExtractError::TooLarge, "invalid"),
            (ExtractError::Unreadable(detail()), "failed"),
            (ExtractError::Damaged(detail()), "invalid"),
            (ExtractError::NoText, "invalid"),
            (ExtractError::HelperFailed(detail()), "failed"),
        ] {
            let value = serde_json::to_value(extract_error(err)).unwrap();
            assert_eq!(value["kind"], kind, "{value}");
            let message = value["message"].as_str().unwrap();
            assert!(
                !message.contains("secret") && !message.contains('\\'),
                "{message}"
            );
            assert!(value.get("field").is_none());
        }
        let too_large = serde_json::to_value(extract_error(ExtractError::TooLarge)).unwrap();
        assert!(too_large["message"]
            .as_str()
            .unwrap()
            .contains(&format!("{} MB", extract::IMPORT_MAX_BYTES / 1024 / 1024)));
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

    #[test]
    fn optimizations_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(OptimizeOutcome::Optimized {
                token: 3,
                reviews_used: 600,
                current: vec![0.5],
                proposed: vec![1.5],
                current_error: 0.25,
                proposed_error: 0.125,
                better: true,
                intervals: vec![IntervalChange {
                    case: optimizer::IntervalCase::SecondGood,
                    current: 2.0,
                    proposed: 4.0,
                }],
            })
            .unwrap(),
            json!({
                "status": "optimized",
                "token": 3,
                "reviewsUsed": 600,
                "current": [0.5],
                "proposed": [1.5],
                "currentError": 0.25,
                "proposedError": 0.125,
                "better": true,
                "intervals": [{ "case": "secondGood", "current": 2.0, "proposed": 4.0 }]
            })
        );
        assert_eq!(
            serde_json::to_value(OptimizeOutcome::Stopped).unwrap(),
            json!({ "status": "stopped" })
        );
        assert_eq!(
            serde_json::to_value(Settings {
                version: 2,
                personal: true,
                parameters: vec![1.5],
                defaults: vec![0.5],
                reviews_used: Some(600),
                applied_at: Some("2026-10-19T09:00:00.000Z".to_string()),
                usable_reviews: 650,
                minimum_reviews: 400,
            })
            .unwrap(),
            json!({
                "version": 2,
                "personal": true,
                "parameters": [1.5],
                "defaults": [0.5],
                "reviewsUsed": 600,
                "appliedAt": "2026-10-19T09:00:00.000Z",
                "usableReviews": 650,
                "minimumReviews": 400
            })
        );
    }

    #[test]
    fn optimizer_errors_tell_react_what_to_do() {
        for (err, kind) in [
            (OptimizerError::NotEnoughReviews, "invalid"),
            (OptimizerError::NotBetter, "invalid"),
            (OptimizerError::Stale, "stale"),
            (OptimizerError::TimedOut, "failed"),
            (OptimizerError::Internal("detail".into()), "failed"),
        ] {
            let value = serde_json::to_value(optimizer_error(err, "testing", "Failed.")).unwrap();
            assert_eq!(value["kind"], kind, "{value}");
            assert!(!value["message"].as_str().unwrap().contains("detail"));
        }
    }
}
