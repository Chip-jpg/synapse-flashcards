//! Typed notes: a user's own plain-text study notes, kept in the local
//! database, listed in a note library, and opened, created, and edited there.
//!
//! Notes are global, not tied to a deck (migration 0007 explains why), and
//! independent of cards: a card written from a note is an ordinary card with
//! no link back, so editing a note never changes a card and cards never change
//! a note. Notes have no lifecycle yet — no deletion, archiving, or tags.
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
use sqlx::SqlitePool;

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

/// Every note, most recently saved first (newest id first on a tie).
pub async fn notes(pool: &SqlitePool) -> Result<Vec<NoteSummary>, sqlx::Error> {
    sqlx::query_as::<_, NoteSummary>(
        "SELECT id, title, created_at, updated_at FROM notes
         ORDER BY updated_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await
}

/// One whole note.
pub async fn note(pool: &SqlitePool, note_id: i64) -> Result<Note, NoteError> {
    sqlx::query_as::<_, Note>(
        "SELECT id, title, body, created_at, updated_at FROM notes WHERE id = ?1",
    )
    .bind(note_id)
    .fetch_optional(pool)
    .await?
    .ok_or(NoteError::NotFound)
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

/// Replaces a note's title and body and returns it. Its id and creation time
/// are kept; `updated_at` becomes `now`.
pub async fn update_note(
    pool: &SqlitePool,
    note_id: i64,
    title: &str,
    body: &str,
    now: DateTime<Utc>,
) -> Result<Note, NoteError> {
    let text = validate_note(title, body)?;
    // One statement, so there's nothing to keep consistent between a check
    // and a write: no row means no such note.
    sqlx::query_as::<_, Note>(
        "UPDATE notes SET title = ?1, body = ?2, updated_at = ?3
         WHERE id = ?4
         RETURNING id, title, body, created_at, updated_at",
    )
    .bind(text.title)
    .bind(text.body)
    .bind(to_db_time(now))
    .bind(note_id)
    .fetch_optional(pool)
    .await?
    .ok_or(NoteError::NotFound)
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

    type NoteRow = (i64, String, String, String, String);

    async fn all_notes(pool: &SqlitePool) -> Vec<NoteRow> {
        sqlx::query_as("SELECT id, title, body, created_at, updated_at FROM notes ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    type StudyRows = (
        Vec<(i64, String, Option<String>, bool, Option<String>)>,
        Vec<(
            i64,
            Option<i64>,
            String,
            String,
            String,
            Option<String>,
            i64,
            Option<String>,
            String,
        )>,
        Vec<(i64, i64, i64, String)>,
        Vec<(i64, i64, Option<String>, i64)>,
        Vec<String>,
    );

    /// Every deck, card, review log, session, and seed marker.
    async fn study_rows(pool: &SqlitePool) -> StudyRows {
        (
            sqlx::query_as(
                "SELECT id, name, description, is_sample, archived_at FROM decks ORDER BY id",
            )
            .fetch_all(pool)
            .await
            .unwrap(),
            sqlx::query_as(
                "SELECT id, deck_id, front, back, fsrs_state, due, reps, deleted_at, updated_at
                 FROM flashcards ORDER BY id",
            )
            .fetch_all(pool)
            .await
            .unwrap(),
            sqlx::query_as("SELECT id, card_id, rating, reviewed_at FROM review_logs ORDER BY id")
                .fetch_all(pool)
                .await
                .unwrap(),
            sqlx::query_as(
                "SELECT id, deck_id, ended_at, cards_reviewed FROM sessions ORDER BY id",
            )
            .fetch_all(pool)
            .await
            .unwrap(),
            sqlx::query_scalar("SELECT name FROM seed_markers ORDER BY name")
                .fetch_all(pool)
                .await
                .unwrap(),
        )
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
            let card_before = card_row(&pool, card).await;
            update_note(
                &pool,
                note.id,
                "Photosynthesis",
                "Rewritten entirely.",
                at(LATER),
            )
            .await
            .unwrap();
            assert_eq!(card_row(&pool, card).await, card_before);
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
}
