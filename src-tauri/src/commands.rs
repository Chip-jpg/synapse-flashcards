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
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use tauri_plugin_dialog::DialogExt;

use crate::aid::{self, CardHelp, HelpKind, Helping};
use crate::anki::{self, AnkiError, AnkiImported, AnkiImports, PicturesLeftOut, Skipped};
use crate::assets::{self, Asset, AssetError, AssetStore};
use crate::authoring::{self, ArchivedDeck, AuthoringError, CardText, DeckDetail, InvalidInput};
use crate::cards_csv::{self, CardsImported, CsvCard, CsvError, Delimiter, Duplicate, RowProblem};
use crate::cloze::{self, ClozeChanges, RestoreBlock};
use crate::db::Database;
use crate::export::{self, ExportError};
use crate::extract::{self, ExtractError, ExtractedText, FileKind};
use crate::import::{self, ImportError, ImportState, RestoreError};
use crate::notes::{self, DeletedNote, DeletedNoteText, InvalidNote, Note, NoteError, NoteSummary};
use crate::occlusion::{self, Mode, OcclusionBox, OcclusionView};
use crate::ocr::{self, PageProblem};
use crate::optimizer::{self, IntervalChange, Optimized, OptimizerError, Optimizing, Settings};
use crate::pictures::{CardPictureIds, PictureProblem};
use crate::quiz::{self, QuizQuestion, Quizzing};
use crate::recognition::{
    self, PageProgress, Recognising, Recognition, RecognitionError, SourceRecognition,
};
use crate::retention::{self, RetentionError};
use crate::scheduler::Rating;
use crate::search::{self, Scope, SearchResults};
use crate::sources::{self, InvalidTitle, Source, SourceError, SourcePage};
use crate::stats::{self, Statistics};
use crate::study::{self, DeckSummary, SessionCard, StartedSession, StudyError, TodaySummary};
use crate::suggest::{self, NotePart, Ollama, SuggestError, SuggestedCard, Suggesting};
use crate::tags::{self, MergeError, MergedTags, TagError, TagProblem, TagSummary};
use crate::vision::{self, ModelSourceReading, VisionError};

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
    /// A cloze card's text.
    Text,
    /// A note's pictures.
    Pictures,
    /// The pictures on a card's front, or on its back.
    FrontPictures,
    BackPictures,
    /// An image occlusion's header.
    Header,
    /// An image occlusion's boxes.
    Boxes,
    /// A card's or note's tags, or a tag's new name.
    Tags,
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
            InvalidInput::ClozeBlank => (Field::Text, "Write the cloze text."),
            InvalidInput::ClozeTooLong => {
                (Field::Text, "A cloze text can be at most 2,000 characters.")
            }
            InvalidInput::ClozeNoDeletion => (
                Field::Text,
                "Hide at least one part of the text, like {{c1::answer}}.",
            ),
            InvalidInput::ClozeUnclosed => (
                Field::Text,
                "Close each {{c1::…}} with }} before the next one starts.",
            ),
            InvalidInput::ClozeEmpty => (
                Field::Text,
                "Each {{c1::…}} needs an answer, and a hint if it has :: after the answer.",
            ),
            InvalidInput::ClozeNumber => (Field::Text, "Number each {{c1::…}} from 1 to 999."),
            InvalidInput::FrontPictures(problem) => {
                (Field::FrontPictures, picture_message(problem, false))
            }
            InvalidInput::BackPictures(problem) => {
                (Field::BackPictures, picture_message(problem, false))
            }
            InvalidInput::OcclusionHeaderTooLong => {
                (Field::Header, "The header can be at most 500 characters.")
            }
            InvalidInput::OcclusionNoBoxes => {
                (Field::Boxes, "Draw at least one box over the picture.")
            }
            InvalidInput::OcclusionTooManyBoxes => {
                (Field::Boxes, "A picture can have at most 200 boxes.")
            }
            InvalidInput::OcclusionBox => (
                Field::Boxes,
                "Each box must be inside the picture and numbered from 1 to 999.",
            ),
            InvalidInput::OcclusionLabelTooLong => {
                (Field::Boxes, "A box's label can be at most 200 characters.")
            }
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
            InvalidNote::Pictures(problem) => (Field::Pictures, picture_message(problem, true)),
            InvalidNote::Tags(problem) => return Self::invalid_tags(problem),
        };
        CommandError {
            kind: ErrorKind::Invalid,
            message,
            field: Some(field),
            index: None,
        }
    }
}

impl CommandError {
    /// A refused tag name or list. The limits must match `tags.rs` (checked by
    /// a test below).
    fn invalid_tags(problem: TagProblem) -> Self {
        let message = match problem {
            TagProblem::Blank => "A tag can't be empty.",
            TagProblem::TooLong => "A tag can be at most 60 characters.",
            TagProblem::BadCharacter => {
                "A tag is one word: no spaces or commas. Use - or :: to join words."
            }
            TagProblem::TooMany => "A card or note can have at most 20 tags.",
            TagProblem::NameTaken => "Another tag already has that name.",
        };
        CommandError {
            kind: ErrorKind::Invalid,
            message,
            field: Some(Field::Tags),
            index: None,
        }
    }
}

/// What to say about a list of pictures that was refused. The limits must
/// match `pictures.rs` (checked by a test below).
fn picture_message(problem: PictureProblem, on_note: bool) -> &'static str {
    match (problem, on_note) {
        (PictureProblem::TooMany, true) => "A note can show at most 20 pictures.",
        (PictureProblem::TooMany, false) => "Each side of a card can show at most 4 pictures.",
        (PictureProblem::NotAPicture, _) => {
            "One of the pictures is no longer available. Add it again."
        }
        (PictureProblem::Repeated, _) => "The same picture was added twice.",
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
        // The edit form always matches the card's kind, which never changes.
        AuthoringError::WrongKind => {
            CommandError::invalid("This card is a different kind: reload and edit it again.")
        }
        // The *Deleted cards* list shows these as not restorable; one arrives
        // here only from a screen that is out of date.
        AuthoringError::ClozeNotRestorable(RestoreBlock::NumberGone) => CommandError::stale(
            "This card's number is no longer in its cloze text, so it can't be restored.",
        ),
        AuthoringError::ClozeNotRestorable(RestoreBlock::Elsewhere) => CommandError::stale(
            "This card's cloze text has its cards in another deck now, so it can't be restored here.",
        ),
        AuthoringError::ClozeSplit => CommandError::invalid(
            "Cards made from one cloze text move together: select all of them.",
        ),
        AuthoringError::OcclusionNotRestorable(RestoreBlock::NumberGone) => CommandError::stale(
            "This card's box is no longer on its picture, so it can't be restored.",
        ),
        AuthoringError::OcclusionNotRestorable(RestoreBlock::Elsewhere) => CommandError::stale(
            "This card's picture has its cards in another deck now, so it can't be restored here.",
        ),
        AuthoringError::OcclusionSplit => CommandError::invalid(
            "Cards made from one picture move together: select all of them.",
        ),
        AuthoringError::Tags(problem) => CommandError::invalid_tags(problem),
        AuthoringError::NoTagChange => CommandError::invalid("Choose a tag to add or remove."),
        AuthoringError::PictureMissing => {
            CommandError::invalid("That picture isn't kept by Synapse. Choose it again.")
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

const TAG_NOT_FOUND: &str = "That tag no longer exists. Go back to the Study Desk.";

/// Every tag on an active card of an active deck, with how many of its cards
/// are due now and in how many decks, for reviewing a tag across decks.
///
/// Returns `[{ id, name, dueCount, decks }]`, sorted by name.
#[tauri::command]
pub async fn get_tags_due(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<study::TagDue>, CommandError> {
    let pool = pool(&app, &database).await?;
    study::tags_due(&pool, Utc::now()).await.map_err(|err| {
        eprintln!("[synapse] loading the tags to review failed: {err}");
        CommandError::failed("Synapse couldn't load your tags.")
    })
}

/// The next card due with a tag, from any active deck, as `get_session_card`
/// sends one (plus `deck`, the card's deck name), or `{ status: "completed",
/// cardsReviewed }`. A tag review stores no session: `reviewed` is how many
/// cards the screen has rated so far, sent back as the progress.
///
/// Arguments: `{ tagId, reviewed }`. A missing tag rejects with "invalid".
#[tauri::command]
pub async fn get_tag_card(
    app: AppHandle,
    database: State<'_, Database>,
    tag_id: i64,
    reviewed: i64,
) -> Result<SessionCard, CommandError> {
    if tag_id < 1 {
        return Err(CommandError::invalid(TAG_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    study::tag_card(&pool, tag_id, reviewed, Utc::now())
        .await
        .map_err(|err| {
            study_error(
                err,
                "loading a tag's card",
                TAG_NOT_FOUND,
                "Synapse couldn't load a flashcard.",
            )
        })
}

/// Saves a rating for the card shown in a review of a tag, as `review_card`
/// does in a deck's session.
///
/// Arguments: `{ tagId, cardId, expectedReps, rating: 1 | 2 | 3 | 4 }`.
/// Returns `null`; a card no longer due with the tag rejects with "stale".
#[tauri::command]
pub async fn review_tag_card(
    app: AppHandle,
    database: State<'_, Database>,
    tag_id: i64,
    card_id: i64,
    expected_reps: i64,
    rating: i64,
) -> Result<(), CommandError> {
    let Some(rating) = Rating::from_number(rating) else {
        eprintln!("[synapse] rejected review with invalid rating {rating}");
        return Err(CommandError::invalid("That rating isn't valid."));
    };
    if tag_id < 1 || card_id < 1 || expected_reps < 0 {
        eprintln!(
            "[synapse] rejected tag review with invalid ids: tag {tag_id}, \
             card {card_id}, expected reps {expected_reps}"
        );
        return Err(CommandError::invalid("That review request isn't valid."));
    }
    let pool = pool(&app, &database).await?;
    study::record_tag_review(&pool, tag_id, card_id, expected_reps, rating, Utc::now())
        .await
        .map_err(|err| {
            study_error(
                err,
                "saving a tag review",
                TAG_NOT_FOUND,
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
    pictures: Option<CardPictureIds>,
    tags: Option<Vec<String>>,
) -> Result<DeckDetail, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    let pictures = pictures.unwrap_or_default();
    authoring::create_flashcard_tagged(
        &pool,
        deck_id,
        &front,
        &back,
        &pictures,
        tags.as_deref(),
        Utc::now(),
    )
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
    pictures: Option<CardPictureIds>,
    tags: Option<Vec<String>>,
) -> Result<DeckDetail, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::update_flashcard_tagged(
        &pool,
        card_id,
        &front,
        &back,
        pictures.as_ref(),
        tags.as_deref(),
        Utc::now(),
    )
    .await
    .map_err(|err| authoring_error(err, "editing a card", "Synapse couldn't save the card."))
}

/// Adds a cloze text to a normal deck: one new card, due immediately, for
/// each distinct `{{cN::…}}` number in it (see `cloze.rs`).
///
/// Arguments: `{ deckId, text, tags? }`, `tags` given to every card. Returns
/// the updated deck (same shape as `get_deck_detail`). A refused text rejects
/// with kind "invalid" and field "text"; refused tags, field "tags".
#[tauri::command]
pub async fn create_cloze(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    text: String,
    tags: Option<Vec<String>>,
) -> Result<DeckDetail, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    cloze::create_cloze_tagged(&pool, deck_id, &text, tags.as_deref(), Utc::now())
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "adding cloze cards",
                "Synapse couldn't save the cards.",
            )
        })
}

/// Replaces the text of a cloze card's cloze: numbers still in it keep their
/// cards and schedules, a number that left it has its card deleted, and a new
/// number gets a card (its deleted one back, if it had one in this deck).
///
/// Arguments: `{ cardId, text, tags? }`, `cardId` being any active card of
/// the text; `tags`, when given, replaces every active card's tags (absent
/// keeps them; a card the edit adds gets the text's tags either way). Returns `{ changes, deck }`: what happened (`{ cards, kept, restored,
/// added, deleted }`) and the card's deck.
#[tauri::command]
pub async fn update_cloze(
    app: AppHandle,
    database: State<'_, Database>,
    card_id: i64,
    text: String,
    tags: Option<Vec<String>>,
) -> Result<ClozeSaved, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    let (changes, deck) =
        cloze::update_cloze_tagged(&pool, card_id, &text, tags.as_deref(), Utc::now())
            .await
            .map_err(|err| {
                authoring_error(
                    err,
                    "editing cloze cards",
                    "Synapse couldn't save the cards.",
                )
            })?;
    Ok(ClozeSaved { changes, deck })
}

/// What `update_cloze` did: `{ changes, deck }`.
#[derive(Debug, serde::Serialize)]
pub struct ClozeSaved {
    changes: ClozeChanges,
    deck: DeckDetail,
}

/// What saving a cloze text would do, without saving it, for the form's live
/// count: `{ cards, kept, restored, added, deleted }`.
///
/// Arguments: `{ text, cardId }`, `cardId` being `null` for a new text, or a
/// card of the text being edited. A text that can't be saved rejects as
/// saving it would (kind "invalid", field "text").
#[tauri::command]
pub async fn preview_cloze(
    app: AppHandle,
    database: State<'_, Database>,
    text: String,
    card_id: Option<i64>,
) -> Result<ClozeChanges, CommandError> {
    if card_id.is_some_and(|id| id < 1) {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    cloze::preview_cloze(&pool, &text, card_id)
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "checking a cloze text",
                "Synapse couldn't check the text.",
            )
        })
}

/// Adds an image occlusion to a normal deck: a kept picture with boxes drawn
/// over it, and one new card, due immediately, for each box number (see
/// `occlusion.rs`).
///
/// Arguments: `{ deckId, assetId, header, mode, boxes, tags? }`, `mode` being
/// `"hideAll"` or `"hideOne"` and each box `{ number, left, top, width,
/// height, label }` with coordinates from 0 to 1. Returns the updated deck
/// (same shape as `get_deck_detail`). A refused box rejects with kind
/// "invalid" and field "boxes"; a refused header, field "header"; refused
/// tags, field "tags". `tags` are given to every card.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_occlusion(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    asset_id: i64,
    header: String,
    mode: Mode,
    boxes: Vec<OcclusionBox>,
    tags: Option<Vec<String>>,
) -> Result<DeckDetail, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    occlusion::create_occlusion_tagged(
        &pool,
        deck_id,
        asset_id,
        &header,
        mode,
        &boxes,
        tags.as_deref(),
        Utc::now(),
    )
    .await
    .map_err(|err| {
        authoring_error(
            err,
            "adding image occlusion cards",
            "Synapse couldn't save the cards.",
        )
    })
}

/// The picture and boxes an image occlusion card was made from, for its edit
/// form: `{ id, picture, header, mode, boxes, nextNumber }`.
///
/// Arguments: `{ cardId }`, any active card of the occlusion.
#[tauri::command]
pub async fn get_occlusion(
    app: AppHandle,
    database: State<'_, Database>,
    card_id: i64,
) -> Result<OcclusionView, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    occlusion::get_occlusion(&pool, card_id)
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "opening image occlusion cards",
                "Synapse couldn't open the picture's cards.",
            )
        })
}

/// Replaces an image occlusion's header, way of asking and boxes: numbers
/// still drawn keep their cards and schedules, a number whose boxes are gone
/// has its card deleted, and a new number gets a card (its deleted one back,
/// if it had one in this deck).
///
/// Arguments: `{ cardId, header, mode, boxes, tags? }`, `cardId` being any
/// active card of the occlusion, `tags` as for `update_cloze`. Returns
/// `{ changes, deck }`, as `update_cloze` does.
#[tauri::command]
pub async fn update_occlusion(
    app: AppHandle,
    database: State<'_, Database>,
    card_id: i64,
    header: String,
    mode: Mode,
    boxes: Vec<OcclusionBox>,
    tags: Option<Vec<String>>,
) -> Result<ClozeSaved, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    let (changes, deck) = occlusion::update_occlusion_tagged(
        &pool,
        card_id,
        &header,
        mode,
        &boxes,
        tags.as_deref(),
        Utc::now(),
    )
    .await
    .map_err(|err| {
        authoring_error(
            err,
            "editing image occlusion cards",
            "Synapse couldn't save the cards.",
        )
    })?;
    Ok(ClozeSaved { changes, deck })
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

/// Restores several soft-deleted cards of one deck at once, each exactly as
/// `restore_flashcard` restores one.
///
/// Arguments: `{ deckId, cardIds }`, the deleted cards the user selected and
/// confirmed. Returns `{ changed, deck }`: how many came back, and the deck
/// with them (same shape as `get_deck_detail`). Rejects with kind "stale" if
/// any of them is no longer a deleted card of the deck, can't be restored now,
/// or the deck has been archived, writing nothing either way.
#[tauri::command]
pub async fn restore_flashcards(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    card_ids: Vec<i64>,
) -> Result<authoring::ChangedCards, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::restore_flashcards(&pool, deck_id, &card_ids, Utc::now())
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "restoring selected cards",
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

/// Every tag some active card (in a deck that isn't archived) or active note
/// has, sorted by name ignoring case. Returns `[{ id, name, cards, notes }]`,
/// with how many active cards and notes have each.
#[tauri::command]
pub async fn get_tags(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<TagSummary>, CommandError> {
    let pool = pool(&app, &database).await?;
    tags::tags(&pool).await.map_err(|err| {
        eprintln!("[synapse] loading tags failed: {err}");
        CommandError::failed("Synapse couldn't load your tags.")
    })
}

/// Replaces one card's tags (any kind of card); its text and schedule are
/// untouched. Arguments: `{ cardId, tags: [name] }`. Returns the card's deck
/// (same shape as `get_deck_detail`). A refused name rejects with kind
/// "invalid" and field "tags"; a deleted card, or one whose deck was
/// archived, with kind "stale".
#[tauri::command]
pub async fn set_card_tags(
    app: AppHandle,
    database: State<'_, Database>,
    card_id: i64,
    tags: Vec<String>,
) -> Result<DeckDetail, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid(CARD_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::set_card_tags(&pool, card_id, &tags, Utc::now())
        .await
        .map_err(|err| authoring_error(err, "tagging a card", "Synapse couldn't save the tags."))
}

/// Adds tags to, and takes tags off, several cards of one deck at once,
/// keeping their other tags. Arguments: `{ deckId, cardIds, add: [name],
/// remove: [name] }`. Returns `{ changed, deck }`: how many cards' tags
/// changed, and the deck. Rejects with kind "stale" if any card is no longer
/// an active card of `deckId`, writing nothing.
#[tauri::command]
pub async fn tag_flashcards(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    card_ids: Vec<i64>,
    add: Vec<String>,
    remove: Vec<String>,
) -> Result<authoring::ChangedCards, CommandError> {
    if deck_id < 1 {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    authoring::tag_flashcards(&pool, deck_id, &card_ids, &add, &remove, Utc::now())
        .await
        .map_err(|err| {
            authoring_error(
                err,
                "tagging selected cards",
                "Synapse couldn't save the tags.",
            )
        })
}

/// Replaces a note's tags; its text and `updatedAt` are untouched.
/// Arguments: `{ noteId, tags: [name] }`. Returns the note (same shape as
/// `get_note`).
#[tauri::command]
pub async fn set_note_tags(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
    tags: Vec<String>,
) -> Result<Note, CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    notes::set_note_tags(&pool, note_id, &tags, Utc::now())
        .await
        .map_err(|err| note_error(err, "tagging a note", "Synapse couldn't save the tags."))
}

/// Renames a tag everywhere it is used. Arguments: `{ tagId, name }`. Returns
/// the tags (same shape as `get_tags`). Rejects with kind "invalid" and field
/// "tags" if the name is refused or another tag has it (tags are never
/// merged); with kind "stale" if the tag doesn't exist.
#[tauri::command]
pub async fn rename_tag(
    app: AppHandle,
    database: State<'_, Database>,
    tag_id: i64,
    name: String,
) -> Result<Vec<TagSummary>, CommandError> {
    let pool = pool(&app, &database).await?;
    tags::rename_tag(&pool, tag_id, &name)
        .await
        .map_err(|err| match err {
            TagError::Invalid(problem) => CommandError::invalid_tags(problem),
            TagError::NotFound => CommandError::stale("That tag is no longer there."),
            TagError::Internal(detail) => {
                eprintln!("[synapse] renaming a tag failed: {detail}");
                CommandError::failed("Synapse couldn't rename the tag.")
            }
        })
}

/// Merges one tag into another: every active card (in a deck that isn't
/// archived) and active note with `fromTagId` has `intoTagId` instead, in one
/// transaction, skipping those that already have it. Deleted cards and notes,
/// and cards in archived decks, keep the old tag (their tags are frozen).
/// Arguments: `{ fromTagId, intoTagId }`. Returns
/// `{ tags, cards, notes, keptCards, keptNotes }`. Rejects with kind
/// "invalid" and field "tags" for the same tag twice; with kind "stale" if
/// either tag doesn't exist or no active card or note has the old one.
#[tauri::command]
pub async fn merge_tags(
    app: AppHandle,
    database: State<'_, Database>,
    from_tag_id: i64,
    into_tag_id: i64,
) -> Result<MergedTags, CommandError> {
    let pool = pool(&app, &database).await?;
    tags::merge_tags(&pool, from_tag_id, into_tag_id)
        .await
        .map_err(|err| match err {
            MergeError::SameTag => CommandError {
                field: Some(Field::Tags),
                ..CommandError::invalid("Choose another tag to merge into.")
            },
            MergeError::NotFound => CommandError::stale("That tag is no longer there."),
            MergeError::NothingToMerge => {
                CommandError::stale("No card or note has that tag any more.")
            }
            MergeError::Internal(detail) => {
                eprintln!("[synapse] merging tags failed: {detail}");
                CommandError::failed("Synapse couldn't merge the tags.")
            }
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

/// The *Search* screen: every active card (in decks that aren't archived) and
/// every active note matching `{ query }`, each word as a prefix, ignoring
/// case and accents, best first. `{ scope }` is "all", "cards" or "notes";
/// `{ deckId }` limits cards to one active deck, with scope "cards" only;
/// `{ tag }` (a tag's name), or a word typed as `#name`, limits both to what
/// has that tag, or one below it. A tag with no words lists what has it.
/// Reads only. Returns `{ results, more }`; see `search::Hit`.
#[tauri::command]
pub async fn search_everything(
    app: AppHandle,
    database: State<'_, Database>,
    query: String,
    scope: Scope,
    deck_id: Option<i64>,
    tag: Option<String>,
) -> Result<SearchResults, CommandError> {
    if query.chars().count() > search::MAX_QUERY_CHARS {
        return Err(CommandError::invalid(
            "Search for at most 500 characters at a time.",
        ));
    }
    if deck_id.is_some() && scope != Scope::Cards {
        return Err(CommandError::invalid(
            "Only cards can be searched in one deck.",
        ));
    }
    if deck_id.is_some_and(|id| id < 1) {
        return Err(CommandError::invalid(DECK_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    search::search_tagged(&pool, &query, scope, deck_id, tag.as_deref())
        .await
        .map_err(|err| {
            study_error(
                err,
                "searching",
                DECK_NOT_FOUND,
                "Synapse couldn't search your cards and notes.",
            )
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

/// One whole deleted note, to read from the library's history. Arguments:
/// `{ noteId }`. Returns `{ id, title, body, createdAt, updatedAt, deletedAt,
/// pictures }`; rejects with kind "stale" if the note isn't deleted (it was
/// restored meanwhile). Writes nothing.
#[tauri::command]
pub async fn get_deleted_note(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
) -> Result<DeletedNoteText, CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    notes::deleted_note(&pool, note_id).await.map_err(|err| {
        note_error(
            err,
            "loading a deleted note",
            "Synapse couldn't load this deleted note.",
        )
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
    pictures: Option<Vec<i64>>,
    tags: Option<Vec<String>>,
) -> Result<Note, CommandError> {
    let pool = pool(&app, &database).await?;
    let pictures = pictures.unwrap_or_default();
    notes::create_note_tagged(&pool, &title, &body, &pictures, tags.as_deref(), Utc::now())
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
    pictures: Option<Vec<i64>>,
    tags: Option<Vec<String>>,
) -> Result<Note, CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    let pool = pool(&app, &database).await?;
    notes::update_note_tagged(
        &pool,
        note_id,
        &title,
        &body,
        pictures.as_deref(),
        tags.as_deref(),
        Utc::now(),
    )
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

/// Turns failed card help into a user-safe error, logging internal detail.
/// Neither the card nor the model's answer is ever logged.
fn help_error(err: SuggestError) -> CommandError {
    match err {
        SuggestError::NoteTooLong | SuggestError::NoteOverflowed => CommandError::failed(
            "This card is too long for the model to read all at once, so no help was written.",
        ),
        SuggestError::Unusable(detail) => {
            eprintln!("[synapse] the model's help was unusable: {detail}");
            CommandError::failed(
                "The model's answer couldn't be used as help. Try again, or choose another model.",
            )
        }
        SuggestError::Ungrounded => CommandError::failed(
            "The model's answer couldn't be used as help. Try again, or choose another model.",
        ),
        SuggestError::Internal(detail) => {
            eprintln!("[synapse] setting up a request to Ollama failed: {detail}");
            CommandError::failed("Synapse couldn't ask Ollama for help.")
        }
        other => suggest_error(other),
    }
}

/// Turns failed help with a picture card into a user-safe error: as
/// [`help_error`], but the model must be one that can see pictures, and may
/// take as long as reading a page.
fn picture_help_error(err: SuggestError) -> CommandError {
    match err {
        // The model list on screen is out of date, so React reloads it.
        SuggestError::ModelMissing => CommandError::stale(
            "That model isn't in Ollama any more, or can't see pictures. Choose another.",
        ),
        SuggestError::NoteTooLong | SuggestError::NoteOverflowed => CommandError::failed(
            "This card's picture and labels were too much for the model to read at once, so no \
             help was written. Try another model.",
        ),
        SuggestError::TimedOut => CommandError::failed(
            "The model took over 10 minutes on this card's picture, so Synapse stopped waiting. \
             Try a smaller model.",
        ),
        other => help_error(other),
    }
}

/// A picture card's picture with the part it asks about framed, as a PNG for
/// a vision model, prepared by the recognition helper (`vision::prepare`), so
/// pictures are never decoded in the app itself. Stopping help stops it.
async fn picture_for_help(
    app: &AppHandle,
    card: &aid::PictureCard,
    stop: tokio_util::sync::CancellationToken,
) -> Result<Vec<u8>, CommandError> {
    const FAILED: &str = "Synapse couldn't prepare this card's picture for the model.";
    let store = asset_store(app, FAILED)?;
    let helper = recognition_helper()?;
    let bytes = recognition::kept_bytes(&store, &card.picture).map_err(|err| {
        eprintln!("[synapse] reading a picture card's picture for help failed: {err:?}");
        CommandError::failed(FAILED)
    })?;
    let job = ocr::Job::MarkedPictureForModel {
        marks: card.marks.clone(),
    };
    match vision::prepare(helper, job, bytes, stop).await {
        Ok(ocr::PageOutcome::Picture(png)) => Ok(png),
        Ok(other) => {
            eprintln!("[synapse] preparing a picture card's picture for help: {other:?}");
            Err(CommandError::failed(FAILED))
        }
        Err(err) => {
            eprintln!("[synapse] preparing a picture card's picture for help failed: {err:?}");
            Err(CommandError::failed(FAILED))
        }
    }
}

/// What asking for help with a card did: `{ "status": "written", "help":
/// { kind, text, unfamiliar } }` or `{ "status": "stopped" }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HelpOutcome {
    /// A draft for the user to read: nothing has been saved.
    Written { help: CardHelp },
    /// `stop_card_help` was called first.
    Stopped,
}

/// Asks a local model in Ollama for help with a card: a mnemonic, a simpler
/// explanation, an analogy, or the question asked another way, written from
/// the card's front and back only. For a picture card (image occlusion), a
/// vision model is given its picture, with the boxes it asks about framed,
/// its header and those boxes' labels. Only Ollama on this computer is asked,
/// and nothing is saved; the user may keep the help with `create_note`.
/// Asking again, or `stop_card_help`, stops this one. Card suggestions are
/// separate.
///
/// Arguments: `{ cardId, model, kind: "mnemonic" | "explanation" | "analogy" |
/// "question" }`, where `model` is a name from `get_models`, or for a picture
/// card from `get_vision_models`. Returns
/// [`HelpOutcome`]; rejects with kind "stale" if the card was deleted or its
/// deck archived, or the model is no longer in Ollama.
#[tauri::command]
pub async fn get_card_help(
    app: AppHandle,
    database: State<'_, Database>,
    helping: State<'_, Helping>,
    card_id: i64,
    model: String,
    kind: HelpKind,
) -> Result<HelpOutcome, CommandError> {
    if card_id < 1 {
        return Err(CommandError::invalid("That card doesn't exist."));
    }
    let request = helping.0.begin();
    let work = async {
        let pool = pool(&app, &database).await?;
        let mut conn = pool.acquire().await.map_err(|err| {
            eprintln!("[synapse] loading a card for help failed: {err}");
            CommandError::failed("Synapse couldn't load this card.")
        })?;
        let (front, back) = aid::card_text(&mut conn, card_id)
            .await
            .map_err(|err| {
                eprintln!("[synapse] loading a card for help failed: {err}");
                CommandError::failed("Synapse couldn't load this card.")
            })?
            .ok_or_else(|| CommandError::stale("This card isn't available any more."))?;
        // A picture card's answer is in its picture: a vision model reads it.
        let picture = occlusion::session_view(&mut conn, card_id)
            .await
            .map_err(|err| {
                eprintln!("[synapse] loading a card for help failed: {err}");
                CommandError::failed("Synapse couldn't load this card.")
            })?;
        drop(conn);
        let ollama = Ollama::local().map_err(help_error)?;
        match picture {
            None => aid::help(&ollama, &model, kind, &front, &back)
                .await
                .map_err(help_error),
            Some(occlusion) => {
                let card = aid::PictureCard::from_review(occlusion);
                let png = picture_for_help(&app, &card, request.token()).await?;
                aid::picture_help(&ollama, &model, kind, &card, &png, vision::READ_TIMEOUT)
                    .await
                    .map_err(picture_help_error)
            }
        }
    };
    match request.run(work).await {
        None => Ok(HelpOutcome::Stopped),
        Some(result) => result.map(|help| HelpOutcome::Written { help }),
    }
}

/// Stops the card help being written, if any; its `get_card_help` call then
/// returns `{ status: "stopped" }`. Takes no arguments; returns `null`.
#[tauri::command]
pub fn stop_card_help(helping: State<'_, Helping>) {
    helping.0.stop();
}

/// Turns a failed quiz into a user-safe error, logging internal detail.
/// Neither the note nor the model's answer is ever logged.
fn quiz_error(err: SuggestError) -> CommandError {
    match err {
        SuggestError::NoteTooLong => CommandError::invalid(
            "This is too long to write a quiz from: up to 12,000 characters of a note can be \
             used at a time.",
        ),
        SuggestError::NoteOverflowed => CommandError::failed(
            "This note was too long for the model to read all at once, so no quiz was written. \
             Try a shorter note.",
        ),
        SuggestError::Unusable(detail) => {
            eprintln!("[synapse] the model's quiz was unusable: {detail}");
            CommandError::failed(
                "The model's answer didn't include any usable questions. Try again, or choose \
                 another model.",
            )
        }
        SuggestError::Ungrounded => CommandError::failed(
            "None of the questions the model wrote could be traced back to your note, so none \
             are shown. Try again, or choose a larger model.",
        ),
        SuggestError::Internal(detail) => {
            eprintln!("[synapse] setting up a request to Ollama failed: {detail}");
            CommandError::failed("Synapse couldn't ask Ollama for a quiz.")
        }
        other => suggest_error(other),
    }
}

/// What asking for a quiz did: `{ "status": "written", "questions": [{
/// question, answer, distractors, source, unfamiliar }], "leftOut": 1,
/// "unsupported": 2 }` or `{ "status": "stopped" }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum QuizOutcome {
    Written {
        /// The questions, to take on screen: nothing has been saved.
        questions: Vec<QuizQuestion>,
        /// Questions left out as unusable, so the user can be told.
        left_out: usize,
        /// Questions left out as not coming from the note.
        unsupported: usize,
    },
    /// `stop_quiz` was called first.
    Stopped,
}

/// Asks a local model in Ollama for a multiple-choice quiz on a note: up to
/// eight questions, each with one correct answer and three wrong ones. Only
/// Ollama on this computer is asked, and nothing is saved; the user may add
/// questions they missed as cards with `create_flashcards`. Asking again, or
/// `stop_quiz`, stops this one. Card suggestions and card help are separate.
///
/// Arguments: `{ noteId, model, part? }`, as for `suggest_cards`. Returns
/// [`QuizOutcome`]; rejects with kind "stale" if the note was deleted, no
/// longer has that part, or the model is no longer in Ollama.
#[tauri::command]
pub async fn quiz_note(
    app: AppHandle,
    database: State<'_, Database>,
    quizzing: State<'_, Quizzing>,
    note_id: i64,
    model: String,
    part: Option<NotePart>,
) -> Result<QuizOutcome, CommandError> {
    if note_id < 1 {
        return Err(CommandError::invalid(NOTE_NOT_FOUND));
    }
    // Tracked from the start, so a stop while the note loads counts too.
    let request = quizzing.0.begin();
    let work = async {
        let pool = pool(&app, &database).await?;
        let note = notes::note(&pool, note_id).await.map_err(|err| {
            note_error(
                err,
                "loading a note to write a quiz from",
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
        let ollama = Ollama::local().map_err(quiz_error)?;
        quiz::quiz(&ollama, &model, &note.title, body)
            .await
            .map_err(quiz_error)
    };
    match request.run(work).await {
        None => Ok(QuizOutcome::Stopped),
        Some(result) => result.map(|written| QuizOutcome::Written {
            questions: written.questions,
            left_out: written.left_out,
            unsupported: written.unsupported,
        }),
    }
}

/// Stops the quiz being written, if any; its `quiz_note` call then returns
/// `{ status: "stopped" }`. Takes no arguments; returns `null`.
#[tauri::command]
pub fn stop_quiz(quizzing: State<'_, Quizzing>) {
    quizzing.0.stop();
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
            "That file has no text Synapse can import. If it's a scanned PDF, add it as a \
             source, where Synapse can read the text on its pages.",
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
    let store = Database::asset_store(&app).map_err(|err| {
        eprintln!("[synapse] locating the kept files failed: {err}");
        CommandError::failed("Synapse couldn't build the export.")
    })?;
    export::write_package(
        &pool,
        &store,
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
        ExportError::MissingAsset(detail) => {
            eprintln!("[synapse] a kept file couldn't be exported: {detail}");
            CommandError::failed(
                "A file Synapse keeps (a source or a picture) is missing or damaged, so the \
                 backup wouldn't be complete. Nothing was saved.",
            )
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
        /// How many files (sources, pictures) it keeps; 0 for a backup made
        /// before Synapse kept files.
        kept_files: usize,
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
    let kept_files = staged.assets.len();
    let token = pending.hold(staged.database);
    let file_name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "your backup".to_string());
    Ok(ImportCheck::Ready {
        token,
        file_name,
        exported_at: staged.manifest.exported_at,
        kept_files,
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
        let store = Database::asset_store(&app).map_err(|err| {
            eprintln!("[synapse] locating the kept files for a restore failed: {err}");
            CommandError::failed(IMPORT_NOT_READY)
        })?;
        import::replace_database(&database, &db_path, &store, &staged, &workspace)
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

/// Turns a refused or failed desired-retention change into a user-safe
/// error, logging internal detail.
fn retention_error(err: RetentionError, context: &str, failed: &'static str) -> CommandError {
    match err {
        RetentionError::OutOfRange => {
            CommandError::invalid("Choose a desired retention from 70% to 97%.")
        }
        RetentionError::NotFound => {
            CommandError::stale("That deck no longer exists. Synapse has reloaded the settings.")
        }
        RetentionError::SampleDeck => {
            CommandError::invalid("The sample deck follows the global desired retention.")
        }
        RetentionError::ArchivedDeck => CommandError::stale(
            "That deck is archived, so it follows the global desired retention. Synapse has \
             reloaded the settings.",
        ),
        RetentionError::Unchanged => {
            CommandError::invalid("That's already the desired retention in use.")
        }
        RetentionError::Stale => CommandError::stale(
            "The desired retention changed since this was shown. Synapse has reloaded it.",
        ),
        RetentionError::Internal(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::failed(failed)
        }
    }
}

/// The desired retention settings. Returns [`retention::Settings`].
#[tauri::command]
pub async fn get_retention(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<retention::Settings, CommandError> {
    let pool = pool(&app, &database).await?;
    retention::settings(&pool).await.map_err(|err| {
        eprintln!("[synapse] loading desired retention failed: {err}");
        CommandError::failed("Synapse couldn't load the desired retention.")
    })
}

/// Estimates the daily reviews with a change, against the values in use.
/// `{ version, deckId, percent }`: `deckId` `null` for the global value;
/// `percent` a whole percentage, or `null` for a deck to follow the global
/// value. Writes nothing. Returns [`retention::Preview`]; rejects as
/// `set_retention` would.
#[tauri::command]
pub async fn preview_retention(
    app: AppHandle,
    database: State<'_, Database>,
    version: i64,
    deck_id: Option<i64>,
    percent: Option<i64>,
) -> Result<retention::Preview, CommandError> {
    let pool = pool(&app, &database).await?;
    let change = retention::Change { deck_id, percent };
    retention::preview(&pool, version, change, Utc::now())
        .await
        .map_err(|err| {
            retention_error(
                err,
                "estimating reviews for a desired retention",
                "Synapse couldn't estimate your reviews. Nothing was changed.",
            )
        })
}

/// Makes a desired-retention change (arguments as `preview_retention`) for
/// every rating from now on. Stored schedules don't change. Returns the new
/// [`retention::Settings`]; rejects with kind "stale" if the settings
/// changed since `version`.
#[tauri::command]
pub async fn set_retention(
    app: AppHandle,
    database: State<'_, Database>,
    version: i64,
    deck_id: Option<i64>,
    percent: Option<i64>,
) -> Result<retention::Settings, CommandError> {
    let pool = pool(&app, &database).await?;
    let change = retention::Change { deck_id, percent };
    let failed = "Synapse couldn't change the desired retention. Nothing was changed.";
    retention::set(&pool, version, change, Utc::now())
        .await
        .map_err(|err| retention_error(err, "changing the desired retention", failed))?;
    get_retention(app, database).await
}

/// One row of a cards file, checked: `{ line, front, back, problem, duplicate, clozeCards, tags }`.
/// `problem` is `{ field, message }` when the row can't become a card, and
/// `duplicate` says why its card may already be there (see `cards_csv.rs`).
#[derive(Debug, serde::Serialize)]
pub struct CardsFileRow {
    line: usize,
    front: String,
    back: String,
    problem: Option<RowProblemMessage>,
    duplicate: Option<Duplicate>,
    /// For a cloze row (a cloze text in the front and nothing in the back):
    /// how many cards it makes. `null` for a front/back row.
    #[serde(rename = "clozeCards")]
    cloze_cards: Option<usize>,
    /// The row's tags, checked; empty without a tags column.
    tags: Vec<String>,
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
                // A cloze text is in the front column.
                let field = match error.field {
                    None | Some(Field::Text) => Field::Front,
                    Some(field) => field,
                };
                (field, error.message)
            }
            RowProblem::Tags(problem) => (Field::Tags, CommandError::invalid_tags(problem).message),
        };
        RowProblemMessage { field, message }
    }
}

/// What choosing a cards file did:
/// `{ "status": "read", "fileName", "delimiter", "header", "rows", "tagsColumn", "rowsWithExtraColumns" }`
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
        /// The header names a third column "Tags", which is read as tags.
        tags_column: bool,
        /// Rows with something in a column that isn't read, which is ignored.
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
    let checked = match pool.acquire().await {
        Ok(mut conn) => authoring::active_normal_deck(&mut conn, deck_id).await,
        Err(err) => Err(err.into()),
    };
    checked.map_err(|err| {
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
                cloze_cards: row.cloze_cards,
                tags: row.tags,
            })
            .collect(),
        tags_column: preview.tags_column,
        rows_with_extra_columns: preview.rows_with_extra_columns,
    })
}

/// Adds the cards the user kept from a cards file to an active normal deck,
/// each checked as `create_flashcard` checks one. All or nothing. With
/// `skipDuplicates`, cards whose front is already in the deck (or on an
/// earlier card sent) are left out, judged afresh at saving.
///
/// Arguments: `{ deckId, cards: [{ front, back, tags? }], skipDuplicates }`. Returns
/// `{ added, skipped, cardCount }`. A refused card rejects with kind
/// "invalid", its `field`, and its `index` in `cards`; nothing new to add
/// rejects with kind "stale".
#[tauri::command]
pub async fn import_cards_csv(
    app: AppHandle,
    database: State<'_, Database>,
    deck_id: i64,
    cards: Vec<CsvCard>,
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

/// Saves an active deck's cards (front, back and tags) as a CSV file the user
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

/// One Anki deck of a package, as the preview shows it: its name in Anki,
/// the name its new deck would get, the cards it would add one way, how many
/// of them have a reversed card too, and its first few cards.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnkiDeckPreview {
    anki_name: String,
    name: String,
    cards: usize,
    reversible: usize,
    examples: Vec<CardText>,
}

/// What choosing an Anki package did:
/// `{ "status": "read", token, fileName, decks, notes, skipped, mediaRemoved, extraRemoved, reviewed,
/// occlusions, occlusionApproximated, cardPictures, picturesLeftOut, tags, tagsLeftOut }`
/// or `{ "status": "cancelled" }`. Nothing has been saved; `token` names the
/// package to `import_anki_package` and `cancel_anki_import`.
// Built once per chosen file and sent straight on, so the size of `Read`
// costs nothing worth boxing it for.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AnkiFile {
    Read {
        token: u64,
        file_name: String,
        decks: Vec<AnkiDeckPreview>,
        /// Every note in the collection.
        notes: usize,
        /// Notes left out, by why (see `anki.rs`).
        skipped: Skipped,
        /// Notes to add that had a sound or video, or a picture in a cloze
        /// note, which is left out.
        media_removed: usize,
        /// Cloze notes to add with text in another field, which is left out.
        extra_removed: usize,
        /// Notes to add that had been reviewed in Anki; they start as new.
        reviewed: usize,
        /// Image occlusion notes to add with their pictures.
        occlusions: usize,
        /// Of those, notes whose round or many-sided shapes became
        /// rectangles, or whose text labels were left out.
        occlusion_approximated: usize,
        /// Pictures that come with front/back cards, one for each side that
        /// shows it (reversed cards not counted).
        card_pictures: usize,
        /// Pictures of front/back cards left out, by why.
        pictures_left_out: PicturesLeftOut,
        /// How many different tags the cards get from their notes.
        tags: usize,
        /// Tags of notes left out (over 60 characters, with a comma, or past
        /// a card's 20), one for each note that had it.
        tags_left_out: usize,
    },
    Cancelled,
}

/// Turns an Anki package that was refused into a user-safe error, logging
/// internal detail (never a card's text). The limits in these messages must
/// match `anki.rs` (checked by a test below).
fn anki_error(err: AnkiError, context: &str, failed: &'static str) -> CommandError {
    match err {
        AnkiError::TooLarge => {
            CommandError::invalid("That file is too large to import: the limit is 2 GB.")
        }
        AnkiError::CollectionTooLarge => CommandError::invalid(
            "The collection in that file is too large to import: the limit is 512 MB.",
        ),
        AnkiError::NotAnki => CommandError::invalid(
            "That file isn't an Anki package. In Anki, export your decks as an .apkg file, \
             then choose that.",
        ),
        AnkiError::Unreadable(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::invalid("Synapse couldn't read the Anki collection in that file.")
        }
        AnkiError::TooManyCards => CommandError::invalid(
            "At most 50,000 cards can be imported at once. Export fewer decks from Anki.",
        ),
        AnkiError::TooManyDecks => CommandError::invalid(
            "At most 1,000 decks can be imported at once. Export fewer decks from Anki.",
        ),
        AnkiError::NothingToImport => {
            CommandError::invalid("There are no cards to add from this file.")
        }
        AnkiError::Io(err) => {
            eprintln!("[synapse] {context} failed: {err}");
            CommandError::failed("Synapse couldn't unpack that file.")
        }
        AnkiError::Authoring(err) => authoring_error(err, context, failed),
        AnkiError::Picture(AssetError::StoreFull) => CommandError::invalid(STORE_FULL),
        AnkiError::Picture(err) => {
            eprintln!("[synapse] {context} failed keeping a picture: {err:?}");
            CommandError::failed(failed)
        }
    }
}

/// Chooses an Anki package (`.apkg` or `.colpkg`) and reads its front/back
/// notes, without saving anything: the preview goes to the screen, and the
/// package waits under a token for `import_anki_package` (only the latest
/// one is kept).
///
/// Returns [`AnkiFile`]; a file that can't be used rejects with kind
/// "invalid" or "failed".
#[tauri::command]
pub async fn read_anki_package(
    app: AppHandle,
    database: State<'_, Database>,
    imports: State<'_, AnkiImports>,
) -> Result<AnkiFile, CommandError> {
    let pool = pool(&app, &database).await?;

    // As in `read_cards_csv`: the dialog answers through a callback.
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose an Anki package to import")
        .add_filter("Anki packages (APKG, COLPKG)", &["apkg", "colpkg"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |path| {
        let _ = sender.try_send(path);
    });

    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the Anki file dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the file window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(AnkiFile::Cancelled);
    };
    let path = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen Anki file's path was unusable: {err}");
        CommandError::invalid("That file can't be used.")
    })?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "your file".to_string());

    let failed = "Synapse couldn't read that file.";
    let package = anki::read_package(&path)
        .await
        .map_err(|err| anki_error(err, "reading an Anki package", failed))?;
    let names = anki::planned_names(&pool, &package)
        .await
        .map_err(|err| anki_error(err, "naming imported decks", failed))?;
    let decks = package
        .decks
        .iter()
        .zip(names)
        .map(|(deck, name)| AnkiDeckPreview {
            anki_name: deck.anki_name.clone(),
            name,
            cards: deck.notes.iter().map(|note| note.cards).sum(),
            reversible: deck.notes.iter().filter(|note| note.reversible).count(),
            examples: deck
                .notes
                .iter()
                .take(anki::EXAMPLES_PER_DECK)
                .map(|note| CardText {
                    front: note.front.clone(),
                    back: note.back.clone(),
                })
                .collect(),
        })
        .collect();
    let (notes, skipped, media_removed, extra_removed, reviewed, occlusion_approximated) = (
        package.notes,
        package.skipped,
        package.media_removed,
        package.extra_removed,
        package.reviewed,
        package.occlusion_approximated,
    );
    let occlusions = package
        .decks
        .iter()
        .flat_map(|deck| &deck.notes)
        .filter(|note| note.occlusion.is_some())
        .count();
    let card_pictures = package
        .decks
        .iter()
        .flat_map(|deck| &deck.notes)
        .map(|note| note.pictures.count())
        .sum();
    let pictures_left_out = package.pictures_left_out;
    let tags = package
        .decks
        .iter()
        .flat_map(|deck| &deck.notes)
        .flat_map(|note| &note.tags)
        .map(|tag| tag.to_lowercase())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let tags_left_out = package.tags_left_out;
    let token = imports.hold(package);
    Ok(AnkiFile::Read {
        token,
        file_name,
        decks,
        notes,
        skipped,
        media_removed,
        extra_removed,
        reviewed,
        occlusions,
        occlusion_approximated,
        card_pictures,
        pictures_left_out,
        tags,
        tags_left_out,
    })
}

/// Adds the Anki package held under `{ token }` as new decks of new cards,
/// all or nothing; with `bothDirections`, also the reversed card of each note
/// whose Anki note type made one. Decks are named afresh at saving.
///
/// Returns `{ decks: [{ id, name, cards }], cards }`. A package no longer
/// held rejects with kind "stale".
#[tauri::command]
pub async fn import_anki_package(
    app: AppHandle,
    database: State<'_, Database>,
    imports: State<'_, AnkiImports>,
    token: u64,
    both_directions: bool,
) -> Result<AnkiImported, CommandError> {
    let Some(package) = imports.get(token) else {
        return Err(CommandError::stale(
            "This preview is out of date. Choose the file again.",
        ));
    };
    let failed = "Synapse couldn't save the cards. Nothing was added.";
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, failed)?;
    let imported = anki::import_package(&pool, &store, &package, both_directions, Utc::now())
        .await
        .map_err(|err| {
            anki_error(
                err,
                "importing an Anki package",
                "Synapse couldn't save the cards. Nothing was added.",
            )
        })?;
    imports.forget(token);
    Ok(imported)
}

/// Forgets the Anki package held under `{ token }`, if it still is.
#[tauri::command]
pub fn cancel_anki_import(imports: State<'_, AnkiImports>, token: u64) {
    imports.forget(token);
}

/// The folder of kept files, beside the database.
fn asset_store(app: &AppHandle, failed: &'static str) -> Result<AssetStore, CommandError> {
    Database::asset_store(app).map_err(|err| {
        eprintln!("[synapse] locating the kept files failed: {err}");
        CommandError::failed(failed)
    })
}

/// The answer to a `synapse:` request from the webview: a kept picture or
/// PDF by its hash (see `assets::respond`), or a plain 404.
pub async fn kept_file(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    let database = app.state::<Database>();
    match (database.pool(app).await, Database::asset_store(app)) {
        (Ok(pool), Ok(store)) => assets::respond(&pool, &store, path).await,
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("[synapse] opening the database to show a kept file failed: {err}");
            assets::not_found()
        }
    }
}

const PICTURE_NOT_KEPT: &str = "Synapse couldn't keep that picture.";

/// Turns a picture that wasn't kept into a user-safe error.
fn picture_error(err: AssetError) -> CommandError {
    match err {
        AssetError::TooLarge => CommandError::invalid("Pictures can be at most 20 MB."),
        AssetError::WrongType => CommandError::invalid(
            "Synapse can keep PNG, JPEG, GIF, and WebP pictures. That file isn't one.",
        ),
        AssetError::StoreFull => CommandError::invalid(STORE_FULL),
        AssetError::Internal(detail) => {
            eprintln!("[synapse] keeping a picture failed: {detail}");
            CommandError::failed(PICTURE_NOT_KEPT)
        }
    }
}

const STORE_FULL: &str = "Synapse already keeps 2 GB of sources and pictures, as much as one \
                          backup can hold, so it can't keep more.";

/// What choosing a picture did: `{ "status": "added", "asset": Asset }` or
/// `{ "status": "cancelled" }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PictureChoice {
    /// The picture is kept. Show it with its `sha256` (see `api.ts`'s
    /// `keptFileUrl`) and refer to it by its `id`.
    Added { asset: Asset },
    /// The user closed the file window without choosing a file.
    Cancelled,
}

/// Chooses a picture file (PNG, JPEG, GIF or WebP) and keeps it, returning
/// its `assets` row. The same picture chosen twice is kept once. Nothing
/// points at it yet: whatever the picture is for records that.
///
/// Takes no arguments. Returns [`PictureChoice`].
#[tauri::command]
pub async fn choose_picture(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<PictureChoice, CommandError> {
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, PICTURE_NOT_KEPT)?;

    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose a picture")
        .add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |path| {
        let _ = sender.try_send(path);
    });
    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the picture file dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the file window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(PictureChoice::Cancelled);
    };
    let path = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen picture's path was unusable: {err}");
        CommandError::invalid("That file can't be used.")
    })?;
    let unreadable = |err: std::io::Error| {
        eprintln!("[synapse] reading a picture failed: {err}");
        CommandError::failed("Synapse couldn't read that file.")
    };
    // The type is read from the bytes, whatever the file's name says.
    if std::fs::metadata(&path).map_err(unreadable)?.len() > assets::MAX_PICTURE_BYTES {
        return Err(picture_error(AssetError::TooLarge));
    }
    let bytes = std::fs::read(&path).map_err(unreadable)?;
    let asset = assets::add_picture(&pool, &store, &bytes, Utc::now())
        .await
        .map_err(picture_error)?;
    Ok(PictureChoice::Added { asset })
}

/// Keeps a picture sent as raw bytes (a pasted or dropped image):
/// `invoke("store_picture", bytes)` with a `Uint8Array`. Returns its
/// `assets` row; bytes that aren't a PNG, JPEG, GIF or WebP are refused.
#[tauri::command]
pub async fn store_picture(
    app: AppHandle,
    database: State<'_, Database>,
    request: tauri::ipc::Request<'_>,
) -> Result<Asset, CommandError> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(CommandError::invalid(
            "Synapse can keep PNG, JPEG, GIF, and WebP pictures. That isn't one.",
        ));
    };
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, PICTURE_NOT_KEPT)?;
    assets::add_picture(&pool, &store, bytes, Utc::now())
        .await
        .map_err(picture_error)
}

const SOURCE_NOT_FOUND: &str = "That source doesn't exist.";

/// Turns a source operation that didn't happen into a user-safe error.
fn source_error(err: SourceError, context: &str, failed: &'static str) -> CommandError {
    match err {
        SourceError::Invalid(problem) => CommandError {
            kind: ErrorKind::Invalid,
            message: match problem {
                InvalidTitle::Blank => "Give the source a title.",
                InvalidTitle::TooLong => "Titles can be at most 200 characters.",
            },
            field: Some(Field::Title),
            index: None,
        },
        SourceError::NotFound => CommandError::invalid(SOURCE_NOT_FOUND),
        SourceError::Deleted => CommandError::stale("This source or note was deleted."),
        SourceError::NotDeleted => CommandError::stale("This source isn't deleted."),
        SourceError::AlreadyLinked => {
            CommandError::stale("This note already says which source it came from.")
        }
        SourceError::Asset(AssetError::TooLarge) => {
            CommandError::invalid("That file is too large to keep: the limit is 50 MB.")
        }
        SourceError::Asset(AssetError::WrongType) => CommandError::invalid(
            "That file isn't the kind of document its name says, so Synapse can't keep it.",
        ),
        SourceError::Asset(AssetError::StoreFull) => CommandError::invalid(STORE_FULL),
        SourceError::Asset(AssetError::Internal(detail)) | SourceError::Internal(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::failed(failed)
        }
    }
}

/// What choosing a document to keep did: `{ "status": "imported", "source":
/// Source, "text": ExtractedText }` or `{ "status": "cancelled" }`. `text`
/// is what `import_file` returns, for a note written from the source.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SourceImport {
    Imported {
        source: Box<Source>,
        text: Box<ExtractedText>,
    },
    Cancelled,
}

/// Chooses a document (PDF, text, Markdown, Word or PowerPoint) and keeps it
/// as a source: the original file and each page's text. Opens the same file
/// window as `import_file`, which it extends: a note saved from the returned
/// text can be linked to the source with `link_note_to_source`.
///
/// Takes no arguments. Returns [`SourceImport`]; a file that can't be read
/// or kept rejects with kind "invalid" or "failed", and nothing is kept.
#[tauri::command]
pub async fn import_source(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<SourceImport, CommandError> {
    const NOT_KEPT: &str = "Synapse couldn't keep that file.";
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, NOT_KEPT)?;

    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose a document to keep as a source")
        .add_filter("Notes and documents", &FileKind::EXTENSIONS);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |path| {
        let _ = sender.try_send(path);
    });
    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the source file dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the file window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(SourceImport::Cancelled);
    };
    let path = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen file's path was unusable: {err}");
        CommandError::invalid("That file can't be used.")
    })?;
    let helper = std::env::current_exe().map_err(|err| {
        eprintln!("[synapse] finding Synapse's own program failed: {err}");
        CommandError::failed("Synapse couldn't read that file.")
    })?;
    let document =
        tauri::async_runtime::spawn_blocking(move || extract::read_document(&path, &helper))
            .await
            .map_err(|err| {
                eprintln!("[synapse] reading a file to keep stopped: {err}");
                CommandError::failed("Synapse couldn't read that file.")
            })?
            .map_err(extract_error)?;
    let source = sources::create_source(&pool, &store, &document, Utc::now())
        .await
        .map_err(|err| source_error(err, "keeping a source", NOT_KEPT))?;
    Ok(SourceImport::Imported {
        source: Box::new(source),
        text: Box::new(document.text),
    })
}

/// Every active source, most recently changed first. Returns `[Source]`.
#[tauri::command]
pub async fn get_sources(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<Source>, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::sources(&pool).await.map_err(|err| {
        source_error(
            err,
            "loading sources",
            "Synapse couldn't load your sources.",
        )
    })
}

/// Every deleted source, most recently deleted first. Returns `[Source]`.
#[tauri::command]
pub async fn get_deleted_sources(
    app: AppHandle,
    database: State<'_, Database>,
) -> Result<Vec<Source>, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::deleted_sources(&pool).await.map_err(|err| {
        source_error(
            err,
            "loading deleted sources",
            "Synapse couldn't load your deleted sources.",
        )
    })
}

/// One active source, by `{ sourceId }`. Returns [`Source`]; a deleted one
/// rejects with kind "stale".
#[tauri::command]
pub async fn get_source(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<Source, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::source(&pool, source_id).await.map_err(|err| {
        source_error(
            err,
            "loading a source",
            "Synapse couldn't open that source.",
        )
    })
}

/// The text of every page of an active source, by `{ sourceId }`. Returns
/// `[{ number, text }]`, numbered from 1.
#[tauri::command]
pub async fn get_source_pages(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<Vec<SourcePage>, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::source_pages(&pool, source_id)
        .await
        .map_err(|err| {
            source_error(
                err,
                "loading a source's pages",
                "Synapse couldn't open that source.",
            )
        })
}

/// Renames an active source: `{ sourceId, title }`. Returns the [`Source`].
#[tauri::command]
pub async fn rename_source(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
    title: String,
) -> Result<Source, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::rename_source(&pool, source_id, &title, Utc::now())
        .await
        .map_err(|err| source_error(err, "renaming a source", "Synapse couldn't rename it."))
}

/// Deletes a source softly, by `{ sourceId }`; its file and pages are kept.
#[tauri::command]
pub async fn delete_source(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<(), CommandError> {
    let pool = pool(&app, &database).await?;
    sources::delete_source(&pool, source_id, Utc::now())
        .await
        .map_err(|err| source_error(err, "deleting a source", "Synapse couldn't delete it."))
}

/// Restores a deleted source exactly as it was, by `{ sourceId }`.
#[tauri::command]
pub async fn restore_source(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<(), CommandError> {
    let pool = pool(&app, &database).await?;
    sources::restore_source(&pool, source_id)
        .await
        .map_err(|err| source_error(err, "restoring a source", "Synapse couldn't restore it."))
}

/// Records that a note was written from a source: `{ noteId, sourceId }`.
/// Both must be active; a note's source is set once and never changes.
#[tauri::command]
pub async fn link_note_to_source(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
    source_id: i64,
) -> Result<(), CommandError> {
    let pool = pool(&app, &database).await?;
    sources::link_note(&pool, note_id, source_id, Utc::now())
        .await
        .map_err(|err| {
            source_error(
                err,
                "linking a note to its source",
                "Synapse couldn't record where the note came from.",
            )
        })
}

/// The id of the source a note was written from, by `{ noteId }`, or null.
#[tauri::command]
pub async fn get_note_source(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
) -> Result<Option<i64>, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::note_source(&pool, note_id).await.map_err(|err| {
        source_error(
            err,
            "finding a note's source",
            "Synapse couldn't load that note's source.",
        )
    })
}

/// Saves a new note written from a source and records that it came from it,
/// together: `{ sourceId, title, body, pictures?, tags? }` (picture ids, in
/// order, and tag names, as `create_note` takes them). Returns the [`Note`]. A refused
/// title or text points at its field, as `create_note` does; a deleted
/// source rejects with kind "stale". Nothing is written either way.
#[tauri::command]
pub async fn create_note_from_source(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
    title: String,
    body: String,
    pictures: Option<Vec<i64>>,
    tags: Option<Vec<String>>,
) -> Result<Note, CommandError> {
    let pool = pool(&app, &database).await?;
    let pictures = pictures.unwrap_or_default();
    sources::create_note(
        &pool,
        source_id,
        &title,
        &body,
        &pictures,
        tags.as_deref(),
        Utc::now(),
    )
    .await
    .map_err(|err| match err {
        sources::NoteFromSourceError::Note(err) => note_error(
            err,
            "saving a note from a source",
            "Synapse couldn't save the note.",
        ),
        sources::NoteFromSourceError::Source(err) => source_error(
            err,
            "saving a note from a source",
            "Synapse couldn't save the note.",
        ),
    })
}

/// The active notes written from an active source, by `{ sourceId }`.
/// Returns `[NoteSummary]`, most recently edited first.
#[tauri::command]
pub async fn get_source_notes(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<Vec<NoteSummary>, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::source_notes(&pool, source_id)
        .await
        .map_err(|err| {
            source_error(
                err,
                "loading a source's notes",
                "Synapse couldn't load the notes written from it.",
            )
        })
}

/// Where quotes from a note are in the source it was written from:
/// `{ noteId, quotes }`. Returns [`sources::QuotedSource`], or null when the
/// note didn't come from a source or its source is deleted.
#[tauri::command]
pub async fn locate_quotes(
    app: AppHandle,
    database: State<'_, Database>,
    note_id: i64,
    quotes: Vec<String>,
) -> Result<Option<sources::QuotedSource>, CommandError> {
    if quotes.len() > sources::MAX_QUOTES {
        return Err(CommandError::invalid(
            "That's more quotes than Synapse looks for at once.",
        ));
    }
    let pool = pool(&app, &database).await?;
    sources::locate_quotes(&pool, note_id, &quotes)
        .await
        .map_err(|err| {
            source_error(
                err,
                "finding quotes in a source",
                "Synapse couldn't find where those quotes came from.",
            )
        })
}

/// One page of an active source with a quote marked on it: `{ sourceId,
/// number, quote }`. Returns `[{ text, hit }]`; an empty quote marks nothing.
#[tauri::command]
pub async fn get_quoted_page(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
    number: u32,
    quote: String,
) -> Result<Vec<search::Piece>, CommandError> {
    let pool = pool(&app, &database).await?;
    sources::quoted_page(&pool, source_id, number, &quote)
        .await
        .map_err(|err| {
            source_error(
                err,
                "loading a source's page",
                "Synapse couldn't open that page.",
            )
        })?
        .ok_or_else(|| CommandError::invalid("That source has no such page."))
}

/// The original file of an active source, checked, for a copy of it.
async fn source_original(
    app: &AppHandle,
    database: &Database,
    source_id: i64,
) -> Result<(Source, Vec<u8>), CommandError> {
    const UNREADABLE: &str = "Synapse couldn't read the kept file.";
    let pool = pool(app, database).await?;
    let store = asset_store(app, UNREADABLE)?;
    sources::original(&pool, &store, source_id)
        .await
        .map_err(|err| source_error(err, "reading a source's file", UNREADABLE))
}

/// Saves a copy of a source's original file where the user chooses, by
/// `{ sourceId }`. Opens the OS "save as" dialog; closing it returns
/// `{ status: "cancelled" }` and writes nothing. Returns [`ExportOutcome`].
#[tauri::command]
pub async fn save_source_copy(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<ExportOutcome, CommandError> {
    let (source, bytes) = source_original(&app, &database, source_id).await?;

    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save a copy of the original")
        .set_file_name(sources::copy_file_name(&source));
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.save_file(move |path| {
        let _ = sender.try_send(path);
    });
    let Some(chosen) = receiver.recv().await else {
        eprintln!("[synapse] the source copy save dialog closed without an answer");
        return Err(CommandError::failed(
            "Synapse couldn't open the save window.",
        ));
    };
    let Some(chosen) = chosen else {
        return Ok(ExportOutcome::Cancelled);
    };
    let destination = chosen.into_path().map_err(|err| {
        eprintln!("[synapse] the chosen copy path was unusable: {err}");
        CommandError::invalid("That save location can't be used.")
    })?;
    if database.is_database_file(&app, &destination) {
        return Err(CommandError::invalid(
            "That's Synapse's own data file. Choose another name.",
        ));
    }
    std::fs::write(&destination, &bytes).map_err(|err| {
        eprintln!("[synapse] writing a source copy failed: {err}");
        CommandError::failed("Synapse couldn't save the copy there. Try another folder.")
    })?;
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(ExportOutcome::Saved { file_name })
}

/// Opens a source's original file in the program the computer uses for that
/// kind of file, by `{ sourceId }`. The kept file has no name of its own, so
/// a copy under the original name goes to a Synapse folder in the system's
/// temporary folder first; the kept file itself is never handed out.
#[tauri::command]
pub async fn open_source_original(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<(), CommandError> {
    use tauri_plugin_opener::OpenerExt;
    const NOT_OPENED: &str = "Synapse couldn't open the original.";
    let (source, bytes) = source_original(&app, &database, source_id).await?;
    let folder = std::env::temp_dir()
        .join("synapse-originals")
        .join(&source.asset.sha256[..16]);
    let path = folder.join(sources::copy_file_name(&source));
    std::fs::create_dir_all(&folder)
        .and_then(|()| std::fs::write(&path, &bytes))
        .map_err(|err| {
            eprintln!("[synapse] writing a copy to open failed: {err}");
            CommandError::failed(NOT_OPENED)
        })?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|err| {
            eprintln!("[synapse] opening a source's original failed: {err}");
            CommandError::failed(
                "Synapse couldn't open the original. Save a copy instead, and open that.",
            )
        })
}

// Text recognition (`recognition.rs`, `ocr.rs`).

/// Turns a recognition that didn't happen into a user-safe error.
fn recognition_error(err: RecognitionError, context: &str) -> CommandError {
    match err {
        RecognitionError::NotFound => {
            CommandError::invalid("That picture or source doesn't exist.")
        }
        RecognitionError::Deleted => CommandError::stale("This source was deleted."),
        RecognitionError::NotReadable => CommandError::invalid(
            "Synapse can only read text in pictures and in the pages of a PDF.",
        ),
        RecognitionError::Picture(PageProblem::TooLarge) => CommandError::invalid(
            "That picture is too large to read: the limit is 80 million pixels.",
        ),
        RecognitionError::Picture(_) => {
            CommandError::invalid("Synapse couldn't read that picture. It may be damaged.")
        }
        RecognitionError::Unreadable(detail) => {
            eprintln!("[synapse] {context}: the PDF couldn't be opened: {detail}");
            CommandError::invalid(
                "Synapse couldn't open that PDF to read its pages. It may be damaged or \
                 password-protected.",
            )
        }
        // Commands turn a stop into `{ status: "stopped" }` before this.
        RecognitionError::Stopped => CommandError::failed("Reading the text was stopped."),
        RecognitionError::Internal(detail) => {
            eprintln!("[synapse] {context} failed: {detail}");
            CommandError::failed("Synapse couldn't read the text. Try again.")
        }
    }
}

fn recognition_helper() -> Result<recognition::Helper, CommandError> {
    recognition::Helper::this_program().map_err(|err| {
        eprintln!("[synapse] finding Synapse's own program failed: {err}");
        CommandError::failed("Synapse couldn't read the text. Try again.")
    })
}

/// What reading a picture did: `{ "status": "read", "recognition":
/// Recognition }` or `{ "status": "stopped" }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PictureReading {
    Read { recognition: Box<Recognition> },
    Stopped,
}

/// Reads the text in a kept picture with the built-in engine: `{ assetId }`.
/// A picture already read returns what was read, without reading it again.
/// Returns [`PictureReading`]. Can be stopped with `stop_recognition`.
#[tauri::command]
pub async fn recognise_picture(
    app: AppHandle,
    database: State<'_, Database>,
    recognising: State<'_, Recognising>,
    asset_id: i64,
) -> Result<PictureReading, CommandError> {
    const CONTEXT: &str = "reading a picture's text";
    let run = recognising.begin();
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, "Synapse couldn't read the text. Try again.")?;
    let helper = recognition_helper()?;
    match recognition::recognise_picture(&pool, &store, helper, asset_id, run.token(), Utc::now())
        .await
    {
        Ok(found) => Ok(PictureReading::Read {
            recognition: Box::new(found),
        }),
        Err(RecognitionError::Stopped) => Ok(PictureReading::Stopped),
        Err(err) => Err(recognition_error(err, CONTEXT)),
    }
}

/// The text read from a kept picture, if it has been read: `{ assetId }`.
/// Returns `Recognition | null`.
#[tauri::command]
pub async fn get_picture_recognition(
    app: AppHandle,
    database: State<'_, Database>,
    asset_id: i64,
) -> Result<Option<Recognition>, CommandError> {
    let pool = pool(&app, &database).await?;
    recognition::latest(&pool, asset_id, recognition::PICTURE_PAGE)
        .await
        .map_err(|err| {
            recognition_error(RecognitionError::Internal(err), "loading a picture's text")
        })
}

/// The pages of a PDF source that reading would cover when no pages are
/// named: `{ sourceId }`. Returns `[number]`: pages with no text that the
/// built-in engine hasn't read yet. Empty for a source that isn't a PDF.
#[tauri::command]
pub async fn get_pages_to_recognise(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
) -> Result<Vec<u32>, CommandError> {
    let pool = pool(&app, &database).await?;
    match recognition::pages_to_read(&pool, source_id).await {
        Err(RecognitionError::NotReadable) => Ok(Vec::new()),
        result => result.map_err(|err| recognition_error(err, "finding scanned pages")),
    }
}

/// Reads the text on pages of an active PDF source with the built-in engine:
/// `{ sourceId, pages?, onProgress }`. Without `pages`, every page with no
/// text that hasn't been read yet. `onProgress` is a channel that receives
/// `{ page, done, total, problem }` as each page finishes; each page is saved
/// as it is read. Returns `{ read, skipped, stopped }`; stopping with
/// `stop_recognition` keeps the pages already read.
#[tauri::command]
pub async fn recognise_source_pages(
    app: AppHandle,
    database: State<'_, Database>,
    recognising: State<'_, Recognising>,
    source_id: i64,
    pages: Option<Vec<u32>>,
    on_progress: Channel<PageProgress>,
) -> Result<SourceRecognition, CommandError> {
    const CONTEXT: &str = "reading a source's pages";
    let run = recognising.begin();
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, "Synapse couldn't read the text. Try again.")?;
    let helper = recognition_helper()?;
    match recognition::recognise_source(
        &pool,
        &store,
        helper,
        source_id,
        pages,
        run.token(),
        Utc::now,
        |progress| {
            let _ = on_progress.send(progress);
        },
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(RecognitionError::Stopped) => Ok(SourceRecognition {
            stopped: true,
            ..SourceRecognition::default()
        }),
        Err(err) => Err(recognition_error(err, CONTEXT)),
    }
}

/// Stops the text being read, if any, by either engine. Takes no arguments;
/// returns `null`.
#[tauri::command]
pub fn stop_recognition(recognising: State<'_, Recognising>) {
    recognising.stop();
}

// Reading text with a vision model in Ollama (`vision.rs`).

/// Turns a model's reading that wasn't kept into a user-safe error, logging
/// internal detail. Neither the picture's text nor the model's answer is ever
/// logged. For a source's pages, the message says the pages already read
/// were kept.
fn vision_error(err: VisionError, context: &str, pages: bool) -> CommandError {
    let either = |picture: &'static str, source: &'static str| if pages { source } else { picture };
    match err {
        VisionError::Recognition(err) => recognition_error(err, context),
        VisionError::Ollama(SuggestError::Unreachable(detail)) => {
            eprintln!("[synapse] {context}: reaching Ollama failed: {detail}");
            CommandError::failed(OLLAMA_UNREACHABLE)
        }
        VisionError::Ollama(SuggestError::Unexpected(detail)) => {
            eprintln!("[synapse] {context}: Ollama answered unexpectedly: {detail}");
            CommandError::failed(OLLAMA_UNEXPECTED)
        }
        // The model list on screen is out of date, so React reloads it.
        VisionError::Ollama(SuggestError::ModelMissing) => CommandError::stale(
            "That model isn't in Ollama any more, or can't read pictures. Choose another.",
        ),
        VisionError::Ollama(SuggestError::NoteOverflowed) => CommandError::failed(either(
            "The picture left the model no room to write its text. Try another model, or the \
             built-in reader.",
            "A page left the model no room to write its text. Pages read before it were kept. \
             Try another model, or the built-in reader.",
        )),
        VisionError::Ollama(SuggestError::TimedOut) => CommandError::failed(either(
            "The model took over 10 minutes on the picture, so Synapse stopped waiting. Try a \
             smaller model.",
            "The model took over 10 minutes on a page, so Synapse stopped waiting. Pages read \
             before it were kept. Try a smaller model.",
        )),
        VisionError::Ollama(SuggestError::ModelFailed(detail)) => {
            eprintln!("[synapse] {context}: Ollama couldn't run the model: {detail}");
            CommandError::failed(either(
                "Ollama couldn't run that model. Try again, or choose another.",
                "Ollama couldn't run that model. Pages read before it stopped were kept. Try \
                 again, or choose another.",
            ))
        }
        VisionError::Ollama(other) => {
            eprintln!("[synapse] {context}: asking Ollama failed: {other:?}");
            CommandError::failed("Synapse couldn't ask Ollama to read the text.")
        }
        VisionError::Empty => CommandError::failed(either(
            "The model's answer had no text in it. Try again, or choose another model.",
            "The model's answer for a page had no text in it. Pages read before it were kept. \
             Try again, or choose another model.",
        )),
        VisionError::TooLong => CommandError::failed(either(
            "The model wrote more than a page can hold, so its answer wasn't kept. It may have \
             repeated itself: try another model.",
            "The model wrote more than a page can hold, so its answer wasn't kept. Pages read \
             before it were kept. It may have repeated itself: try another model.",
        )),
    }
}

/// The models text can be read with besides the built-in engine: Ollama's
/// local models that can read pictures, by name, sorted. Cloud models are
/// never listed. Takes no arguments. Returns `["qwen2.5vl:7b", ...]`, empty if
/// Ollama has none; rejects with kind "failed" if Ollama can't be reached.
#[tauri::command]
pub async fn get_vision_models() -> Result<Vec<String>, CommandError> {
    let ollama = Ollama::local().map_err(suggest_error)?;
    ollama.vision_models().await.map_err(suggest_error)
}

/// What reading a picture with a model did: `{ "status": "read",
/// "recognition": Recognition, "agreement": 87 | null }` or `{ "status":
/// "stopped" }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ModelPictureReading {
    Read {
        recognition: Box<Recognition>,
        agreement: Option<u8>,
    },
    Stopped,
}

/// Reads the text in a kept picture with a vision model in Ollama on this
/// computer, and keeps it as the picture's text: `{ assetId, model }`, where
/// `model` is a name from `get_vision_models`. A picture whose text in use
/// is already that model's isn't sent again. Returns [`ModelPictureReading`];
/// rejects with kind "stale" if the model is no longer in Ollama. Can be
/// stopped with `stop_recognition`.
#[tauri::command]
pub async fn recognise_picture_with_model(
    app: AppHandle,
    database: State<'_, Database>,
    recognising: State<'_, Recognising>,
    asset_id: i64,
    model: String,
) -> Result<ModelPictureReading, CommandError> {
    const CONTEXT: &str = "reading a picture's text with a model";
    let run = recognising.begin();
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, "Synapse couldn't read the text. Try again.")?;
    let helper = recognition_helper()?;
    let ollama = Ollama::local().map_err(suggest_error)?;
    match vision::read_picture(
        &pool,
        &store,
        helper,
        &ollama,
        asset_id,
        &model,
        run.token(),
        Utc::now(),
    )
    .await
    {
        Ok(read) => Ok(ModelPictureReading::Read {
            recognition: Box::new(read.recognition),
            agreement: read.agreement,
        }),
        Err(VisionError::Recognition(RecognitionError::Stopped)) => {
            Ok(ModelPictureReading::Stopped)
        }
        Err(err) => Err(vision_error(err, CONTEXT, false)),
    }
}

/// The pages of a PDF source that reading with `model` would cover when no
/// pages are named: `{ sourceId, model }`. Returns `[number]`: pages with no
/// text whose text in use isn't already that model's. Empty for a source
/// that isn't a PDF.
#[tauri::command]
pub async fn get_pages_to_recognise_with_model(
    app: AppHandle,
    database: State<'_, Database>,
    source_id: i64,
    model: String,
) -> Result<Vec<u32>, CommandError> {
    let pool = pool(&app, &database).await?;
    match vision::pages_to_read(&pool, source_id, &model).await {
        Err(VisionError::Recognition(RecognitionError::NotReadable)) => Ok(Vec::new()),
        result => result.map_err(|err| vision_error(err, "finding scanned pages", false)),
    }
}

/// Reads the text on pages of an active PDF source with a vision model in
/// Ollama on this computer: `{ sourceId, pages?, model, onProgress }`.
/// Without `pages`, every page `get_pages_to_recognise_with_model` lists.
/// `onProgress` receives `{ page, done, total, problem }` as each page
/// finishes; each page is kept as it is read. Returns `{ read, skipped,
/// stopped, agreement: [{ page, percent }] }`; stopping with
/// `stop_recognition` keeps the pages already read, and so does a failure.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn recognise_source_pages_with_model(
    app: AppHandle,
    database: State<'_, Database>,
    recognising: State<'_, Recognising>,
    source_id: i64,
    pages: Option<Vec<u32>>,
    model: String,
    on_progress: Channel<PageProgress>,
) -> Result<ModelSourceReading, CommandError> {
    const CONTEXT: &str = "reading a source's pages with a model";
    let run = recognising.begin();
    let pool = pool(&app, &database).await?;
    let store = asset_store(&app, "Synapse couldn't read the text. Try again.")?;
    let helper = recognition_helper()?;
    let ollama = Ollama::local().map_err(suggest_error)?;
    match vision::read_source(
        &pool,
        &store,
        helper,
        &ollama,
        source_id,
        pages,
        &model,
        run.token(),
        Utc::now,
        |progress| {
            let _ = on_progress.send(progress);
        },
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(VisionError::Recognition(RecognitionError::Stopped)) => Ok(ModelSourceReading {
            stopped: true,
            ..ModelSourceReading::default()
        }),
        Err(err) => Err(vision_error(err, CONTEXT, true)),
    }
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
                kept_files: 2,
            })
            .unwrap(),
            json!({
                "status": "ready",
                "token": 3,
                "fileName": "synapse-export-2026-09-16.zip",
                "exportedAt": "2026-09-16T09:30:00.000Z",
                "keptFiles": 2
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
                pictures: Vec::new(),
                tags: vec!["bio".to_string()],
            })
            .unwrap(),
            json!({
                "id": 1,
                "title": "Cells",
                "body": "one\ntwo",
                "createdAt": "2026-09-16T12:00:00.000Z",
                "updatedAt": "2026-09-16T12:00:00.000Z",
                "pictures": [],
                "tags": ["bio"]
            })
        );
        assert_eq!(
            serde_json::to_value(NoteSummary {
                id: 1,
                title: "Cells".to_string(),
                created_at: time.clone(),
                updated_at: time,
                tags: Vec::new(),
            })
            .unwrap(),
            json!({
                "id": 1,
                "title": "Cells",
                "createdAt": "2026-09-16T12:00:00.000Z",
                "updatedAt": "2026-09-16T12:00:00.000Z",
                "tags": []
            })
        );
    }

    #[test]
    fn tag_errors_point_at_the_tags_and_match_the_limits() {
        let to_json = |problem| serde_json::to_value(CommandError::invalid_tags(problem)).unwrap();
        assert_eq!(to_json(TagProblem::BadCharacter)["field"], json!("tags"));
        assert!(to_json(TagProblem::TooLong)["message"]
            .as_str()
            .unwrap()
            .contains(&tags::TAG_MAX_CHARS.to_string()));
        assert!(to_json(TagProblem::TooMany)["message"]
            .as_str()
            .unwrap()
            .contains(&tags::TAGS_MAX.to_string()));
        assert_eq!(
            serde_json::to_value(authoring_error(
                AuthoringError::Tags(TagProblem::Blank),
                "t",
                "f"
            ))
            .unwrap()["field"],
            json!("tags")
        );
        assert_eq!(
            serde_json::to_value(note_error(
                NoteError::Invalid(InvalidNote::Tags(TagProblem::NameTaken)),
                "t",
                "f"
            ))
            .unwrap()["field"],
            json!("tags")
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
    fn picture_errors_point_at_the_pictures_and_match_the_limits() {
        use crate::pictures::{CARD_SIDE_PICTURES_MAX, NOTE_PICTURES_MAX};
        let note = serde_json::to_value(note_error(
            NoteError::Invalid(InvalidNote::Pictures(PictureProblem::TooMany)),
            "test",
            "failed",
        ))
        .unwrap();
        assert_eq!(note["field"], "pictures");
        assert!(note["message"]
            .as_str()
            .unwrap()
            .contains(&format!("at most {NOTE_PICTURES_MAX} pictures")));
        for (problem, field) in [
            (
                InvalidInput::FrontPictures(PictureProblem::TooMany),
                "frontPictures",
            ),
            (
                InvalidInput::BackPictures(PictureProblem::TooMany),
                "backPictures",
            ),
        ] {
            let card = serde_json::to_value(CommandError::invalid_input(problem)).unwrap();
            assert_eq!(card["field"], field);
            assert!(card["message"]
                .as_str()
                .unwrap()
                .contains(&format!("at most {CARD_SIDE_PICTURES_MAX} pictures")));
        }
        for problem in [PictureProblem::NotAPicture, PictureProblem::Repeated] {
            let card = serde_json::to_value(CommandError::invalid_input(
                InvalidInput::BackPictures(problem),
            ))
            .unwrap();
            assert_eq!(card["kind"], "invalid");
        }
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
                    cloze_cards: None,
                    tags: Vec::new(),
                },
                CardsFileRow {
                    line: 3,
                    front: "Alone".to_string(),
                    back: String::new(),
                    problem: Some(RowProblemMessage::of(RowProblem::NoBack)),
                    duplicate: Some(Duplicate::InDeck),
                    cloze_cards: None,
                    tags: Vec::new(),
                },
                CardsFileRow {
                    line: 4,
                    front: String::new(),
                    back: "b".to_string(),
                    problem: Some(RowProblemMessage::of(RowProblem::Card(
                        InvalidInput::FrontBlank,
                    ))),
                    duplicate: None,
                    cloze_cards: None,
                    tags: Vec::new(),
                },
                CardsFileRow {
                    line: 5,
                    front: "The {{c1::cell}} has a {{c2::nucleus}}.".to_string(),
                    back: String::new(),
                    problem: None,
                    duplicate: None,
                    cloze_cards: Some(2),
                    tags: Vec::new(),
                },
                CardsFileRow {
                    line: 6,
                    front: "The {{c1::cell".to_string(),
                    back: String::new(),
                    problem: Some(RowProblemMessage::of(RowProblem::Card(
                        InvalidInput::ClozeUnclosed,
                    ))),
                    duplicate: None,
                    cloze_cards: None,
                    tags: Vec::new(),
                },
            ],
            tags_column: false,
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
                        "problem": null, "duplicate": { "kind": "inFile", "line": 1 },
                        "clozeCards": null, "tags": []
                    },
                    {
                        "line": 3, "front": "Alone", "back": "",
                        "problem": {
                            "field": "back",
                            "message": "This row has no back: put the back in a second column."
                        },
                        "duplicate": { "kind": "inDeck" },
                        "clozeCards": null, "tags": []
                    },
                    {
                        "line": 4, "front": "", "back": "b",
                        "problem": { "field": "front", "message": "Enter the front of the card." },
                        "duplicate": null,
                        "clozeCards": null, "tags": []
                    },
                    {
                        "line": 5, "front": "The {{c1::cell}} has a {{c2::nucleus}}.", "back": "",
                        "problem": null, "duplicate": null, "clozeCards": 2, "tags": []
                    },
                    {
                        "line": 6, "front": "The {{c1::cell", "back": "",
                        "problem": {
                            "field": "front",
                            "message": "Close each {{c1::…}} with }} before the next one starts."
                        },
                        "duplicate": null,
                        "clozeCards": null, "tags": []
                    }
                ],
                "tagsColumn": false,
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
    fn quizzes_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(QuizOutcome::Written {
                questions: vec![QuizQuestion {
                    question: "What does mitosis produce?".to_string(),
                    answer: "Two identical cells".to_string(),
                    distractors: vec!["One cell".into(), "Four cells".into(), "No cells".into()],
                    source: "It produces two identical cells.".to_string(),
                    unfamiliar: vec![],
                }],
                left_out: 1,
                unsupported: 2,
            })
            .unwrap(),
            json!({
                "status": "written",
                "questions": [{
                    "question": "What does mitosis produce?",
                    "answer": "Two identical cells",
                    "distractors": ["One cell", "Four cells", "No cells"],
                    "source": "It produces two identical cells.",
                    "unfamiliar": []
                }],
                "leftOut": 1,
                "unsupported": 2
            })
        );
        assert_eq!(
            serde_json::to_value(QuizOutcome::Stopped).unwrap(),
            json!({ "status": "stopped" })
        );
    }

    #[test]
    fn failed_quizzes_are_reported_without_detail_and_never_mention_cards() {
        let detail = || -> crate::db::DbError { "http://127.0.0.1:11434/api/chat: secret".into() };
        for (err, kind) in [
            (SuggestError::Unreachable(detail()), "failed"),
            (SuggestError::ModelMissing, "stale"),
            (SuggestError::NoteTooLong, "invalid"),
            (SuggestError::NoteOverflowed, "failed"),
            (SuggestError::TimedOut, "failed"),
            (SuggestError::Unusable(detail()), "failed"),
            (SuggestError::Ungrounded, "failed"),
            (SuggestError::Internal(detail()), "failed"),
        ] {
            let value = serde_json::to_value(quiz_error(err)).unwrap();
            assert_eq!(value["kind"], kind, "{value}");
            let message = value["message"].as_str().unwrap();
            assert!(
                !message.contains("secret") && !message.contains("11434"),
                "{message}"
            );
            assert!(
                !message.contains("card") && !message.contains("suggest"),
                "{message}"
            );
        }
    }

    #[test]
    fn failed_model_readings_are_reported_without_detail_and_say_what_was_kept() {
        let detail = || -> crate::db::DbError { "http://127.0.0.1:11434/api/chat: secret".into() };
        let errors = || {
            [
                (
                    VisionError::Ollama(SuggestError::Unreachable(detail())),
                    "failed",
                ),
                (
                    VisionError::Ollama(SuggestError::Unexpected(detail())),
                    "failed",
                ),
                (VisionError::Ollama(SuggestError::ModelMissing), "stale"),
                (VisionError::Ollama(SuggestError::NoteOverflowed), "failed"),
                (VisionError::Ollama(SuggestError::TimedOut), "failed"),
                (
                    VisionError::Ollama(SuggestError::ModelFailed(detail())),
                    "failed",
                ),
                (
                    VisionError::Ollama(SuggestError::Internal(detail())),
                    "failed",
                ),
                (VisionError::Empty, "failed"),
                (VisionError::TooLong, "failed"),
                (
                    VisionError::Recognition(RecognitionError::Internal(detail())),
                    "failed",
                ),
                (VisionError::Recognition(RecognitionError::Deleted), "stale"),
            ]
        };
        for pages in [false, true] {
            for (err, kind) in errors() {
                let model_failure = matches!(
                    err,
                    VisionError::Empty
                        | VisionError::TooLong
                        | VisionError::Ollama(
                            SuggestError::NoteOverflowed
                                | SuggestError::TimedOut
                                | SuggestError::ModelFailed(_)
                        )
                );
                let value = serde_json::to_value(vision_error(err, "test", pages)).unwrap();
                assert_eq!(value["kind"], kind, "{value}");
                let message = value["message"].as_str().unwrap();
                assert!(
                    !message.contains("secret") && !message.contains("11434"),
                    "{message}"
                );
                assert!(
                    !message.contains("card") && !message.contains("suggest"),
                    "{message}"
                );
                if model_failure {
                    assert_eq!(message.contains("Pages read before"), pages, "{message}");
                }
            }
        }
    }

    #[test]
    fn model_readings_serialize_to_the_shape_react_expects() {
        assert_eq!(
            serde_json::to_value(ModelPictureReading::Stopped).unwrap(),
            json!({ "status": "stopped" })
        );
        assert_eq!(
            serde_json::to_value(ModelSourceReading {
                read: vec![2],
                skipped: Vec::new(),
                stopped: false,
                agreement: vec![vision::PageAgreement {
                    page: 2,
                    percent: 67
                }],
            })
            .unwrap(),
            json!({
                "read": [2],
                "skipped": [],
                "stopped": false,
                "agreement": [{ "page": 2, "percent": 67 }]
            })
        );
    }

    #[test]
    fn failed_card_help_is_reported_without_detail_and_never_mentions_notes() {
        let detail = || -> crate::db::DbError { "http://127.0.0.1:11434/api/chat: secret".into() };
        for (err, kind) in [
            (SuggestError::Unreachable(detail()), "failed"),
            (SuggestError::ModelMissing, "stale"),
            (SuggestError::NoteTooLong, "failed"),
            (SuggestError::NoteOverflowed, "failed"),
            (SuggestError::TimedOut, "failed"),
            (SuggestError::Unusable(detail()), "failed"),
            (SuggestError::Ungrounded, "failed"),
            (SuggestError::Internal(detail()), "failed"),
        ] {
            let value = serde_json::to_value(help_error(err)).unwrap();
            assert_eq!(value["kind"], kind, "{value}");
            let message = value["message"].as_str().unwrap();
            assert!(
                !message.contains("secret") && !message.contains("11434"),
                "{message}"
            );
            assert!(
                !message.contains("note") && !message.contains("suggest"),
                "{message}"
            );
        }
    }

    #[test]
    fn failed_picture_card_help_says_the_model_must_see_pictures_and_hides_detail() {
        let detail = || -> crate::db::DbError { "http://127.0.0.1:11434/api/chat: secret".into() };
        for (err, kind) in [
            (SuggestError::Unreachable(detail()), "failed"),
            (SuggestError::ModelMissing, "stale"),
            (SuggestError::NoteOverflowed, "failed"),
            (SuggestError::TimedOut, "failed"),
            (SuggestError::Unusable(detail()), "failed"),
            (SuggestError::Internal(detail()), "failed"),
        ] {
            let value = serde_json::to_value(picture_help_error(err)).unwrap();
            assert_eq!(value["kind"], kind, "{value}");
            let message = value["message"].as_str().unwrap();
            assert!(
                !message.contains("secret") && !message.contains("11434"),
                "{message}"
            );
            assert!(
                !message.contains("note") && !message.contains("suggest"),
                "{message}"
            );
        }
        let missing = serde_json::to_value(picture_help_error(SuggestError::ModelMissing)).unwrap();
        assert!(missing["message"]
            .as_str()
            .unwrap()
            .contains("can't see pictures"));
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

    #[test]
    fn anki_errors_state_the_real_limits() {
        use crate::anki::{
            ANKI_CARDS_MAX, ANKI_COLLECTION_MAX_BYTES, ANKI_DECKS_MAX, ANKI_FILE_MAX_BYTES,
        };
        let message = |err| anki_error(err, "test", "failed").message;
        assert!(message(AnkiError::TooLarge)
            .contains(&format!("{} GB", ANKI_FILE_MAX_BYTES / 1024 / 1024 / 1024)));
        assert!(message(AnkiError::CollectionTooLarge)
            .contains(&format!("{} MB", ANKI_COLLECTION_MAX_BYTES / 1024 / 1024)));
        assert!(message(AnkiError::TooManyCards).contains("50,000"));
        assert_eq!(ANKI_CARDS_MAX, 50_000);
        assert!(message(AnkiError::TooManyDecks).contains("1,000"));
        assert_eq!(ANKI_DECKS_MAX, 1_000);
        // Internal detail stays out of the message.
        let unreadable = message(AnkiError::Unreadable("secret detail".to_string()));
        assert!(!unreadable.contains("detail"));
        assert_eq!(
            message(AnkiError::Authoring(AuthoringError::DeckArchived)),
            DECK_ARCHIVED
        );
    }

    #[test]
    fn anki_previews_serialize_to_the_shape_react_expects() {
        let file = AnkiFile::Read {
            token: 3,
            file_name: "French.apkg".to_string(),
            decks: vec![AnkiDeckPreview {
                anki_name: "French::Verbs".to_string(),
                name: "French::Verbs (2)".to_string(),
                cards: 2,
                reversible: 1,
                examples: vec![CardText {
                    front: "être".to_string(),
                    back: "to be".to_string(),
                }],
            }],
            notes: 4,
            skipped: Skipped {
                image_occlusion: 1,
                bad_cloze: 2,
                other_note_type: 1,
                ..Skipped::default()
            },
            media_removed: 1,
            extra_removed: 3,
            reviewed: 2,
            occlusions: 2,
            occlusion_approximated: 1,
            card_pictures: 5,
            pictures_left_out: PicturesLeftOut {
                missing: 1,
                too_many: 2,
                ..PicturesLeftOut::default()
            },
            tags: 4,
            tags_left_out: 1,
        };
        assert_eq!(
            serde_json::to_value(file).unwrap(),
            json!({
                "status": "read",
                "token": 3,
                "fileName": "French.apkg",
                "decks": [{
                    "ankiName": "French::Verbs",
                    "name": "French::Verbs (2)",
                    "cards": 2,
                    "reversible": 1,
                    "examples": [{ "front": "être", "back": "to be" }],
                }],
                "notes": 4,
                "skipped": {
                    "imageOcclusion": 1,
                    "badCloze": 2,
                    "otherNoteType": 1,
                    "empty": 0,
                    "tooLong": 0,
                    "noCard": 0,
                },
                "mediaRemoved": 1,
                "extraRemoved": 3,
                "reviewed": 2,
                "occlusions": 2,
                "occlusionApproximated": 1,
                "cardPictures": 5,
                "picturesLeftOut": {
                    "missing": 1,
                    "unsupported": 0,
                    "tooLarge": 0,
                    "tooMany": 2,
                },
                "tags": 4,
                "tagsLeftOut": 1,
            })
        );
        assert_eq!(
            serde_json::to_value(AnkiFile::Cancelled).unwrap(),
            json!({ "status": "cancelled" })
        );
    }
}
