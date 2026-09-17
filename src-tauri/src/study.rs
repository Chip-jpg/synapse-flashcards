//! Studying: the deck dashboard, review sessions, and recording reviews.
//!
//! A session belongs to one deck, and every card query here is scoped to that
//! deck. Session lifecycle:
//! 1. `start_session` creates a session, but only when the deck has due cards
//!    (or resumes the deck's unfinished session).
//! 2. `session_card` hands out the deck's next due card, one at a time.
//! 3. `record_review` saves a rating and counts it towards the session.
//! 4. When no due cards remain, the session gets its `ended_at`, exactly once.
//!
//! Soft-deleted cards (`deleted_at` set) are invisible here: every card query
//! filters on `deleted_at IS NULL`, so they're never counted, offered, or rated.
//!
//! Archived decks (`archived_at` set) are left off the dashboard and can't
//! start or resume a session. Archiving ends the deck's unfinished session in
//! the same transaction, and migration 0006 refuses to open one in an archived
//! deck, so an open session always belongs to an active deck.

use chrono::{DateTime, Utc};
use fsrs::MemoryState;
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{to_db_time, DbError};
use crate::scheduler::{self, CardState, Rating, Schedule};

/// One deck on the dashboard: `{ id, name, description, isSample, dueCount }`.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DeckSummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    /// The built-in sample deck, which can't be opened for editing.
    pub is_sample: bool,
    /// Cards in this deck that are due now.
    pub due_count: i64,
}

/// One flashcard as sent to the frontend: `{ id, front, back, reps }`.
///
/// `reps` (how many times the card has been reviewed) lets a review say
/// which version of the card it was answering; see [`record_review`].
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct Flashcard {
    pub id: i64,
    pub front: String,
    pub back: String,
    pub reps: i64,
}

/// Result of starting a review:
/// `{ "status": "started", "sessionId": 1 }` or `{ "status": "noneDue" }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StartedSession {
    Started {
        session_id: i64,
    },
    /// Nothing in the deck is due, so no session was created.
    NoneDue,
}

/// What a session shows next:
/// `{ "status": "due", "card": {...} }` or `{ "status": "completed", "cardsReviewed": 3 }`.
#[derive(Debug, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionCard {
    Due { card: Flashcard },
    Completed { cards_reviewed: i64 },
}

/// Why a study operation didn't happen.
#[derive(Debug)]
pub enum StudyError {
    /// The deck or session doesn't exist.
    NotFound,
    /// The deck has been archived, so it can't be reviewed.
    DeckArchived,
    /// The request no longer matches the data: the session has ended, or the
    /// card isn't in this deck, isn't due, or was already reviewed since it
    /// was shown (e.g. a double-click or a late duplicate call).
    Stale,
    /// Anything else (database or FSRS failure). Detail is for logs only.
    Internal(DbError),
}

impl From<sqlx::Error> for StudyError {
    fn from(err: sqlx::Error) -> Self {
        StudyError::Internal(err.into())
    }
}

impl From<DbError> for StudyError {
    fn from(err: DbError) -> Self {
        StudyError::Internal(err)
    }
}

/// Every active (not archived) deck with how many of its cards are due at `now`.
pub async fn decks(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<DeckSummary>, sqlx::Error> {
    sqlx::query_as::<_, DeckSummary>(
        "SELECT d.id, d.name, d.description, d.is_sample,
                (SELECT COUNT(*) FROM flashcards f
                 WHERE f.deck_id = d.id AND f.deleted_at IS NULL
                   AND (f.due IS NULL OR f.due <= ?1)) AS due_count
         FROM decks d
         WHERE d.archived_at IS NULL
         ORDER BY d.id",
    )
    .bind(to_db_time(now))
    .fetch_all(pool)
    .await
}

/// The deck's next due card at `now`, if any.
///
/// Order: cards already in review (earliest `due` first), then new cards
/// (`due` is NULL); ties are broken by `id`, so the choice is deterministic.
async fn next_due_card(
    conn: &mut SqliteConnection,
    deck_id: i64,
    now: &str,
) -> Result<Option<Flashcard>, sqlx::Error> {
    sqlx::query_as::<_, Flashcard>(
        "SELECT id, front, back, reps FROM flashcards
         WHERE deck_id = ?1 AND deleted_at IS NULL AND (due IS NULL OR due <= ?2)
         ORDER BY due IS NULL, due, id
         LIMIT 1",
    )
    .bind(deck_id)
    .bind(now)
    .fetch_optional(conn)
    .await
}

/// Marks a session finished. The `ended_at IS NULL` condition makes this a
/// no-op for a session that already ended, so its end time is set only once.
async fn finish_session(
    conn: &mut SqliteConnection,
    session_id: i64,
    now: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET ended_at = ?1 WHERE id = ?2 AND ended_at IS NULL")
        .bind(now)
        .bind(session_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Finishes the deck's unfinished session, if it has one. Its
/// `cards_reviewed` count is kept as it was.
pub(crate) async fn finish_open_session(
    conn: &mut SqliteConnection,
    deck_id: i64,
    now: &str,
) -> Result<(), sqlx::Error> {
    let active: Option<i64> =
        sqlx::query_scalar("SELECT id FROM sessions WHERE deck_id = ?1 AND ended_at IS NULL")
            .bind(deck_id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(session_id) = active {
        finish_session(conn, session_id, now).await?;
    }
    Ok(())
}

/// Finishes the deck's unfinished session if none of its cards are due any
/// more. Called in the same transaction that deletes a card, so deleting the
/// last due card ends the session just as reviewing it would.
pub(crate) async fn finish_session_if_nothing_due(
    conn: &mut SqliteConnection,
    deck_id: i64,
    now: &str,
) -> Result<(), sqlx::Error> {
    if next_due_card(conn, deck_id, now).await?.is_some() {
        return Ok(());
    }
    finish_open_session(conn, deck_id, now).await
}

/// Starts (or resumes) a review session for an active deck, if it has due cards.
pub async fn start_session(
    pool: &SqlitePool,
    deck_id: i64,
    now: DateTime<Utc>,
) -> Result<StartedSession, StudyError> {
    let now = to_db_time(now);
    // `BEGIN IMMEDIATE` takes SQLite's write lock up front, so the checks
    // below can't be invalidated by another write before we act on them.
    // Returning early drops `tx`, which rolls the transaction back.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // `None` if there's no such deck, otherwise whether it's archived.
    let archived: Option<bool> =
        sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM decks WHERE id = ?1")
            .bind(deck_id)
            .fetch_optional(&mut *tx)
            .await?;
    match archived {
        None => return Err(StudyError::NotFound),
        Some(true) => return Err(StudyError::DeckArchived),
        Some(false) => {}
    }

    let has_due = next_due_card(&mut tx, deck_id, &now).await?.is_some();
    let active: Option<i64> =
        sqlx::query_scalar("SELECT id FROM sessions WHERE deck_id = ?1 AND ended_at IS NULL")
            .bind(deck_id)
            .fetch_optional(&mut *tx)
            .await?;

    let result = match (active, has_due) {
        // An unfinished session (e.g. the app was closed mid-review) resumes.
        (Some(session_id), true) => StartedSession::Started { session_id },
        // An unfinished session with nothing left to do is simply finished.
        (Some(session_id), false) => {
            finish_session(&mut tx, session_id, &now).await?;
            StartedSession::NoneDue
        }
        (None, true) => {
            let session_id = sqlx::query_scalar(
                "INSERT INTO sessions (deck_id, started_at) VALUES (?1, ?2) RETURNING id",
            )
            .bind(deck_id)
            .bind(&now)
            .fetch_one(&mut *tx)
            .await?;
            StartedSession::Started { session_id }
        }
        // Never create an empty session.
        (None, false) => StartedSession::NoneDue,
    };

    tx.commit().await?;
    Ok(result)
}

/// The session's next due card, or its completed state. If the session is
/// still open but its deck has no due cards left, this finishes it.
pub async fn session_card(
    pool: &SqlitePool,
    session_id: i64,
    now: DateTime<Utc>,
) -> Result<SessionCard, StudyError> {
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let session: Option<(i64, Option<String>, i64)> =
        sqlx::query_as("SELECT deck_id, ended_at, cards_reviewed FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((deck_id, ended_at, cards_reviewed)) = session else {
        return Err(StudyError::NotFound);
    };

    if ended_at.is_none() {
        if let Some(card) = next_due_card(&mut tx, deck_id, &now).await? {
            tx.commit().await?;
            return Ok(SessionCard::Due { card });
        }
        finish_session(&mut tx, session_id, &now).await?;
    }

    tx.commit().await?;
    Ok(SessionCard::Completed { cards_reviewed })
}

/// A card's scheduling columns, as read inside a review transaction.
#[derive(sqlx::FromRow)]
struct ScheduleRow {
    fsrs_state: String,
    fsrs_stability: Option<f64>,
    fsrs_difficulty: Option<f64>,
    last_review: Option<String>,
    reps: i64,
    lapses: i64,
}

impl ScheduleRow {
    fn into_schedule(self) -> Result<Schedule, DbError> {
        let state = CardState::parse(&self.fsrs_state)
            .ok_or_else(|| format!("unknown fsrs_state {:?}", self.fsrs_state))?;
        // Both halves of the memory state are written together; a card has
        // one only after its first review.
        let memory = match (self.fsrs_stability, self.fsrs_difficulty) {
            (Some(stability), Some(difficulty)) => Some(MemoryState {
                stability: stability as f32,
                difficulty: difficulty as f32,
            }),
            _ => None,
        };
        let last_review = match self.last_review {
            Some(text) => Some(DateTime::parse_from_rfc3339(&text)?.with_timezone(&Utc)),
            None => None,
        };
        Ok(Schedule {
            state,
            memory,
            last_review,
            reps: self.reps,
            lapses: self.lapses,
        })
    }
}

/// Records one review within a session. In a single transaction it:
/// runs FSRS, updates the card, appends a `review_logs` row, adds one to the
/// session's `cards_reviewed`, and finishes the session if that was the
/// deck's last due card. Either all of that is saved or none of it is.
///
/// `expected_reps` is the card's `reps` as the frontend last saw it. If the
/// card has been reviewed since (a double submit or a late duplicate call),
/// the numbers no longer match and nothing is written.
pub async fn record_review(
    pool: &SqlitePool,
    session_id: i64,
    card_id: i64,
    expected_reps: i64,
    rating: Rating,
    now: DateTime<Utc>,
) -> Result<(), StudyError> {
    let now_time = now;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let session: Option<(i64, bool)> =
        sqlx::query_as("SELECT deck_id, ended_at IS NOT NULL FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
    let (deck_id, ended) = session.ok_or(StudyError::NotFound)?;
    if ended {
        return Err(StudyError::Stale);
    }

    let row = sqlx::query_as::<_, ScheduleRow>(
        "SELECT fsrs_state, fsrs_stability, fsrs_difficulty, last_review, reps, lapses
         FROM flashcards
         WHERE id = ?1 AND deck_id = ?2 AND reps = ?3 AND deleted_at IS NULL
           AND (due IS NULL OR due <= ?4)",
    )
    .bind(card_id)
    .bind(deck_id)
    .bind(expected_reps)
    .bind(&now)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(StudyError::Stale);
    };

    let current = row.into_schedule()?;
    let outcome = scheduler::review(&current, rating, now_time)?;

    let updated = sqlx::query(
        "UPDATE flashcards
         SET fsrs_state = ?1, fsrs_stability = ?2, fsrs_difficulty = ?3,
             due = ?4, last_review = ?5, reps = ?6, lapses = ?7, updated_at = ?5
         WHERE id = ?8 AND reps = ?9 AND deleted_at IS NULL",
    )
    .bind(outcome.state.as_str())
    .bind(f64::from(outcome.memory.stability))
    .bind(f64::from(outcome.memory.difficulty))
    .bind(to_db_time(outcome.due))
    .bind(&now)
    .bind(outcome.reps)
    .bind(outcome.lapses)
    .bind(card_id)
    .bind(expected_reps)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StudyError::Stale);
    }

    sqlx::query(
        "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                  elapsed_days, stability_after, difficulty_after, reviewed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(card_id)
    .bind(rating.number())
    .bind(current.state.as_str())
    .bind(outcome.scheduled_days)
    .bind(outcome.elapsed_days)
    .bind(f64::from(outcome.memory.stability))
    .bind(f64::from(outcome.memory.difficulty))
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    let counted = sqlx::query(
        "UPDATE sessions SET cards_reviewed = cards_reviewed + 1
         WHERE id = ?1 AND ended_at IS NULL",
    )
    .bind(session_id)
    .execute(&mut *tx)
    .await?;
    if counted.rows_affected() != 1 {
        return Err(StudyError::Stale);
    }

    // Finishing here, in the same transaction, means closing the app right
    // after the last rating can't leave the session open.
    if next_due_card(&mut tx, deck_id, &now).await?.is_none() {
        finish_session(&mut tx, session_id, &now).await?;
    }

    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::*;
    use crate::db::{migrate_and_seed, open_file};
    use chrono::TimeDelta;

    const NOW: &str = "2026-09-15T12:00:00Z";

    async fn seeded() -> (SqlitePool, i64) {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        let deck = sample_deck_id(&pool).await;
        (pool, deck)
    }

    async fn add_deck(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar("INSERT INTO decks (name) VALUES (?1) RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Adds a card to `deck`; `due: None` makes it a new card.
    async fn add_card(pool: &SqlitePool, deck: i64, due: Option<DateTime<Utc>>) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO flashcards (deck_id, front, back, fsrs_state, due)
             VALUES (?1, 'q', 'a', CASE WHEN ?2 IS NULL THEN 'New' ELSE 'Review' END, ?2)
             RETURNING id",
        )
        .bind(deck)
        .bind(due.map(to_db_time))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn start(pool: &SqlitePool, deck: i64, now: DateTime<Utc>) -> i64 {
        match start_session(pool, deck, now).await.unwrap() {
            StartedSession::Started { session_id } => session_id,
            StartedSession::NoneDue => panic!("expected a session to start"),
        }
    }

    async fn due_card(pool: &SqlitePool, session: i64, now: DateTime<Utc>) -> Option<Flashcard> {
        match session_card(pool, session, now).await.unwrap() {
            SessionCard::Due { card } => Some(card),
            SessionCard::Completed { .. } => None,
        }
    }

    async fn session_row(pool: &SqlitePool, id: i64) -> (i64, String, Option<String>, i64) {
        sqlx::query_as(
            "SELECT deck_id, started_at, ended_at, cards_reviewed FROM sessions WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[test]
    fn dashboard_counts_due_cards_per_deck() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);
            let other = add_deck(&pool, "Other").await;
            add_card(&pool, other, Some(now - TimeDelta::hours(1))).await;
            add_card(&pool, other, Some(now + TimeDelta::days(1))).await;

            let summary: Vec<(i64, String, i64)> = decks(&pool, now)
                .await
                .unwrap()
                .into_iter()
                .map(|d| (d.id, d.name, d.due_count))
                .collect();
            assert_eq!(
                summary,
                vec![
                    (sample, "Sample deck".into(), 1),
                    (other, "Other".into(), 1)
                ]
            );
        });
    }

    #[test]
    fn no_session_is_created_when_nothing_is_due() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);

            // A deck with only a future card, and a deck with no cards at all.
            let later = add_deck(&pool, "Later").await;
            add_card(&pool, later, Some(now + TimeDelta::days(1))).await;
            let empty = add_deck(&pool, "Empty").await;
            for deck in [later, empty] {
                assert_eq!(
                    start_session(&pool, deck, now).await.unwrap(),
                    StartedSession::NoneDue
                );
            }
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 0);

            // After the sample deck's only card is reviewed, it has nothing due either.
            let session = start(&pool, sample, now).await;
            record_review(&pool, session, 1, 0, Rating::Good, now)
                .await
                .unwrap();
            assert_eq!(
                start_session(&pool, sample, now).await.unwrap(),
                StartedSession::NoneDue
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);

            assert!(matches!(
                start_session(&pool, 999, now).await,
                Err(StudyError::NotFound)
            ));
        });
    }

    #[test]
    fn session_starts_when_a_card_is_due_and_resumes_if_unfinished() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);

            let session = start(&pool, sample, now).await;
            assert_eq!(
                session_row(&pool, session).await,
                (sample, "2026-09-15T12:00:00.000Z".to_string(), None, 0)
            );
            let card = due_card(&pool, session, now).await.unwrap();
            assert_eq!((card.id, card.reps), (1, 0));

            // Starting again (e.g. after a restart mid-review) resumes the same session.
            assert_eq!(
                start(&pool, sample, now + TimeDelta::minutes(5)).await,
                session
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);
        });
    }

    #[test]
    fn review_logs_once_counts_once_and_completes_the_session_at_the_end() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);
            add_card(&pool, sample, None).await; // card 2: a second due card

            let session = start(&pool, sample, now).await;
            record_review(&pool, session, 1, 0, Rating::Good, now)
                .await
                .unwrap();

            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 1);
            let (_, _, ended_at, reviewed) = session_row(&pool, session).await;
            assert_eq!((ended_at, reviewed), (None, 1));
            let (state, reps, due): (String, i64, String) =
                sqlx::query_as("SELECT fsrs_state, reps, due FROM flashcards WHERE id = 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!((state.as_str(), reps), ("Review", 1));
            assert!(due > to_db_time(now));

            // The next due card is card 2; reviewing it exhausts the queue.
            assert_eq!(due_card(&pool, session, now).await.unwrap().id, 2);
            record_review(&pool, session, 2, 0, Rating::Easy, now)
                .await
                .unwrap();

            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 2);
            let (_, _, ended_at, reviewed) = session_row(&pool, session).await;
            assert_eq!(
                (ended_at.as_deref(), reviewed),
                (Some("2026-09-15T12:00:00.000Z"), 2)
            );
            assert!(matches!(
                session_card(&pool, session, now).await.unwrap(),
                SessionCard::Completed { cards_reviewed: 2 }
            ));
        });
    }

    #[test]
    fn stale_or_duplicate_reviews_write_nothing() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);
            add_card(&pool, sample, None).await; // card 2 keeps the session open
            let other = add_deck(&pool, "Other").await;
            let foreign = add_card(&pool, other, None).await;
            let future = add_card(&pool, sample, Some(now + TimeDelta::days(3))).await;

            let session = start(&pool, sample, now).await;
            record_review(&pool, session, 1, 0, Rating::Good, now)
                .await
                .unwrap();
            let snapshot = (
                count(&pool, "SELECT COUNT(*) FROM review_logs").await,
                session_row(&pool, session).await,
            );

            let attempts = [
                (1, 0),       // duplicate of the review that just succeeded
                (2, 5),       // wrong expected reps
                (foreign, 0), // a card from another deck
                (future, 0),  // a card that isn't due
                (12345, 0),   // a card that doesn't exist
            ];
            for (card, reps) in attempts {
                let result = record_review(&pool, session, card, reps, Rating::Good, now).await;
                assert!(matches!(result, Err(StudyError::Stale)), "card {card}");
            }
            assert!(matches!(
                record_review(&pool, 999, 2, 0, Rating::Good, now).await,
                Err(StudyError::NotFound)
            ));

            let after = (
                count(&pool, "SELECT COUNT(*) FROM review_logs").await,
                session_row(&pool, session).await,
            );
            assert_eq!(after, snapshot);
            assert_eq!(after.1 .3, 1);
        });
    }

    #[test]
    fn reviews_on_a_completed_session_are_rejected() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);
            let session = start(&pool, sample, now).await;
            record_review(&pool, session, 1, 0, Rating::Again, now)
                .await
                .unwrap();

            // The session completed; even once the card is due again, the old session
            // can't record more reviews.
            let later = now + TimeDelta::days(1);
            let result = record_review(&pool, session, 1, 1, Rating::Good, later).await;
            assert!(matches!(result, Err(StudyError::Stale)));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 1);
            assert_eq!(session_row(&pool, session).await.3, 1);
        });
    }

    #[test]
    fn completing_again_keeps_the_first_end_time() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);
            let session = start(&pool, sample, now).await;

            // Finish the queue outside the review path (e.g. the card was rescheduled),
            // then request completion repeatedly at later times.
            sqlx::query("UPDATE flashcards SET due = ?1")
                .bind(to_db_time(now + TimeDelta::days(9)))
                .execute(&pool)
                .await
                .unwrap();
            for minutes in [1, 2, 60] {
                let when = now + TimeDelta::minutes(minutes);
                assert!(matches!(
                    session_card(&pool, session, when).await.unwrap(),
                    SessionCard::Completed { cards_reviewed: 0 }
                ));
            }

            let (_, _, ended_at, reviewed) = session_row(&pool, session).await;
            assert_eq!(
                (ended_at.as_deref(), reviewed),
                (Some("2026-09-15T12:01:00.000Z"), 0)
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);
            assert!(matches!(
                session_card(&pool, 999, now).await,
                Err(StudyError::NotFound)
            ));
        });
    }

    #[test]
    fn due_selection_is_deck_scoped_and_deterministic() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await; // card 1: new, in the sample deck
            let now = at(NOW);
            let yesterday = now - TimeDelta::days(1);
            let other = add_deck(&pool, "Other").await;

            let _earlier_elsewhere = add_card(&pool, other, Some(now - TimeDelta::days(5))).await;
            let _future = add_card(&pool, sample, Some(now + TimeDelta::days(2))).await;
            let tie_low = add_card(&pool, sample, Some(yesterday)).await;
            let tie_high = add_card(&pool, sample, Some(yesterday)).await;

            let session = start(&pool, sample, now).await;
            // Overdue before new, equal due times by lower id, other decks ignored.
            let order = [tie_low, tie_high, 1];
            for (i, expected) in order.into_iter().enumerate() {
                let card = due_card(&pool, session, now).await.unwrap();
                assert_eq!(card.id, expected, "position {i}");
                record_review(&pool, session, card.id, card.reps, Rating::Good, now)
                    .await
                    .unwrap();
            }
            assert!(due_card(&pool, session, now).await.is_none());
        });
    }

    #[test]
    fn failed_review_leaves_no_partial_changes() {
        tauri::async_runtime::block_on(async {
            let (pool, sample) = seeded().await;
            let now = at(NOW);
            let session = start(&pool, sample, now).await;
            let card_before: (String, i64, Option<String>) =
                sqlx::query_as("SELECT fsrs_state, reps, due FROM flashcards WHERE id = 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();

            // Make the session count update (after the card update and log insert) fail.
            sqlx::query(
                "CREATE TRIGGER fail_count BEFORE UPDATE OF cards_reviewed ON sessions
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            let result = record_review(&pool, session, 1, 0, Rating::Good, now).await;
            assert!(matches!(result, Err(StudyError::Internal(_))));

            let card_after: (String, i64, Option<String>) =
                sqlx::query_as("SELECT fsrs_state, reps, due FROM flashcards WHERE id = 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(card_after, card_before);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 0);
            assert_eq!(session_row(&pool, session).await.2, None);
        });
    }

    #[test]
    fn study_state_persists_across_reopening_the_database_file() {
        tauri::async_runtime::block_on(async {
            // A real file (under the build's `target/` folder), closed and reopened
            // like an app restart.
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("persist");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");
            let now = at(NOW);

            let pool = open_file(&path).await.unwrap();
            let deck = sample_deck_id(&pool).await;
            let session = start(&pool, deck, now).await;
            record_review(&pool, session, 1, 0, Rating::Good, now)
                .await
                .unwrap();
            assert!(matches!(
                session_card(&pool, session, now).await.unwrap(),
                SessionCard::Completed { cards_reviewed: 1 }
            ));
            pool.close().await;

            let pool = open_file(&path).await.unwrap();
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 1);
            assert_eq!(
                session_row(&pool, session).await,
                (
                    deck,
                    "2026-09-15T12:00:00.000Z".into(),
                    Some("2026-09-15T12:00:00.000Z".into()),
                    1
                )
            );
            let (state, reps): (String, i64) =
                sqlx::query_as("SELECT fsrs_state, reps FROM flashcards WHERE id = 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!((state.as_str(), reps), ("Review", 1));
            let summary = decks(&pool, now).await.unwrap();
            assert_eq!(summary[0].due_count, 0);
            assert_eq!(
                start_session(&pool, deck, now).await.unwrap(),
                StartedSession::NoneDue
            );
            pool.close().await;

            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
