//! The local SQLite database: where the file lives, opening it, running
//! migrations, and seeding the example card. Study queries live in `study.rs`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use tauri::async_runtime::Mutex;
use tauri::{AppHandle, Manager};

/// Folder inside the OS data directory, e.g. `%APPDATA%\synapse` on Windows.
const DB_FOLDER: &str = "synapse";
const DB_FILE: &str = "synapse.sqlite";

/// The folder beside the database where an import checks a backup before it
/// replaces anything (see `import.rs`). Only files Synapse names itself are
/// ever written or removed there.
const IMPORT_WORKSPACE: &str = "import-workspace";

/// The example card inserted into the sample deck of a new database.
const SEED_FRONT: &str = "What is active recall?";
const SEED_BACK: &str = "Retrieving an answer from memory before checking it. \
It strengthens memory far more than rereading.";

/// The `seed_markers` row recording that the example card was seeded. Migration
/// 0005 writes the same name for databases that were seeded before it existed.
const SAMPLE_CARD_SEED: &str = "sample_card";

/// Internal errors carry full detail for the developer console only.
/// Commands turn them into short, user-safe messages before they reach React.
pub type DbError = Box<dyn std::error::Error + Send + Sync>;

/// Database state shared by all commands (registered with `.manage()`).
///
/// The pool starts empty and is opened on first use. If opening fails it
/// stays empty, so the next call (e.g. the UI's Retry button) tries again.
/// It's an async `Mutex` because we hold the lock across `.await`s while
/// opening, so two simultaneous first calls can't both open the database.
#[derive(Default)]
pub struct Database {
    pool: Mutex<Option<SqlitePool>>,
}

impl Database {
    /// Returns the connection pool, opening and migrating the database first
    /// if this is the first call. `SqlitePool` is a cheap handle to shared
    /// connections, so cloning it doesn't copy anything heavy.
    pub async fn pool(&self, app: &AppHandle) -> Result<SqlitePool, DbError> {
        let mut slot = self.pool.lock().await;
        if let Some(pool) = slot.as_ref() {
            return Ok(pool.clone());
        }
        let path = Self::file_path(app)?;
        let pool = open_file(&path).await?;
        eprintln!("[synapse] database ready at {}", path.display());
        *slot = Some(pool.clone());
        Ok(pool)
    }

    /// Where `synapse.sqlite` lives.
    pub fn file_path(app: &AppHandle) -> Result<PathBuf, DbError> {
        Ok(database_dir(app)?.join(DB_FILE))
    }

    /// The private folder an import prepares a backup in, beside the database
    /// (so moving a checked backup into place is a rename on the same drive).
    pub fn import_workspace(app: &AppHandle) -> Result<PathBuf, DbError> {
        Ok(database_dir(app)?.join(IMPORT_WORKSPACE))
    }

    /// The slot holding the open pool. Restoring a backup locks it for the
    /// whole replacement, so no command can use the old database or reopen
    /// it halfway through; commands that arrive meanwhile wait, then get the
    /// new pool.
    pub(crate) fn slot(&self) -> &Mutex<Option<SqlitePool>> {
        &self.pool
    }

    /// Whether `path` is this database's own file, or one of the sidecar
    /// files SQLite keeps beside it (`-journal`, `-wal`, `-shm`). Used to
    /// refuse writing an export over any of them, which would destroy or
    /// corrupt every card.
    ///
    /// Folders are compared after resolving `..` and symlinks where possible.
    /// The database's folder always exists by the time this is asked, so the
    /// fallback only applies to a destination whose folder doesn't exist —
    /// which by definition isn't next to the database.
    pub fn is_database_file(&self, app: &AppHandle, path: &Path) -> bool {
        let Ok(dir) = database_dir(app) else {
            return false;
        };
        let resolve =
            |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        // Same folder, and a name SQLite owns.
        resolve(&dir) == resolve(folder) && name.to_string_lossy().starts_with(DB_FILE)
    }
}

/// The folder holding `synapse.sqlite`.
fn database_dir(app: &AppHandle) -> Result<PathBuf, DbError> {
    // Development builds only: `SYNAPSE_DB_DIR` points the app at a separate
    // database so testing never touches the everyday one. Release builds
    // compile this out and always use the OS data directory.
    // An empty value is ignored: it would otherwise mean "the working
    // directory", which is never what clearing the variable is meant to do.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("SYNAPSE_DB_DIR").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }

    // Tauri resolves the per-user data directory for the current OS,
    // so no user-specific path is hardcoded here.
    Ok(app.path().data_dir()?.join(DB_FOLDER))
}

/// Creates (if needed), opens, migrates, and seeds the database at `path`.
pub async fn open_file(path: &Path) -> Result<SqlitePool, DbError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(options).await?;
    if let Err(err) = migrate_and_seed(&pool).await {
        // Close before reporting, rather than leaving the connections to be
        // dropped in the background: a restore that has to put the previous
        // database back must know this file is no longer open.
        pool.close().await;
        return Err(err);
    }
    Ok(pool)
}

/// The migrations built into this Synapse, but only up to `version`. Used to
/// recreate the exact schema an older build left behind, which is what an
/// imported backup at that version must match.
pub fn migrations_up_to(version: i64) -> Migrator {
    let mut migrator = sqlx::migrate!();
    migrator.migrations = migrator
        .migrations
        .iter()
        .filter(|m| m.version <= version)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    migrator
}

/// The newest schema this build knows: its highest migration version.
pub fn latest_schema_version() -> i64 {
    sqlx::migrate!()
        .iter()
        .map(|m| m.version)
        .max()
        .unwrap_or(0)
}

/// Brings the schema up to date and seeds the example card. Safe to run on
/// every launch:
/// - sqlx records applied migrations in `_sqlx_migrations` and skips them.
///   Migration 0003 creates the sample deck, so it exists exactly once.
/// - The example card is seeded at most once per database, ever. The seed
///   first claims its row in `seed_markers`; only a successful claim inserts
///   the card, in the same transaction, so the marker never exists without
///   the card. Once the marker exists nothing is seeded again, even if every
///   card is deleted later.
pub async fn migrate_and_seed(pool: &SqlitePool) -> Result<(), DbError> {
    // `migrate!` embeds the files in `src-tauri/migrations/` at compile time.
    sqlx::migrate!().run(pool).await?;

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let claimed =
        sqlx::query("INSERT INTO seed_markers (name) VALUES (?1) ON CONFLICT (name) DO NOTHING")
            .bind(SAMPLE_CARD_SEED)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;

    if claimed {
        sqlx::query(
            "INSERT INTO flashcards (deck_id, front, back)
             SELECT id, ?1, ?2 FROM decks WHERE is_sample = 1",
        )
        .bind(SEED_FRONT)
        .bind(SEED_BACK)
        .execute(&mut *tx)
        .await?;
    }

    // Dropping `tx` on an error above rolls back the claim too.
    tx.commit().await?;
    Ok(())
}

/// Formats a time the way it's stored: ISO-8601 UTC with milliseconds, e.g.
/// `2026-09-15T12:00:00.000Z` (the same shape SQLite's `created_at` default
/// produces). One fixed format means text comparison orders times correctly.
pub fn to_db_time(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Helpers shared by the test modules in this crate.
#[cfg(test)]
pub mod test_support {
    pub use super::migrations_up_to;
    use super::*;

    /// An in-memory database. One connection only, because every new
    /// in-memory connection would otherwise be a separate, empty database.
    pub async fn memory_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    pub fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    // sqlx 0.9 only accepts `'static` SQL text, which rules out building
    // queries from runtime strings (a guard against SQL injection).
    pub async fn count(pool: &SqlitePool, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    pub async fn sample_deck_id(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT id FROM decks WHERE is_sample = 1")
            .fetch_one(pool)
            .await
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    type CardRow = (
        i64,
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

    /// Every Slice 1–2 column of every card, for exact before/after comparison.
    async fn all_cards(pool: &SqlitePool) -> Vec<CardRow> {
        sqlx::query_as(
            "SELECT id, front, back, fsrs_state, fsrs_stability, fsrs_difficulty,
                    due, last_review, reps, lapses, created_at, updated_at
             FROM flashcards ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn seed_markers(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT name FROM seed_markers ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[test]
    fn fresh_setup_creates_one_sample_deck_with_one_linked_card() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            migrate_and_seed(&pool).await.unwrap();

            let decks: Vec<(i64, String, bool)> =
                sqlx::query_as("SELECT id, name, is_sample FROM decks")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(decks.len(), 1);
            let (deck_id, name, is_sample) = &decks[0];
            assert_eq!(name, "Sample deck");
            assert!(is_sample);

            let cards: Vec<(Option<i64>, String, String, String)> =
                sqlx::query_as("SELECT deck_id, front, back, fsrs_state FROM flashcards")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                cards,
                vec![(
                    Some(*deck_id),
                    SEED_FRONT.to_string(),
                    SEED_BACK.to_string(),
                    "New".to_string()
                )]
            );
            assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 0);
        });
    }

    #[test]
    fn repeated_setup_duplicates_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            migrate_and_seed(&pool).await.unwrap();

            // Add some history, then "relaunch" several times.
            let now = at("2026-09-15T12:00:00Z");
            let deck = sample_deck_id(&pool).await;
            let crate::study::StartedSession::Started { session_id } =
                crate::study::start_session(&pool, deck, now).await.unwrap()
            else {
                panic!("the seeded card should be due");
            };
            crate::study::record_review(
                &pool,
                session_id,
                1,
                0,
                crate::scheduler::Rating::Good,
                now,
            )
            .await
            .unwrap();
            let cards_before = all_cards(&pool).await;

            for _ in 0..3 {
                migrate_and_seed(&pool).await.unwrap();
            }

            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 1);
            assert_eq!(all_cards(&pool).await, cards_before);
        });
    }

    #[test]
    fn migrates_forward_from_a_slice_1_database() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What Slice 1 left behind: migration 0001 plus the seeded card.
            migrations_up_to(1).run(&pool).await.unwrap();
            sqlx::query("INSERT INTO flashcards (front, back) VALUES (?1, ?2)")
                .bind(SEED_FRONT)
                .bind(SEED_BACK)
                .execute(&pool)
                .await
                .unwrap();

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 1);
            let (state, reps, deck_id): (String, i64, Option<i64>) =
                sqlx::query_as("SELECT fsrs_state, reps, deck_id FROM flashcards WHERE id = 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!((state.as_str(), reps), ("New", 0));
            assert_eq!(deck_id, Some(sample_deck_id(&pool).await));
        });
    }

    #[test]
    fn migrates_forward_from_a_slice_2_database_without_data_loss() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What Slice 2 left behind: migrations 0001–0002, the seeded card
            // after one "Good" review, and that review's log row.
            migrations_up_to(2).run(&pool).await.unwrap();
            sqlx::query(
                "INSERT INTO flashcards (front, back, fsrs_state, fsrs_stability, fsrs_difficulty,
                                         due, last_review, reps, lapses, created_at, updated_at)
                 VALUES (?1, ?2, 'Review', 2.3065, 2.1181, '2026-09-17T21:50:14.690Z',
                         '2026-09-15T14:28:53.094Z', 1, 0, '2026-09-15T14:11:51.067Z',
                         '2026-09-15T14:28:53.094Z')",
            )
            .bind(SEED_FRONT)
            .bind(SEED_BACK)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (1, 3, 'New', 2.3065, 0, 2.3065, 2.1181, '2026-09-15T14:28:53.094Z')",
            )
            .execute(&pool)
            .await
            .unwrap();
            let cards_before = all_cards(&pool).await;
            let logs_sql = "SELECT id, card_id, rating, state_before, scheduled_days, elapsed_days,
                                   stability_after, difficulty_after, reviewed_at
                            FROM review_logs ORDER BY id";
            type LogRow = (i64, i64, i64, String, f64, f64, f64, f64, String);
            let logs_before: Vec<LogRow> = sqlx::query_as(logs_sql).fetch_all(&pool).await.unwrap();

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);
            assert_eq!(all_cards(&pool).await, cards_before);
            let logs_after: Vec<LogRow> = sqlx::query_as(logs_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(logs_after, logs_before);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sessions").await, 0);
            let deck_id: Option<i64> =
                sqlx::query_scalar("SELECT deck_id FROM flashcards WHERE id = 1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(deck_id, Some(sample_deck_id(&pool).await));
        });
    }

    #[test]
    fn migrates_forward_from_a_slice_3_database_without_data_loss() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What Slice 3 left behind: migrations 0001–0003, the seeded card in
            // the sample deck after one "Good" review, its log row, and the
            // finished session. Written as plain SQL, because today's study code
            // expects the newer schema.
            migrations_up_to(3).run(&pool).await.unwrap();
            insert_reviewed_seed_card(&pool).await;
            sqlx::query(
                "INSERT INTO sessions (deck_id, started_at, ended_at, cards_reviewed)
                 SELECT id, '2026-09-15T11:59:30.000Z', '2026-09-15T12:00:00.000Z', 1
                 FROM decks WHERE is_sample = 1",
            )
            .execute(&pool)
            .await
            .unwrap();

            type DeckRow = (i64, String, Option<String>, bool, String);
            type SessionRow = (i64, i64, String, Option<String>, i64);
            let decks_sql = "SELECT id, name, description, is_sample, created_at FROM decks";
            let sessions_sql =
                "SELECT id, deck_id, started_at, ended_at, cards_reviewed FROM sessions";
            let decks_before: Vec<DeckRow> =
                sqlx::query_as(decks_sql).fetch_all(&pool).await.unwrap();
            let sessions_before: Vec<SessionRow> =
                sqlx::query_as(sessions_sql).fetch_all(&pool).await.unwrap();
            let cards_before = all_cards(&pool).await;

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            let decks_after: Vec<DeckRow> =
                sqlx::query_as(decks_sql).fetch_all(&pool).await.unwrap();
            let sessions_after: Vec<SessionRow> =
                sqlx::query_as(sessions_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(decks_after, decks_before);
            assert_eq!(sessions_after, sessions_before);
            assert_eq!(all_cards(&pool).await, cards_before);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 1);
            assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);

            // The new rules now apply to the upgraded database.
            let duplicate = sqlx::query("INSERT INTO decks (name) VALUES ('sample DECK')")
                .execute(&pool)
                .await;
            assert!(duplicate.is_err());
        });
    }

    /// The seeded card after one "Good" review, and that review's log row, as
    /// SQL that works on any schema from Slice 3 onwards.
    async fn insert_reviewed_seed_card(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                     fsrs_difficulty, due, last_review, reps, lapses,
                                     created_at, updated_at)
             SELECT id, ?1, ?2, 'Review', 2.3065, 2.1181, '2026-09-17T19:21:21.600Z',
                    '2026-09-15T12:00:00.000Z', 1, 0, '2026-09-15T11:59:00.000Z',
                    '2026-09-15T12:00:00.000Z'
             FROM decks WHERE is_sample = 1",
        )
        .bind(SEED_FRONT)
        .bind(SEED_BACK)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                      elapsed_days, stability_after, difficulty_after, reviewed_at)
             VALUES (1, 3, 'New', 2.3065, 0, 2.3065, 2.1181, '2026-09-15T12:00:00.000Z')",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn migrates_forward_from_a_slice_4a_database_without_data_loss() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What Slice 4A left behind: migrations 0001–0004, the reviewed seed
            // card, a user deck with a reviewed and a new card, both review logs,
            // and an unfinished session on the user deck.
            migrations_up_to(4).run(&pool).await.unwrap();
            insert_reviewed_seed_card(&pool).await;
            for sql in [
                "INSERT INTO decks (name, description, is_sample, created_at)
                 VALUES ('Biology', 'Cells', 0, '2026-09-15T12:01:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'Line one' || char(10) || 'Line two', 'Answer', 'Review', 8.2956,
                         1.0, '2026-09-23T19:06:00.000Z', '2026-09-15T12:03:00.000Z', 1, 0,
                         '2026-09-15T12:02:00.000Z', '2026-09-15T12:03:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at)
                 VALUES (2, 'Q2', 'A2', '2026-09-15T12:02:30.000Z', '2026-09-15T12:02:30.000Z')",
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (2, 4, 'New', 8.2956, 0, 8.2956, 1.0, '2026-09-15T12:03:00.000Z')",
                "INSERT INTO sessions (deck_id, started_at, cards_reviewed)
                 VALUES (2, '2026-09-15T12:02:45.000Z', 1)",
            ] {
                sqlx::query(sql).execute(&pool).await.unwrap();
            }

            type DeckRow = (i64, String, Option<String>, bool, String);
            type SessionRow = (i64, i64, String, Option<String>, i64);
            type LogRow = (i64, i64, i64, String, f64, f64, f64, f64, String);
            let decks_sql = "SELECT id, name, description, is_sample, created_at FROM decks";
            let sessions_sql =
                "SELECT id, deck_id, started_at, ended_at, cards_reviewed FROM sessions";
            let logs_sql = "SELECT id, card_id, rating, state_before, scheduled_days, elapsed_days,
                                   stability_after, difficulty_after, reviewed_at
                            FROM review_logs ORDER BY id";
            let deck_ids_sql = "SELECT deck_id FROM flashcards ORDER BY id";
            let decks_before: Vec<DeckRow> =
                sqlx::query_as(decks_sql).fetch_all(&pool).await.unwrap();
            let sessions_before: Vec<SessionRow> =
                sqlx::query_as(sessions_sql).fetch_all(&pool).await.unwrap();
            let logs_before: Vec<LogRow> = sqlx::query_as(logs_sql).fetch_all(&pool).await.unwrap();
            let deck_ids_before: Vec<Option<i64>> = sqlx::query_scalar(deck_ids_sql)
                .fetch_all(&pool)
                .await
                .unwrap();
            let cards_before = all_cards(&pool).await;

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            // Already seeded, so the marker is recorded and no card is added.
            assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);
            assert_eq!(all_cards(&pool).await, cards_before);
            let deck_ids_after: Vec<Option<i64>> = sqlx::query_scalar(deck_ids_sql)
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(deck_ids_after, deck_ids_before);
            let decks_after: Vec<DeckRow> =
                sqlx::query_as(decks_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(decks_after, decks_before);
            let sessions_after: Vec<SessionRow> =
                sqlx::query_as(sessions_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(sessions_after, sessions_before);
            let logs_after: Vec<LogRow> = sqlx::query_as(logs_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(logs_after, logs_before);
            // Every existing card is active.
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM flashcards WHERE deleted_at IS NULL"
                )
                .await,
                3
            );
        });
    }

    #[test]
    fn migrates_forward_from_a_slice_4b_database_without_data_loss() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What Slice 4B left behind: migrations 0001–0005 and its seed
            // marker, the reviewed seed card, a user deck with a reviewed card,
            // a soft-deleted card, and a new card, the review logs, a finished
            // session on the sample deck, and an unfinished one on the user deck.
            migrations_up_to(5).run(&pool).await.unwrap();
            insert_reviewed_seed_card(&pool).await;
            for sql in [
                "INSERT INTO seed_markers (name, recorded_at)
                 VALUES ('sample_card', '2026-09-15T11:59:00.000Z')",
                "INSERT INTO decks (name, description, is_sample, created_at)
                 VALUES ('Biology', 'Cells', 0, '2026-09-15T12:01:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'Q1', 'A1', 'Review', 8.2956, 1.0, '2026-09-23T19:06:00.000Z',
                         '2026-09-15T12:03:00.000Z', 1, 0, '2026-09-15T12:02:00.000Z',
                         '2026-09-15T12:03:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at, deleted_at)
                 VALUES (2, 'Gone', 'gone', '2026-09-15T12:02:10.000Z',
                         '2026-09-15T12:04:00.000Z', '2026-09-15T12:04:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at)
                 VALUES (2, 'Q3', 'A3', '2026-09-15T12:02:30.000Z', '2026-09-15T12:02:30.000Z')",
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (2, 4, 'New', 8.2956, 0, 8.2956, 1.0, '2026-09-15T12:03:00.000Z')",
                "INSERT INTO sessions (deck_id, started_at, ended_at, cards_reviewed)
                 VALUES (1, '2026-09-15T11:59:30.000Z', '2026-09-15T12:00:00.000Z', 1)",
                "INSERT INTO sessions (deck_id, started_at, cards_reviewed)
                 VALUES (2, '2026-09-15T12:02:45.000Z', 1)",
            ] {
                sqlx::query(sql).execute(&pool).await.unwrap();
            }

            type DeckRow = (i64, String, Option<String>, bool, String);
            type SessionRow = (i64, i64, String, Option<String>, i64);
            type LogRow = (i64, i64, i64, String, f64, f64, f64, f64, String);
            let decks_sql = "SELECT id, name, description, is_sample, created_at FROM decks";
            let sessions_sql =
                "SELECT id, deck_id, started_at, ended_at, cards_reviewed FROM sessions";
            let logs_sql = "SELECT id, card_id, rating, state_before, scheduled_days, elapsed_days,
                                   stability_after, difficulty_after, reviewed_at
                            FROM review_logs ORDER BY id";
            let links_sql = "SELECT deck_id, deleted_at FROM flashcards ORDER BY id";
            let markers_sql = "SELECT name, recorded_at FROM seed_markers";
            let decks_before: Vec<DeckRow> =
                sqlx::query_as(decks_sql).fetch_all(&pool).await.unwrap();
            let sessions_before: Vec<SessionRow> =
                sqlx::query_as(sessions_sql).fetch_all(&pool).await.unwrap();
            let logs_before: Vec<LogRow> = sqlx::query_as(logs_sql).fetch_all(&pool).await.unwrap();
            let links_before: Vec<(Option<i64>, Option<String>)> =
                sqlx::query_as(links_sql).fetch_all(&pool).await.unwrap();
            let markers_before: Vec<(String, String)> =
                sqlx::query_as(markers_sql).fetch_all(&pool).await.unwrap();
            let cards_before = all_cards(&pool).await;

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            // Every row is kept as it was, and nothing is seeded again.
            let decks_after: Vec<DeckRow> =
                sqlx::query_as(decks_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(decks_after, decks_before);
            let sessions_after: Vec<SessionRow> =
                sqlx::query_as(sessions_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(sessions_after, sessions_before);
            let logs_after: Vec<LogRow> = sqlx::query_as(logs_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(logs_after, logs_before);
            let links_after: Vec<(Option<i64>, Option<String>)> =
                sqlx::query_as(links_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(links_after, links_before);
            let markers_after: Vec<(String, String)> =
                sqlx::query_as(markers_sql).fetch_all(&pool).await.unwrap();
            assert_eq!(markers_after, markers_before);
            assert_eq!(all_cards(&pool).await, cards_before);

            // Every existing deck is active, and the new rules now apply.
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM decks WHERE archived_at IS NULL"
                )
                .await,
                2
            );
            let err = sqlx::query("DELETE FROM decks WHERE id = 2")
                .execute(&pool)
                .await
                .unwrap_err();
            assert!(
                err.to_string()
                    .contains("decks are archived, never removed"),
                "{err}"
            );
            // The unfinished session resumes as before.
            assert_eq!(
                crate::study::start_session(&pool, 2, at("2026-09-15T12:05:00Z"))
                    .await
                    .unwrap(),
                crate::study::StartedSession::Started { session_id: 2 }
            );
        });
    }

    /// Every column of every study row, each value rendered by SQLite's
    /// `quote()`, for exact before/after comparison.
    async fn study_history(pool: &SqlitePool) -> Vec<Vec<String>> {
        let mut tables = Vec::new();
        for sql in [
            "SELECT quote(id) || '|' || quote(name) || '|' || quote(description) || '|' ||
                    quote(is_sample) || '|' || quote(created_at) || '|' || quote(archived_at)
             FROM decks ORDER BY id",
            "SELECT quote(id) || '|' || quote(deck_id) || '|' || quote(front) || '|' ||
                    quote(back) || '|' || quote(fsrs_stability) || '|' ||
                    quote(fsrs_difficulty) || '|' || quote(fsrs_state) || '|' || quote(due) ||
                    '|' || quote(last_review) || '|' || quote(reps) || '|' || quote(lapses) ||
                    '|' || quote(created_at) || '|' || quote(updated_at) || '|' ||
                    quote(deleted_at)
             FROM flashcards ORDER BY id",
            "SELECT quote(id) || '|' || quote(card_id) || '|' || quote(rating) || '|' ||
                    quote(state_before) || '|' || quote(scheduled_days) || '|' ||
                    quote(elapsed_days) || '|' || quote(stability_after) || '|' ||
                    quote(difficulty_after) || '|' || quote(reviewed_at)
             FROM review_logs ORDER BY id",
            "SELECT quote(id) || '|' || quote(deck_id) || '|' || quote(started_at) || '|' ||
                    quote(ended_at) || '|' || quote(cards_reviewed)
             FROM sessions ORDER BY id",
            "SELECT quote(name) || '|' || quote(recorded_at) FROM seed_markers ORDER BY name",
            "SELECT quote(version) || '|' || quote(checksum) || '|' || quote(success)
             FROM _sqlx_migrations WHERE version <= 6 ORDER BY version",
        ] {
            tables.push(sqlx::query_scalar(sql).fetch_all(pool).await.unwrap());
        }
        tables
    }

    #[test]
    fn migrates_forward_from_a_slice_6_database_without_data_loss() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What Slices 5–6 left behind (export added no schema): migrations
            // 0001–0006 and the seed marker, the reviewed seed card, an active
            // deck with a reviewed card, a soft-deleted card, and a multiline
            // new card with an unfinished session, and an archived deck with
            // its card and finished session.
            migrations_up_to(6).run(&pool).await.unwrap();
            insert_reviewed_seed_card(&pool).await;
            for sql in [
                "INSERT INTO seed_markers (name, recorded_at)
                 VALUES ('sample_card', '2026-09-15T11:59:00.000Z')",
                "INSERT INTO decks (name, description, is_sample, created_at)
                 VALUES ('Biology', 'Cells', 0, '2026-09-15T12:01:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'Q1', 'A1', 'Review', 8.2956, 1.0, '2026-09-23T19:06:00.000Z',
                         '2026-09-15T12:03:00.000Z', 1, 0, '2026-09-15T12:02:00.000Z',
                         '2026-09-15T12:03:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at, deleted_at)
                 VALUES (2, 'Gone', 'gone', '2026-09-15T12:02:10.000Z',
                         '2026-09-15T12:04:00.000Z', '2026-09-15T12:04:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at)
                 VALUES (2, 'Line one' || char(10) || 'Line two', 'A3',
                         '2026-09-15T12:02:30.000Z', '2026-09-15T12:02:30.000Z')",
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (2, 4, 'New', 8.2956, 0, 8.2956, 1.0, '2026-09-15T12:03:00.000Z')",
                "INSERT INTO sessions (deck_id, started_at, cards_reviewed)
                 VALUES (2, '2026-09-15T12:02:45.000Z', 1)",
                "INSERT INTO decks (name, is_sample, created_at)
                 VALUES ('Chemistry', 0, '2026-09-15T12:05:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at)
                 VALUES (3, 'X', 'x', '2026-09-15T12:05:10.000Z', '2026-09-15T12:05:10.000Z')",
                "INSERT INTO sessions (deck_id, started_at, ended_at, cards_reviewed)
                 VALUES (3, '2026-09-15T12:05:20.000Z', '2026-09-15T12:05:30.000Z', 0)",
                "UPDATE decks SET archived_at = '2026-09-15T12:06:00.000Z' WHERE id = 3",
            ] {
                sqlx::query(sql).execute(&pool).await.unwrap();
            }
            let before = study_history(&pool).await;

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            // Every row is kept exactly, nothing is seeded again, and there
            // are no notes yet.
            assert_eq!(study_history(&pool).await, before);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM notes").await, 0);

            // Notes now work, and the old rules still apply.
            crate::notes::create_note(&pool, "First note", "text", at("2026-09-16T12:00:00Z"))
                .await
                .unwrap();
            let err = sqlx::query("UPDATE decks SET name = 'Renamed' WHERE id = 3")
                .execute(&pool)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("deck is archived"), "{err}");
            assert_eq!(
                crate::study::start_session(&pool, 2, at("2026-09-16T12:00:00Z"))
                    .await
                    .unwrap(),
                crate::study::StartedSession::Started { session_id: 1 }
            );
        });
    }

    #[test]
    fn migrates_forward_from_a_slice_8_database_without_data_loss() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What V1.3 left behind: migrations 0001–0008 and the seed marker,
            // the reviewed seed card, an active deck with a reviewed card, a
            // soft-deleted card, and a new card, an archived deck with its card
            // and finished session, and two notes, one of them deleted.
            migrations_up_to(8).run(&pool).await.unwrap();
            insert_reviewed_seed_card(&pool).await;
            for sql in [
                "INSERT INTO seed_markers (name, recorded_at)
                 VALUES ('sample_card', '2026-09-15T11:59:00.000Z')",
                "INSERT INTO decks (name, description, is_sample, created_at)
                 VALUES ('Biology', 'Cells', 0, '2026-09-15T12:01:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'Q1', 'A1', 'Review', 8.2956, 1.0, '2026-09-23T19:06:00.000Z',
                         '2026-09-15T12:03:00.000Z', 1, 0, '2026-09-15T12:02:00.000Z',
                         '2026-09-15T12:03:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at, deleted_at)
                 VALUES (2, 'Gone', 'gone', '2026-09-15T12:02:10.000Z',
                         '2026-09-15T12:04:00.000Z', '2026-09-15T12:04:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at)
                 VALUES (2, 'Q3', 'A3', '2026-09-15T12:02:30.000Z', '2026-09-15T12:02:30.000Z')",
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (2, 4, 'New', 8.2956, 0, 8.2956, 1.0, '2026-09-15T12:03:00.000Z')",
                "INSERT INTO decks (name, is_sample, created_at)
                 VALUES ('Chemistry', 0, '2026-09-15T12:05:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, created_at, updated_at)
                 VALUES (3, 'X', 'x', '2026-09-15T12:05:10.000Z', '2026-09-15T12:05:10.000Z')",
                "INSERT INTO sessions (deck_id, started_at, ended_at, cards_reviewed)
                 VALUES (3, '2026-09-15T12:05:20.000Z', '2026-09-15T12:05:30.000Z', 1)",
                "UPDATE decks SET archived_at = '2026-09-15T12:06:00.000Z' WHERE id = 3",
                "INSERT INTO notes (title, body, created_at, updated_at)
                 VALUES ('Kept', 'text', '2026-09-16T09:00:00.000Z', '2026-09-16T09:00:00.000Z')",
                // A note is always written active (0008), so the deleted one
                // is inserted and then deleted, as the app does.
                "INSERT INTO notes (title, body, created_at, updated_at)
                 VALUES ('Binned', 'text', '2026-09-16T09:01:00.000Z',
                         '2026-09-16T09:01:00.000Z')",
                "UPDATE notes SET deleted_at = '2026-09-16T09:02:00.000Z' WHERE title = 'Binned'",
            ] {
                sqlx::query(sql).execute(&pool).await.unwrap();
            }
            let notes_sql =
                "SELECT quote(id) || '|' || quote(title) || '|' || quote(body) || '|' ||
                                    quote(created_at) || '|' || quote(updated_at) || '|' ||
                                    quote(deleted_at)
                             FROM notes ORDER BY id";
            let before = study_history(&pool).await;
            let notes_before: Vec<String> = sqlx::query_scalar(notes_sql)
                .fetch_all(&pool)
                .await
                .unwrap();

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            // Every deck, card, log, session, marker, and note is kept exactly,
            // the archived deck included, and nothing is seeded again.
            assert_eq!(study_history(&pool).await, before);
            let notes_after: Vec<String> = sqlx::query_scalar(notes_sql)
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(notes_after, notes_before);
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM decks WHERE archived_at IS NOT NULL"
                )
                .await,
                1
            );

            // The deck archived before the upgrade can now be unarchived, and
            // comes back with its card, session, and history untouched.
            crate::authoring::unarchive_deck(&pool, 3).await.unwrap();
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM decks WHERE archived_at IS NOT NULL"
                )
                .await,
                0
            );
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM flashcards WHERE deck_id = 3").await,
                1
            );
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM sessions WHERE ended_at IS NULL"
                )
                .await,
                0
            );

            // The older rules still apply to the upgraded database.
            let err = sqlx::query("DELETE FROM decks WHERE id = 3")
                .execute(&pool)
                .await
                .unwrap_err();
            assert!(
                err.to_string()
                    .contains("decks are archived, never removed"),
                "{err}"
            );
        });
    }

    #[test]
    fn migrates_forward_from_a_v0_1_0_database_and_everything_can_be_brought_back() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;

            // What v0.1.0, the first public release, left behind: migrations
            // 0001–0007, when nothing put away could come back yet. The seed
            // marker and reviewed seed card (id 1); an active deck with a
            // reviewed card (2) and a reviewed card deleted later (3); an
            // archived deck with a reviewed card (4) and a card that lapsed
            // and was deleted before the deck was archived (5), and its
            // finished session; and two notes.
            migrations_up_to(7).run(&pool).await.unwrap();
            insert_reviewed_seed_card(&pool).await;
            for sql in [
                "INSERT INTO seed_markers (name, recorded_at)
                 VALUES ('sample_card', '2026-09-15T11:59:00.000Z')",
                "INSERT INTO decks (name, description, is_sample, created_at)
                 VALUES ('Biology', 'Cells', 0, '2026-09-15T12:01:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'Q1', 'A1', 'Review', 8.2956, 1.0, '2026-09-23T19:06:00.000Z',
                         '2026-09-15T12:03:00.000Z', 1, 0, '2026-09-15T12:02:00.000Z',
                         '2026-09-15T12:03:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at, deleted_at)
                 VALUES (2, 'Gone', 'gone', 'Review', 2.3065, 2.1181,
                         '2026-09-17T19:24:00.000Z', '2026-09-15T12:03:30.000Z', 1, 0,
                         '2026-09-15T12:02:10.000Z', '2026-09-15T12:04:00.000Z',
                         '2026-09-15T12:04:00.000Z')",
                "INSERT INTO decks (name, is_sample, created_at)
                 VALUES ('Chemistry', 0, '2026-09-15T12:05:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (3, 'X', 'x', 'Review', 2.3065, 2.1181, '2026-09-17T19:25:20.000Z',
                         '2026-09-15T12:05:20.000Z', 1, 0, '2026-09-15T12:05:10.000Z',
                         '2026-09-15T12:05:20.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at, deleted_at)
                 VALUES (3, 'Y', 'y', 'Relearning', 0.4, 7.2, '2026-09-15T12:15:25.000Z',
                         '2026-09-15T12:05:25.000Z', 2, 1, '2026-09-15T12:05:10.000Z',
                         '2026-09-15T12:05:40.000Z', '2026-09-15T12:05:40.000Z')",
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (2, 4, 'New', 8.2956, 0, 8.2956, 1.0, '2026-09-15T12:03:00.000Z'),
                        (3, 3, 'New', 2.3065, 0, 2.3065, 2.1181, '2026-09-15T12:03:30.000Z'),
                        (4, 3, 'New', 2.3065, 0, 2.3065, 2.1181, '2026-09-15T12:05:20.000Z'),
                        (5, 3, 'New', 2.3065, 0, 2.3065, 2.1181, '2026-09-15T12:05:12.000Z'),
                        (5, 1, 'Review', 0.0069, 0.0001, 0.4, 7.2, '2026-09-15T12:05:25.000Z')",
                "INSERT INTO sessions (deck_id, started_at, ended_at, cards_reviewed)
                 VALUES (3, '2026-09-15T12:05:11.000Z', '2026-09-15T12:06:00.000Z', 3)",
                "UPDATE decks SET archived_at = '2026-09-15T12:06:00.000Z' WHERE id = 3",
                "INSERT INTO notes (title, body, created_at, updated_at)
                 VALUES ('Lecture 1', 'Cells' || char(10) || char(10) || '  Genes',
                         '2026-09-16T09:00:00.000Z', '2026-09-16T09:00:00.000Z')",
                "INSERT INTO notes (title, body, created_at, updated_at)
                 VALUES ('Lecture 2', 'Proteins', '2026-09-16T09:01:00.000Z',
                         '2026-09-16T09:30:00.000Z')",
            ] {
                sqlx::query(sql).execute(&pool).await.unwrap();
            }
            let notes_sql =
                "SELECT quote(id) || '|' || quote(title) || '|' || quote(body) || '|' ||
                        quote(created_at) || '|' || quote(updated_at)
                 FROM notes ORDER BY id";
            let before = study_history(&pool).await;
            let notes_before: Vec<String> = sqlx::query_scalar(notes_sql)
                .fetch_all(&pool)
                .await
                .unwrap();

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                latest_schema_version()
            );
            // Every deck, card, log, session, and marker is kept exactly — the
            // archived deck still archived, both deleted cards still deleted —
            // every note word for word and active, and nothing is seeded again.
            assert_eq!(study_history(&pool).await, before);
            let active_notes = "SELECT COUNT(*) FROM notes WHERE deleted_at IS NULL";
            assert_eq!(count(&pool, active_notes).await, 2);

            // Everything v0.1.0 could only put away now comes back. A card in
            // the archived deck waits for its deck, as any card there does.
            let now = at("2026-09-21T12:00:00Z");
            assert!(matches!(
                crate::authoring::restore_flashcard(&pool, 5, now).await,
                Err(crate::authoring::AuthoringError::DeckArchived)
            ));
            crate::authoring::unarchive_deck(&pool, 3).await.unwrap();
            for card in [3, 5] {
                crate::authoring::restore_flashcard(&pool, card, now)
                    .await
                    .unwrap();
            }
            crate::notes::delete_note(&pool, 1, now).await.unwrap();
            crate::notes::restore_note(&pool, 1).await.unwrap();

            // Each of those wrote only the column saying the row was put
            // away, which `study_history` renders last for decks and cards:
            // set that aside, and every row is exactly what v0.1.0 left, FSRS
            // state included. Review logs, sessions, markers, and migration
            // history weren't touched at all.
            fn without_state(rows: &[String]) -> Vec<&str> {
                rows.iter()
                    .map(|row| row.rsplit_once('|').unwrap().0)
                    .collect()
            }
            let after = study_history(&pool).await;
            for table in 0..2 {
                assert_eq!(without_state(&after[table]), without_state(&before[table]));
            }
            assert_eq!(after[2..], before[2..]);
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM decks WHERE archived_at IS NOT NULL"
                )
                .await,
                0
            );
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM flashcards WHERE deleted_at IS NOT NULL"
                )
                .await,
                0
            );
            let notes_after: Vec<String> = sqlx::query_scalar(notes_sql)
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(notes_after, notes_before);
            assert_eq!(count(&pool, active_notes).await, 2);
        });
    }

    #[test]
    fn a_deleted_or_archived_row_freezes_every_column_but_its_state() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            migrate_and_seed(&pool).await.unwrap();

            // Migrations 0008–0010 freeze a put-away row by listing its
            // columns, so that a restore gives back exactly what was put
            // away, and each says a column added later belongs in its list.
            // This is what notices if one is forgotten: every column of the
            // table as it is now must be frozen, except the state column the
            // restore clears.
            for (table, trigger, state) in [
                (
                    "flashcards",
                    "flashcards_deleted_are_read_only",
                    "deleted_at",
                ),
                ("decks", "decks_archived_are_read_only", "archived_at"),
                ("notes", "notes_deleted_are_read_only", "deleted_at"),
            ] {
                let sql: String = sqlx::query_scalar(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                )
                .bind(trigger)
                .fetch_one(&pool)
                .await
                .unwrap();
                let columns: Vec<String> =
                    sqlx::query_scalar("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
                        .bind(table)
                        .fetch_all(&pool)
                        .await
                        .unwrap();
                assert!(columns.iter().any(|column| column == state), "{table}");
                for column in &columns {
                    let frozen = sql.contains(&format!("NEW.{column} IS NOT OLD.{column}"));
                    assert_eq!(frozen, column != state, "{trigger}: {column}");
                }
            }
        });
    }

    #[test]
    fn the_sample_card_never_returns_after_every_card_is_deleted() {
        tauri::async_runtime::block_on(async {
            // A real file (under the build's `target/` folder), reopened like restarts.
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-dbs")
                .join("seed-marker");
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.join("synapse.sqlite");

            let pool = open_file(&path).await.unwrap();
            for sql in [
                "INSERT INTO decks (name) VALUES ('Biology')",
                "INSERT INTO flashcards (deck_id, front, back) VALUES (2, 'q', 'a')",
                // Soft-delete everything, the seeded card included.
                "UPDATE flashcards SET deleted_at = '2026-09-15T12:00:00.000Z'",
            ] {
                sqlx::query(sql).execute(&pool).await.unwrap();
            }
            let cards_before = all_cards(&pool).await;
            pool.close().await;

            for _ in 0..2 {
                let pool = open_file(&path).await.unwrap();
                assert_eq!(all_cards(&pool).await, cards_before);
                assert_eq!(
                    count(
                        &pool,
                        "SELECT COUNT(*) FROM flashcards WHERE deleted_at IS NULL"
                    )
                    .await,
                    0
                );
                assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);
                pool.close().await;
            }

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_failed_seed_leaves_no_marker_and_is_retried() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            // A new database whose seed card insert fails once.
            sqlx::migrate!().run(&pool).await.unwrap();
            sqlx::query(
                "CREATE TRIGGER fail_seed BEFORE INSERT ON flashcards
                 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END",
            )
            .execute(&pool)
            .await
            .unwrap();

            assert!(migrate_and_seed(&pool).await.is_err());
            assert!(seed_markers(&pool).await.is_empty());
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 0);

            sqlx::query("DROP TRIGGER fail_seed")
                .execute(&pool)
                .await
                .unwrap();
            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();
            assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
        });
    }

    #[test]
    fn an_upgraded_database_that_was_never_seeded_is_seeded_once() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            // Not reachable in normal use (every launch seeded before 0005), but
            // an older database with no cards at all has clearly never been seeded.
            migrations_up_to(4).run(&pool).await.unwrap();

            migrate_and_seed(&pool).await.unwrap();
            migrate_and_seed(&pool).await.unwrap();

            assert_eq!(seed_markers(&pool).await, vec![SAMPLE_CARD_SEED]);
            let cards: Vec<(Option<i64>, String)> =
                sqlx::query_as("SELECT deck_id, front FROM flashcards")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                cards,
                vec![(Some(sample_deck_id(&pool).await), SEED_FRONT.to_string())]
            );
        });
    }
}
