//! Authoring: creating, renaming, and archiving normal decks, adding, editing,
//! and deleting their flashcards, and the deck screen that lists a deck's
//! cards and counts.
//!
//! Every rule about what makes a valid deck or card is checked here, so
//! nothing the frontend sends is trusted. Lengths count characters (Unicode
//! scalar values) after outer whitespace is trimmed:
//! - Deck name: required, at most [`DECK_NAME_MAX_CHARS`], and unique ignoring
//!   letter case (including the sample deck's name).
//! - Deck description: optional (blank means none), at most
//!   [`DECK_DESCRIPTION_MAX_CHARS`].
//! - Card front and back: both required, each at most [`CARD_TEXT_MAX_CHARS`].
//!   Only outer whitespace is trimmed; line breaks inside are kept as typed.
//!
//! Several cards can be added at once ([`create_flashcards`], used to save
//! suggested cards): each is checked exactly as a single card is, and either
//! all of them are saved or, if any is refused, none.
//!
//! Editing a card changes only its text: its FSRS scheduling and review
//! history stay exactly as they were. Deleting a card is a soft delete: the
//! row gets a `deleted_at` time and disappears from lists, counts, and
//! reviews, but it and its review logs are kept.
//!
//! Restoring a card (migration 0010) reverses the deletion, and is the only
//! way back: `deleted_at` is cleared and nothing else is written, so the card
//! returns to its own deck with the same id, text, FSRS state, `due` date,
//! `reps`, `lapses`, `last_review`, and review logs. The one thing a restore
//! doesn't put back is `updated_at`, which the deletion moved to the deletion
//! time and no longer knows the old value of; a restored card therefore reads
//! as last written when it was deleted. Nothing orders or schedules by it, so
//! this is cosmetic — but it is why this says "reverses the deletion" rather
//! than "returns the row to exactly what it was". It is not reset to a new
//! card, and it is due again only if the `due` date it already had has
//! passed. No review session is started or resumed — a restore just puts the
//! card back, and the user starts a review from the deck as usual. Only a
//! deleted card in an active normal deck can be restored: a card whose deck
//! is archived must wait until that deck is unarchived, and the sample deck's
//! cards are refused like every other change to them.
//!
//! *Restore all* ([`restore_deleted_cards`]) restores every deleted card of
//! one deck in a single transaction, by the same rules: each card loses only
//! its `deleted_at`, and the deck is refused exactly as a single restore
//! refuses a card's deck.
//!
//! Several of a deck's cards can be changed at once, as the user selected
//! them on the deck's screen: [`delete_flashcards`] soft-deletes them exactly
//! as [`delete_flashcard`] deletes one, and [`move_flashcards`] moves them to
//! another active normal deck. A move changes only the card's deck (and
//! `updated_at`): it keeps its id, text, FSRS state, `due` date, `reps`,
//! `lapses`, `last_review`, and review logs, so its schedule carries on in the
//! new deck exactly where it was. Both take the ids the user confirmed and
//! work in one transaction: if any of them is no longer an active card of the
//! deck the screen showed, nothing is written and the screen is stale. The
//! sample deck and archived decks are refused on either side of a move.
//!
//! Renaming a deck changes only its name (same rules as creating one; its own
//! current name doesn't count as taken). Archiving a deck puts it away: the
//! row gets an `archived_at` time and the deck leaves the dashboard, due
//! counts, and reviews, and can't be opened, renamed, or given cards. Its
//! cards, their FSRS state and review logs, and its sessions are all kept; an
//! unfinished session is finished in the same transaction.
//!
//! Unarchiving (migration 0009) is the exact reverse, and the only way back:
//! `archived_at` is cleared and nothing else is written, so the deck returns
//! as the active normal deck it was, with the same id, name, description,
//! cards, card states, sessions, and review logs. It starts no review session
//! — the deck simply appears on the dashboard again, where the user can start
//! one — and its cards become due only according to the `due` dates they
//! already had. A card soft-deleted before the deck was archived comes back
//! still deleted, under *Deleted cards*, and returns to the deck only when the
//! user restores that card: unarchiving a deck restores none of them by
//! itself. Only an archived normal deck can be unarchived; the sample deck is
//! never archived, so it can never be unarchived either.
//!
//! The sample deck is read-only here: it can't be opened for editing, renamed,
//! or archived, and its cards can't be added to, edited, or deleted.
//!
//! Cloze cards (migration 0012, `cloze.rs`) go through every path here as
//! front/back cards do, with three additions: a cloze card is edited through
//! its text, never as a front and back; a deleted cloze card is restored only
//! when `cloze::restore_block` allows it (and *Restore all* brings back only
//! those), its front and back rewritten from the text as it is now; and a
//! move must take every active card of a cloze text together.

use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};

use crate::cloze::{self, RestoreBlock};
use crate::occlusion;
use crate::db::{to_db_time, DbError};
use crate::study;

pub const DECK_NAME_MAX_CHARS: usize = 100;
pub const DECK_DESCRIPTION_MAX_CHARS: usize = 500;
pub const CARD_TEXT_MAX_CHARS: usize = 2_000;
/// The most cards [`create_flashcards`] adds in one go: far more than one
/// suggestion offers, but a bound on what the frontend can ask for.
pub const CARD_BATCH_MAX: usize = 100;

/// The most cards one bulk delete or move takes ([`delete_flashcards`],
/// [`move_flashcards`]): more than any deck is likely to hold, but a bound on
/// what the frontend can send.
pub const CARD_SELECTION_MAX: usize = 10_000;

/// A normal deck as shown on its own screen:
/// `{ id, name, description, cardCount, dueCount, cards, deletedCards }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckDetail {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    /// Every active (not deleted) card in the deck.
    pub card_count: i64,
    /// Active cards in the deck that are due now.
    pub due_count: i64,
    /// The active cards, oldest first.
    pub cards: Vec<DeckCard>,
    /// The deck's soft-deleted cards, most recently deleted first. They are
    /// in no count above and in no review; each can be restored.
    pub deleted_cards: Vec<DeletedCard>,
}

/// An archived deck, as listed in the dashboard's history:
/// `{ id, name, description, cardCount, archivedAt }`.
#[derive(Debug, PartialEq, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedDeck {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    /// Active (not deleted) cards kept in the deck.
    pub card_count: i64,
    /// When it was archived (ISO-8601 UTC).
    pub archived_at: String,
}

/// One card in a deck's card list: `{ id, front, back, cloze, occlusion }`,
/// where `cloze` and `occlusion` are `null` for a front/back card.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct DeckCard {
    pub id: i64,
    pub front: String,
    pub back: String,
    pub cloze: Option<CardCloze>,
    pub occlusion: Option<CardOcclusion>,
}

/// Which image occlusion a card was made from, and its box number:
/// `{ occlusionId, number }`. The edit form loads the picture and boxes with
/// `get_occlusion`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardOcclusion {
    pub occlusion_id: i64,
    pub number: i64,
}

/// Which cloze text a card was made from, and its number in it:
/// `{ textId, number, text }`. Every card of one text carries the same text,
/// which is what the edit form opens with.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardCloze {
    pub text_id: i64,
    pub number: i64,
    pub text: String,
}

/// A deleted cloze card's number, and why it can't be restored now, if it
/// can't: `{ number, blocked }`, `blocked` being `null`, `"numberGone"`, or
/// `"elsewhere"`.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct DeletedCloze {
    pub number: i64,
    pub blocked: Option<RestoreBlock>,
}

/// A card row as the deck screen reads it, before it is shaped.
#[derive(sqlx::FromRow)]
struct CardRow {
    id: i64,
    front: String,
    back: String,
    cloze_text_id: Option<i64>,
    cloze_number: Option<i64>,
    cloze_text: Option<String>,
    occlusion_id: Option<i64>,
    occlusion_number: Option<i64>,
    deleted_at: Option<String>,
}

/// One soft-deleted card, as listed under a deck's *Deleted cards*:
/// `{ id, front, back, deletedAt }`.
///
/// Its text is shown so the user can tell which card they would be bringing
/// back. Its schedule isn't sent: a restore keeps whatever schedule the card
/// already had, so there is nothing for the user to decide about it.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedCard {
    pub id: i64,
    pub front: String,
    pub back: String,
    /// When it was deleted (ISO-8601 UTC).
    pub deleted_at: String,
    /// `null` for a front/back card.
    pub cloze: Option<DeletedCloze>,
    /// An image occlusion card's box number, and why it can't be restored
    /// now, if it can't (as for a cloze card); `null` for any other card.
    pub occlusion: Option<DeletedCloze>,
}

impl DeletedCard {
    /// Whether *Restore card* (and *Restore all*) can bring it back now.
    pub fn restorable(&self) -> bool {
        [&self.cloze, &self.occlusion]
            .into_iter()
            .all(|group| group.as_ref().is_none_or(|group| group.blocked.is_none()))
    }
}

/// A card's text as sent, before it is checked: `{ front, back }`. Also the
/// shape of a suggested card (`suggest.rs`), which is saved the same way.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CardText {
    pub front: String,
    pub back: String,
}

/// What was wrong with the text a user entered. Nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidInput {
    NameBlank,
    NameTooLong,
    /// Another deck already has this name (ignoring letter case).
    NameTaken,
    DescriptionTooLong,
    FrontBlank,
    FrontTooLong,
    BackBlank,
    BackTooLong,
    /// The cloze text is blank.
    ClozeBlank,
    ClozeTooLong,
    /// The cloze text has no `{{c1::…}}` deletion.
    ClozeNoDeletion,
    /// A deletion isn't closed with `}}`, or another starts inside it.
    ClozeUnclosed,
    /// A deletion's answer, or its hint, is blank.
    ClozeEmpty,
    /// A deletion's number is 0 or above `cloze::CLOZE_NUMBER_MAX`.
    ClozeNumber,
    /// An image occlusion's header is over `occlusion::HEADER_MAX_CHARS`.
    OcclusionHeaderTooLong,
    /// An image occlusion has no box.
    OcclusionNoBoxes,
    /// More than `occlusion::BOXES_MAX` boxes.
    OcclusionTooManyBoxes,
    /// A box is outside the picture, too small, or numbered outside 1 to
    /// `occlusion::NUMBER_MAX`.
    OcclusionBox,
    /// A box's label is over `occlusion::LABEL_MAX_CHARS`, or one number's
    /// labels together are over a card's 2,000 characters.
    OcclusionLabelTooLong,
}

/// Why an authoring operation didn't happen.
#[derive(Debug)]
pub enum AuthoringError {
    Invalid(InvalidInput),
    /// One of several cards was refused: which one (counting from 0), and why.
    InvalidCard(usize, InvalidInput),
    /// No cards were sent to add.
    NoCards,
    /// More than [`CARD_BATCH_MAX`] cards were sent to add at once.
    TooManyCards,
    /// No deck has this id.
    DeckNotFound,
    /// The deck (or the card's deck) is the sample deck, which can't be edited.
    SampleDeck,
    /// The deck (or the card's deck) has been archived, so it can't change.
    DeckArchived,
    /// The deck isn't archived, so there's nothing to unarchive.
    DeckNotArchived,
    /// No card has this id.
    CardNotFound,
    /// The card was deleted, so it can't be edited or deleted again. It can
    /// still be restored, which is the one thing a deleted card allows.
    CardDeleted,
    /// The card isn't deleted, so there's nothing to restore.
    CardNotDeleted,
    /// Restoring all of a deck's deleted cards was asked for, but the deck
    /// has none, or not the number the user confirmed.
    DeletedCardsChanged,
    /// No cards were selected to delete or move.
    NoneSelected,
    /// More than [`CARD_SELECTION_MAX`] cards were selected at once.
    TooManySelected,
    /// The same card was selected twice, which the deck screen never sends.
    DuplicateSelected,
    /// A selected card is no longer an active card of the deck it was shown
    /// in (it was deleted, moved, or never there), so the screen is out of
    /// date. Nothing was written.
    SelectionChanged,
    /// Cards were to be moved into the deck they are already in.
    SameDeck,
    /// The deck cards were to be moved into is missing, archived, or the
    /// sample deck, so it can't take them. Nothing was written.
    MoveTargetUnavailable,
    /// A card was edited as the other kind: a cloze card as a front and back,
    /// or a front/back card as a cloze text.
    WrongKind,
    /// A deleted cloze card can't be restored now (see `cloze.rs`).
    ClozeNotRestorable(RestoreBlock),
    /// A deleted image occlusion card can't be restored now (see
    /// `occlusion.rs`).
    OcclusionNotRestorable(RestoreBlock),
    /// A move selected some, but not all, of a cloze text's active cards.
    ClozeSplit,
    /// A move selected some, but not all, of an image occlusion's active
    /// cards.
    OcclusionSplit,
    /// The picture for an image occlusion isn't a kept picture.
    PictureMissing,
    /// Anything else (a database failure). Detail is for logs only.
    Internal(DbError),
}

impl From<InvalidInput> for AuthoringError {
    fn from(problem: InvalidInput) -> Self {
        AuthoringError::Invalid(problem)
    }
}

impl From<sqlx::Error> for AuthoringError {
    fn from(err: sqlx::Error) -> Self {
        AuthoringError::Internal(err.into())
    }
}

/// Trims outer whitespace, then checks the text is neither blank nor too long.
/// Shared with `notes.rs`, which reports its own kinds of problem.
pub(crate) fn required_text<E>(
    text: &str,
    max_chars: usize,
    blank: E,
    too_long: E,
) -> Result<&str, E> {
    let text = text.trim();
    if text.is_empty() {
        Err(blank)
    } else if text.chars().count() > max_chars {
        Err(too_long)
    } else {
        Ok(text)
    }
}

/// A deck's fields after validation.
#[derive(Debug, PartialEq)]
struct NewDeck {
    name: String,
    description: Option<String>,
}

fn validate_deck_name(name: &str) -> Result<&str, InvalidInput> {
    required_text(
        name,
        DECK_NAME_MAX_CHARS,
        InvalidInput::NameBlank,
        InvalidInput::NameTooLong,
    )
}

fn validate_deck(name: &str, description: Option<&str>) -> Result<NewDeck, InvalidInput> {
    let name = validate_deck_name(name)?;

    // A blank description is stored as no description.
    let description = description.map(str::trim).filter(|text| !text.is_empty());
    if description.is_some_and(|text| text.chars().count() > DECK_DESCRIPTION_MAX_CHARS) {
        return Err(InvalidInput::DescriptionTooLong);
    }

    Ok(NewDeck {
        name: name.to_string(),
        description: description.map(str::to_string),
    })
}

/// A card's text after validation.
#[derive(Debug, PartialEq)]
pub(crate) struct NewCard {
    pub(crate) front: String,
    pub(crate) back: String,
}

/// Shared with `cards_csv.rs`, which checks each row of a file as a typed card.
pub(crate) fn validate_card(front: &str, back: &str) -> Result<NewCard, InvalidInput> {
    let front = required_text(
        front,
        CARD_TEXT_MAX_CHARS,
        InvalidInput::FrontBlank,
        InvalidInput::FrontTooLong,
    )?;
    let back = required_text(
        back,
        CARD_TEXT_MAX_CHARS,
        InvalidInput::BackBlank,
        InvalidInput::BackTooLong,
    )?;
    Ok(NewCard {
        front: front.to_string(),
        back: back.to_string(),
    })
}

/// Refuses `name` if any deck other than `except` already has it, ignoring
/// letter case. Archived decks and the sample deck count too.
async fn ensure_name_free(
    conn: &mut SqliteConnection,
    name: &str,
    except: Option<i64>,
) -> Result<(), AuthoringError> {
    // Compare lowercase names in Rust: unlike SQLite's own case folding, it
    // also matches non-ASCII letters ('É' and 'é'). There are few decks, so
    // reading every name is cheap. `id IS NOT NULL` is true for every row, so
    // with no `except` every name is read.
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM decks WHERE id IS NOT ?1")
        .bind(except)
        .fetch_all(conn)
        .await?;
    let wanted = name.to_lowercase();
    if names.iter().any(|name| name.to_lowercase() == wanted) {
        return Err(InvalidInput::NameTaken.into());
    }
    Ok(())
}

/// A failed deck write, where the database's unique name index (the backstop
/// for [`ensure_name_free`]) turns into `NameTaken`.
fn deck_write_error(err: sqlx::Error) -> AuthoringError {
    let duplicate = matches!(&err, sqlx::Error::Database(db) if db.is_unique_violation());
    if duplicate {
        InvalidInput::NameTaken.into()
    } else {
        AuthoringError::from(err)
    }
}

/// Creates a normal deck and returns it (with no cards yet).
pub async fn create_deck(
    pool: &SqlitePool,
    name: &str,
    description: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let deck = validate_deck(name, description)?;

    // `BEGIN IMMEDIATE` takes the write lock first, so no other deck can be
    // created between the name check and the insert.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    ensure_name_free(&mut tx, &deck.name, None).await?;

    // `is_sample = 0`: a deck created here is never the sample deck.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO decks (name, description, is_sample, created_at)
         VALUES (?1, ?2, 0, ?3)
         RETURNING id",
    )
    .bind(&deck.name)
    .bind(&deck.description)
    .bind(to_db_time(now))
    .fetch_one(&mut *tx)
    .await
    .map_err(deck_write_error)?;

    tx.commit().await?;
    Ok(DeckDetail {
        id,
        name: deck.name,
        description: deck.description,
        card_count: 0,
        due_count: 0,
        cards: Vec::new(),
        deleted_cards: Vec::new(),
    })
}

/// Checks that `deck_id` is an active normal deck, refusing it exactly as
/// [`load_deck_detail`] does (missing, sample, then archived) but reading only
/// the two columns that decide it: the checks made before a write don't need
/// the deck's cards, which can number in the tens of thousands.
pub(crate) async fn active_normal_deck(
    conn: &mut SqliteConnection,
    deck_id: i64,
) -> Result<(), AuthoringError> {
    let deck: Option<(bool, bool)> =
        sqlx::query_as("SELECT is_sample, archived_at IS NOT NULL FROM decks WHERE id = ?1")
            .bind(deck_id)
            .fetch_optional(conn)
            .await?;
    match deck {
        None => Err(AuthoringError::DeckNotFound),
        Some((true, _)) => Err(AuthoringError::SampleDeck),
        Some((false, true)) => Err(AuthoringError::DeckArchived),
        Some((false, false)) => Ok(()),
    }
}

/// A deck row with its counts, before checking it's an active normal deck.
#[derive(sqlx::FromRow)]
struct DeckRow {
    id: i64,
    name: String,
    description: Option<String>,
    is_sample: bool,
    archived: bool,
    card_count: i64,
    due_count: i64,
}

/// Loads an active normal deck with its counts and cards at `now`. The sample
/// deck and archived decks are refused. Deleted cards are left out of every
/// count and out of `cards`; they are listed separately in `deleted_cards`,
/// which is the only place they appear and the list *Restore card* works from.
pub(crate) async fn load_deck_detail(
    conn: &mut SqliteConnection,
    deck_id: i64,
    now: &str,
) -> Result<DeckDetail, AuthoringError> {
    // "Due" means the same as everywhere in `study.rs`: `due` is NULL (a new
    // card) or not later than now, and the card isn't a buried cloze sibling.
    let since = study::bury_since(now)?;
    let row = sqlx::query_as::<_, DeckRow>(concat!(
        "SELECT d.id, d.name, d.description, d.is_sample,
                d.archived_at IS NOT NULL AS archived,
                (SELECT COUNT(*) FROM flashcards f
                 WHERE f.deck_id = d.id AND f.deleted_at IS NULL) AS card_count,
                (SELECT COUNT(*) FROM flashcards f
                 WHERE f.deck_id = d.id AND f.deleted_at IS NULL
                   AND (f.due IS NULL OR f.due <= ?2)
                   AND f.id NOT IN (",
        study::buried_cards!("?3"),
        ")) AS due_count
         FROM decks d
         WHERE d.id = ?1"
    ))
    .bind(deck_id)
    .bind(now)
    .bind(&since)
    .fetch_optional(&mut *conn)
    .await?;

    let row = match row {
        None => return Err(AuthoringError::DeckNotFound),
        Some(row) if row.is_sample => return Err(AuthoringError::SampleDeck),
        Some(row) if row.archived => return Err(AuthoringError::DeckArchived),
        Some(row) => row,
    };

    let cards = sqlx::query_as::<_, CardRow>(
        "SELECT f.id, f.front, f.back, f.cloze_text_id, f.cloze_number,
                t.text AS cloze_text, f.occlusion_id, f.occlusion_number, f.deleted_at
         FROM flashcards f LEFT JOIN cloze_texts t ON t.id = f.cloze_text_id
         WHERE f.deck_id = ?1 AND f.deleted_at IS NULL
         ORDER BY f.id",
    )
    .bind(deck_id)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|row| DeckCard {
        cloze: match (row.cloze_text_id, row.cloze_number, row.cloze_text) {
            (Some(text_id), Some(number), Some(text)) => Some(CardCloze {
                text_id,
                number,
                text,
            }),
            _ => None,
        },
        occlusion: match (row.occlusion_id, row.occlusion_number) {
            (Some(occlusion_id), Some(number)) => Some(CardOcclusion {
                occlusion_id,
                number,
            }),
            _ => None,
        },
        id: row.id,
        front: row.front,
        back: row.back,
    })
    .collect();

    // Most recently deleted first, as the archived-deck and deleted-note
    // histories are ordered: the card the user just deleted is the one they
    // are most likely to want back.
    let rows = sqlx::query_as::<_, CardRow>(
        "SELECT id, front, back, cloze_text_id, cloze_number, NULL AS cloze_text,
                occlusion_id, occlusion_number, deleted_at
         FROM flashcards
         WHERE deck_id = ?1 AND deleted_at IS NOT NULL
         ORDER BY deleted_at DESC, id DESC",
    )
    .bind(deck_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut deleted_cards = Vec::with_capacity(rows.len());
    for row in rows {
        let cloze = match row.cloze_number {
            Some(number) => Some(DeletedCloze {
                number,
                blocked: cloze::restore_block(conn, row.id).await?,
            }),
            None => None,
        };
        let occlusion = match row.occlusion_number {
            Some(number) => Some(DeletedCloze {
                number,
                blocked: occlusion::restore_block(conn, row.id).await?,
            }),
            None => None,
        };
        deleted_cards.push(DeletedCard {
            id: row.id,
            front: row.front,
            back: row.back,
            deleted_at: row.deleted_at.unwrap_or_default(),
            cloze,
            occlusion,
        });
    }

    Ok(DeckDetail {
        id: row.id,
        name: row.name,
        description: row.description,
        card_count: row.card_count,
        due_count: row.due_count,
        cards,
        deleted_cards,
    })
}

/// An active normal deck with its card counts and cards at `now`.
pub async fn deck_detail(
    pool: &SqlitePool,
    deck_id: i64,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let mut conn = pool.acquire().await?;
    load_deck_detail(&mut conn, deck_id, &to_db_time(now)).await
}

/// Adds a card to a normal deck and returns the deck with updated counts.
///
/// The card starts as a brand-new FSRS card: state `New`, no memory state,
/// no reviews, and `due` NULL, which the study queries treat as due now. So
/// it's offered in this deck's next review session, and in no other deck's.
pub async fn create_flashcard(
    pool: &SqlitePool,
    deck_id: i64,
    front: &str,
    back: &str,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let card = validate_card(front, back)?;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the deck is missing, archived,
    // or the sample deck. Returning early drops `tx`, which rolls it back.
    active_normal_deck(&mut tx, deck_id).await?;
    insert_new_card(&mut tx, deck_id, &card, &now).await?;

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// Adds several cards to a normal deck at once, in the order given, and
/// returns the deck with updated counts. Each card is checked and starts
/// exactly as one added by [`create_flashcard`] does.
///
/// All or nothing: every card is checked before anything is written, and the
/// writes share one transaction, so if any card is refused (it is reported by
/// its position, counting from 0) or the deck can't take cards, none is added.
pub async fn create_flashcards(
    pool: &SqlitePool,
    deck_id: i64,
    cards: &[CardText],
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    if cards.is_empty() {
        return Err(AuthoringError::NoCards);
    }
    if cards.len() > CARD_BATCH_MAX {
        return Err(AuthoringError::TooManyCards);
    }
    let cards = cards
        .iter()
        .enumerate()
        .map(|(index, card)| {
            validate_card(&card.front, &card.back)
                .map_err(|problem| AuthoringError::InvalidCard(index, problem))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // As in `create_flashcard`: refused here, before any card is written.
    active_normal_deck(&mut tx, deck_id).await?;
    for card in &cards {
        insert_new_card(&mut tx, deck_id, card, &now).await?;
    }

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// Writes one checked card into `deck_id` as a brand-new FSRS card (see
/// [`create_flashcard`]). The caller has already checked the deck.
pub(crate) async fn insert_new_card(
    conn: &mut SqliteConnection,
    deck_id: i64,
    card: &NewCard,
    now: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO flashcards (deck_id, front, back, fsrs_state, due, reps, lapses,
                                 created_at, updated_at)
         VALUES (?1, ?2, ?3, 'New', NULL, 0, 0, ?4, ?4)",
    )
    .bind(deck_id)
    .bind(&card.front)
    .bind(&card.back)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Where a card sits and what state it is in: everything needed to decide
/// whether a given change to it is allowed.
struct CardPlacement {
    deck_id: i64,
    /// The card is in the sample deck, which is read-only.
    is_sample: bool,
    /// Its deck is archived, so nothing in that deck may change.
    deck_archived: bool,
    /// The card is soft-deleted.
    deleted: bool,
}

impl CardPlacement {
    /// Refuses a card whose deck takes no changes at all: the sample deck,
    /// which is read-only, and an archived deck, where nothing happens until
    /// it is unarchived. Both apply whatever the change is, so editing,
    /// deleting, and restoring all start here and in this order.
    fn changeable_deck(&self) -> Result<(), AuthoringError> {
        if self.is_sample {
            return Err(AuthoringError::SampleDeck);
        }
        if self.deck_archived {
            return Err(AuthoringError::DeckArchived);
        }
        Ok(())
    }
}

/// Reads `card_id`'s placement, or `CardNotFound` if there is no such card.
async fn card_placement(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<CardPlacement, AuthoringError> {
    let card: Option<(i64, bool, bool, bool)> = sqlx::query_as(
        "SELECT f.deck_id, d.is_sample, d.archived_at IS NOT NULL, f.deleted_at IS NOT NULL
         FROM flashcards f JOIN decks d ON d.id = f.deck_id
         WHERE f.id = ?1",
    )
    .bind(card_id)
    .fetch_optional(conn)
    .await?;

    let (deck_id, is_sample, deck_archived, deleted) = card.ok_or(AuthoringError::CardNotFound)?;
    Ok(CardPlacement {
        deck_id,
        is_sample,
        deck_archived,
        deleted,
    })
}

/// The deck of a card that may be edited or deleted: the card must exist, not
/// be deleted, and belong to an active normal deck.
pub(crate) async fn changeable_card_deck(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<i64, AuthoringError> {
    let card = card_placement(conn, card_id).await?;
    card.changeable_deck()?;
    if card.deleted {
        return Err(AuthoringError::CardDeleted);
    }
    Ok(card.deck_id)
}

/// The deck of a card that may be restored: the card must exist, *be*
/// deleted, and belong to an active normal deck.
///
/// The checks are in the same order as [`changeable_card_deck`], so a deleted
/// card in an archived deck is refused as archived rather than restored: its
/// deck must be unarchived first. Only the last check is the mirror image —
/// a card that isn't deleted has nothing to restore.
async fn restorable_card_deck(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<i64, AuthoringError> {
    let card = card_placement(conn, card_id).await?;
    card.changeable_deck()?;
    if !card.deleted {
        return Err(AuthoringError::CardNotDeleted);
    }
    Ok(card.deck_id)
}

/// Replaces a card's front and back and returns its deck.
///
/// Only the text (and `updated_at`) changes. The card's FSRS state, due date,
/// repetitions, lapses, and review logs are untouched, so its schedule carries
/// on as before.
pub async fn update_flashcard(
    pool: &SqlitePool,
    card_id: i64,
    front: &str,
    back: &str,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let card = validate_card(front, back)?;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let deck_id = changeable_card_deck(&mut tx, card_id).await?;
    // A cloze card's front and back are written from its text, and only from
    // it (`cloze::update_cloze`); an image occlusion card's from its boxes
    // (`occlusion::update_occlusion`). Migration 0018 explains why such a card
    // is still of kind 'basic'.
    let (kind, occlusion_id): (String, Option<i64>) =
        sqlx::query_as("SELECT kind, occlusion_id FROM flashcards WHERE id = ?1")
            .bind(card_id)
            .fetch_one(&mut *tx)
            .await?;
    if kind != "basic" || occlusion_id.is_some() {
        return Err(AuthoringError::WrongKind);
    }
    sqlx::query(
        "UPDATE flashcards SET front = ?1, back = ?2, updated_at = ?3
         WHERE id = ?4 AND deleted_at IS NULL",
    )
    .bind(&card.front)
    .bind(&card.back)
    .bind(&now)
    .bind(card_id)
    .execute(&mut *tx)
    .await?;

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// Soft-deletes a card and returns its deck, without the card.
///
/// The row stays, with every column as it was plus `deleted_at`, and so do its
/// review logs. If this was the deck's last due card and the deck has an
/// unfinished review session, that session is finished in the same
/// transaction.
pub async fn delete_flashcard(
    pool: &SqlitePool,
    card_id: i64,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let deck_id = changeable_card_deck(&mut tx, card_id).await?;
    sqlx::query(
        "UPDATE flashcards SET deleted_at = ?1, updated_at = ?1
         WHERE id = ?2 AND deleted_at IS NULL",
    )
    .bind(&now)
    .bind(card_id)
    .execute(&mut *tx)
    .await?;
    study::finish_session_if_nothing_due(&mut tx, deck_id, &now).await?;

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// Restores a soft-deleted card and returns its deck, with the card back in it.
///
/// Only `deleted_at` is cleared, so the card comes back exactly as it was
/// deleted: same deck, text, FSRS state, `due` date, `reps`, `lapses`,
/// `last_review`, and the same `review_logs` rows. It is not reset to a new
/// card and gets no fresh due date — it is due again only if the date it
/// already had has passed, which the deck's counts then reflect.
///
/// No review session is started, resumed, or reopened. If the deck has an
/// unfinished session, the restored card simply joins the cards that session
/// draws from, exactly as a newly added card would.
///
/// A card that isn't deleted is refused and nothing is written, so restoring
/// twice changes nothing. A card in an archived deck is refused until the
/// deck is unarchived, and the sample deck's cards are refused outright.
///
/// Takes no time: a restore records none. The card's `updated_at` is left as
/// the deletion left it, so a restore adds no history of its own.
pub async fn restore_flashcard(
    pool: &SqlitePool,
    card_id: i64,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the card is missing, in the
    // sample deck or an archived one, or not deleted at all. Returning early
    // drops `tx`, which rolls it back.
    let deck_id = restorable_card_deck(&mut tx, card_id).await?;
    if let Some(block) = cloze::restore_block(&mut tx, card_id).await? {
        return Err(AuthoringError::ClozeNotRestorable(block));
    }
    if let Some(block) = occlusion::restore_block(&mut tx, card_id).await? {
        return Err(AuthoringError::OcclusionNotRestorable(block));
    }

    // The statement repeats the card's own state check, as `update_flashcard`
    // and `delete_flashcard` repeat theirs, so it can only ever clear a
    // deletion that is really there. The other two checks above are about the
    // card's *deck*, and are left to the transaction (nothing can archive the
    // deck inside it) and to migration 0006, which refuses every update to a
    // card in an archived deck. It sets `deleted_at` and nothing else:
    // migration 0010 freezes every other column while the card is deleted, so
    // this is the whole restore.
    let restored = sqlx::query(
        "UPDATE flashcards SET deleted_at = NULL
         WHERE id = ?1 AND deleted_at IS NOT NULL",
    )
    .bind(card_id)
    .execute(&mut *tx)
    .await?;
    // As in `rename_deck`: the card was checked above inside this same
    // transaction, so never report a restore that didn't happen.
    if restored.rows_affected() != 1 {
        return Err(AuthoringError::CardNotDeleted);
    }
    // Only now that it is active may a cloze card's front and back change:
    // its text may have been edited while it was deleted.
    cloze::refresh_restored(&mut tx, card_id, &now).await?;
    occlusion::refresh_restored(&mut tx, card_id, &now).await?;

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// What [`restore_deleted_cards`] did: `{ restored, deck }`, the number of
/// cards brought back and their deck with them in it.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct RestoredCards {
    pub restored: usize,
    pub deck: DeckDetail,
}

/// Restores every soft-deleted card of an active normal deck at once, and
/// returns how many came back with the deck.
///
/// Each card is restored exactly as [`restore_flashcard`] restores one: only
/// `deleted_at` is cleared, so every card keeps its id, text, FSRS state,
/// `due` date, `reps`, `lapses`, `last_review`, and review logs, and no review
/// session is started, resumed, or reopened. All of them come back in one
/// transaction, or none do.
///
/// The deck is refused as a single restore refuses a card's deck: missing,
/// the sample deck, or archived (it must be unarchived first). `expected` is
/// how many deleted cards the user was shown and confirmed; if the deck has
/// none, or a different number, the screen is out of date and
/// [`AuthoringError::DeletedCardsChanged`] is returned with nothing written,
/// so the user never restores more (or fewer) cards than they agreed to.
pub async fn restore_deleted_cards(
    pool: &SqlitePool,
    deck_id: i64,
    expected: usize,
    now: DateTime<Utc>,
) -> Result<RestoredCards, AuthoringError> {
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the deck is missing, the sample
    // deck, or archived. Returning early drops `tx`, which rolls it back.
    // Only the cards that can come back count: a cloze card whose number
    // has left its text stays deleted (see `cloze.rs`).
    let restorable: Vec<i64> = load_deck_detail(&mut tx, deck_id, &now)
        .await?
        .deleted_cards
        .iter()
        .filter(|card| card.restorable())
        .map(|card| card.id)
        .collect();
    let deleted = restorable.len();
    if deleted == 0 || deleted != expected {
        return Err(AuthoringError::DeletedCardsChanged);
    }

    for &card_id in &restorable {
        // The same statement as `restore_flashcard`: it sets `deleted_at` and
        // nothing else, and migration 0010's triggers check each row.
        let restored = sqlx::query(
            "UPDATE flashcards SET deleted_at = NULL
             WHERE id = ?1 AND deck_id = ?2 AND deleted_at IS NOT NULL",
        )
        .bind(card_id)
        .bind(deck_id)
        .execute(&mut *tx)
        .await?;
        // Counted above inside this same transaction, so never report a
        // restore that didn't happen exactly as confirmed.
        if restored.rows_affected() != 1 {
            return Err(AuthoringError::DeletedCardsChanged);
        }
        cloze::refresh_restored(&mut tx, card_id, &now).await?;
        occlusion::refresh_restored(&mut tx, card_id, &now).await?;
    }

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(RestoredCards {
        restored: deleted,
        deck,
    })
}

/// What a bulk delete or move did: `{ changed, deck }`, the number of cards
/// deleted or moved, and the deck they were selected in, now without them.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct ChangedCards {
    pub changed: usize,
    pub deck: DeckDetail,
}

/// Checks a selection before any transaction starts: at least one card, at
/// most [`CARD_SELECTION_MAX`], and no card twice.
fn check_selection(card_ids: &[i64]) -> Result<(), AuthoringError> {
    if card_ids.is_empty() {
        return Err(AuthoringError::NoneSelected);
    }
    if card_ids.len() > CARD_SELECTION_MAX {
        return Err(AuthoringError::TooManySelected);
    }
    let mut seen = std::collections::HashSet::with_capacity(card_ids.len());
    if !card_ids.iter().all(|id| seen.insert(*id)) {
        return Err(AuthoringError::DuplicateSelected);
    }
    Ok(())
}

/// Checks that every selected card is still an active card of `deck`, as
/// loaded inside the transaction that is about to change them. Anything else
/// (deleted, moved, or never in this deck) means the screen is out of date.
fn check_selected_in(deck: &DeckDetail, card_ids: &[i64]) -> Result<(), AuthoringError> {
    let active: std::collections::HashSet<i64> = deck.cards.iter().map(|card| card.id).collect();
    if card_ids.iter().all(|id| active.contains(id)) {
        Ok(())
    } else {
        Err(AuthoringError::SelectionChanged)
    }
}

/// Soft-deletes several cards of one active normal deck at once, and returns
/// how many with the deck, now listing them under its deleted cards.
///
/// Each card is deleted exactly as [`delete_flashcard`] deletes one: the row
/// stays, with every column as it was plus `deleted_at`, and so do its review
/// logs; each can be restored later. All of them are deleted in one
/// transaction, or none are.
///
/// `card_ids` are the cards the user selected and confirmed. The deck is
/// refused if it is missing, the sample deck, or archived; and if any of the
/// cards is no longer an active card of this deck,
/// [`AuthoringError::SelectionChanged`] is returned with nothing written, so
/// the user never deletes a card they didn't see. If no due card is left and
/// the deck has an unfinished review session, it is finished, as deleting
/// one card would.
pub async fn delete_flashcards(
    pool: &SqlitePool,
    deck_id: i64,
    card_ids: &[i64],
    now: DateTime<Utc>,
) -> Result<ChangedCards, AuthoringError> {
    check_selection(card_ids)?;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the deck can't change or the
    // selection is out of date. Returning early drops `tx`, which rolls it
    // back.
    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    check_selected_in(&deck, card_ids)?;

    for &card_id in card_ids {
        // The same statement as `delete_flashcard`, also pinned to the deck.
        let deleted = sqlx::query(
            "UPDATE flashcards SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2 AND deck_id = ?3 AND deleted_at IS NULL",
        )
        .bind(&now)
        .bind(card_id)
        .bind(deck_id)
        .execute(&mut *tx)
        .await?;
        // Checked above inside this same transaction, so never report a
        // deletion that didn't happen.
        if deleted.rows_affected() != 1 {
            return Err(AuthoringError::SelectionChanged);
        }
    }
    study::finish_session_if_nothing_due(&mut tx, deck_id, &now).await?;

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(ChangedCards {
        changed: card_ids.len(),
        deck,
    })
}

/// Moves several cards of one active normal deck into another, and returns
/// how many with the deck they came from, now without them.
///
/// Only the card's deck changes (and `updated_at`, as for any edit): each
/// card keeps its id, text, FSRS state, stability and difficulty, `due` date,
/// `last_review`, `reps`, `lapses`, and every `review_logs` row, so it is due
/// in its new deck exactly when it would have been in the old one. Past
/// sessions keep counting the reviews they recorded. All of the cards move in
/// one transaction, or none do.
///
/// Both decks must be active normal decks, and different. The deck the cards
/// come from is refused as [`delete_flashcards`] refuses it; the deck they go
/// to is refused with [`AuthoringError::MoveTargetUnavailable`] if it is
/// missing, archived, or the sample deck. `card_ids` are the cards the user
/// selected and confirmed; if any is no longer an active card of `from_deck`,
/// [`AuthoringError::SelectionChanged`] is returned with nothing written.
///
/// If no due card is left in `from_deck` and it has an unfinished review
/// session, that session is finished, as deleting its last due card would. An
/// unfinished session in `to_deck` carries on, and the moved cards that are
/// due simply join it, as newly added cards would.
pub async fn move_flashcards(
    pool: &SqlitePool,
    from_deck: i64,
    card_ids: &[i64],
    to_deck: i64,
    now: DateTime<Utc>,
) -> Result<ChangedCards, AuthoringError> {
    check_selection(card_ids)?;
    if from_deck == to_deck {
        return Err(AuthoringError::SameDeck);
    }
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if either deck can't change or the
    // selection is out of date. Returning early drops `tx`, which rolls it
    // back.
    let deck = load_deck_detail(&mut tx, from_deck, &now).await?;
    check_selected_in(&deck, card_ids)?;
    // A cloze text's cards stay in one deck.
    cloze::check_whole_texts(&mut tx, card_ids).await?;
    // So do an image occlusion's.
    occlusion::check_whole_occlusions(&mut tx, card_ids).await?;
    match active_normal_deck(&mut tx, to_deck).await {
        Ok(_) => {}
        Err(
            AuthoringError::DeckNotFound
            | AuthoringError::SampleDeck
            | AuthoringError::DeckArchived,
        ) => return Err(AuthoringError::MoveTargetUnavailable),
        Err(other) => return Err(other),
    }

    for &card_id in card_ids {
        // Only `deck_id` and `updated_at` are written. Migration 0006 refuses
        // a move out of or into an archived deck and 0010 a move of a deleted
        // card, so the statement can't do either even if the checks above
        // were wrong.
        let moved = sqlx::query(
            "UPDATE flashcards SET deck_id = ?1, updated_at = ?2
             WHERE id = ?3 AND deck_id = ?4 AND deleted_at IS NULL",
        )
        .bind(to_deck)
        .bind(&now)
        .bind(card_id)
        .bind(from_deck)
        .execute(&mut *tx)
        .await?;
        // Checked above inside this same transaction, so never report a move
        // that didn't happen.
        if moved.rows_affected() != 1 {
            return Err(AuthoringError::SelectionChanged);
        }
    }
    study::finish_session_if_nothing_due(&mut tx, from_deck, &now).await?;

    let deck = load_deck_detail(&mut tx, from_deck, &now).await?;
    tx.commit().await?;
    Ok(ChangedCards {
        changed: card_ids.len(),
        deck,
    })
}

/// Renames an active normal deck and returns it with its counts and cards.
///
/// Only the name changes: the deck keeps its id, description, cards, review
/// history, and sessions, and an unfinished session carries on.
pub async fn rename_deck(
    pool: &SqlitePool,
    deck_id: i64,
    name: &str,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let name = validate_deck_name(name)?;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the deck is missing, archived,
    // or the sample deck.
    active_normal_deck(&mut tx, deck_id).await?;
    // The deck's own name doesn't count, so changing only letter case works.
    ensure_name_free(&mut tx, name, Some(deck_id)).await?;

    let renamed = sqlx::query("UPDATE decks SET name = ?1 WHERE id = ?2 AND archived_at IS NULL")
        .bind(name)
        .bind(deck_id)
        .execute(&mut *tx)
        .await
        .map_err(deck_write_error)?;
    // The deck was checked above inside this same transaction, so it can't
    // normally have gone. If it somehow did, report a stale screen rather
    // than claiming a rename that didn't happen.
    if renamed.rows_affected() != 1 {
        return Err(AuthoringError::DeckArchived);
    }

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// Archives an active normal deck. Nothing is removed.
///
/// In one transaction: the deck's unfinished review session (if any) is
/// finished, keeping its `cards_reviewed` count, and the deck gets its
/// `archived_at` time. Either both happen or neither does.
pub async fn archive_deck(
    pool: &SqlitePool,
    deck_id: i64,
    now: DateTime<Utc>,
) -> Result<(), AuthoringError> {
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the deck is missing, already
    // archived, or the sample deck.
    active_normal_deck(&mut tx, deck_id).await?;

    // The session must end first: migration 0006 refuses to archive a deck
    // that still has one open.
    study::finish_open_session(&mut tx, deck_id, &now).await?;
    let archived = sqlx::query(
        "UPDATE decks SET archived_at = ?1
         WHERE id = ?2 AND archived_at IS NULL AND is_sample = 0",
    )
    .bind(&now)
    .bind(deck_id)
    .execute(&mut *tx)
    .await?;
    // As in `rename_deck`: the deck was checked above inside this same
    // transaction, so never report an archive that didn't happen.
    if archived.rows_affected() != 1 {
        return Err(AuthoringError::DeckArchived);
    }

    tx.commit().await?;
    Ok(())
}

/// Checks that `deck_id` is a normal deck that is archived, so it can be
/// unarchived. Reads only the two columns that decide it: a deck's name and
/// description aren't needed to put it back, and `load_deck_detail` can't be
/// reused here because it refuses archived decks by design.
async fn unarchivable_deck(
    conn: &mut SqliteConnection,
    deck_id: i64,
) -> Result<(), AuthoringError> {
    let deck: Option<(bool, bool)> =
        sqlx::query_as("SELECT is_sample, archived_at IS NOT NULL FROM decks WHERE id = ?1")
            .bind(deck_id)
            .fetch_optional(conn)
            .await?;

    match deck {
        None => Err(AuthoringError::DeckNotFound),
        // Unreachable in practice: the sample deck can never be archived
        // (migration 0006), so it can never be archived *and* a sample deck.
        // Checked anyway, so it's refused by name rather than by luck.
        Some((true, _)) => Err(AuthoringError::SampleDeck),
        Some((false, false)) => Err(AuthoringError::DeckNotArchived),
        Some((false, true)) => Ok(()),
    }
}

/// Unarchives an archived normal deck, returning it to the dashboard exactly
/// as it was. Nothing else is written.
///
/// Only `archived_at` is cleared, so the deck keeps its id, name, description,
/// cards, their FSRS state and review logs, and its sessions. No review
/// session is created or resumed: the deck's cards are due again only
/// according to the `due` dates they already had, and the user starts a review
/// from the dashboard as for any other deck. Cards that were soft-deleted stay
/// deleted. A deck that isn't archived is refused and nothing is written, so
/// unarchiving twice changes nothing.
///
/// Takes no time: an unarchive records none. The deck's `created_at` is when
/// it was created, and it keeps meaning that.
pub async fn unarchive_deck(pool: &SqlitePool, deck_id: i64) -> Result<(), AuthoringError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the deck is missing, the sample
    // deck, or not archived. Returning early drops `tx`, which rolls it back.
    unarchivable_deck(&mut tx, deck_id).await?;

    // The statement repeats the checks it relies on, as every conditional
    // write here does, so it can only ever clear an archive that is really
    // there, on a deck that is really a normal one.
    let unarchived = sqlx::query(
        "UPDATE decks SET archived_at = NULL
         WHERE id = ?1 AND archived_at IS NOT NULL AND is_sample = 0",
    )
    .bind(deck_id)
    .execute(&mut *tx)
    .await?;
    // As in `rename_deck`: the deck was checked above inside this same
    // transaction, so never report an unarchive that didn't happen.
    if unarchived.rows_affected() != 1 {
        return Err(AuthoringError::DeckNotArchived);
    }

    tx.commit().await?;
    Ok(())
}

/// Every archived deck, most recently archived first, with its active card count.
pub async fn archived_decks(pool: &SqlitePool) -> Result<Vec<ArchivedDeck>, sqlx::Error> {
    sqlx::query_as::<_, ArchivedDeck>(
        "SELECT d.id, d.name, d.description, d.archived_at,
                (SELECT COUNT(*) FROM flashcards f
                 WHERE f.deck_id = d.id AND f.deleted_at IS NULL) AS card_count
         FROM decks d
         WHERE d.archived_at IS NOT NULL
         ORDER BY d.archived_at DESC, d.id DESC",
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;
    use crate::db::{migrate_and_seed, open_file};
    use crate::scheduler::Rating;
    use crate::study::{self, SessionCard, StartedSession, StudyError};
    use chrono::TimeDelta;

    const NOW: &str = "2026-09-15T12:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    /// The validation problem a call failed with; panics on anything else.
    fn problem<T: std::fmt::Debug>(result: Result<T, AuthoringError>) -> InvalidInput {
        match result {
            Err(AuthoringError::Invalid(problem)) => problem,
            other => panic!("expected invalid input, got {other:?}"),
        }
    }

    async fn deck_id(pool: &SqlitePool, name: &str) -> i64 {
        create_deck(pool, name, None, at(NOW)).await.unwrap().id
    }

    async fn card_ids(pool: &SqlitePool, deck: i64) -> Vec<i64> {
        sqlx::query_scalar("SELECT id FROM flashcards WHERE deck_id = ?1 ORDER BY id")
            .bind(deck)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    type StoredCard = (
        Option<i64>,
        String,
        String,
        String,
        Option<f64>,
        Option<f64>,
        Option<String>,
        Option<String>,
        i64,
        i64,
        String,
        String,
    );

    async fn stored_card(pool: &SqlitePool, id: i64) -> StoredCard {
        sqlx::query_as(
            "SELECT deck_id, front, back, fsrs_state, fsrs_stability, fsrs_difficulty,
                    due, last_review, reps, lapses, created_at, updated_at
             FROM flashcards WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn deleted_at(pool: &SqlitePool, id: i64) -> Option<String> {
        sqlx::query_scalar("SELECT deleted_at FROM flashcards WHERE id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    type LogRow = (i64, i64, i64, String, f64, f64, f64, f64, String);

    async fn review_logs(pool: &SqlitePool) -> Vec<LogRow> {
        sqlx::query_as(
            "SELECT id, card_id, rating, state_before, scheduled_days, elapsed_days,
                    stability_after, difficulty_after, reviewed_at
             FROM review_logs ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    type SessionRow = (i64, Option<String>, i64);

    async fn session_row(pool: &SqlitePool, id: i64) -> SessionRow {
        sqlx::query_as("SELECT deck_id, ended_at, cards_reviewed FROM sessions WHERE id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Creates a normal deck holding one card per `(front, back)`; returns the
    /// deck and its card ids in creation order.
    async fn deck_with_cards(
        pool: &SqlitePool,
        name: &str,
        cards: &[(&str, &str)],
    ) -> (i64, Vec<i64>) {
        let deck = deck_id(pool, name).await;
        for (front, back) in cards {
            create_flashcard(pool, deck, front, back, at(NOW))
                .await
                .unwrap();
        }
        (deck, card_ids(pool, deck).await)
    }

    async fn start(pool: &SqlitePool, deck: i64, now: DateTime<Utc>) -> i64 {
        match study::start_session(pool, deck, now).await.unwrap() {
            StartedSession::Started { session_id } => session_id,
            StartedSession::NoneDue => panic!("expected a session to start"),
        }
    }

    /// Every deck, every card, and the review-log and session counts.
    type Snapshot = (
        Vec<(i64, String, Option<String>, bool)>,
        Vec<StoredCard>,
        i64,
        i64,
    );

    async fn snapshot(pool: &SqlitePool) -> Snapshot {
        let decks =
            sqlx::query_as("SELECT id, name, description, is_sample FROM decks ORDER BY id")
                .fetch_all(pool)
                .await
                .unwrap();
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM flashcards ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
        let mut cards = Vec::new();
        for id in ids {
            cards.push(stored_card(pool, id).await);
        }
        (
            decks,
            cards,
            count(pool, "SELECT COUNT(*) FROM review_logs").await,
            count(pool, "SELECT COUNT(*) FROM sessions").await,
        )
    }

    #[test]
    fn a_created_deck_is_trimmed_saved_and_listed_on_the_dashboard() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let sample = sample_deck_id(&pool).await;
            let now = at(NOW);

            let biology = create_deck(&pool, "  Biology ", Some(" Cells and genes \n"), now)
                .await
                .unwrap();
            assert_eq!(
                biology,
                DeckDetail {
                    id: biology.id,
                    name: "Biology".into(),
                    description: Some("Cells and genes".into()),
                    card_count: 0,
                    due_count: 0,
                    cards: Vec::new(),
                    deleted_cards: Vec::new(),
                }
            );
            let blank = create_deck(&pool, "Chemistry", Some(" \t "), now)
                .await
                .unwrap();
            let missing = create_deck(&pool, "History", None, now).await.unwrap();
            assert_eq!((blank.description, missing.description), (None, None));

            let listed: Vec<(i64, String, bool, i64)> = study::decks(&pool, now)
                .await
                .unwrap()
                .into_iter()
                .map(|d| (d.id, d.name, d.is_sample, d.due_count))
                .collect();
            assert_eq!(
                listed,
                vec![
                    (sample, "Sample deck".into(), true, 1),
                    (biology.id, "Biology".into(), false, 0),
                    (blank.id, "Chemistry".into(), false, 0),
                    (missing.id, "History".into(), false, 0),
                ]
            );

            assert_eq!(deck_detail(&pool, biology.id, now).await.unwrap(), biology);
            let created_at: String =
                sqlx::query_scalar("SELECT created_at FROM decks WHERE id = ?1")
                    .bind(biology.id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(created_at, "2026-09-15T12:00:00.000Z");
        });
    }

    #[test]
    fn blank_deck_names_are_rejected() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            for name in ["", "   ", "\t\r\n ", "\u{00A0}\u{2003}"] {
                let result = create_deck(&pool, name, Some("A description"), at(NOW)).await;
                assert_eq!(problem(result), InvalidInput::NameBlank, "{name:?}");
            }
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 1);
        });
    }

    #[test]
    fn deck_length_limits_count_trimmed_characters() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);

            // Two bytes per character, so this is 100 characters but 200 bytes.
            let longest_name = "é".repeat(DECK_NAME_MAX_CHARS);
            let padded_description = format!("  {}  ", "d".repeat(DECK_DESCRIPTION_MAX_CHARS));
            create_deck(&pool, &longest_name, Some(&padded_description), now)
                .await
                .unwrap();

            let too_long_name = "n".repeat(DECK_NAME_MAX_CHARS + 1);
            let too_long_description = "d".repeat(DECK_DESCRIPTION_MAX_CHARS + 1);
            assert_eq!(
                problem(create_deck(&pool, &too_long_name, None, now).await),
                InvalidInput::NameTooLong
            );
            assert_eq!(
                problem(create_deck(&pool, "Physics", Some(&too_long_description), now).await),
                InvalidInput::DescriptionTooLong
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 2);
        });
    }

    #[test]
    fn duplicate_deck_names_are_rejected_ignoring_case() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            deck_id(&pool, "Biology").await;
            deck_id(&pool, "Écoles").await;

            for name in [
                "Biology",
                "biology",
                "  BIOLOGY ",
                "écoles",
                "ÉCOLES",
                "sample deck",
                "SAMPLE DECK",
            ] {
                let result = create_deck(&pool, name, None, at(NOW)).await;
                assert_eq!(problem(result), InvalidInput::NameTaken, "{name:?}");
            }
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 3);

            // An archived deck keeps its name reserved: archiving is not a way
            // to free a name up, because the deck is kept as history.
            let physics = deck_id(&pool, "Physics").await;
            archive_deck(&pool, physics, at(NOW)).await.unwrap();
            for name in ["Physics", "  PHYSICS "] {
                let result = create_deck(&pool, name, None, at(NOW)).await;
                assert_eq!(problem(result), InvalidInput::NameTaken, "{name:?}");
            }
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 4);
        });
    }

    #[test]
    fn creating_decks_never_changes_the_sample_deck() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let sample_sql = "SELECT id, name, description, is_sample, created_at
                              FROM decks WHERE is_sample = 1";
            type DeckRow = (i64, String, Option<String>, bool, String);
            let sample_before: Vec<DeckRow> =
                sqlx::query_as(sample_sql).fetch_all(&pool).await.unwrap();

            for name in ["Biology", "Chemistry", "Sample"] {
                deck_id(&pool, name).await;
            }

            let sample_after: Vec<DeckRow> =
                sqlx::query_as(sample_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(sample_after, sample_before);
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM decks WHERE is_sample = 0").await,
                3
            );
            // The seeded card stays the sample deck's only card.
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
            assert_eq!(
                card_ids(&pool, sample_after[0].0).await,
                vec![1],
                "seed card moved"
            );
        });
    }

    #[test]
    fn the_database_itself_rejects_duplicate_names_and_blank_text() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let sample = sample_deck_id(&pool).await;

            // Writes that skip the Rust validation entirely.
            let duplicate = sqlx::query("INSERT INTO decks (name) VALUES ('SAMPLE DECK')")
                .execute(&pool)
                .await
                .unwrap_err();
            assert!(
                matches!(&duplicate, sqlx::Error::Database(db) if db.is_unique_violation()),
                "{duplicate}"
            );

            let blank_writes = [
                (
                    "INSERT INTO decks (name) VALUES (' \t\n')",
                    "deck name is blank",
                ),
                (
                    "UPDATE decks SET name = '  ' WHERE is_sample = 1",
                    "deck name is blank",
                ),
                (
                    "UPDATE flashcards SET front = char(10) WHERE id = 1",
                    "flashcard text is blank",
                ),
            ];
            for (sql, reason) in blank_writes {
                let err = sqlx::query(sql).execute(&pool).await.unwrap_err();
                assert!(err.to_string().contains(reason), "{sql}: {err}");
            }
            let err =
                sqlx::query("INSERT INTO flashcards (deck_id, front, back) VALUES (?1, 'q', ' ')")
                    .bind(sample)
                    .execute(&pool)
                    .await
                    .unwrap_err();
            assert!(err.to_string().contains("flashcard text is blank"), "{err}");

            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
        });
    }

    #[test]
    fn a_created_card_is_new_due_now_and_keeps_its_line_breaks() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let deck = deck_id(&pool, "Biology").await;

            let front = "Name two parts of a cell:\n  - nucleus\n  - membrane";
            let detail = create_flashcard(
                &pool,
                deck,
                &format!("\n  {front} \n"),
                " Mitochondria\t",
                now,
            )
            .await
            .unwrap();
            assert_eq!(
                (detail.id, detail.card_count, detail.due_count),
                (deck, 1, 1)
            );

            let first = card_ids(&pool, deck).await[0];
            let at_now = "2026-09-15T12:00:00.000Z".to_string();
            assert_eq!(
                stored_card(&pool, first).await,
                (
                    Some(deck),
                    front.to_string(),
                    "Mitochondria".to_string(),
                    "New".to_string(),
                    None,
                    None,
                    None,
                    None,
                    0,
                    0,
                    at_now.clone(),
                    at_now,
                )
            );

            let detail = create_flashcard(&pool, deck, "Q2", "A2", now)
                .await
                .unwrap();
            assert_eq!((detail.card_count, detail.due_count), (2, 2));
            assert_eq!(deck_detail(&pool, deck, now).await.unwrap(), detail);

            // Exactly at the limit is fine, even with multi-byte characters.
            let longest = "ü".repeat(CARD_TEXT_MAX_CHARS);
            create_flashcard(&pool, deck, &longest, &longest, now)
                .await
                .unwrap();
            assert_eq!(card_ids(&pool, deck).await.len(), 3);
        });
    }

    #[test]
    fn blank_or_too_long_card_text_is_rejected() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_id(&pool, "Biology").await;
            let too_long = "x".repeat(CARD_TEXT_MAX_CHARS + 1);

            let attempts = [
                ("", "answer", InvalidInput::FrontBlank),
                (" \n\t ", "answer", InvalidInput::FrontBlank),
                ("question", "", InvalidInput::BackBlank),
                ("question", "\r\n  ", InvalidInput::BackBlank),
                ("", "", InvalidInput::FrontBlank),
                (too_long.as_str(), "answer", InvalidInput::FrontTooLong),
                ("question", too_long.as_str(), InvalidInput::BackTooLong),
            ];
            for (front, back, expected) in attempts {
                let result = create_flashcard(&pool, deck, front, back, at(NOW)).await;
                assert_eq!(problem(result), expected, "{front:?} / {back:?}");
            }
            assert!(card_ids(&pool, deck).await.is_empty());
        });
    }

    #[test]
    fn missing_and_sample_decks_are_refused() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;

            for id in [0, -1, 999, i64::MAX] {
                assert!(
                    matches!(
                        create_flashcard(&pool, id, "q", "a", now).await,
                        Err(AuthoringError::DeckNotFound)
                    ),
                    "create in deck {id}"
                );
                assert!(
                    matches!(
                        deck_detail(&pool, id, now).await,
                        Err(AuthoringError::DeckNotFound)
                    ),
                    "detail of deck {id}"
                );
            }

            assert!(matches!(
                create_flashcard(&pool, sample, "q", "a", now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                deck_detail(&pool, sample, now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
        });
    }

    fn card(front: &str, back: &str) -> CardText {
        CardText {
            front: front.to_string(),
            back: back.to_string(),
        }
    }

    #[test]
    fn several_cards_are_added_in_order_each_as_a_new_card() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let deck = deck_id(&pool, "Biology").await;
            create_flashcard(&pool, deck, "Already here", "a", now)
                .await
                .unwrap();

            let detail = create_flashcards(
                &pool,
                deck,
                &[
                    card(" What is mitosis? ", "Cell division\ninto two cells.\n"),
                    card(
                        "Name the phases.",
                        " Prophase, metaphase, anaphase, telophase.",
                    ),
                ],
                now,
            )
            .await
            .unwrap();

            let texts: Vec<(&str, &str)> = detail
                .cards
                .iter()
                .map(|card| (card.front.as_str(), card.back.as_str()))
                .collect();
            assert_eq!(
                texts,
                vec![
                    ("Already here", "a"),
                    ("What is mitosis?", "Cell division\ninto two cells."),
                    (
                        "Name the phases.",
                        "Prophase, metaphase, anaphase, telophase."
                    ),
                ]
            );
            assert_eq!((detail.card_count, detail.due_count), (3, 3));
            assert_eq!(deck_detail(&pool, deck, now).await.unwrap(), detail);

            // Each starts exactly as a card typed on the deck screen does.
            let at_now = "2026-09-15T12:00:00.000Z".to_string();
            for id in &card_ids(&pool, deck).await[1..] {
                let (deck_of, _, _, state, stability, difficulty, due, last, reps, lapses, c, u) =
                    stored_card(&pool, *id).await;
                assert_eq!(
                    (deck_of, state, stability, difficulty, due, last, reps, lapses, c, u),
                    (
                        Some(deck),
                        "New".to_string(),
                        None,
                        None,
                        None,
                        None,
                        0,
                        0,
                        at_now.clone(),
                        at_now.clone()
                    )
                );
            }
        });
    }

    #[test]
    fn if_any_card_is_refused_none_is_added() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_id(&pool, "Biology").await;
            let too_long = "x".repeat(CARD_TEXT_MAX_CHARS + 1);

            let attempts = [
                (
                    vec![card("q1", "a1"), card("q2", "a2"), card("q3", " \n")],
                    2,
                    InvalidInput::BackBlank,
                ),
                (
                    vec![card(&too_long, "a1"), card("", "")],
                    0,
                    InvalidInput::FrontTooLong,
                ),
                (
                    vec![card("q1", "a1"), card("", "a2")],
                    1,
                    InvalidInput::FrontBlank,
                ),
            ];
            for (cards, index, expected) in attempts {
                let result = create_flashcards(&pool, deck, &cards, at(NOW)).await;
                assert!(
                    matches!(
                        result,
                        Err(AuthoringError::InvalidCard(i, problem)) if i == index && problem == expected
                    ),
                    "{result:?}"
                );
            }
            assert!(card_ids(&pool, deck).await.is_empty());
        });
    }

    #[test]
    fn an_empty_or_oversized_batch_is_refused() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = deck_id(&pool, "Biology").await;

            assert!(matches!(
                create_flashcards(&pool, deck, &[], at(NOW)).await,
                Err(AuthoringError::NoCards)
            ));
            let too_many = vec![card("q", "a"); CARD_BATCH_MAX + 1];
            assert!(matches!(
                create_flashcards(&pool, deck, &too_many, at(NOW)).await,
                Err(AuthoringError::TooManyCards)
            ));
            assert!(card_ids(&pool, deck).await.is_empty());

            // Exactly at the limit is fine.
            let most = vec![card("q", "a"); CARD_BATCH_MAX];
            let detail = create_flashcards(&pool, deck, &most, at(NOW))
                .await
                .unwrap();
            assert_eq!(detail.card_count, CARD_BATCH_MAX as i64);
        });
    }

    #[test]
    fn a_batch_for_a_missing_sample_or_archived_deck_adds_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let archived = deck_id(&pool, "Old").await;
            archive_deck(&pool, archived, now).await.unwrap();
            let cards = [card("q1", "a1"), card("q2", "a2")];

            assert!(matches!(
                create_flashcards(&pool, 999, &cards, now).await,
                Err(AuthoringError::DeckNotFound)
            ));
            assert!(matches!(
                create_flashcards(&pool, sample, &cards, now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                create_flashcards(&pool, archived, &cards, now).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
        });
    }

    #[test]
    fn created_cards_are_reviewed_only_in_their_own_deck() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let biology = deck_id(&pool, "Biology").await;
            let chemistry = deck_id(&pool, "Chemistry").await;
            create_flashcard(&pool, biology, "B1", "b1", now)
                .await
                .unwrap();
            create_flashcard(&pool, chemistry, "C1", "c1", now)
                .await
                .unwrap();
            create_flashcard(&pool, biology, "B2", "b2", now)
                .await
                .unwrap();
            let biology_cards = card_ids(&pool, biology).await;
            let chemistry_card = card_ids(&pool, chemistry).await[0];

            let StartedSession::Started { session_id } =
                study::start_session(&pool, biology, now).await.unwrap()
            else {
                panic!("new cards should be due");
            };

            // Another deck's card can't be rated in this session.
            assert!(matches!(
                study::record_review(&pool, session_id, chemistry_card, 0, Rating::Good, now).await,
                Err(StudyError::Stale)
            ));

            let mut offered = Vec::new();
            while let SessionCard::Due { card, .. } =
                study::session_card(&pool, session_id, now).await.unwrap()
            {
                assert!(
                    offered.len() < biology_cards.len(),
                    "offered too many cards"
                );
                offered.push(card.id);
                study::record_review(&pool, session_id, card.id, card.reps, Rating::Good, now)
                    .await
                    .unwrap();
            }
            assert_eq!(offered, biology_cards);

            // Each review went through FSRS and was logged once.
            for id in &biology_cards {
                let (_, _, _, state, stability, _, due, _, reps, ..) =
                    stored_card(&pool, *id).await;
                assert_eq!((state.as_str(), reps), ("Review", 1));
                assert!(stability.is_some());
                assert!(due.unwrap() > to_db_time(now));
            }
            let logged: Vec<i64> =
                sqlx::query_scalar("SELECT card_id FROM review_logs ORDER BY id")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(logged, biology_cards);

            assert_eq!(deck_detail(&pool, biology, now).await.unwrap().due_count, 0);
            let chemistry_detail = deck_detail(&pool, chemistry, now).await.unwrap();
            assert_eq!(
                (chemistry_detail.card_count, chemistry_detail.due_count),
                (1, 1)
            );
            let sample_due = study::decks(&pool, now)
                .await
                .unwrap()
                .into_iter()
                .find(|d| d.id == sample)
                .unwrap()
                .due_count;
            assert_eq!(sample_due, 1);
        });
    }

    #[test]
    fn created_decks_and_cards_survive_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            // A real file (under the build's `target/` folder), closed and reopened
            // like an app restart.
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("authoring");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");
            let now = at(NOW);

            let pool = open_file(&path).await.unwrap();
            let deck = create_deck(&pool, "Biology", Some("Cells"), now)
                .await
                .unwrap()
                .id;
            create_flashcard(&pool, deck, "Line one\nLine two", "Answer", now)
                .await
                .unwrap();
            create_flashcard(&pool, deck, "Q2", "A2", now)
                .await
                .unwrap();
            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck, now).await.unwrap()
            else {
                panic!("new cards should be due");
            };
            let SessionCard::Due { card, .. } =
                study::session_card(&pool, session_id, now).await.unwrap()
            else {
                panic!("a card should be due");
            };
            study::record_review(&pool, session_id, card.id, card.reps, Rating::Easy, now)
                .await
                .unwrap();

            let before = snapshot(&pool).await;
            pool.close().await;

            // Reopen twice: each open migrates and seeds again, which must be a no-op.
            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(snapshot(&pool).await, before);
                pool.close().await;
            }

            let pool = open_file(&path).await.unwrap();
            let (decks, cards, logs, sessions) = before;
            assert_eq!((decks.len(), cards.len(), logs, sessions), (2, 3, 1, 1));
            assert_eq!(cards[1].1, "Line one\nLine two");
            let detail = deck_detail(&pool, deck, now).await.unwrap();
            assert_eq!((detail.card_count, detail.due_count), (2, 1));
            assert_eq!(
                problem(create_deck(&pool, "BIOLOGY", None, now).await),
                InvalidInput::NameTaken
            );
            // The unfinished session resumes with the remaining card.
            assert_eq!(
                study::start_session(&pool, deck, now).await.unwrap(),
                StartedSession::Started { session_id }
            );
            pool.close().await;

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn editing_a_card_changes_only_its_text() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("Old front", "Old back"), ("Q2", "A2")]).await;
            let card = cards[0];

            // Give the card real scheduling state and a review log first.
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, card, 0, Rating::Good, now)
                .await
                .unwrap();
            let before = stored_card(&pool, card).await;
            assert_eq!((before.3.as_str(), before.8), ("Review", 1));
            let logs_before = review_logs(&pool).await;

            let later = now + TimeDelta::hours(2);
            let detail = update_flashcard(
                &pool,
                card,
                "  New front\n  second line \n",
                " New back ",
                later,
            )
            .await
            .unwrap();
            let new_front = "New front\n  second line";
            assert_eq!(
                detail.cards[0],
                DeckCard {
                    id: card,
                    front: new_front.into(),
                    back: "New back".into(),
                    cloze: None,
                    occlusion: None,
                }
            );
            assert_eq!((detail.card_count, detail.due_count), (2, 1));

            // Front, back, and `updated_at` change; FSRS state, due date, reps,
            // lapses, and creation time don't.
            let mut expected = before.clone();
            expected.1 = new_front.into();
            expected.2 = "New back".into();
            expected.11 = "2026-09-15T14:00:00.000Z".into();
            assert_eq!(stored_card(&pool, card).await, expected);
            assert_eq!(review_logs(&pool).await, logs_before);
            assert_eq!(deleted_at(&pool, card).await, None);
            // The unfinished session carries on with the other card.
            assert_eq!(session_row(&pool, session).await, (deck, None, 1));
        });
    }

    #[test]
    fn invalid_edits_are_rejected_and_change_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let (_, cards) = deck_with_cards(&pool, "Biology", &[("Front", "Back")]).await;
            let before = stored_card(&pool, cards[0]).await;
            let too_long = "x".repeat(CARD_TEXT_MAX_CHARS + 1);

            let attempts = [
                (" \n ", "Back", InvalidInput::FrontBlank),
                ("Front", "\t", InvalidInput::BackBlank),
                (too_long.as_str(), "Back", InvalidInput::FrontTooLong),
                ("Front", too_long.as_str(), InvalidInput::BackTooLong),
            ];
            for (front, back, expected) in attempts {
                let result = update_flashcard(&pool, cards[0], front, back, at(NOW)).await;
                assert_eq!(problem(result), expected, "{front:?} / {back:?}");
            }
            assert_eq!(stored_card(&pool, cards[0]).await, before);
        });
    }

    #[test]
    fn deleting_a_card_hides_it_but_keeps_its_row_and_history() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            let before = stored_card(&pool, cards[0]).await;
            let logs_before = review_logs(&pool).await;

            let later = now + TimeDelta::hours(1);
            let detail = delete_flashcard(&pool, cards[0], later).await.unwrap();
            let listed: Vec<i64> = detail.cards.iter().map(|card| card.id).collect();
            assert_eq!(listed, cards[1..].to_vec());
            assert_eq!((detail.card_count, detail.due_count), (2, 2));
            assert_eq!(deck_detail(&pool, deck, later).await.unwrap(), detail);

            // The row keeps every column (only `updated_at` moves) and its log.
            let mut expected = before.clone();
            expected.11 = "2026-09-15T13:00:00.000Z".into();
            assert_eq!(stored_card(&pool, cards[0]).await, expected);
            assert_eq!(
                deleted_at(&pool, cards[0]).await.as_deref(),
                Some("2026-09-15T13:00:00.000Z")
            );
            assert_eq!(review_logs(&pool).await, logs_before);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 4);
        });
    }

    #[test]
    fn deleted_cards_are_never_counted_offered_or_reviewed() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let (chemistry, chemistry_cards) =
                deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;

            // Delete the card on screen: rating it afterwards writes nothing.
            let session = start(&pool, biology, now).await;
            let SessionCard::Due { card, .. } =
                study::session_card(&pool, session, now).await.unwrap()
            else {
                panic!("a card should be due");
            };
            assert_eq!(card.id, cards[0]);
            delete_flashcard(&pool, cards[0], now).await.unwrap();
            assert!(matches!(
                study::record_review(&pool, session, cards[0], 0, Rating::Good, now).await,
                Err(StudyError::Stale)
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 0);
            // Card B is still due, so the session stays open.
            delete_flashcard(&pool, cards[2], now).await.unwrap();
            assert_eq!(session_row(&pool, session).await, (biology, None, 0));

            // The session skips both deleted cards and ends after the last active one.
            let SessionCard::Due { card, .. } =
                study::session_card(&pool, session, now).await.unwrap()
            else {
                panic!("card B should be due");
            };
            assert_eq!(card.id, cards[1]);
            study::record_review(&pool, session, cards[1], 0, Rating::Good, now)
                .await
                .unwrap();
            assert!(matches!(
                study::session_card(&pool, session, now).await.unwrap(),
                SessionCard::Completed { cards_reviewed: 1 }
            ));

            // A deck whose only card is deleted has nothing to review.
            delete_flashcard(&pool, chemistry_cards[0], now)
                .await
                .unwrap();
            assert_eq!(
                study::start_session(&pool, chemistry, now).await.unwrap(),
                StartedSession::NoneDue
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);

            let dashboard: Vec<(i64, i64)> = study::decks(&pool, now)
                .await
                .unwrap()
                .into_iter()
                .map(|deck| (deck.id, deck.due_count))
                .collect();
            assert_eq!(dashboard, vec![(sample, 1), (biology, 0), (chemistry, 0)]);
            let biology_detail = deck_detail(&pool, biology, now).await.unwrap();
            assert_eq!(
                (biology_detail.card_count, biology_detail.due_count),
                (1, 0)
            );
            assert_eq!(biology_detail.cards.len(), 1);
            let chemistry_detail = deck_detail(&pool, chemistry, now).await.unwrap();
            assert_eq!(
                (chemistry_detail.card_count, chemistry_detail.due_count),
                (0, 0)
            );
            assert!(chemistry_detail.cards.is_empty());
        });
    }

    #[test]
    fn deleting_the_last_due_card_finishes_the_open_session_once() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            assert_eq!(session_row(&pool, session).await, (deck, None, 1));

            delete_flashcard(&pool, cards[1], now + TimeDelta::minutes(5))
                .await
                .unwrap();
            let ended = Some("2026-09-15T12:05:00.000Z".to_string());
            assert_eq!(session_row(&pool, session).await, (deck, ended.clone(), 1));

            // Later lookups keep the first end time, and no new session starts.
            let later = now + TimeDelta::hours(1);
            assert!(matches!(
                study::session_card(&pool, session, later).await.unwrap(),
                SessionCard::Completed { cards_reviewed: 1 }
            ));
            assert_eq!(
                study::start_session(&pool, deck, later).await.unwrap(),
                StartedSession::NoneDue
            );
            assert_eq!(session_row(&pool, session).await, (deck, ended, 1));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);
        });
    }

    #[test]
    fn missing_sample_and_deleted_cards_cannot_be_changed() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (_, cards) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;
            let seed_card = card_ids(&pool, sample_deck_id(&pool).await).await[0];
            let seed_before = stored_card(&pool, seed_card).await;

            for id in [0, -1, 999, i64::MAX] {
                assert!(
                    matches!(
                        update_flashcard(&pool, id, "q", "a", now).await,
                        Err(AuthoringError::CardNotFound)
                    ),
                    "edit {id}"
                );
                assert!(
                    matches!(
                        delete_flashcard(&pool, id, now).await,
                        Err(AuthoringError::CardNotFound)
                    ),
                    "delete {id}"
                );
            }

            assert!(matches!(
                update_flashcard(&pool, seed_card, "q", "a", now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                delete_flashcard(&pool, seed_card, now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert_eq!(stored_card(&pool, seed_card).await, seed_before);
            assert_eq!(deleted_at(&pool, seed_card).await, None);

            delete_flashcard(&pool, cards[0], now).await.unwrap();
            let deleted = stored_card(&pool, cards[0]).await;
            assert!(matches!(
                update_flashcard(&pool, cards[0], "q", "a", now).await,
                Err(AuthoringError::CardDeleted)
            ));
            assert!(matches!(
                delete_flashcard(&pool, cards[0], now + TimeDelta::hours(1)).await,
                Err(AuthoringError::CardDeleted)
            ));
            assert_eq!(stored_card(&pool, cards[0]).await, deleted);
            assert_eq!(
                deleted_at(&pool, cards[0]).await.as_deref(),
                Some("2026-09-15T12:00:00.000Z")
            );
        });
    }

    #[test]
    fn changing_cards_in_one_deck_leaves_other_decks_alone() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (biology, biology_cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let (chemistry, _) =
                deck_with_cards(&pool, "Chemistry", &[("C", "c"), ("D", "d")]).await;
            let chemistry_before = deck_detail(&pool, chemistry, now).await.unwrap();

            let edited = update_flashcard(&pool, biology_cards[0], "A2", "a2", now)
                .await
                .unwrap();
            assert_eq!(edited.id, biology);
            let after_delete = delete_flashcard(&pool, biology_cards[1], now)
                .await
                .unwrap();
            assert_eq!(after_delete.id, biology);
            assert_eq!(
                after_delete.cards,
                vec![DeckCard {
                    id: biology_cards[0],
                    front: "A2".into(),
                    back: "a2".into(),
                    cloze: None,
                    occlusion: None,
                }]
            );
            assert_eq!(
                deck_detail(&pool, chemistry, now).await.unwrap(),
                chemistry_before
            );
        });
    }

    #[test]
    fn the_database_refuses_hard_deletes_and_changes_to_deleted_cards() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;

            // Writes that skip the Rust code entirely.
            let err = sqlx::query("DELETE FROM flashcards WHERE id = ?1")
                .bind(cards[0])
                .execute(&pool)
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("soft-deleted, never removed"),
                "{err}"
            );

            delete_flashcard(&pool, cards[1], at(NOW)).await.unwrap();
            let before = stored_card(&pool, cards[1]).await;
            // A deleted card is frozen: every column but `deleted_at` is
            // refused while it is deleted, so a restore can only ever give
            // back exactly what was deleted (migration 0010).
            for sql in [
                "UPDATE flashcards SET front = 'changed' WHERE id = ?1",
                "UPDATE flashcards SET back = 'changed' WHERE id = ?1",
                "UPDATE flashcards SET due = '2030-01-01T00:00:00.000Z' WHERE id = ?1",
                "UPDATE flashcards SET fsrs_state = 'Review' WHERE id = ?1",
                "UPDATE flashcards SET fsrs_stability = 9.0 WHERE id = ?1",
                "UPDATE flashcards SET fsrs_difficulty = 9.0 WHERE id = ?1",
                "UPDATE flashcards SET last_review = '2030-01-01T00:00:00.000Z' WHERE id = ?1",
                "UPDATE flashcards SET reps = reps + 1 WHERE id = ?1",
                "UPDATE flashcards SET lapses = lapses + 1 WHERE id = ?1",
                "UPDATE flashcards SET created_at = '2030-01-01T00:00:00.000Z' WHERE id = ?1",
                "UPDATE flashcards SET updated_at = '2030-01-01T00:00:00.000Z' WHERE id = ?1",
                // The one that matters most: a deleted card can't be moved to
                // another deck (deck 1 is the sample deck) and restored there.
                "UPDATE flashcards SET deck_id = 1 WHERE id = ?1",
            ] {
                let err = sqlx::query(sql)
                    .bind(cards[1])
                    .execute(&pool)
                    .await
                    .unwrap_err();
                assert!(
                    err.to_string().contains("flashcard is deleted"),
                    "{sql}: {err}"
                );
            }

            // Deleting again can't move the time the card has been showing.
            let err = sqlx::query(
                "UPDATE flashcards SET deleted_at = '2030-01-01T00:00:00.000Z' WHERE id = ?1",
            )
            .bind(cards[1])
            .execute(&pool)
            .await
            .unwrap_err();
            assert!(err.to_string().contains("already deleted"), "{err}");

            // A card is never born deleted: that would be a card nobody wrote,
            // appearing straight into a deck's deleted history.
            let err = sqlx::query(
                "INSERT INTO flashcards (deck_id, front, back, deleted_at)
                 VALUES (?1, 'X', 'x', '2026-09-15T12:00:00.000Z')",
            )
            .bind(deck)
            .execute(&pool)
            .await
            .unwrap_err();
            assert!(
                err.to_string().contains("cannot already be deleted"),
                "{err}"
            );

            assert_eq!(stored_card(&pool, cards[1]).await, before);
            assert!(deleted_at(&pool, cards[1]).await.is_some());
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 3);

            // Clearing `deleted_at` is the one change a deleted card allows:
            // it is the restore, and it is accepted.
            sqlx::query("UPDATE flashcards SET deleted_at = NULL WHERE id = ?1")
                .bind(cards[1])
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(deleted_at(&pool, cards[1]).await, None);
            assert_eq!(stored_card(&pool, cards[1]).await, before);
        });
    }

    #[test]
    fn edits_and_deletions_survive_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("card-management");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");
            let now = at(NOW);

            let pool = open_file(&path).await.unwrap();
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("Old", "old"), ("Gone", "gone")]).await;
            update_flashcard(&pool, cards[0], "New\nfront", "new back", now)
                .await
                .unwrap();
            delete_flashcard(&pool, cards[1], now).await.unwrap();
            let before = snapshot(&pool).await;
            let detail = deck_detail(&pool, deck, now).await.unwrap();
            pool.close().await;

            assert_eq!(
                detail.cards,
                vec![DeckCard {
                    id: cards[0],
                    front: "New\nfront".into(),
                    back: "new back".into(),
                    cloze: None,
                    occlusion: None,
                }]
            );
            // The seed card plus both created cards, the deleted one included.
            assert_eq!(before.1.len(), 3);

            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(snapshot(&pool).await, before);
                assert_eq!(deck_detail(&pool, deck, now).await.unwrap(), detail);
                assert_eq!(
                    deleted_at(&pool, cards[1]).await.as_deref(),
                    Some("2026-09-15T12:00:00.000Z")
                );
                pool.close().await;
            }

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    type FullDeckRow = (i64, String, Option<String>, bool, String, Option<String>);

    /// Every column of every deck.
    async fn all_decks(pool: &SqlitePool) -> Vec<FullDeckRow> {
        sqlx::query_as(
            "SELECT id, name, description, is_sample, created_at, archived_at
             FROM decks ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    type FullSessionRow = (i64, i64, String, Option<String>, i64);

    /// Every column of every session.
    async fn all_sessions(pool: &SqlitePool) -> Vec<FullSessionRow> {
        sqlx::query_as(
            "SELECT id, deck_id, started_at, ended_at, cards_reviewed
             FROM sessions ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// Every card row in full, including which deck it's in and `deleted_at`.
    async fn all_card_rows(pool: &SqlitePool) -> Vec<(StoredCard, Option<String>)> {
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM flashcards ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
        let mut rows = Vec::new();
        for id in ids {
            rows.push((stored_card(pool, id).await, deleted_at(pool, id).await));
        }
        rows
    }

    async fn archived_at(pool: &SqlitePool, deck: i64) -> Option<String> {
        sqlx::query_scalar("SELECT archived_at FROM decks WHERE id = ?1")
            .bind(deck)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn dashboard_ids(decks: Vec<study::DeckSummary>) -> Vec<i64> {
        decks.into_iter().map(|deck| deck.id).collect()
    }

    #[test]
    fn renaming_a_deck_keeps_its_id_cards_history_and_sessions() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let deck = create_deck(&pool, "Biology", Some("Cells"), now)
                .await
                .unwrap()
                .id;
            for (front, back) in [("A", "a"), ("B", "b")] {
                create_flashcard(&pool, deck, front, back, now)
                    .await
                    .unwrap();
            }
            let cards = card_ids(&pool, deck).await;

            // Real history first: a review, and a session that's still open.
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            let cards_before = all_card_rows(&pool).await;
            let logs_before = review_logs(&pool).await;
            let sessions_before = all_sessions(&pool).await;
            let deck_before = all_decks(&pool).await;

            let later = now + TimeDelta::hours(1);
            let detail = rename_deck(&pool, deck, "  Cell biology \n", later)
                .await
                .unwrap();
            assert_eq!(
                (
                    detail.id,
                    detail.name.as_str(),
                    detail.description.as_deref()
                ),
                (deck, "Cell biology", Some("Cells"))
            );
            assert_eq!((detail.card_count, detail.due_count), (2, 1));
            assert_eq!(deck_detail(&pool, deck, later).await.unwrap(), detail);

            // Only the name changed: same id, description, and creation time,
            // and every card (FSRS columns included), log, and session as before.
            let mut expected_decks = deck_before;
            expected_decks[1].1 = "Cell biology".into();
            assert_eq!(all_decks(&pool).await, expected_decks);
            assert_eq!(all_card_rows(&pool).await, cards_before);
            assert_eq!(review_logs(&pool).await, logs_before);
            assert_eq!(all_sessions(&pool).await, sessions_before);

            // The dashboard shows the new name, and the open session resumes.
            let listed: Vec<(i64, String)> = study::decks(&pool, later)
                .await
                .unwrap()
                .into_iter()
                .map(|d| (d.id, d.name))
                .collect();
            assert_eq!(listed[1], (deck, "Cell biology".to_string()));
            assert_eq!(
                study::start_session(&pool, deck, later).await.unwrap(),
                StartedSession::Started {
                    session_id: session
                }
            );

            // Changing only letter case, or keeping the same name, is allowed.
            for name in ["CELL BIOLOGY", "CELL BIOLOGY"] {
                let detail = rename_deck(&pool, deck, name, later).await.unwrap();
                assert_eq!((detail.id, detail.name.as_str()), (deck, name));
            }
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 2);
        });
    }

    #[test]
    fn invalid_deck_renames_are_rejected_and_change_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let deck = deck_id(&pool, "Biology").await;
            deck_id(&pool, "Écoles").await;
            let archived = deck_id(&pool, "Old notes").await;
            archive_deck(&pool, archived, now).await.unwrap();
            let before = all_decks(&pool).await;

            let too_long = "n".repeat(DECK_NAME_MAX_CHARS + 1);
            let attempts = [
                ("", InvalidInput::NameBlank),
                ("   ", InvalidInput::NameBlank),
                ("\t\r\n ", InvalidInput::NameBlank),
                ("\u{00A0}\u{2003}", InvalidInput::NameBlank),
                (too_long.as_str(), InvalidInput::NameTooLong),
                ("Écoles", InvalidInput::NameTaken),
                ("écoles", InvalidInput::NameTaken),
                ("  ÉCOLES ", InvalidInput::NameTaken),
                ("sample deck", InvalidInput::NameTaken),
                ("SAMPLE DECK", InvalidInput::NameTaken),
                // Archived decks keep their names.
                ("old NOTES", InvalidInput::NameTaken),
            ];
            for (name, expected) in attempts {
                let result = rename_deck(&pool, deck, name, now).await;
                assert_eq!(problem(result), expected, "{name:?}");
            }
            assert_eq!(all_decks(&pool).await, before);

            // Exactly at the limit is fine, even with multi-byte characters.
            let longest = "é".repeat(DECK_NAME_MAX_CHARS);
            let detail = rename_deck(&pool, deck, &longest, now).await.unwrap();
            assert_eq!(detail.name, longest);
        });
    }

    #[test]
    fn the_sample_deck_cannot_be_renamed_or_archived() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            // An unfinished session on it must stay open, too.
            let session = start(&pool, sample, now).await;
            let decks_before = all_decks(&pool).await;
            let sessions_before = all_sessions(&pool).await;

            assert!(matches!(
                rename_deck(&pool, sample, "My deck", now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                archive_deck(&pool, sample, now).await,
                Err(AuthoringError::SampleDeck)
            ));

            assert_eq!(all_decks(&pool).await, decks_before);
            assert_eq!(all_sessions(&pool).await, sessions_before);
            assert_eq!(
                study::start_session(&pool, sample, now).await.unwrap(),
                StartedSession::Started {
                    session_id: session
                }
            );
        });
    }

    #[test]
    fn missing_and_archived_decks_cannot_be_renamed_or_archived() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);

            for id in [0, -1, 999, i64::MAX] {
                assert!(
                    matches!(
                        rename_deck(&pool, id, "Name", now).await,
                        Err(AuthoringError::DeckNotFound)
                    ),
                    "rename {id}"
                );
                assert!(
                    matches!(
                        archive_deck(&pool, id, now).await,
                        Err(AuthoringError::DeckNotFound)
                    ),
                    "archive {id}"
                );
            }

            let deck = deck_id(&pool, "Biology").await;
            archive_deck(&pool, deck, now).await.unwrap();
            let before = all_decks(&pool).await;
            assert_eq!(
                archived_at(&pool, deck).await.as_deref(),
                Some("2026-09-15T12:00:00.000Z")
            );

            // Archiving is final: no rename, and archiving again keeps the first time.
            let later = now + TimeDelta::hours(1);
            assert!(matches!(
                rename_deck(&pool, deck, "Renamed", later).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert!(matches!(
                archive_deck(&pool, deck, later).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(all_decks(&pool).await, before);
        });
    }

    #[test]
    fn archiving_keeps_every_record_but_leaves_active_workflows() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;

            // History: two reviews, a soft-deleted card, and a completed session.
            let session = start(&pool, biology, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            delete_flashcard(&pool, cards[2], now).await.unwrap();
            study::record_review(&pool, session, cards[1], 0, Rating::Easy, now)
                .await
                .unwrap();
            assert!(session_row(&pool, session).await.1.is_some());

            let cards_before = all_card_rows(&pool).await;
            let logs_before = review_logs(&pool).await;
            let sessions_before = all_sessions(&pool).await;
            let chemistry_before = deck_detail(&pool, chemistry, now).await.unwrap();

            let later = now + TimeDelta::hours(1);
            archive_deck(&pool, biology, later).await.unwrap();

            // Every record is kept exactly as it was; only the deck is marked.
            assert_eq!(all_card_rows(&pool).await, cards_before);
            assert_eq!(review_logs(&pool).await, logs_before);
            assert_eq!(all_sessions(&pool).await, sessions_before);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 3);
            assert_eq!(
                archived_at(&pool, biology).await.as_deref(),
                Some("2026-09-15T13:00:00.000Z")
            );

            // It's listed only as history, counting its two remaining active cards.
            assert_eq!(
                archived_decks(&pool).await.unwrap(),
                vec![ArchivedDeck {
                    id: biology,
                    name: "Biology".into(),
                    description: None,
                    card_count: 2,
                    archived_at: "2026-09-15T13:00:00.000Z".into(),
                }]
            );

            // A year on, its cards are due again, but it stays out of every
            // active workflow.
            let much_later = now + TimeDelta::days(365);
            assert_eq!(
                dashboard_ids(study::decks(&pool, much_later).await.unwrap()),
                vec![sample, chemistry]
            );
            assert!(matches!(
                study::start_session(&pool, biology, much_later).await,
                Err(StudyError::DeckArchived)
            ));
            assert!(matches!(
                study::record_review(&pool, session, cards[0], 1, Rating::Good, much_later).await,
                Err(StudyError::Stale)
            ));
            assert!(matches!(
                deck_detail(&pool, biology, much_later).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert!(matches!(
                create_flashcard(&pool, biology, "q", "a", much_later).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert!(matches!(
                update_flashcard(&pool, cards[0], "q", "a", much_later).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert!(matches!(
                delete_flashcard(&pool, cards[1], much_later).await,
                Err(AuthoringError::DeckArchived)
            ));

            // None of those refused attempts changed anything, here or elsewhere.
            assert_eq!(all_card_rows(&pool).await, cards_before);
            assert_eq!(review_logs(&pool).await, logs_before);
            assert_eq!(all_sessions(&pool).await, sessions_before);
            assert_eq!(
                deck_detail(&pool, chemistry, now).await.unwrap(),
                chemistry_before
            );
        });
    }

    #[test]
    fn archived_decks_are_listed_newest_first_with_their_details() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);

            // Two decks to archive, one of them described, plus one that stays
            // active and so must never appear in the history list.
            let biology = create_deck(&pool, "Biology", Some(" Cells and genes "), now)
                .await
                .unwrap()
                .id;
            for (front, back) in [("A", "a"), ("B", "b")] {
                create_flashcard(&pool, biology, front, back, now)
                    .await
                    .unwrap();
            }
            // A deleted card is kept but never counted as one of the cards kept.
            delete_flashcard(&pool, card_ids(&pool, biology).await[1], now)
                .await
                .unwrap();
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;
            let (physics, _) = deck_with_cards(&pool, "Physics", &[("F", "ma")]).await;

            assert_eq!(archived_decks(&pool).await.unwrap(), vec![]);

            // Chemistry first, Biology an hour later, so Biology is the newest.
            archive_deck(&pool, chemistry, now + TimeDelta::hours(1))
                .await
                .unwrap();
            archive_deck(&pool, biology, now + TimeDelta::hours(2))
                .await
                .unwrap();

            assert_eq!(
                archived_decks(&pool).await.unwrap(),
                vec![
                    ArchivedDeck {
                        id: biology,
                        name: "Biology".into(),
                        // Trimmed when the deck was created, and kept verbatim.
                        description: Some("Cells and genes".into()),
                        card_count: 1,
                        archived_at: "2026-09-15T14:00:00.000Z".into(),
                    },
                    ArchivedDeck {
                        id: chemistry,
                        name: "Chemistry".into(),
                        description: None,
                        card_count: 1,
                        archived_at: "2026-09-15T13:00:00.000Z".into(),
                    },
                ]
            );
            // The active deck is on the dashboard and out of the history list.
            assert_eq!(
                dashboard_ids(study::decks(&pool, now).await.unwrap()),
                vec![sample_deck_id(&pool).await, physics]
            );
        });
    }

    #[test]
    fn archiving_finishes_an_unfinished_session_in_the_same_transaction() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            assert_eq!(session_row(&pool, session).await, (deck, None, 1));

            let later = now + TimeDelta::minutes(5);
            archive_deck(&pool, deck, later).await.unwrap();

            // The session ends at the archive time, keeping its review count.
            let ended = Some("2026-09-15T12:05:00.000Z".to_string());
            assert_eq!(session_row(&pool, session).await, (deck, ended.clone(), 1));
            assert_eq!(archived_at(&pool, deck).await, ended);

            // The card still on screen can't be rated; the session reads as complete.
            assert!(matches!(
                study::record_review(&pool, session, cards[1], 0, Rating::Good, later).await,
                Err(StudyError::Stale)
            ));
            assert!(matches!(
                study::session_card(&pool, session, later).await.unwrap(),
                SessionCard::Completed { cards_reviewed: 1 }
            ));
            assert!(matches!(
                study::start_session(&pool, deck, later).await,
                Err(StudyError::DeckArchived)
            ));
            assert_eq!(session_row(&pool, session).await, (deck, ended, 1));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 1);
            assert_eq!(stored_card(&pool, cards[1]).await.8, 0);
        });
    }

    #[test]
    fn a_failed_archive_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();

            // Make the deck update fail, after the session has been finished.
            sqlx::query(
                "CREATE TRIGGER fail_archive BEFORE UPDATE OF archived_at ON decks
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            let later = now + TimeDelta::minutes(5);
            assert!(matches!(
                archive_deck(&pool, deck, later).await,
                Err(AuthoringError::Internal(_))
            ));
            // Rolled back together: the session is still open and the deck active.
            assert_eq!(session_row(&pool, session).await, (deck, None, 1));
            assert_eq!(archived_at(&pool, deck).await, None);

            sqlx::query("DROP TRIGGER fail_archive")
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(
                study::start_session(&pool, deck, later).await.unwrap(),
                StartedSession::Started {
                    session_id: session
                }
            );
            archive_deck(&pool, deck, later).await.unwrap();
            assert!(session_row(&pool, session).await.1.is_some());
        });
    }

    /// Runs `sql` with `id` bound as `?1` and checks the database refuses it
    /// with `reason`.
    async fn refused(pool: &SqlitePool, sql: &'static str, id: i64, reason: &str) {
        let err = sqlx::query(sql).bind(id).execute(pool).await.unwrap_err();
        assert!(err.to_string().contains(reason), "{sql}: {err}");
    }

    #[test]
    fn the_database_itself_enforces_the_archiving_rules() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("C", "c")]).await;
            let archive_sql =
                "UPDATE decks SET archived_at = '2026-09-15T12:00:00.000Z' WHERE id = ?1";

            // Writes that skip the Rust code entirely.
            refused(
                &pool,
                "DELETE FROM decks WHERE id = ?1",
                chemistry,
                "decks are archived, never removed",
            )
            .await;
            refused(
                &pool,
                archive_sql,
                sample,
                "the sample deck cannot be archived",
            )
            .await;
            let session = start(&pool, biology, now).await;
            refused(
                &pool,
                archive_sql,
                biology,
                "deck has an unfinished session",
            )
            .await;
            assert_eq!(archived_at(&pool, biology).await, None);

            // Archived the Rust way (which ends the session), the deck is frozen.
            archive_deck(&pool, biology, now).await.unwrap();
            let decks_before = all_decks(&pool).await;
            let cards_before = all_card_rows(&pool).await;
            let sessions_before = all_sessions(&pool).await;
            let writes: [(&'static str, i64, &str); 12] = [
                (
                    "UPDATE decks SET name = 'Renamed' WHERE id = ?1",
                    biology,
                    "deck is archived",
                ),
                (
                    "UPDATE decks SET description = 'Rewritten' WHERE id = ?1",
                    biology,
                    "deck is archived",
                ),
                (
                    "UPDATE decks SET created_at = '2000-01-01T00:00:00.000Z' WHERE id = ?1",
                    biology,
                    "deck is archived",
                ),
                // The freeze on `is_sample` is what keeps an archived deck
                // from turning itself into the sample deck (or the reverse),
                // so it is checked like the rest of the columns.
                (
                    "UPDATE decks SET is_sample = 1 WHERE id = ?1",
                    biology,
                    "deck is archived",
                ),
                // Clearing `archived_at` is the one allowed update (migration
                // 0009), so what stays refused here is moving the archive time
                // rather than clearing it.
                (
                    "UPDATE decks SET archived_at = '2026-09-16T12:00:00.000Z' WHERE id = ?1",
                    biology,
                    "deck is already archived",
                ),
                (
                    "DELETE FROM decks WHERE id = ?1",
                    biology,
                    "decks are archived, never removed",
                ),
                (
                    "INSERT INTO flashcards (deck_id, front, back) VALUES (?1, 'q', 'a')",
                    biology,
                    "deck is archived",
                ),
                (
                    "UPDATE flashcards SET front = 'changed' WHERE id = ?1",
                    cards[0],
                    "deck is archived",
                ),
                (
                    "UPDATE flashcards SET deleted_at = '2026-09-15T12:00:00.000Z' WHERE id = ?1",
                    cards[1],
                    "deck is archived",
                ),
                (
                    "UPDATE flashcards SET deck_id = ?1
                     WHERE deck_id = (SELECT id FROM decks WHERE name = 'Chemistry')",
                    biology,
                    "deck is archived",
                ),
                (
                    "INSERT INTO sessions (deck_id, started_at)
                     VALUES (?1, '2026-09-15T12:00:00.000Z')",
                    biology,
                    "deck is archived",
                ),
                (
                    "UPDATE sessions SET ended_at = NULL WHERE id = ?1",
                    session,
                    "deck is archived",
                ),
            ];
            for (sql, id, reason) in writes {
                refused(&pool, sql, id, reason).await;
            }
            assert_eq!(all_decks(&pool).await, decks_before);
            assert_eq!(all_card_rows(&pool).await, cards_before);
            assert_eq!(all_sessions(&pool).await, sessions_before);

            // A deck can't be born archived either, so nothing can appear
            // straight into the archive history.
            let born_archived = sqlx::query(
                "INSERT INTO decks (name, is_sample, created_at, archived_at)
                 VALUES ('Born archived', 0, '2026-09-15T12:00:00.000Z',
                         '2026-09-15T12:00:00.000Z')",
            )
            .execute(&pool)
            .await
            .unwrap_err();
            assert!(
                born_archived
                    .to_string()
                    .contains("a new deck cannot already be archived"),
                "{born_archived}"
            );

            // Clearing `archived_at` is the single update an archived deck
            // accepts, and it changes nothing else in the database: every
            // other deck column is exactly what it was before.
            let identity_sql = "SELECT id, name, description, is_sample, created_at
                                FROM decks ORDER BY id";
            type DeckIdentity = (i64, String, Option<String>, bool, String);
            let identity_before: Vec<DeckIdentity> =
                sqlx::query_as(identity_sql).fetch_all(&pool).await.unwrap();
            sqlx::query("UPDATE decks SET archived_at = NULL WHERE id = ?1")
                .bind(biology)
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(archived_at(&pool, biology).await, None);
            let identity_after: Vec<DeckIdentity> =
                sqlx::query_as(identity_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(identity_after, identity_before);
            assert_eq!(all_card_rows(&pool).await, cards_before);
            assert_eq!(all_sessions(&pool).await, sessions_before);
        });
    }

    #[test]
    fn archiving_a_deck_again_records_the_new_time_not_the_old_one() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let first = at(NOW);
            let (biology, _) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;

            archive_deck(&pool, biology, first).await.unwrap();
            assert_eq!(
                archived_at(&pool, biology).await.as_deref(),
                Some("2026-09-15T12:00:00.000Z")
            );

            // While it stays archived the date is fixed: archiving again is
            // refused outright and can't move it.
            let later = first + TimeDelta::hours(3);
            assert!(matches!(
                archive_deck(&pool, biology, later).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(
                archived_at(&pool, biology).await.as_deref(),
                Some("2026-09-15T12:00:00.000Z")
            );

            // Unarchiving and archiving again is a fresh archiving, so it
            // records when that happened rather than keeping the old date.
            unarchive_deck(&pool, biology).await.unwrap();
            archive_deck(&pool, biology, later).await.unwrap();
            assert_eq!(
                archived_at(&pool, biology).await.as_deref(),
                Some("2026-09-15T15:00:00.000Z")
            );
            // The history list shows that newer date, and the deck is listed
            // once, not twice.
            let listed: Vec<(i64, String)> = archived_decks(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|deck| (deck.id, deck.archived_at))
                .collect();
            assert_eq!(listed, vec![(biology, "2026-09-15T15:00:00.000Z".into())]);
        });
    }

    #[test]
    fn the_database_itself_enforces_the_unarchiving_rules() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let (biology, _) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;
            archive_deck(&pool, biology, now).await.unwrap();

            // An unarchive must not resurrect an abandoned session. A deck can
            // never be archived with one open, so this is only reachable by
            // forcing a session open behind the triggers' back.
            sqlx::query(
                "INSERT INTO sessions (deck_id, started_at, ended_at)
                 VALUES (?1, '2026-09-15T12:00:00.000Z', '2026-09-15T12:00:30.000Z')",
            )
            .bind(biology)
            .execute(&pool)
            .await
            .unwrap();
            let forced: i64 =
                sqlx::query_scalar("SELECT id FROM sessions WHERE deck_id = ?1 ORDER BY id DESC")
                    .bind(biology)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            sqlx::query("DROP TRIGGER sessions_archived_deck_update")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE sessions SET ended_at = NULL WHERE id = ?1")
                .bind(forced)
                .execute(&pool)
                .await
                .unwrap();

            refused(
                &pool,
                "UPDATE decks SET archived_at = NULL WHERE id = ?1",
                biology,
                "deck has an unfinished session",
            )
            .await;
            assert!(archived_at(&pool, biology).await.is_some());
            assert!(matches!(
                unarchive_deck(&pool, biology).await,
                Err(AuthoringError::Internal(_))
            ));
            assert!(archived_at(&pool, biology).await.is_some());

            // With the session closed again, the same write is accepted.
            sqlx::query("UPDATE sessions SET ended_at = ?2 WHERE id = ?1")
                .bind(forced)
                .bind("2026-09-15T12:01:00.000Z")
                .execute(&pool)
                .await
                .unwrap();
            unarchive_deck(&pool, biology).await.unwrap();
            assert_eq!(archived_at(&pool, biology).await, None);

            // The sample deck can't be archived, so it is never unarchivable.
            assert!(matches!(
                unarchive_deck(&pool, sample).await,
                Err(AuthoringError::SampleDeck)
            ));
        });
    }

    #[test]
    fn unarchiving_restores_the_deck_with_every_record_intact() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;

            // A reviewed card (so it has FSRS state, a due date, and a log), a
            // soft-deleted card, and an untouched new card.
            let session = start(&pool, biology, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            delete_flashcard(&pool, cards[1], now).await.unwrap();

            archive_deck(&pool, biology, now + TimeDelta::minutes(1))
                .await
                .unwrap();
            // Every deck column except `archived_at`, which is the one column
            // an unarchive is allowed to change.
            let identity_sql = "SELECT id, name, description, is_sample, created_at
                                FROM decks ORDER BY id";
            type DeckIdentity = (i64, String, Option<String>, bool, String);
            let identity_before: Vec<DeckIdentity> =
                sqlx::query_as(identity_sql).fetch_all(&pool).await.unwrap();
            let cards_before = all_card_rows(&pool).await;
            let sessions_before = all_sessions(&pool).await;
            let logs_before = review_logs(&pool).await;

            unarchive_deck(&pool, biology).await.unwrap();

            // The deck is active again, with the same id, name, description,
            // cards, card states, sessions, and review logs. Only
            // `archived_at` differs from the archived snapshot.
            assert_eq!(archived_at(&pool, biology).await, None);
            let identity_after: Vec<DeckIdentity> =
                sqlx::query_as(identity_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(identity_after, identity_before);
            assert_eq!(all_card_rows(&pool).await, cards_before);
            assert_eq!(all_sessions(&pool).await, sessions_before);
            assert_eq!(review_logs(&pool).await, logs_before);

            // It's back on the dashboard and can be opened and reviewed, and
            // it no longer appears in the archive history.
            assert_eq!(
                dashboard_ids(study::decks(&pool, now).await.unwrap()),
                vec![sample, biology]
            );
            assert!(archived_decks(&pool).await.unwrap().is_empty());
            let detail = deck_detail(&pool, biology, now).await.unwrap();
            assert_eq!(detail.name, "Biology");
            // The deleted card is still deleted: two active cards, not three.
            assert_eq!(detail.card_count, 2);
            assert_eq!(
                detail.cards.iter().map(|card| card.id).collect::<Vec<_>>(),
                vec![cards[0], cards[2]]
            );
            assert!(deleted_at(&pool, cards[1]).await.is_some());

            // Unarchiving started no session: the only one is the finished one
            // from before the archive.
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM sessions WHERE ended_at IS NULL"
                )
                .await,
                0
            );
        });
    }

    #[test]
    fn unarchived_cards_come_back_due_only_on_their_own_dates() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;

            // Review one card so it is scheduled into the future; the other
            // stays a new card, which is due immediately.
            let session = start(&pool, biology, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Easy, now)
                .await
                .unwrap();
            let due_after_review: Option<String> =
                sqlx::query_scalar("SELECT due FROM flashcards WHERE id = ?1")
                    .bind(cards[0])
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let due_date = at(due_after_review.as_deref().unwrap());

            archive_deck(&pool, biology, now + TimeDelta::minutes(1))
                .await
                .unwrap();
            unarchive_deck(&pool, biology).await.unwrap();

            // Straight after the unarchive only the new card is due; the
            // reviewed one keeps the date FSRS gave it.
            assert_eq!(deck_detail(&pool, biology, now).await.unwrap().due_count, 1);
            let first = start(&pool, biology, now).await;
            let SessionCard::Due { card, .. } =
                study::session_card(&pool, first, now).await.unwrap()
            else {
                panic!("the new card should be due");
            };
            assert_eq!(card.id, cards[1]);

            // Once its due date arrives, the reviewed card is offered too, with
            // the repetition count it already had.
            let later = due_date + TimeDelta::minutes(1);
            assert_eq!(
                deck_detail(&pool, biology, later).await.unwrap().due_count,
                2
            );
            let due_ids: Vec<i64> = sqlx::query_scalar(
                "SELECT id FROM flashcards
                 WHERE deck_id = ?1 AND deleted_at IS NULL AND (due IS NULL OR due <= ?2)
                 ORDER BY id",
            )
            .bind(biology)
            .bind(to_db_time(later))
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(due_ids, vec![cards[0], cards[1]]);
        });
    }

    #[test]
    fn missing_active_and_sample_decks_cannot_be_unarchived() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let sample = sample_deck_id(&pool).await;
            let biology = deck_id(&pool, "Biology").await;
            let before = all_decks(&pool).await;

            for id in [0, -1, 999, i64::MAX] {
                assert!(
                    matches!(
                        unarchive_deck(&pool, id).await,
                        Err(AuthoringError::DeckNotFound)
                    ),
                    "unarchive {id}"
                );
            }
            // An active deck has nothing to unarchive, and the sample deck is
            // refused as the sample deck rather than as an active one.
            assert!(matches!(
                unarchive_deck(&pool, biology).await,
                Err(AuthoringError::DeckNotArchived)
            ));
            assert!(matches!(
                unarchive_deck(&pool, sample).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert_eq!(all_decks(&pool).await, before);

            // Unarchiving twice: the second is refused and writes nothing.
            archive_deck(&pool, biology, now).await.unwrap();
            unarchive_deck(&pool, biology).await.unwrap();
            let after = all_decks(&pool).await;
            assert!(matches!(
                unarchive_deck(&pool, biology).await,
                Err(AuthoringError::DeckNotArchived)
            ));
            assert_eq!(all_decks(&pool).await, after);
        });
    }

    #[test]
    fn an_archived_name_stays_reserved_and_unarchiving_never_clashes() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let biology = deck_id(&pool, "Biology").await;
            archive_deck(&pool, biology, now).await.unwrap();

            // An archived deck's name is still taken, so nothing can be
            // created that the unarchive would then collide with.
            assert_eq!(
                problem(create_deck(&pool, "biology", None, now).await),
                InvalidInput::NameTaken
            );
            unarchive_deck(&pool, biology).await.unwrap();
            assert_eq!(
                deck_detail(&pool, biology, now).await.unwrap().name,
                "Biology"
            );

            // Back to an ordinary active deck: it can be renamed, which frees
            // the old name, and archived and unarchived again.
            rename_deck(&pool, biology, "Cell biology", now)
                .await
                .unwrap();
            let chemistry = create_deck(&pool, "Biology", None, now).await.unwrap().id;
            archive_deck(&pool, biology, now).await.unwrap();
            unarchive_deck(&pool, biology).await.unwrap();
            assert_eq!(
                deck_detail(&pool, biology, now).await.unwrap().name,
                "Cell biology"
            );
            assert_eq!(
                deck_detail(&pool, chemistry, now).await.unwrap().name,
                "Biology"
            );
        });
    }

    #[test]
    fn a_failed_unarchive_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (biology, _) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;
            archive_deck(&pool, biology, now).await.unwrap();
            let before = all_decks(&pool).await;

            sqlx::query(
                "CREATE TRIGGER fail_unarchive BEFORE UPDATE OF archived_at ON decks
                 WHEN NEW.archived_at IS NULL
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            assert!(matches!(
                unarchive_deck(&pool, biology).await,
                Err(AuthoringError::Internal(_))
            ));
            assert_eq!(all_decks(&pool).await, before);
            assert!(archived_at(&pool, biology).await.is_some());

            sqlx::query("DROP TRIGGER fail_unarchive")
                .execute(&pool)
                .await
                .unwrap();
            unarchive_deck(&pool, biology).await.unwrap();
            assert_eq!(archived_at(&pool, biology).await, None);
        });
    }

    #[test]
    fn renames_and_archives_survive_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("deck-maintenance");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");
            let now = at(NOW);

            let pool = open_file(&path).await.unwrap();
            let sample = sample_deck_id(&pool).await;
            let (biology, _) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("C", "c")]).await;
            rename_deck(&pool, biology, "Cell biology", now)
                .await
                .unwrap();
            start(&pool, chemistry, now).await;
            archive_deck(&pool, chemistry, now + TimeDelta::minutes(1))
                .await
                .unwrap();
            let decks_before = all_decks(&pool).await;
            let cards_before = all_card_rows(&pool).await;
            let sessions_before = all_sessions(&pool).await;
            pool.close().await;

            // Reopen twice: each open migrates and seeds again, which must be a no-op.
            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(all_decks(&pool).await, decks_before);
                assert_eq!(all_card_rows(&pool).await, cards_before);
                assert_eq!(all_sessions(&pool).await, sessions_before);
                assert_eq!(
                    dashboard_ids(study::decks(&pool, now).await.unwrap()),
                    vec![sample, biology]
                );
                let archived: Vec<i64> = archived_decks(&pool)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|deck| deck.id)
                    .collect();
                assert_eq!(archived, vec![chemistry]);
                assert!(matches!(
                    study::start_session(&pool, chemistry, now).await,
                    Err(StudyError::DeckArchived)
                ));
                // No duplicate sample content.
                assert_eq!(
                    count(&pool, "SELECT COUNT(*) FROM decks WHERE is_sample = 1").await,
                    1
                );
                assert_eq!(card_ids(&pool, sample).await.len(), 1);
                assert_eq!(count(&pool, "SELECT COUNT(*) FROM seed_markers").await, 1);
                pool.close().await;
            }

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn unarchives_survive_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("deck-unarchive");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");
            let now = at(NOW);

            let pool = open_file(&path).await.unwrap();
            let sample = sample_deck_id(&pool).await;
            let (biology, biology_cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("C", "c")]).await;

            // History worth keeping across the round trip: one reviewed card,
            // one deleted card, and a finished session.
            let session = start(&pool, biology, now).await;
            study::record_review(&pool, session, biology_cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            delete_flashcard(&pool, biology_cards[1], now)
                .await
                .unwrap();

            // Biology goes away and comes back; Chemistry stays archived.
            archive_deck(&pool, biology, now + TimeDelta::minutes(1))
                .await
                .unwrap();
            archive_deck(&pool, chemistry, now + TimeDelta::minutes(2))
                .await
                .unwrap();
            unarchive_deck(&pool, biology).await.unwrap();

            let decks_before = all_decks(&pool).await;
            let cards_before = all_card_rows(&pool).await;
            let sessions_before = all_sessions(&pool).await;
            let logs_before = review_logs(&pool).await;
            pool.close().await;

            // Reopen twice: each open migrates and seeds again, which must be a no-op.
            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(all_decks(&pool).await, decks_before);
                assert_eq!(all_card_rows(&pool).await, cards_before);
                assert_eq!(all_sessions(&pool).await, sessions_before);
                assert_eq!(review_logs(&pool).await, logs_before);

                // The unarchived deck is an ordinary active deck again, and
                // the one still archived is still history.
                assert_eq!(
                    dashboard_ids(study::decks(&pool, now).await.unwrap()),
                    vec![sample, biology]
                );
                let archived: Vec<i64> = archived_decks(&pool)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|deck| deck.id)
                    .collect();
                assert_eq!(archived, vec![chemistry]);
                assert!(matches!(
                    study::start_session(&pool, chemistry, now).await,
                    Err(StudyError::DeckArchived)
                ));

                // The deleted card stayed deleted, and nothing reopened a session.
                let detail = deck_detail(&pool, biology, now).await.unwrap();
                assert_eq!(detail.card_count, 1);
                assert_eq!(
                    count(
                        &pool,
                        "SELECT COUNT(*) FROM sessions WHERE ended_at IS NULL"
                    )
                    .await,
                    0
                );
                // No duplicate sample content.
                assert_eq!(
                    count(&pool, "SELECT COUNT(*) FROM decks WHERE is_sample = 1").await,
                    1
                );
                assert_eq!(count(&pool, "SELECT COUNT(*) FROM seed_markers").await, 1);
                pool.close().await;
            }

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Every deleted card of a deck, as the deck screen lists them.
    async fn deleted_card_ids(pool: &SqlitePool, deck: i64, now: DateTime<Utc>) -> Vec<i64> {
        deck_detail(pool, deck, now)
            .await
            .unwrap()
            .deleted_cards
            .into_iter()
            .map(|card| card.id)
            .collect()
    }

    #[test]
    fn a_deleted_card_is_listed_under_its_deck_with_its_text_and_deletion_time() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;

            let detail = delete_flashcard(&pool, cards[1], now).await.unwrap();
            assert_eq!(
                detail.deleted_cards,
                vec![DeletedCard {
                    id: cards[1],
                    front: "B".into(),
                    back: "b".into(),
                    deleted_at: "2026-09-15T12:00:00.000Z".into(),
                    cloze: None,
                    occlusion: None,
                }]
            );
            // It is in no count and in no active list.
            assert_eq!((detail.card_count, detail.due_count), (2, 2));
            let listed: Vec<i64> = detail.cards.iter().map(|card| card.id).collect();
            assert_eq!(listed, vec![cards[0], cards[2]]);
            assert_eq!(deck_detail(&pool, deck, now).await.unwrap(), detail);

            // Most recently deleted first, so the card just deleted is on top.
            delete_flashcard(&pool, cards[0], now + TimeDelta::hours(1))
                .await
                .unwrap();
            assert_eq!(
                deleted_card_ids(&pool, deck, now).await,
                vec![cards[0], cards[1]]
            );

            // One deck's deleted cards never appear under another's.
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;
            assert!(deleted_card_ids(&pool, chemistry, now).await.is_empty());
        });
    }

    #[test]
    fn restoring_a_card_brings_back_its_exact_state_and_history() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;

            // Review it first, so it has real FSRS state, a due date in the
            // future, reps, a last review, and a log row.
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            let reviewed = stored_card(&pool, cards[0]).await;
            let logs_before = review_logs(&pool).await;
            assert_eq!(logs_before.len(), 1);

            delete_flashcard(&pool, cards[0], now + TimeDelta::hours(1))
                .await
                .unwrap();
            let deleted = stored_card(&pool, cards[0]).await;

            let later = now + TimeDelta::hours(2);
            let detail = restore_flashcard(&pool, cards[0], later).await.unwrap();

            // Every scheduling column is exactly what the review left, and the
            // restore wrote no time of its own: `updated_at` is still the
            // deletion's, not `later`.
            assert_eq!(stored_card(&pool, cards[0]).await, deleted);
            assert_eq!(deleted_at(&pool, cards[0]).await, None);
            let mut expected = reviewed.clone();
            expected.11 = "2026-09-15T13:00:00.000Z".into();
            assert_eq!(stored_card(&pool, cards[0]).await, expected);
            // Spelled out, because this is the promise the feature makes.
            let (_, _, _, state, stability, difficulty, due, last_review, reps, lapses, _, _) =
                &stored_card(&pool, cards[0]).await;
            assert_eq!(state, &reviewed.3);
            assert_eq!(stability, &reviewed.4);
            assert_eq!(difficulty, &reviewed.5);
            assert_eq!(due, &reviewed.6);
            assert_eq!(last_review, &reviewed.7);
            assert_eq!((*reps, *lapses), (1, 0));

            // The review log is untouched: no row added, removed, or changed.
            assert_eq!(review_logs(&pool).await, logs_before);

            // It is back in its own deck, and in no other.
            assert_eq!(detail.id, deck);
            let listed: Vec<i64> = detail.cards.iter().map(|card| card.id).collect();
            assert_eq!(listed, vec![cards[0], cards[1]]);
            assert!(detail.deleted_cards.is_empty());
            assert_eq!(detail.card_count, 2);
            assert_eq!(deck_detail(&pool, deck, later).await.unwrap(), detail);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 3);
        });
    }

    #[test]
    fn a_restored_card_is_due_only_on_the_date_it_already_had() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;

            // Card A is reviewed, so it's scheduled into the future; B stays new.
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            let due: Option<String> =
                sqlx::query_scalar("SELECT due FROM flashcards WHERE id = ?1")
                    .bind(cards[0])
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let due = at(&due.unwrap());
            assert!(due > now, "a reviewed card should be scheduled ahead");

            delete_flashcard(&pool, cards[0], now + TimeDelta::minutes(1))
                .await
                .unwrap();
            restore_flashcard(&pool, cards[0], now + TimeDelta::minutes(2))
                .await
                .unwrap();

            // Restoring did not make it due: it is not a new card again.
            let soon = now + TimeDelta::minutes(3);
            let detail = deck_detail(&pool, deck, soon).await.unwrap();
            assert_eq!((detail.card_count, detail.due_count), (2, 1));
            let SessionCard::Due { card, .. } =
                study::session_card(&pool, session, soon).await.unwrap()
            else {
                panic!("only card B should be due");
            };
            assert_eq!(card.id, cards[1]);

            // On its own due date it comes back into the queue by itself, and
            // the session that was already open simply draws it like any other
            // due card — the restore neither started that session nor had to.
            let detail = deck_detail(&pool, deck, due).await.unwrap();
            assert_eq!(detail.due_count, 2);
            assert_eq!(
                study::start_session(&pool, deck, due).await.unwrap(),
                StartedSession::Started {
                    session_id: session
                }
            );
            let SessionCard::Due { card, .. } =
                study::session_card(&pool, session, due).await.unwrap()
            else {
                panic!("the restored card should be offered");
            };
            // Ordered by `due IS NULL, due, id`, so the restored card (which
            // has a real due date) comes before the still-new card B.
            assert_eq!(card.id, cards[0]);
            study::record_review(&pool, session, cards[0], 1, Rating::Good, due)
                .await
                .unwrap();
            assert_eq!(
                review_logs(&pool).await.len(),
                2,
                "reviewing a restored card appends to the history it kept"
            );
        });
    }

    #[test]
    fn restoring_the_last_deleted_card_needs_no_session_and_starts_none() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;

            // Deleting the deck's only due card finishes its open session.
            let session = start(&pool, deck, now).await;
            delete_flashcard(&pool, cards[0], now).await.unwrap();
            assert_eq!(
                session_row(&pool, session).await,
                (deck, Some("2026-09-15T12:00:00.000Z".into()), 0)
            );

            // Restoring brings the card back but opens no session of its own.
            let later = now + TimeDelta::hours(1);
            let detail = restore_flashcard(&pool, cards[0], later).await.unwrap();
            assert_eq!((detail.card_count, detail.due_count), (1, 1));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);
            assert_eq!(
                session_row(&pool, session).await,
                (deck, Some("2026-09-15T12:00:00.000Z".into()), 0)
            );

            // The user starts one from the deck, as for any other deck, and it
            // is a new session that offers the restored card.
            let StartedSession::Started {
                session_id: resumed,
            } = study::start_session(&pool, deck, later).await.unwrap()
            else {
                panic!("the restored card should be due");
            };
            assert_ne!(resumed, session);
            let SessionCard::Due { card, .. } =
                study::session_card(&pool, resumed, later).await.unwrap()
            else {
                panic!("the restored card should be offered");
            };
            assert_eq!(card.id, cards[0]);
        });
    }

    #[test]
    fn missing_active_and_sample_cards_cannot_be_restored() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;
            let seed_card = card_ids(&pool, sample_deck_id(&pool).await).await[0];
            let before = snapshot(&pool).await;

            for id in [0, -1, 999, i64::MAX] {
                assert!(
                    matches!(
                        restore_flashcard(&pool, id, now).await,
                        Err(AuthoringError::CardNotFound)
                    ),
                    "restore {id}"
                );
            }

            // An active card has nothing to restore.
            assert!(matches!(
                restore_flashcard(&pool, cards[0], now).await,
                Err(AuthoringError::CardNotDeleted)
            ));

            // The sample deck stays out of the restore workflow entirely, and
            // is refused by name rather than as "not deleted".
            assert!(matches!(
                restore_flashcard(&pool, seed_card, now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert_eq!(deleted_at(&pool, seed_card).await, None);
            assert_eq!(snapshot(&pool).await, before);

            // Even a sample card that somehow *is* deleted (nothing in Synapse
            // deletes one, so only a write from outside could) stays refused
            // as a sample card, not restored. This is what pins the order of
            // the checks: the sample deck is refused before the card's own
            // deleted state is even considered.
            sqlx::query("UPDATE flashcards SET deleted_at = ?1 WHERE id = ?2")
                .bind("2026-09-15T11:00:00.000Z")
                .bind(seed_card)
                .execute(&pool)
                .await
                .unwrap();
            let forced = stored_card(&pool, seed_card).await;
            assert!(matches!(
                restore_flashcard(&pool, seed_card, now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert_eq!(stored_card(&pool, seed_card).await, forced);
            assert_eq!(
                deleted_at(&pool, seed_card).await.as_deref(),
                Some("2026-09-15T11:00:00.000Z"),
                "the sample card stays exactly as it was found"
            );
            assert!(deleted_card_ids(&pool, deck, now).await.is_empty());
        });
    }

    #[test]
    fn a_card_in_an_archived_deck_is_restored_only_after_the_deck_comes_back() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            delete_flashcard(&pool, cards[0], now).await.unwrap();
            archive_deck(&pool, deck, now + TimeDelta::minutes(1))
                .await
                .unwrap();
            let before = all_card_rows(&pool).await;

            // While the deck is archived nothing in it may change, restoring
            // included: the deck must be unarchived first.
            assert!(matches!(
                restore_flashcard(&pool, cards[0], now + TimeDelta::minutes(2)).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(all_card_rows(&pool).await, before);
            assert!(deleted_at(&pool, cards[0]).await.is_some());

            // The database refuses it too. Before migration 0010 this was
            // covered by `flashcards_deleted_are_final`; now 0006's
            // `flashcards_archived_deck_update` is the only rule holding it,
            // so check that rule directly rather than by luck.
            let err = sqlx::query("UPDATE flashcards SET deleted_at = NULL WHERE id = ?1")
                .bind(cards[0])
                .execute(&pool)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("deck is archived"), "{err}");
            assert_eq!(all_card_rows(&pool).await, before);

            // Unarchiving restores no card by itself.
            unarchive_deck(&pool, deck).await.unwrap();
            assert_eq!(all_card_rows(&pool).await, before);
            assert_eq!(
                deleted_card_ids(&pool, deck, now).await,
                vec![cards[0]],
                "the card is still deleted, and listed as such"
            );

            // Now it can be restored, unchanged.
            let detail = restore_flashcard(&pool, cards[0], now + TimeDelta::hours(1))
                .await
                .unwrap();
            let listed: Vec<i64> = detail.cards.iter().map(|card| card.id).collect();
            assert_eq!(listed, vec![cards[0], cards[1]]);
            assert!(detail.deleted_cards.is_empty());
        });
    }

    #[test]
    fn restoring_a_card_again_is_refused_and_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            delete_flashcard(&pool, cards[0], now + TimeDelta::minutes(1))
                .await
                .unwrap();
            restore_flashcard(&pool, cards[0], now + TimeDelta::minutes(2))
                .await
                .unwrap();
            let after_restore = all_card_rows(&pool).await;
            let logs = review_logs(&pool).await;

            // Every later restore is refused, and each one writes nothing —
            // no log is added, and no column moves.
            for minute in 3..6 {
                assert!(matches!(
                    restore_flashcard(&pool, cards[0], now + TimeDelta::minutes(minute)).await,
                    Err(AuthoringError::CardNotDeleted)
                ));
                assert_eq!(all_card_rows(&pool).await, after_restore);
                assert_eq!(review_logs(&pool).await, logs);
            }

            // Deleting again is a fresh deletion and records the later time.
            delete_flashcard(&pool, cards[0], now + TimeDelta::hours(3))
                .await
                .unwrap();
            assert_eq!(
                deleted_at(&pool, cards[0]).await.as_deref(),
                Some("2026-09-15T15:00:00.000Z")
            );
            assert_eq!(review_logs(&pool).await, logs);
        });
    }

    #[test]
    fn a_failed_restore_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;
            delete_flashcard(&pool, cards[0], now).await.unwrap();
            let before = all_card_rows(&pool).await;

            sqlx::query(
                "CREATE TRIGGER fail_restore BEFORE UPDATE OF deleted_at ON flashcards
                 WHEN NEW.deleted_at IS NULL
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            assert!(matches!(
                restore_flashcard(&pool, cards[0], now).await,
                Err(AuthoringError::Internal(_))
            ));
            assert_eq!(all_card_rows(&pool).await, before);
            assert!(deleted_at(&pool, cards[0]).await.is_some());
            assert_eq!(deleted_card_ids(&pool, deck, now).await, vec![cards[0]]);

            sqlx::query("DROP TRIGGER fail_restore")
                .execute(&pool)
                .await
                .unwrap();
            restore_flashcard(&pool, cards[0], now).await.unwrap();
            assert_eq!(deleted_at(&pool, cards[0]).await, None);
        });
    }

    #[test]
    fn restores_survive_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("card-restore");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");
            let now = at(NOW);

            let pool = open_file(&path).await.unwrap();
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            // One card deleted and restored, one left deleted.
            delete_flashcard(&pool, cards[0], now + TimeDelta::minutes(1))
                .await
                .unwrap();
            delete_flashcard(&pool, cards[1], now + TimeDelta::minutes(2))
                .await
                .unwrap();
            restore_flashcard(&pool, cards[0], now + TimeDelta::minutes(3))
                .await
                .unwrap();

            let cards_before = all_card_rows(&pool).await;
            let logs_before = review_logs(&pool).await;
            let detail = deck_detail(&pool, deck, now).await.unwrap();
            pool.close().await;

            let listed: Vec<i64> = detail.cards.iter().map(|card| card.id).collect();
            assert_eq!(listed, vec![cards[0], cards[2]]);
            let deleted: Vec<i64> = detail.deleted_cards.iter().map(|card| card.id).collect();
            assert_eq!(deleted, vec![cards[1]]);

            // Reopen twice: each open migrates and seeds again, which must be
            // a no-op, and must not resurrect or re-delete anything.
            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(all_card_rows(&pool).await, cards_before);
                assert_eq!(review_logs(&pool).await, logs_before);
                assert_eq!(deck_detail(&pool, deck, now).await.unwrap(), detail);
                assert_eq!(deleted_at(&pool, cards[0]).await, None);
                assert_eq!(
                    deleted_at(&pool, cards[1]).await.as_deref(),
                    Some("2026-09-15T12:02:00.000Z")
                );
                pool.close().await;
            }

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn cards_deleted_before_this_migration_can_be_restored_afterwards() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            // A database left by the Synapse before card restore existed.
            migrations_up_to(9).run(&pool).await.unwrap();
            let deck: i64 = sqlx::query_scalar(
                "INSERT INTO decks (name, is_sample) VALUES ('Biology', 0) RETURNING id",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            let card: i64 = sqlx::query_scalar(
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, due, reps, lapses)
                 VALUES (?1, 'A', 'a', 'Review', '2026-10-01T12:00:00.000Z', 3, 1)
                 RETURNING id",
            )
            .bind(deck)
            .fetch_one(&pool)
            .await
            .unwrap();
            sqlx::query(
                "UPDATE flashcards SET deleted_at = '2026-09-01T12:00:00.000Z' WHERE id = ?1",
            )
            .bind(card)
            .execute(&pool)
            .await
            .unwrap();
            let before = stored_card(&pool, card).await;

            // Upgrading keeps the card exactly as it was, still deleted.
            migrate_and_seed(&pool).await.unwrap();
            assert_eq!(stored_card(&pool, card).await, before);
            assert_eq!(
                deleted_at(&pool, card).await.as_deref(),
                Some("2026-09-01T12:00:00.000Z")
            );

            // And it can now be restored, with the schedule it had all along.
            let now = at(NOW);
            let detail = restore_flashcard(&pool, card, now).await.unwrap();
            assert_eq!(stored_card(&pool, card).await, before);
            assert_eq!(deleted_at(&pool, card).await, None);
            assert_eq!(detail.card_count, 1);
            // Due in October, so still not due in September.
            assert_eq!(detail.due_count, 0);
        });
    }

    #[test]
    fn restoring_all_brings_every_deleted_card_back_exactly() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let (chemistry, other) = deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;

            // Card A gets real FSRS state and a log row; B stays new. The
            // session stays open, since C is still due.
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            delete_flashcard(&pool, cards[0], now + TimeDelta::minutes(1))
                .await
                .unwrap();
            delete_flashcard(&pool, cards[1], now + TimeDelta::minutes(2))
                .await
                .unwrap();
            // Another deck's deleted card is not this deck's to restore.
            delete_flashcard(&pool, other[0], now + TimeDelta::minutes(3))
                .await
                .unwrap();
            let deleted = [
                stored_card(&pool, cards[0]).await,
                stored_card(&pool, cards[1]).await,
            ];
            let other_before = (
                stored_card(&pool, other[0]).await,
                deleted_at(&pool, other[0]).await,
            );
            let logs = review_logs(&pool).await;
            let sessions = all_sessions(&pool).await;

            let later = now + TimeDelta::hours(1);
            let outcome = restore_deleted_cards(&pool, deck, 2, later).await.unwrap();
            assert_eq!(outcome.restored, 2);

            // Each card lost its `deleted_at` and nothing else: same text,
            // FSRS state, due date, reps, lapses, last review, and times.
            for (card, before) in cards[..2].iter().zip(&deleted) {
                assert_eq!(&stored_card(&pool, *card).await, before);
                assert_eq!(deleted_at(&pool, *card).await, None);
            }
            assert_eq!(review_logs(&pool).await, logs);
            assert_eq!(all_sessions(&pool).await, sessions);

            // Back in their own deck; the other deck's card is still deleted.
            let listed: Vec<i64> = outcome.deck.cards.iter().map(|card| card.id).collect();
            assert_eq!(listed, cards);
            assert!(outcome.deck.deleted_cards.is_empty());
            // A is scheduled ahead, so only B and C are due.
            assert_eq!((outcome.deck.card_count, outcome.deck.due_count), (3, 2));
            assert_eq!(deck_detail(&pool, deck, later).await.unwrap(), outcome.deck);
            assert_eq!(
                (
                    stored_card(&pool, other[0]).await,
                    deleted_at(&pool, other[0]).await
                ),
                other_before
            );
            assert_eq!(deleted_card_ids(&pool, chemistry, later).await, other);
        });
    }

    #[test]
    fn restoring_all_refuses_missing_sample_and_archived_decks_and_writes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let sample = sample_deck_id(&pool).await;
            let seed_card = card_ids(&pool, sample).await[0];
            delete_flashcard(&pool, cards[0], now).await.unwrap();
            delete_flashcard(&pool, cards[1], now).await.unwrap();

            // As with a single restore, a sample card that is somehow deleted
            // stays refused as a sample card.
            sqlx::query("UPDATE flashcards SET deleted_at = ?1 WHERE id = ?2")
                .bind("2026-09-15T11:00:00.000Z")
                .bind(seed_card)
                .execute(&pool)
                .await
                .unwrap();
            archive_deck(&pool, deck, now + TimeDelta::minutes(1))
                .await
                .unwrap();
            let before = all_card_rows(&pool).await;

            for id in [0, -1, 999, i64::MAX] {
                assert!(
                    matches!(
                        restore_deleted_cards(&pool, id, 2, now).await,
                        Err(AuthoringError::DeckNotFound)
                    ),
                    "restore all in deck {id}"
                );
            }
            assert!(matches!(
                restore_deleted_cards(&pool, sample, 1, now).await,
                Err(AuthoringError::SampleDeck)
            ));
            // An archived deck must be unarchived first.
            assert!(matches!(
                restore_deleted_cards(&pool, deck, 2, now).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(all_card_rows(&pool).await, before);

            unarchive_deck(&pool, deck).await.unwrap();
            let outcome = restore_deleted_cards(&pool, deck, 2, now).await.unwrap();
            assert_eq!(outcome.restored, 2);
            assert!(deleted_at(&pool, seed_card).await.is_some());
        });
    }

    #[test]
    fn restoring_all_from_an_out_of_date_list_is_refused_and_writes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;

            // Nothing deleted at all.
            assert!(matches!(
                restore_deleted_cards(&pool, deck, 2, now).await,
                Err(AuthoringError::DeletedCardsChanged)
            ));

            delete_flashcard(&pool, cards[0], now).await.unwrap();
            delete_flashcard(&pool, cards[1], now).await.unwrap();
            let before = all_card_rows(&pool).await;

            // The user confirmed a different number than the deck now has.
            for expected in [0, 1, 3] {
                assert!(
                    matches!(
                        restore_deleted_cards(&pool, deck, expected, now).await,
                        Err(AuthoringError::DeletedCardsChanged)
                    ),
                    "expected {expected}"
                );
                assert_eq!(all_card_rows(&pool).await, before);
            }

            restore_deleted_cards(&pool, deck, 2, now).await.unwrap();
            let after = all_card_rows(&pool).await;

            // Pressing it again finds nothing left to restore.
            assert!(matches!(
                restore_deleted_cards(&pool, deck, 2, now).await,
                Err(AuthoringError::DeletedCardsChanged)
            ));
            assert_eq!(all_card_rows(&pool).await, after);
        });
    }

    #[test]
    fn a_failed_restore_all_restores_none() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) = deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            delete_flashcard(&pool, cards[0], now).await.unwrap();
            delete_flashcard(&pool, cards[1], now).await.unwrap();
            let before = all_card_rows(&pool).await;

            // Fails on card B only, after card A may already be cleared.
            sqlx::query(
                "CREATE TRIGGER fail_restore BEFORE UPDATE OF deleted_at ON flashcards
                 WHEN NEW.deleted_at IS NULL AND OLD.front = 'B'
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            assert!(matches!(
                restore_deleted_cards(&pool, deck, 2, now).await,
                Err(AuthoringError::Internal(_))
            ));
            assert_eq!(all_card_rows(&pool).await, before);
            assert_eq!(deleted_card_ids(&pool, deck, now).await.len(), 2);
        });
    }

    // Bulk delete and move

    /// The ids of a deck's active cards, as its screen lists them.
    async fn active_card_ids(pool: &SqlitePool, deck: i64, now: DateTime<Utc>) -> Vec<i64> {
        deck_detail(pool, deck, now)
            .await
            .unwrap()
            .cards
            .into_iter()
            .map(|card| card.id)
            .collect()
    }

    #[test]
    fn deleting_several_cards_deletes_each_as_one_delete_would() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (deck, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let session = start(&pool, deck, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            let before = [
                stored_card(&pool, cards[0]).await,
                stored_card(&pool, cards[2]).await,
            ];
            let logs = review_logs(&pool).await;

            let later = now + TimeDelta::minutes(5);
            let outcome = delete_flashcards(&pool, deck, &[cards[2], cards[0]], later)
                .await
                .unwrap();
            assert_eq!(outcome.changed, 2);
            assert_eq!(active_card_ids(&pool, deck, later).await, vec![cards[1]]);
            assert_eq!(outcome.deck, deck_detail(&pool, deck, later).await.unwrap());
            let mut deleted = deleted_card_ids(&pool, deck, later).await;
            deleted.sort();
            assert_eq!(deleted, vec![cards[0], cards[2]]);

            // Each row is kept, with only `deleted_at` and `updated_at` moved,
            // and its review logs stay.
            for (card, before) in [cards[0], cards[2]].iter().zip(&before) {
                let mut expected = before.clone();
                expected.11 = to_db_time(later);
                assert_eq!(stored_card(&pool, *card).await, expected);
                assert_eq!(deleted_at(&pool, *card).await, Some(to_db_time(later)));
            }
            assert_eq!(review_logs(&pool).await, logs);
            // B is still due, so the session carries on.
            assert_eq!(session_row(&pool, session).await.1, None);

            // Deleting the last due card finishes the session.
            delete_flashcards(&pool, deck, &[cards[1]], later)
                .await
                .unwrap();
            assert_eq!(session_row(&pool, session).await.1, Some(to_db_time(later)));
            // And the cards can be restored like any other.
            restore_deleted_cards(&pool, deck, 3, later).await.unwrap();
            assert_eq!(active_card_ids(&pool, deck, later).await, cards);
        });
    }

    #[test]
    fn moving_cards_keeps_their_schedule_and_history() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let (chemistry, theirs) =
                deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;

            // A gets real FSRS state and a log row; the session stays open.
            let session = start(&pool, biology, now).await;
            study::record_review(&pool, session, cards[0], 0, Rating::Good, now)
                .await
                .unwrap();
            delete_flashcard(&pool, cards[2], now).await.unwrap();
            let before = stored_card(&pool, cards[0]).await;
            let logs = review_logs(&pool).await;
            let sessions = all_sessions(&pool).await;

            let later = now + TimeDelta::minutes(5);
            let outcome = move_flashcards(&pool, biology, &[cards[0]], chemistry, later)
                .await
                .unwrap();
            assert_eq!(outcome.changed, 1);
            assert_eq!(
                outcome.deck,
                deck_detail(&pool, biology, later).await.unwrap()
            );
            assert_eq!(active_card_ids(&pool, biology, later).await, vec![cards[1]]);
            // Listed by id, so the older card comes first.
            assert_eq!(
                active_card_ids(&pool, chemistry, later).await,
                vec![cards[0], theirs[0]]
            );

            // Same id; only the deck and `updated_at` changed. Its FSRS state,
            // due date, reps, lapses, last review, and logs are untouched.
            let mut expected = before.clone();
            expected.0 = Some(chemistry);
            expected.11 = to_db_time(later);
            assert_eq!(stored_card(&pool, cards[0]).await, expected);
            assert_eq!(deleted_at(&pool, cards[0]).await, None);
            assert_eq!(review_logs(&pool).await, logs);
            // B is still due in Biology, so its session carries on unchanged.
            assert_eq!(all_sessions(&pool).await, sessions);
            // A is scheduled ahead: Chemistry has its own card due, not A.
            let chem = deck_detail(&pool, chemistry, later).await.unwrap();
            assert_eq!((chem.card_count, chem.due_count), (2, 1));
            // The deleted card stayed where it was.
            assert_eq!(
                deleted_card_ids(&pool, biology, later).await,
                vec![cards[2]]
            );

            // Moving Biology's last due card away finishes its session.
            move_flashcards(&pool, biology, &[cards[1]], chemistry, later)
                .await
                .unwrap();
            assert_eq!(session_row(&pool, session).await.1, Some(to_db_time(later)));
        });
    }

    #[test]
    fn moving_refuses_sample_and_archived_decks_on_either_side_and_writes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (biology, cards) = deck_with_cards(&pool, "Biology", &[("A", "a")]).await;
            let (physics, physics_cards) = deck_with_cards(&pool, "Physics", &[("F", "ma")]).await;
            let (archived, archived_cards) = deck_with_cards(&pool, "Old", &[("O", "o")]).await;
            archive_deck(&pool, archived, now).await.unwrap();
            let sample = sample_deck_id(&pool).await;
            let seed_card = card_ids(&pool, sample).await[0];
            let before = all_card_rows(&pool).await;

            // Into the sample deck, an archived deck, or no deck at all.
            for target in [sample, archived, 999] {
                assert!(
                    matches!(
                        move_flashcards(&pool, biology, &cards, target, now).await,
                        Err(AuthoringError::MoveTargetUnavailable)
                    ),
                    "move into {target}"
                );
            }
            // Out of the sample deck or an archived deck.
            assert!(matches!(
                move_flashcards(&pool, sample, &[seed_card], biology, now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                move_flashcards(&pool, archived, &archived_cards, biology, now).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert!(matches!(
                move_flashcards(&pool, 999, &cards, biology, now).await,
                Err(AuthoringError::DeckNotFound)
            ));
            // Into the deck they're already in.
            assert!(matches!(
                move_flashcards(&pool, biology, &cards, biology, now).await,
                Err(AuthoringError::SameDeck)
            ));
            // Deleting is refused in the same decks.
            assert!(matches!(
                delete_flashcards(&pool, sample, &[seed_card], now).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                delete_flashcards(&pool, archived, &archived_cards, now).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(all_card_rows(&pool).await, before);

            // A card of another deck can't be moved as if it were this one's.
            assert!(matches!(
                move_flashcards(&pool, biology, &physics_cards, physics, now).await,
                Err(AuthoringError::SelectionChanged)
            ));
            assert_eq!(all_card_rows(&pool).await, before);
        });
    }

    #[test]
    fn a_selection_that_is_out_of_date_or_malformed_is_refused_and_writes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b"), ("C", "c")]).await;
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;
            delete_flashcard(&pool, cards[2], now).await.unwrap();
            let before = all_card_rows(&pool).await;

            // A deleted card, an unknown card, or one moved away meanwhile.
            for selection in [vec![cards[0], cards[2]], vec![cards[0], 999]] {
                assert!(matches!(
                    delete_flashcards(&pool, biology, &selection, now).await,
                    Err(AuthoringError::SelectionChanged)
                ));
                assert!(matches!(
                    move_flashcards(&pool, biology, &selection, chemistry, now).await,
                    Err(AuthoringError::SelectionChanged)
                ));
                assert_eq!(all_card_rows(&pool).await, before);
            }
            move_flashcards(&pool, biology, &[cards[1]], chemistry, now)
                .await
                .unwrap();
            let moved = all_card_rows(&pool).await;
            assert!(matches!(
                delete_flashcards(&pool, biology, &[cards[0], cards[1]], now).await,
                Err(AuthoringError::SelectionChanged)
            ));
            // Pressing Move again finds the card already gone.
            assert!(matches!(
                move_flashcards(&pool, biology, &[cards[1]], chemistry, now).await,
                Err(AuthoringError::SelectionChanged)
            ));
            assert_eq!(all_card_rows(&pool).await, moved);

            // Nothing selected, the same card twice, or far too many.
            assert!(matches!(
                delete_flashcards(&pool, biology, &[], now).await,
                Err(AuthoringError::NoneSelected)
            ));
            assert!(matches!(
                move_flashcards(&pool, biology, &[cards[0], cards[0]], chemistry, now).await,
                Err(AuthoringError::DuplicateSelected)
            ));
            let too_many: Vec<i64> = (1..=CARD_SELECTION_MAX as i64 + 1).collect();
            assert!(matches!(
                delete_flashcards(&pool, biology, &too_many, now).await,
                Err(AuthoringError::TooManySelected)
            ));
            assert_eq!(all_card_rows(&pool).await, moved);
        });
    }

    #[test]
    fn a_failed_bulk_change_changes_no_card() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let now = at(NOW);
            let (biology, cards) =
                deck_with_cards(&pool, "Biology", &[("A", "a"), ("B", "b")]).await;
            let (chemistry, _) = deck_with_cards(&pool, "Chemistry", &[("Na", "Sodium")]).await;
            let before = all_card_rows(&pool).await;

            // Fails on card B only, after card A may already be written.
            sqlx::query(
                "CREATE TRIGGER fail_bulk BEFORE UPDATE ON flashcards
                 WHEN OLD.front = 'B'
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            assert!(matches!(
                move_flashcards(&pool, biology, &cards, chemistry, now).await,
                Err(AuthoringError::Internal(_))
            ));
            assert_eq!(all_card_rows(&pool).await, before);
            assert!(matches!(
                delete_flashcards(&pool, biology, &cards, now).await,
                Err(AuthoringError::Internal(_))
            ));
            assert_eq!(all_card_rows(&pool).await, before);
        });
    }
}
