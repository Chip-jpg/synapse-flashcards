//! Typed notes: a user's own plain-text study notes, kept in the local
//! database, listed in a note library, and opened, created, and edited there.
//!
//! Notes are global, not tied to a deck (migration 0007 explains why), and
//! independent of cards: a card written from a note is an ordinary card with
//! no link back, so editing a note never changes a card, deleting a note never
//! changes a card, and cards never change a note. Notes have no tags or
//! folders.
//!
//! Deleting a note is a soft delete (migration 0008): the row gets a
//! `deleted_at` time and the note leaves the library, so it can't be opened,
//! edited, or used to write a card. Nothing is removed, and unlike a deleted
//! card or an archived deck a deleted note can be restored, which is why it
//! must come back exactly as it was:
//! - Deleting writes only `deleted_at`. The title, body, `created_at`, and
//!   `updated_at` are left alone, so `updated_at` keeps meaning "when the text
//!   was last saved" and a restored note returns to its old place in the
//!   library instead of jumping to the top.
//! - Restoring clears `deleted_at` and writes nothing else.
//! - So `now` is only ever read for the deletion time; a restore doesn't need
//!   it, and neither changes the note's own history.
//!
//! Every rule about a valid note is checked here, so nothing the frontend
//! sends is trusted. Lengths count characters (Unicode scalar values) after
//! outer whitespace is trimmed; inner whitespace and line breaks are kept
//! exactly as typed:
//! - Title: required, at most [`NOTE_TITLE_MAX_CHARS`].
//! - Body: required, at most [`NOTE_BODY_MAX_CHARS`].
//!
//! Titles don't have to be unique: notes are told apart by id.

use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};

use crate::authoring::required_text;
use crate::db::{to_db_time, DbError};

pub const NOTE_TITLE_MAX_CHARS: usize = 200;
/// About 15,000 words: room for a long lecture's notes.
pub const NOTE_BODY_MAX_CHARS: usize = 100_000;

/// One note in the library: `{ id, title, createdAt, updatedAt }`.
#[derive(Debug, PartialEq, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    pub id: i64,
    pub title: String,
    /// When it was created (ISO-8601 UTC).
    pub created_at: String,
    /// When it was last saved (ISO-8601 UTC).
    pub updated_at: String,
}

/// A whole note: `{ id, title, body, createdAt, updatedAt }`.
#[derive(Debug, PartialEq, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: i64,
    pub title: String,
    /// The plain text, with its line breaks.
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A deleted note, as listed in the library's history:
/// `{ id, title, createdAt, updatedAt, deletedAt }`.
///
/// The body isn't sent: the history lists deleted notes, it doesn't open them.
#[derive(Debug, PartialEq, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DeletedNote {
    pub id: i64,
    pub title: String,
    pub created_at: String,
    /// When its text was last saved, before it was deleted (ISO-8601 UTC).
    pub updated_at: String,
    /// When it was deleted (ISO-8601 UTC).
    pub deleted_at: String,
}

/// What was wrong with a note the user wrote. Nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidNote {
    TitleBlank,
    TitleTooLong,
    BodyBlank,
    BodyTooLong,
}

/// Why a note operation didn't happen.
#[derive(Debug)]
pub enum NoteError {
    Invalid(InvalidNote),
    /// No note has this id.
    NotFound,
    /// The note was deleted, so it can't be read, edited, or deleted again.
    Deleted,
    /// The note isn't deleted, so there's nothing to restore.
    NotDeleted,
    /// Anything else (a database failure). Detail is for logs only.
    Internal(DbError),
}

impl From<InvalidNote> for NoteError {
    fn from(problem: InvalidNote) -> Self {
        NoteError::Invalid(problem)
    }
}

impl From<sqlx::Error> for NoteError {
    fn from(err: sqlx::Error) -> Self {
        NoteError::Internal(err.into())
    }
}

/// A note's text after validation.
#[derive(Debug, PartialEq)]
struct NoteText<'a> {
    title: &'a str,
    body: &'a str,
}

fn validate_note<'a>(title: &'a str, body: &'a str) -> Result<NoteText<'a>, InvalidNote> {
    Ok(NoteText {
        title: required_text(
            title,
            NOTE_TITLE_MAX_CHARS,
            InvalidNote::TitleBlank,
            InvalidNote::TitleTooLong,
        )?,
        body: required_text(
            body,
            NOTE_BODY_MAX_CHARS,
            InvalidNote::BodyBlank,
            InvalidNote::BodyTooLong,
        )?,
    })
}

/// Every active (not deleted) note, most recently saved first (newest id
/// first on a tie).
pub async fn notes(pool: &SqlitePool) -> Result<Vec<NoteSummary>, sqlx::Error> {
    sqlx::query_as::<_, NoteSummary>(
        "SELECT id, title, created_at, updated_at FROM notes
         WHERE deleted_at IS NULL
         ORDER BY updated_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await
}

/// Every deleted note, most recently deleted first. Read-only history.
pub async fn deleted_notes(pool: &SqlitePool) -> Result<Vec<DeletedNote>, sqlx::Error> {
    sqlx::query_as::<_, DeletedNote>(
        "SELECT id, title, created_at, updated_at, deleted_at FROM notes
         WHERE deleted_at IS NOT NULL
         ORDER BY deleted_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await
}

/// A note as stored, whether or not it has been deleted.
#[derive(sqlx::FromRow)]
struct NoteRow {
    id: i64,
    title: String,
    body: String,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
}

/// When a note was deleted, or `None` if it's active. Reads no text: deleting
/// and restoring only ever need this one column, and a body can be 100,000
/// characters.
async fn deletion_state(
    conn: &mut SqliteConnection,
    note_id: i64,
) -> Result<Option<String>, NoteError> {
    sqlx::query_scalar::<_, Option<String>>("SELECT deleted_at FROM notes WHERE id = ?1")
        .bind(note_id)
        .fetch_optional(conn)
        .await?
        .ok_or(NoteError::NotFound)
}

/// A note that may be read or edited: it must exist and not be deleted. The
/// whole row is read, so a missing note and a deleted one are told apart
/// without a second query.
async fn load_active_note(conn: &mut SqliteConnection, note_id: i64) -> Result<Note, NoteError> {
    let row = sqlx::query_as::<_, NoteRow>(
        "SELECT id, title, body, created_at, updated_at, deleted_at FROM notes WHERE id = ?1",
    )
    .bind(note_id)
    .fetch_optional(conn)
    .await?
    .ok_or(NoteError::NotFound)?;
    if row.deleted_at.is_some() {
        return Err(NoteError::Deleted);
    }
    Ok(Note {
        id: row.id,
        title: row.title,
        body: row.body,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

/// One whole active note. A deleted note is refused, so it can't be opened,
/// edited, or used to write a card.
pub async fn note(pool: &SqlitePool, note_id: i64) -> Result<Note, NoteError> {
    let mut conn = pool.acquire().await?;
    load_active_note(&mut conn, note_id).await
}

/// Saves a new note and returns it.
pub async fn create_note(
    pool: &SqlitePool,
    title: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<Note, NoteError> {
    let text = validate_note(title, body)?;
    let note = sqlx::query_as::<_, Note>(
        "INSERT INTO notes (title, body, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         RETURNING id, title, body, created_at, updated_at",
    )
    .bind(text.title)
    .bind(text.body)
    .bind(to_db_time(now))
    .fetch_one(pool)
    .await?;
    Ok(note)
}

/// Replaces an active note's title and body and returns it. Its id and
/// creation time are kept; `updated_at` becomes `now`. A deleted note is
/// refused and nothing is written.
pub async fn update_note(
    pool: &SqlitePool,
    note_id: i64,
    title: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<Note, NoteError> {
    let text = validate_note(title, body)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the note is missing or deleted.
    // Returning early drops `tx`, which rolls it back. Only `deleted_at` is
    // read: the old body is about to be replaced, so there's no reason to
    // fetch it.
    if deletion_state(&mut tx, note_id).await?.is_some() {
        return Err(NoteError::Deleted);
    }
    let note = sqlx::query_as::<_, Note>(
        "UPDATE notes SET title = ?1, body = ?2, updated_at = ?3
         WHERE id = ?4 AND deleted_at IS NULL
         RETURNING id, title, body, created_at, updated_at",
    )
    .bind(text.title)
    .bind(text.body)
    .bind(to_db_time(now))
    .bind(note_id)
    .fetch_optional(&mut *tx)
    .await?
    // The note was checked above inside this same transaction, so it can't
    // normally have gone. If it somehow did, say the screen is out of date
    // rather than claim a save that didn't happen.
    .ok_or(NoteError::Deleted)?;

    tx.commit().await?;
    Ok(note)
}

/// Soft-deletes a note: it leaves the library and can no longer be opened,
/// edited, or used to write a card, but the row and its text are kept and it
/// can be restored.
///
/// Only `deleted_at` is written. Cards written from this note are ordinary
/// cards with no link back to it, so none of them changes in any way.
pub async fn delete_note(
    pool: &SqlitePool,
    note_id: i64,
    now: DateTime<Utc>,
) -> Result<(), NoteError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the note is missing or was
    // already deleted, so deleting twice changes nothing at all.
    if deletion_state(&mut tx, note_id).await?.is_some() {
        return Err(NoteError::Deleted);
    }
    let deleted =
        sqlx::query("UPDATE notes SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL")
            .bind(to_db_time(now))
            .bind(note_id)
            .execute(&mut *tx)
            .await?;
    if deleted.rows_affected() != 1 {
        return Err(NoteError::Deleted);
    }

    tx.commit().await?;
    Ok(())
}

/// Restores a deleted note: it returns to the library exactly as it was, in
/// its old place (its `updated_at` never moved).
///
/// Only `deleted_at` is cleared. A note that isn't deleted is refused.
pub async fn restore_note(pool: &SqlitePool, note_id: i64) -> Result<(), NoteError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    if deletion_state(&mut tx, note_id).await?.is_none() {
        return Err(NoteError::NotDeleted);
    }
    // The statement repeats the check it relies on, as every conditional write
    // here does, so it can only ever clear a deletion that is really there.
    let restored =
        sqlx::query("UPDATE notes SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL")
            .bind(note_id)
            .execute(&mut *tx)
            .await?;
    // As in `update_note`: checked above in this same transaction.
    if restored.rows_affected() != 1 {
        return Err(NoteError::NotDeleted);
    }

    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring;
    use crate::db::test_support::*;
    use crate::db::{migrate_and_seed, open_file};
    use crate::scheduler::Rating;
    use crate::study::{self, SessionCard, StartedSession};
    use std::path::Path;

    const NOW: &str = "2026-09-16T12:00:00Z";
    const LATER: &str = "2026-09-16T13:00:00Z";
    /// Later still, so a deletion time is never mistaken for a save time.
    const DELETED: &str = "2026-09-16T14:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    /// The validation problem a call failed with; panics on anything else.
    fn problem<T: std::fmt::Debug>(result: Result<T, NoteError>) -> InvalidNote {
        match result {
            Err(NoteError::Invalid(problem)) => problem,
            other => panic!("expected invalid input, got {other:?}"),
        }
    }

    /// One note exactly as stored, deletion state included, so a before/after
    /// comparison catches any change at all.
    type StoredNote = (i64, String, String, String, String, Option<String>);

    async fn all_notes(pool: &SqlitePool) -> Vec<StoredNote> {
        sqlx::query_as(
            "SELECT id, title, body, created_at, updated_at, deleted_at FROM notes ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// The titles of the library, in the order it lists them.
    async fn library(pool: &SqlitePool) -> Vec<String> {
        notes(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|note| note.title)
            .collect()
    }

    /// The error a note call failed with; panics if it succeeded.
    fn failure<T: std::fmt::Debug>(result: Result<T, NoteError>) -> NoteError {
        match result {
            Err(err) => err,
            Ok(value) => panic!("expected a refusal, got {value:?}"),
        }
    }

    /// Every row of every deck, card, review log, session, and seed marker,
    /// every column of each, rendered by SQLite's `quote()`. `import.rs`
    /// compares whole databases the same way; nothing can change unnoticed.
    async fn study_rows(pool: &SqlitePool) -> Vec<Vec<String>> {
        let mut tables = Vec::new();
        for sql in [
            "SELECT quote(id) || ' ' || quote(name) || ' ' || quote(description) || ' '
                 || quote(is_sample) || ' ' || quote(created_at) || ' ' || quote(archived_at)
             FROM decks ORDER BY id",
            "SELECT quote(id) || ' ' || quote(deck_id) || ' ' || quote(front) || ' '
                 || quote(back) || ' ' || quote(created_at) || ' ' || quote(updated_at) || ' '
                 || quote(fsrs_stability) || ' ' || quote(fsrs_difficulty) || ' '
                 || quote(fsrs_state) || ' ' || quote(due) || ' ' || quote(last_review) || ' '
                 || quote(reps) || ' ' || quote(lapses) || ' ' || quote(deleted_at)
             FROM flashcards ORDER BY id",
            "SELECT quote(id) || ' ' || quote(card_id) || ' ' || quote(rating) || ' '
                 || quote(state_before) || ' ' || quote(scheduled_days) || ' '
                 || quote(elapsed_days) || ' ' || quote(stability_after) || ' '
                 || quote(difficulty_after) || ' ' || quote(reviewed_at)
             FROM review_logs ORDER BY id",
            "SELECT quote(id) || ' ' || quote(deck_id) || ' ' || quote(started_at) || ' '
                 || quote(ended_at) || ' ' || quote(cards_reviewed)
             FROM sessions ORDER BY id",
            "SELECT quote(name) || ' ' || quote(recorded_at) FROM seed_markers ORDER BY name",
        ] {
            tables.push(
                sqlx::query_scalar::<_, String>(sql)
                    .fetch_all(pool)
                    .await
                    .unwrap(),
            );
        }
        tables
    }

    /// One card, every column, for exact before/after comparison.
    async fn whole_card(pool: &SqlitePool, id: i64) -> String {
        sqlx::query_scalar::<_, String>(
            "SELECT quote(deck_id) || ' ' || quote(front) || ' ' || quote(back) || ' '
                 || quote(created_at) || ' ' || quote(updated_at) || ' ' || quote(fsrs_stability)
                 || ' ' || quote(fsrs_difficulty) || ' ' || quote(fsrs_state) || ' '
                 || quote(due) || ' ' || quote(last_review) || ' ' || quote(reps) || ' '
                 || quote(lapses) || ' ' || quote(deleted_at)
             FROM flashcards WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[test]
    fn a_created_note_is_trimmed_keeps_its_line_breaks_and_is_listed() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let body = "\n  First line\n\n    indented second line\r\nthird\t \n";

            let note = create_note(&pool, "  Cell biology  ", body, at(NOW))
                .await
                .unwrap();

            assert_eq!(
                note,
                Note {
                    id: note.id,
                    title: "Cell biology".to_string(),
                    // Only the outer whitespace goes; the blank line, the
                    // indentation, and the Windows line break inside stay.
                    body: "First line\n\n    indented second line\r\nthird".to_string(),
                    created_at: "2026-09-16T12:00:00.000Z".to_string(),
                    updated_at: "2026-09-16T12:00:00.000Z".to_string(),
                }
            );
            assert_eq!(super::note(&pool, note.id).await.unwrap(), note);
            assert_eq!(
                notes(&pool).await.unwrap(),
                vec![NoteSummary {
                    id: note.id,
                    title: "Cell biology".to_string(),
                    created_at: note.created_at.clone(),
                    updated_at: note.updated_at.clone(),
                }]
            );
        });
    }

    #[test]
    fn blank_or_too_long_notes_are_rejected_and_nothing_is_saved() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let at_limit = |n: usize| "é".repeat(n);

            for blank in ["", "   ", "\n\t \r\n", "\u{2003}"] {
                assert_eq!(
                    problem(create_note(&pool, blank, "body", at(NOW)).await),
                    InvalidNote::TitleBlank
                );
                assert_eq!(
                    problem(create_note(&pool, "title", blank, at(NOW)).await),
                    InvalidNote::BodyBlank
                );
            }
            assert_eq!(
                problem(
                    create_note(&pool, &at_limit(NOTE_TITLE_MAX_CHARS + 1), "body", at(NOW)).await
                ),
                InvalidNote::TitleTooLong
            );
            assert_eq!(
                problem(
                    create_note(&pool, "title", &at_limit(NOTE_BODY_MAX_CHARS + 1), at(NOW)).await
                ),
                InvalidNote::BodyTooLong
            );
            assert!(all_notes(&pool).await.is_empty());

            // Exactly at the limits (counted in characters, after trimming) is fine.
            let title = format!("  {}  ", at_limit(NOTE_TITLE_MAX_CHARS));
            let body = format!("\n{}\n", at_limit(NOTE_BODY_MAX_CHARS));
            let note = create_note(&pool, &title, &body, at(NOW)).await.unwrap();
            assert_eq!(note.title.chars().count(), NOTE_TITLE_MAX_CHARS);
            assert_eq!(note.body.chars().count(), NOTE_BODY_MAX_CHARS);
        });
    }

    #[test]
    fn editing_a_note_keeps_its_id_and_creation_time() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note = create_note(&pool, "Draft", "one", at(NOW)).await.unwrap();

            let edited = update_note(&pool, note.id, " Final ", "one\ntwo ", at(LATER))
                .await
                .unwrap();

            assert_eq!(
                edited,
                Note {
                    id: note.id,
                    title: "Final".to_string(),
                    body: "one\ntwo".to_string(),
                    created_at: note.created_at,
                    updated_at: "2026-09-16T13:00:00.000Z".to_string(),
                }
            );
            assert_eq!(super::note(&pool, note.id).await.unwrap(), edited);
        });
    }

    #[test]
    fn invalid_edits_and_missing_notes_change_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note = create_note(&pool, "Title", "Body", at(NOW)).await.unwrap();
            let before = all_notes(&pool).await;

            assert_eq!(
                problem(update_note(&pool, note.id, " ", "Body", at(LATER)).await),
                InvalidNote::TitleBlank
            );
            assert_eq!(
                problem(update_note(&pool, note.id, "Title", "\n", at(LATER)).await),
                InvalidNote::BodyBlank
            );
            assert!(matches!(
                update_note(&pool, note.id + 1, "Title", "Body", at(LATER)).await,
                Err(NoteError::NotFound)
            ));
            assert!(matches!(
                super::note(&pool, note.id + 1).await,
                Err(NoteError::NotFound)
            ));
            assert_eq!(all_notes(&pool).await, before);
        });
    }

    #[test]
    fn notes_are_listed_most_recently_saved_first() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let first = create_note(&pool, "First", "a", at(NOW)).await.unwrap();
            let second = create_note(&pool, "Second", "b", at(NOW)).await.unwrap();
            let third = create_note(&pool, "Third", "c", at("2026-09-16T12:30:00Z"))
                .await
                .unwrap();
            // Two notes may share a title.
            let fourth = create_note(&pool, "Third", "d", at("2026-09-16T12:10:00Z"))
                .await
                .unwrap();

            let order =
                |notes: Vec<NoteSummary>| notes.into_iter().map(|n| n.id).collect::<Vec<_>>();
            // Same time: the newer note first.
            assert_eq!(
                order(notes(&pool).await.unwrap()),
                vec![third.id, fourth.id, second.id, first.id]
            );

            // Editing a note moves it to the top.
            update_note(&pool, first.id, "First", "edited", at(LATER))
                .await
                .unwrap();
            assert_eq!(
                order(notes(&pool).await.unwrap()),
                vec![first.id, third.id, fourth.id, second.id]
            );
        });
    }

    #[test]
    fn the_database_itself_refuses_blank_notes() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            for sql in [
                "INSERT INTO notes (title, body) VALUES (' ', 'body')",
                "INSERT INTO notes (title, body) VALUES ('title', char(10, 9))",
            ] {
                let err = sqlx::query(sql).execute(&pool).await.unwrap_err();
                assert!(err.to_string().contains("note text is blank"), "{err}");
            }
            sqlx::query("INSERT INTO notes (title, body) VALUES ('title', 'body')")
                .execute(&pool)
                .await
                .unwrap();
            for sql in [
                "UPDATE notes SET title = ' ' WHERE id = 1",
                "UPDATE notes SET body = '  ' WHERE id = 1",
            ] {
                let err = sqlx::query(sql).execute(&pool).await.unwrap_err();
                assert!(err.to_string().contains("note text is blank"), "{err}");
            }
        });
    }

    #[test]
    fn studying_and_authoring_never_touch_notes() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            create_note(&pool, "Kept", "line one\n  line two", at(NOW))
                .await
                .unwrap();
            let edited = create_note(&pool, "Draft", "draft", at(NOW)).await.unwrap();
            update_note(&pool, edited.id, "Edited", "final", at(LATER))
                .await
                .unwrap();
            let before = all_notes(&pool).await;

            // Every kind of study and authoring change there is.
            let deck = authoring::create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            for (front, back) in [("A", "a"), ("B", "b"), ("C", "c")] {
                authoring::create_flashcard(&pool, deck, front, back, at(NOW))
                    .await
                    .unwrap();
            }
            let cards: Vec<i64> =
                sqlx::query_scalar("SELECT id FROM flashcards WHERE deck_id = ?1 ORDER BY id")
                    .bind(deck)
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck, at(NOW)).await.unwrap()
            else {
                panic!("new cards should be due");
            };
            study::record_review(&pool, session_id, cards[0], 0, Rating::Again, at(NOW))
                .await
                .unwrap();
            authoring::update_flashcard(&pool, cards[1], "B2", "b2", at(NOW))
                .await
                .unwrap();
            authoring::delete_flashcard(&pool, cards[2], at(NOW))
                .await
                .unwrap();
            authoring::rename_deck(&pool, deck, "Cell biology", at(NOW))
                .await
                .unwrap();
            authoring::archive_deck(&pool, deck, at(LATER))
                .await
                .unwrap();

            assert_eq!(all_notes(&pool).await, before);
        });
    }

    #[test]
    fn notes_never_touch_decks_cards_reviews_or_sessions() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            // Some study history first: a deck with a reviewed card and an
            // unfinished session.
            let deck = authoring::create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            for (front, back) in [("A", "a"), ("B", "b")] {
                authoring::create_flashcard(&pool, deck, front, back, at(NOW))
                    .await
                    .unwrap();
            }
            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck, at(NOW)).await.unwrap()
            else {
                panic!("new cards should be due");
            };
            let SessionCard::Due { card } = study::session_card(&pool, session_id, at(NOW))
                .await
                .unwrap()
            else {
                panic!("a card should be due");
            };
            study::record_review(&pool, session_id, card.id, 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            let before = study_rows(&pool).await;
            let dashboard_before = format!("{:?}", study::decks(&pool, at(NOW)).await.unwrap());

            let note = create_note(&pool, "Biology notes", "Cells\nGenes", at(NOW))
                .await
                .unwrap();
            update_note(
                &pool,
                note.id,
                "Biology notes",
                "Cells\nGenes\nProteins",
                at(LATER),
            )
            .await
            .unwrap();
            notes(&pool).await.unwrap();

            assert_eq!(study_rows(&pool).await, before);
            assert_eq!(
                format!("{:?}", study::decks(&pool, at(NOW)).await.unwrap()),
                dashboard_before
            );
        });
    }

    /// Every column of a card, for exact before/after comparison.
    async fn card_row(
        pool: &SqlitePool,
        id: i64,
    ) -> (
        Option<i64>,
        String,
        String,
        String,
        Option<String>,
        i64,
        Option<String>,
    ) {
        sqlx::query_as(
            "SELECT deck_id, front, back, fsrs_state, due, reps, deleted_at
             FROM flashcards WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    // Phase 3: writing a card from a note uses the ordinary card command
    // (`authoring::create_flashcard`), so these tests drive exactly that, in
    // the note workflow's order.

    #[test]
    fn a_card_written_from_a_note_is_an_ordinary_card_in_the_chosen_deck_only() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note = create_note(
                &pool,
                "Photosynthesis",
                "Happens in chloroplasts.\n\n  Light reactions: thylakoids\nCalvin cycle: stroma",
                at(NOW),
            )
            .await
            .unwrap();
            let chosen = authoring::create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            let other = authoring::create_deck(&pool, "Chemistry", None, at(NOW))
                .await
                .unwrap()
                .id;
            let notes_before = all_notes(&pool).await;
            let cards_before = count(&pool, "SELECT COUNT(*) FROM flashcards").await;

            // The same validation as anywhere else: a blank side adds nothing.
            assert!(matches!(
                authoring::create_flashcard(&pool, chosen, "Where?", " \n ", at(NOW)).await,
                Err(authoring::AuthoringError::Invalid(
                    authoring::InvalidInput::BackBlank
                ))
            ));
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM flashcards").await,
                cards_before
            );

            // Text copied from the note, with its line breaks and outer
            // whitespace, is trimmed exactly like any card's.
            let deck = authoring::create_flashcard(
                &pool,
                chosen,
                "  Where does the Calvin cycle happen?\n",
                "In the stroma.\n(Light reactions: thylakoids)\n\n",
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(deck.id, chosen);
            assert_eq!((deck.card_count, deck.due_count), (1, 1));
            let card = deck.cards[0].id;
            assert_eq!(
                card_row(&pool, card).await,
                (
                    Some(chosen),
                    "Where does the Calvin cycle happen?".to_string(),
                    "In the stroma.\n(Light reactions: thylakoids)".to_string(),
                    "New".to_string(),
                    None,
                    0,
                    None
                )
            );
            // Only the chosen deck has it, and the note is untouched.
            let other_deck = authoring::deck_detail(&pool, other, at(NOW)).await.unwrap();
            assert_eq!((other_deck.card_count, other_deck.cards.len()), (0, 0));
            assert_eq!(all_notes(&pool).await, notes_before);

            // It enters the normal review flow of its own deck, and no other.
            assert_eq!(
                study::start_session(&pool, other, at(NOW)).await.unwrap(),
                StartedSession::NoneDue
            );
            let StartedSession::Started { session_id } =
                study::start_session(&pool, chosen, at(NOW)).await.unwrap()
            else {
                panic!("the new card should be due");
            };
            let SessionCard::Due { card: due } = study::session_card(&pool, session_id, at(NOW))
                .await
                .unwrap()
            else {
                panic!("the new card should be offered");
            };
            assert_eq!(due.id, card);
            study::record_review(&pool, session_id, card, 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            assert!(matches!(
                study::session_card(&pool, session_id, at(NOW))
                    .await
                    .unwrap(),
                SessionCard::Completed { cards_reviewed: 1 }
            ));
            assert_eq!(all_notes(&pool).await, notes_before);

            // Editing the note later never changes the card.
            let card_before = whole_card(&pool, card).await;
            update_note(
                &pool,
                note.id,
                "Photosynthesis",
                "Rewritten entirely.",
                at(LATER),
            )
            .await
            .unwrap();
            assert_eq!(whole_card(&pool, card).await, card_before);
        });
    }

    #[test]
    fn a_card_from_a_note_is_refused_for_archived_sample_and_missing_decks() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            create_note(&pool, "Note", "Some text", at(NOW))
                .await
                .unwrap();
            let archived = authoring::create_deck(&pool, "Old", None, at(NOW))
                .await
                .unwrap()
                .id;
            authoring::archive_deck(&pool, archived, at(NOW))
                .await
                .unwrap();
            let sample = sample_deck_id(&pool).await;
            let notes_before = all_notes(&pool).await;
            let cards_before = count(&pool, "SELECT COUNT(*) FROM flashcards").await;

            assert!(matches!(
                authoring::create_flashcard(&pool, archived, "Q", "A", at(NOW)).await,
                Err(authoring::AuthoringError::DeckArchived)
            ));
            assert!(matches!(
                authoring::create_flashcard(&pool, sample, "Q", "A", at(NOW)).await,
                Err(authoring::AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                authoring::create_flashcard(&pool, archived + 100, "Q", "A", at(NOW)).await,
                Err(authoring::AuthoringError::DeckNotFound)
            ));

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM flashcards").await,
                cards_before
            );
            assert_eq!(all_notes(&pool).await, notes_before);
        });
    }

    #[test]
    fn the_decks_offered_for_a_card_from_a_note_are_the_ones_that_take_cards() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let kept = authoring::create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            let archived = authoring::create_deck(&pool, "Old", None, at(NOW))
                .await
                .unwrap()
                .id;
            authoring::archive_deck(&pool, archived, at(NOW))
                .await
                .unwrap();

            // React offers the dashboard's decks (`get_decks`, active only)
            // minus the sample deck. Each of those must take a card, and the
            // one it leaves out must not.
            let listed = study::decks(&pool, at(NOW)).await.unwrap();
            assert!(listed.iter().all(|deck| deck.id != archived));
            let offered: Vec<i64> = listed
                .iter()
                .filter(|deck| !deck.is_sample)
                .map(|deck| deck.id)
                .collect();
            assert_eq!(offered, vec![kept]);
            for deck in &listed {
                let added = authoring::create_flashcard(&pool, deck.id, "Q", "A", at(NOW)).await;
                if deck.is_sample {
                    assert!(matches!(added, Err(authoring::AuthoringError::SampleDeck)));
                } else {
                    assert_eq!(added.unwrap().id, deck.id);
                }
            }
        });
    }

    #[test]
    fn notes_survive_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            // A real file (under the build's `target/` folder), reopened like restarts.
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("notes-reopen");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");

            let pool = open_file(&path).await.unwrap();
            let kept = create_note(&pool, "Kept", "line one\nline two", at(NOW))
                .await
                .unwrap();
            let edited = create_note(&pool, "Draft", "draft", at(NOW)).await.unwrap();
            let edited = update_note(&pool, edited.id, "Edited", "final\n\ntext", at(LATER))
                .await
                .unwrap();
            let rows = all_notes(&pool).await;
            pool.close().await;

            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(all_notes(&pool).await, rows);
                assert_eq!(super::note(&pool, kept.id).await.unwrap(), kept);
                assert_eq!(super::note(&pool, edited.id).await.unwrap(), edited);
                pool.close().await;
            }

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    // Phase 1-2: soft-deleting a note, and restoring one.

    #[test]
    fn deleting_a_note_takes_it_out_of_the_library_and_keeps_every_word_of_it() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let kept = create_note(&pool, "Kept", "still here", at(NOW))
                .await
                .unwrap();
            let doomed = create_note(
                &pool,
                "Lecture 3",
                "  First line\n\n    indented\r\nlast  ",
                at(NOW),
            )
            .await
            .unwrap();
            let doomed = update_note(&pool, doomed.id, "Lecture 3", &doomed.body, at(LATER))
                .await
                .unwrap();

            delete_note(&pool, doomed.id, at(DELETED)).await.unwrap();

            // The row is exactly as it was, plus the deletion time: the text,
            // its line breaks, and both of its own timestamps are untouched.
            assert_eq!(
                all_notes(&pool).await,
                vec![
                    (
                        kept.id,
                        "Kept".to_string(),
                        "still here".to_string(),
                        kept.created_at.clone(),
                        kept.updated_at.clone(),
                        None
                    ),
                    (
                        doomed.id,
                        "Lecture 3".to_string(),
                        "First line\n\n    indented\r\nlast".to_string(),
                        doomed.created_at.clone(),
                        // Deleting is not a save: `updated_at` stays at the edit.
                        "2026-09-16T13:00:00.000Z".to_string(),
                        Some("2026-09-16T14:00:00.000Z".to_string())
                    ),
                ]
            );
            // Gone from the library, and listed only as history.
            assert_eq!(library(&pool).await, vec!["Kept".to_string()]);
            assert_eq!(
                deleted_notes(&pool).await.unwrap(),
                vec![DeletedNote {
                    id: doomed.id,
                    title: "Lecture 3".to_string(),
                    created_at: doomed.created_at.clone(),
                    updated_at: "2026-09-16T13:00:00.000Z".to_string(),
                    deleted_at: "2026-09-16T14:00:00.000Z".to_string(),
                }]
            );

            // It can't be opened or edited any more, and the refusal says the
            // screen is out of date rather than that the note never existed.
            let before = all_notes(&pool).await;
            assert!(matches!(
                failure(super::note(&pool, doomed.id).await),
                NoteError::Deleted
            ));
            assert!(matches!(
                failure(update_note(&pool, doomed.id, "New title", "New body", at(DELETED)).await),
                NoteError::Deleted
            ));
            // Invalid text is still caught first, so nothing reaches the row.
            assert!(matches!(
                failure(update_note(&pool, doomed.id, " ", "x", at(DELETED)).await),
                NoteError::Invalid(InvalidNote::TitleBlank)
            ));
            assert_eq!(all_notes(&pool).await, before);

            // The other note carries on as normal.
            assert_eq!(super::note(&pool, kept.id).await.unwrap(), kept);
            update_note(&pool, kept.id, "Kept", "edited", at(DELETED))
                .await
                .unwrap();
            assert_eq!(library(&pool).await, vec!["Kept".to_string()]);
        });
    }

    #[test]
    fn deleting_a_note_twice_or_deleting_one_that_never_existed_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note = create_note(&pool, "Once", "body", at(NOW)).await.unwrap();
            delete_note(&pool, note.id, at(DELETED)).await.unwrap();
            let before = all_notes(&pool).await;

            // A repeat of the same delete (a stale screen, or a double press)
            // is refused, and can never move the recorded deletion time.
            for again in [DELETED, "2026-09-17T09:00:00Z"] {
                assert!(matches!(
                    failure(delete_note(&pool, note.id, at(again)).await),
                    NoteError::Deleted
                ));
            }
            // A note that isn't there is a different answer: nothing to delete.
            for missing in [note.id + 1, 0, -1] {
                assert!(matches!(
                    failure(delete_note(&pool, missing, at(DELETED)).await),
                    NoteError::NotFound
                ));
                assert!(matches!(
                    failure(restore_note(&pool, missing).await),
                    NoteError::NotFound
                ));
            }

            assert_eq!(all_notes(&pool).await, before);
            assert!(library(&pool).await.is_empty());
            assert_eq!(deleted_notes(&pool).await.unwrap().len(), 1);
        });
    }

    #[test]
    fn a_restored_note_comes_back_exactly_as_it_was_and_in_its_old_place() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let oldest = create_note(&pool, "Oldest", "a", at(NOW)).await.unwrap();
            let middle = create_note(&pool, "Middle", "b\n\n  c", at("2026-09-16T12:30:00Z"))
                .await
                .unwrap();
            let newest = create_note(&pool, "Newest", "d", at(LATER)).await.unwrap();
            let before = all_notes(&pool).await;
            let order_before = library(&pool).await;

            delete_note(&pool, middle.id, at(DELETED)).await.unwrap();
            assert_eq!(
                library(&pool).await,
                vec!["Newest".to_string(), "Oldest".to_string()]
            );

            restore_note(&pool, middle.id).await.unwrap();

            // Every column is back to what it was, so the note reappears
            // between the other two rather than at the top.
            assert_eq!(all_notes(&pool).await, before);
            assert_eq!(library(&pool).await, order_before);
            assert!(deleted_notes(&pool).await.unwrap().is_empty());
            assert_eq!(super::note(&pool, middle.id).await.unwrap(), middle);
            // And it can be edited again, like any active note.
            update_note(
                &pool,
                middle.id,
                "Middle",
                "rewritten",
                at("2026-09-17T09:00:00Z"),
            )
            .await
            .unwrap();
            assert_eq!(
                library(&pool).await,
                vec![
                    "Middle".to_string(),
                    "Newest".to_string(),
                    "Oldest".to_string()
                ]
            );

            // Restoring a note that isn't deleted is refused and writes nothing.
            let before = all_notes(&pool).await;
            for id in [oldest.id, middle.id, newest.id] {
                assert!(matches!(
                    failure(restore_note(&pool, id).await),
                    NoteError::NotDeleted
                ));
            }
            assert_eq!(all_notes(&pool).await, before);
        });
    }

    #[test]
    fn a_restored_note_can_be_deleted_again_and_records_the_new_time() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note = create_note(&pool, "Lecture", "Cells\n  Genes", at(NOW))
                .await
                .unwrap();

            delete_note(&pool, note.id, at(DELETED)).await.unwrap();
            restore_note(&pool, note.id).await.unwrap();
            // The second deletion is a fresh one: it must be allowed, and it
            // must record when it happened, not when the first one did.
            let again = "2026-09-18T08:00:00Z";
            delete_note(&pool, note.id, at(again)).await.unwrap();

            assert_eq!(
                deleted_notes(&pool).await.unwrap(),
                vec![DeletedNote {
                    id: note.id,
                    title: "Lecture".to_string(),
                    created_at: note.created_at.clone(),
                    updated_at: note.updated_at.clone(),
                    deleted_at: "2026-09-18T08:00:00.000Z".to_string(),
                }]
            );
            // Around the whole cycle, the note itself never changed.
            assert_eq!(
                all_notes(&pool).await,
                vec![(
                    note.id,
                    "Lecture".to_string(),
                    "Cells\n  Genes".to_string(),
                    note.created_at.clone(),
                    note.updated_at.clone(),
                    Some("2026-09-18T08:00:00.000Z".to_string())
                )]
            );
            restore_note(&pool, note.id).await.unwrap();
            assert_eq!(super::note(&pool, note.id).await.unwrap(), note);
        });
    }

    #[test]
    fn a_failed_restore_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note = create_note(&pool, "Lecture", "Cells\n  Genes", at(NOW))
                .await
                .unwrap();
            delete_note(&pool, note.id, at(DELETED)).await.unwrap();
            let before = all_notes(&pool).await;
            let history = deleted_notes(&pool).await.unwrap();

            sqlx::query(
                "CREATE TRIGGER fail_restore BEFORE UPDATE OF deleted_at ON notes
                 WHEN NEW.deleted_at IS NULL
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            // The note stays deleted, with the same deletion time, still
            // listed under *Deleted notes* and still out of the library.
            assert!(matches!(
                failure(restore_note(&pool, note.id).await),
                NoteError::Internal(_)
            ));
            assert_eq!(all_notes(&pool).await, before);
            assert_eq!(deleted_notes(&pool).await.unwrap(), history);
            assert!(library(&pool).await.is_empty());

            sqlx::query("DROP TRIGGER fail_restore")
                .execute(&pool)
                .await
                .unwrap();
            restore_note(&pool, note.id).await.unwrap();
            assert_eq!(super::note(&pool, note.id).await.unwrap(), note);
        });
    }

    #[test]
    fn deleting_a_note_never_changes_a_card_written_from_it_or_any_study_data() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note = create_note(&pool, "Photosynthesis", "Light reactions", at(NOW))
                .await
                .unwrap();
            let deck = authoring::create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            // A card the user wrote while reading this note: an ordinary card.
            let deck_detail =
                authoring::create_flashcard(&pool, deck, "Where?", "Thylakoids", at(NOW))
                    .await
                    .unwrap();
            let card = deck_detail.cards[0].id;
            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck, at(NOW)).await.unwrap()
            else {
                panic!("the new card should be due");
            };
            study::record_review(&pool, session_id, card, 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            let study_before = study_rows(&pool).await;
            let card_before = whole_card(&pool, card).await;
            let dashboard_before = format!("{:?}", study::decks(&pool, at(LATER)).await.unwrap());

            delete_note(&pool, note.id, at(DELETED)).await.unwrap();

            // The card stays exactly where it was, with its schedule and
            // history: nothing links it to the note it was written from.
            assert_eq!(study_rows(&pool).await, study_before);
            assert_eq!(whole_card(&pool, card).await, card_before);
            assert_eq!(
                format!("{:?}", study::decks(&pool, at(LATER)).await.unwrap()),
                dashboard_before
            );
            let detail = authoring::deck_detail(&pool, deck, at(LATER))
                .await
                .unwrap();
            assert_eq!((detail.card_count, detail.cards.len()), (1, 1));
            assert_eq!(detail.cards[0].front, "Where?");

            // Restoring changes nothing on that side either.
            restore_note(&pool, note.id).await.unwrap();
            assert_eq!(study_rows(&pool).await, study_before);
            assert_eq!(whole_card(&pool, card).await, card_before);
        });
    }

    #[test]
    fn the_database_itself_refuses_removing_or_rewriting_a_deleted_note() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            // A second note, so nothing here can pass by matching the only row.
            create_note(&pool, "Other", "Other body", at(NOW))
                .await
                .unwrap();
            let note = create_note(&pool, "Title", "Body", at(NOW)).await.unwrap();

            /// The id is bound, never written into the SQL.
            async fn refused(pool: &SqlitePool, sql: &'static str, id: i64, reason: &str) {
                let err = sqlx::query(sql).bind(id).execute(pool).await.unwrap_err();
                assert!(err.to_string().contains(reason), "{sql}: {err}");
            }

            // A note is never born deleted.
            let err = sqlx::query(
                "INSERT INTO notes (title, body, created_at, updated_at, deleted_at)
                 VALUES ('Title', 'Body', ?1, ?1, ?1)",
            )
            .bind(to_db_time(at(NOW)))
            .execute(&pool)
            .await
            .unwrap_err();
            assert!(
                err.to_string()
                    .contains("a new note cannot already be deleted"),
                "{err}"
            );

            // No code path may remove an active note...
            let no_removal = "notes are soft-deleted, never removed";
            refused(
                &pool,
                "DELETE FROM notes WHERE id = ?1",
                note.id,
                no_removal,
            )
            .await;

            delete_note(&pool, note.id, at(DELETED)).await.unwrap();

            // ...nor a deleted one, which is the whole point of keeping it.
            refused(
                &pool,
                "DELETE FROM notes WHERE id = ?1",
                note.id,
                no_removal,
            )
            .await;
            let before = all_notes(&pool).await;

            // A deleted note is frozen: nothing but its deletion time can change.
            for sql in [
                "UPDATE notes SET title = 'Rewritten' WHERE id = ?1",
                "UPDATE notes SET body = 'Rewritten' WHERE id = ?1",
                "UPDATE notes SET created_at = '2020-01-01T00:00:00.000Z' WHERE id = ?1",
                "UPDATE notes SET updated_at = '2020-01-01T00:00:00.000Z' WHERE id = ?1",
                "UPDATE notes SET id = 99 WHERE id = ?1",
                // The likeliest mistake a later change could make: restoring a
                // note and moving it to the top of the library at the same time.
                "UPDATE notes SET deleted_at = NULL, updated_at = '2026-09-18T08:00:00.000Z'
                 WHERE id = ?1",
            ] {
                refused(&pool, sql, note.id, "note is deleted").await;
            }
            // And when it was deleted can never be moved.
            refused(
                &pool,
                "UPDATE notes SET deleted_at = '2020-01-01T00:00:00.000Z' WHERE id = ?1",
                note.id,
                "note is already deleted",
            )
            .await;
            assert_eq!(all_notes(&pool).await, before);

            // Clearing the deletion time, and nothing else, is the one change
            // allowed: the restore.
            sqlx::query("UPDATE notes SET deleted_at = NULL WHERE id = ?1")
                .bind(note.id)
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(
                library(&pool).await,
                vec!["Title".to_string(), "Other".to_string()]
            );
        });
    }

    #[test]
    fn notes_written_before_the_migration_stay_active_and_word_for_word() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            // A database left by the previous Synapse, which had no deletion.
            migrations_up_to(7).run(&pool).await.unwrap();
            for (title, body) in [
                ("Lecture 1", "Cells\n\n  Genes"),
                ("Lecture 2", "Proteins\r\nEnzymes"),
            ] {
                sqlx::query(
                    "INSERT INTO notes (title, body, created_at, updated_at)
                     VALUES (?1, ?2, '2026-09-10T08:00:00.000Z', '2026-09-11T09:00:00.000Z')",
                )
                .bind(title)
                .bind(body)
                .execute(&pool)
                .await
                .unwrap();
            }
            let before: Vec<(i64, String, String, String, String)> = sqlx::query_as(
                "SELECT id, title, body, created_at, updated_at FROM notes ORDER BY id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();

            // Upgrading adds only the deletion column.
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                all_notes(&pool).await,
                before
                    .iter()
                    .map(|(id, title, body, created, updated)| (
                        *id,
                        title.clone(),
                        body.clone(),
                        created.clone(),
                        updated.clone(),
                        None
                    ))
                    .collect::<Vec<_>>()
            );
            // Every one of them is still in the library, and none is deleted.
            assert_eq!(
                library(&pool).await,
                vec!["Lecture 2".to_string(), "Lecture 1".to_string()]
            );
            assert!(deleted_notes(&pool).await.unwrap().is_empty());
            assert_eq!(
                super::note(&pool, before[0].0).await.unwrap().body,
                "Cells\n\n  Genes"
            );
        });
    }

    #[test]
    fn deleted_and_restored_notes_survive_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("notes-delete-reopen");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");

            let pool = open_file(&path).await.unwrap();
            let kept = create_note(&pool, "Kept", "line one\nline two", at(NOW))
                .await
                .unwrap();
            let removed = create_note(&pool, "Removed", "still stored", at(NOW))
                .await
                .unwrap();
            delete_note(&pool, removed.id, at(DELETED)).await.unwrap();
            let rows = all_notes(&pool).await;
            let history = deleted_notes(&pool).await.unwrap();
            pool.close().await;

            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(all_notes(&pool).await, rows);
                assert_eq!(library(&pool).await, vec!["Kept".to_string()]);
                assert_eq!(deleted_notes(&pool).await.unwrap(), history);
                assert_eq!(super::note(&pool, kept.id).await.unwrap(), kept);
                assert!(matches!(
                    failure(super::note(&pool, removed.id).await),
                    NoteError::Deleted
                ));
                pool.close().await;
            }

            // A restore survives a restart just as well.
            let pool = open_file(&path).await.unwrap();
            restore_note(&pool, removed.id).await.unwrap();
            pool.close().await;
            let pool = open_file(&path).await.unwrap();
            assert_eq!(
                library(&pool).await,
                vec!["Removed".to_string(), "Kept".to_string()]
            );
            assert_eq!(super::note(&pool, removed.id).await.unwrap(), removed);
            pool.close().await;

            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
