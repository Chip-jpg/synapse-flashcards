//! Exporting a backup of everything Synapse stores, as one `.zip` the user
//! can copy anywhere. This is the export half of the portability contract in
//! `docs/Synapse_Project_Spec.md` §7; `import.rs` restores these packages.
//!
//! The package holds exactly two entries:
//!
//! ```text
//! synapse-export-YYYY-MM-DD.zip
//! ├── manifest.json        # what this package is, and which schema it holds
//! └── db/synapse.sqlite    # the whole local database, as one consistent copy
//! ```
//!
//! The database *is* the study data: decks (active and archived), cards
//! (active and soft-deleted), their FSRS scheduling, every `review_logs` row,
//! every session, the `seed_markers` rows, and the user's typed notes.
//! Exporting the file whole means an import restores history exactly, with no
//! field left behind by a hand-written serializer.
//!
//! Deliberately **not** included, because Synapse does not have them yet:
//! `notes/raw/`, `notes/extracted/`, and `audio/` (typed notes are plain text
//! in the `notes` table, so they need no files; PDF and slide ingestion, spec
//! §5.7, doesn't exist) and `config/settings.json` (the `settings` table, spec
//! §5.6). Nothing outside Synapse's own database is ever read, so
//! no credential, key, or unrelated file can reach an export. Synapse stores
//! no credentials at all (spec §8).
//!
//! Writing is all-or-nothing. The snapshot and the growing archive are
//! written beside the destination under unique temporary names, and the
//! finished archive is renamed into place as the last step, so the
//! destination is only ever the old file or a complete new one — never a
//! half-written file that looks like a good backup.
//!
//! Entries carry no modification time (file managers show 1980-01-01, the
//! zip format's zero date). That is deliberate: an unchanged database always
//! packs to the same bytes, and the host clock never travels in a backup.
//! (The manifest still records `exported_at`, so two exports of the same
//! database are not identical files.)

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::db::{to_db_time, DbError};

/// Identifies the package, so an import can reject an unrelated `.zip`.
pub const FORMAT: &str = "synapse-export";

/// The package layout's own version, separate from the database schema
/// version. It changes only if the entries below are rearranged.
pub const FORMAT_VERSION: u32 = 1;

pub const MANIFEST_ENTRY: &str = "manifest.json";
pub const DATABASE_ENTRY: &str = "db/synapse.sqlite";

/// What `manifest.json` holds. An import reads this first and can refuse the
/// package before touching any data:
/// - `format` and `format_version` say it is a Synapse export it understands;
/// - `schema_version` is the highest applied migration, so an import can
///   refuse a package from a newer Synapse, or migrate an older one forward;
/// - `contents` names the payload entry, so the layout is self-describing.
///
/// Beyond that, every entry carries a CRC-32 that any zip reader verifies
/// while extracting, and the extracted database can be checked with
/// `PRAGMA integrity_check` before a single row is read.
///
/// Reading is strict: a manifest with a key this version doesn't know is
/// refused rather than half-understood (see `import.rs`).
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub format_version: u32,
    /// The highest applied migration version in the exported database.
    pub schema_version: i64,
    /// The Synapse version that wrote the package.
    pub app_version: String,
    /// When it was written (ISO-8601 UTC, same format as every stored time).
    pub exported_at: String,
    pub contents: Contents,
}

/// Where each part of the payload lives inside the package.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    pub database: String,
}

/// Why an export didn't happen. Detail is for logs only; commands replace it
/// with a short, user-safe message.
#[derive(Debug)]
pub enum ExportError {
    /// The chosen destination can't be written to (a folder that disappeared,
    /// a read-only drive, no space).
    Destination(DbError),
    /// Anything else: reading the database, or building the archive.
    Internal(DbError),
}

impl From<sqlx::Error> for ExportError {
    fn from(err: sqlx::Error) -> Self {
        ExportError::Internal(err.into())
    }
}

/// The file name offered in the save dialog, e.g.
/// `synapse-export-2026-09-16.zip`. The user can change it.
pub fn suggested_file_name(now: DateTime<Utc>) -> String {
    format!("synapse-export-{}.zip", now.format("%Y-%m-%d"))
}

/// The highest applied migration version, which is the schema the exported
/// database uses.
async fn schema_version(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
}

/// Two temporary paths beside `destination`, named after it plus the export
/// time and this process, so they can't collide with each other, with another
/// running copy of Synapse, or with anything the user already has. Only paths
/// we created are ever removed.
///
/// `set_file_name` replaces the last component, so both paths always sit in
/// the same folder as `destination` and can never climb out of it.
fn temp_paths(destination: &Path, now: DateTime<Utc>) -> (PathBuf, PathBuf) {
    let stamp = format!("{}-{}", now.format("%Y%m%d%H%M%S%3f"), std::process::id());
    // `file_name` is None only for paths ending in `..`, which the save
    // dialog can't return; fall back rather than fail.
    let base = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "synapse-export".to_string());
    let beside = |suffix: &str| {
        let mut path = destination.to_path_buf();
        path.set_file_name(format!("{base}.synapse-tmp-{stamp}{suffix}"));
        path
    };
    (beside(".sqlite"), beside(".zip"))
}

/// Writes the export package for `pool` to `destination`, replacing it only
/// once the archive is complete.
///
/// `destination` must be a path the user chose in a native save dialog, and
/// `pool` must be a file-backed database (every Synapse pool is; see
/// [`crate::db::open_file`]).
///
/// `destination` is only ever written by the final rename, so whatever is
/// there stays untouched until a complete archive replaces it: a failed
/// export can never look like a successful one. On a handled failure the
/// temporary files are removed too. They can outlive a crash or a power cut,
/// because that skips the cleanup — but `destination` is still either the old
/// file or the new one, never a half-written mixture.
pub async fn write_package(
    pool: &SqlitePool,
    destination: &Path,
    now: DateTime<Utc>,
    app_version: &str,
) -> Result<(), ExportError> {
    let (snapshot_path, archive_path) = temp_paths(destination, now);

    let result = build_package(
        pool,
        destination,
        &snapshot_path,
        &archive_path,
        now,
        app_version,
    )
    .await;

    // The snapshot is a full copy of the user's database; never leave it
    // lying next to the export, whether or not the export worked.
    let _ = std::fs::remove_file(&snapshot_path);
    if result.is_err() {
        let _ = std::fs::remove_file(&archive_path);
    }
    result
}

/// The steps of an export, so [`write_package`] can clean up after any of them.
async fn build_package(
    pool: &SqlitePool,
    destination: &Path,
    snapshot_path: &Path,
    archive_path: &Path,
    now: DateTime<Utc>,
    app_version: &str,
) -> Result<(), ExportError> {
    let manifest = Manifest {
        format: FORMAT.to_string(),
        format_version: FORMAT_VERSION,
        schema_version: schema_version(pool).await?,
        app_version: app_version.to_string(),
        exported_at: to_db_time(now),
        contents: Contents {
            database: DATABASE_ENTRY.to_string(),
        },
    };
    let manifest =
        serde_json::to_vec_pretty(&manifest).map_err(|err| ExportError::Internal(Box::new(err)))?;

    // SQLite is handed the path as text, so a folder name this platform
    // allows but UTF-8 can't express would silently become a different path.
    let snapshot_text = snapshot_path.to_str().ok_or_else(|| {
        ExportError::Destination(
            format!("path is not valid UTF-8: {}", snapshot_path.display()).into(),
        )
    })?;

    // SQLite writes the snapshot itself, as one consistent copy taken between
    // transactions. Copying the database file directly could catch a
    // half-written page, or miss committed data still in a journal.
    // `VACUUM INTO` also refuses to overwrite, so the unique name above is
    // the only file it can write.
    //
    // This is also the first write into the folder the user chose, so a
    // read-only drive, a full disk, or a vanished folder shows up here, and
    // is reported as a destination problem rather than a Synapse failure.
    sqlx::query("VACUUM INTO ?1")
        .bind(snapshot_text.to_string())
        .execute(pool)
        .await
        .map_err(|err| ExportError::Destination(err.into()))?;

    write_archive(snapshot_path, archive_path, &manifest)?;

    // The archive is complete and flushed, so publishing it is one rename on
    // the same folder: either the old file is there or the new one is.
    std::fs::rename(archive_path, destination)
        .map_err(|err| ExportError::Destination(Box::new(err)))?;
    Ok(())
}

/// Builds the `.zip` at `archive_path` from `manifest` and the database
/// snapshot, and flushes it to disk before returning.
fn write_archive(
    snapshot_path: &Path,
    archive_path: &Path,
    manifest: &[u8],
) -> Result<(), ExportError> {
    let file = File::create(archive_path).map_err(|err| ExportError::Destination(Box::new(err)))?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    // Deflate: a SQLite file compresses well, and every zip tool reads it.
    // The entry time is pinned to the zip zero date rather than left to the
    // default, which follows the host clock when the crate's `time` feature
    // is on (it isn't here, but a future dependency could turn it on).
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());

    let mut write = || -> std::io::Result<()> {
        zip.start_file(MANIFEST_ENTRY, options)?;
        zip.write_all(manifest)?;

        zip.start_file(DATABASE_ENTRY, options)?;
        let mut snapshot = BufReader::new(File::open(snapshot_path)?);
        std::io::copy(&mut snapshot, &mut zip)?;
        Ok(())
    };
    write().map_err(|err| ExportError::Internal(Box::new(err)))?;

    // Finish the archive, then make sure it's really on disk before it gets
    // renamed into place as a finished backup.
    let mut file = zip
        .finish()
        .map_err(|err| ExportError::Internal(Box::new(err)))?;
    file.flush()
        .and_then(|()| file.into_inner().map_err(std::io::Error::other))
        .and_then(|file| file.sync_all())
        .map_err(|err| ExportError::Destination(Box::new(err)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_file;
    use crate::db::test_support::*;
    use std::io::Read;

    const NOW: &str = "2026-09-16T09:30:00Z";
    const APP_VERSION: &str = "0.1.0";

    /// A fresh folder under the build's `target/`, used as the "destination"
    /// the user picked. Removed by the caller when the test ends.
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-dbs")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every entry name in the archive, in order.
    fn entry_names(archive: &Path) -> Vec<String> {
        let mut zip = zip::ZipArchive::new(File::open(archive).unwrap()).unwrap();
        (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect()
    }

    fn read_entry(archive: &Path, name: &str) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(File::open(archive).unwrap()).unwrap();
        let mut entry = zip.by_name(name).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        bytes
    }

    fn manifest_of(archive: &Path) -> Manifest {
        serde_json::from_slice(&read_entry(archive, MANIFEST_ENTRY)).unwrap()
    }

    /// Extracts the exported database to `path` and opens it read-only, so the
    /// export can be inspected exactly as a future import would.
    async fn exported_database(archive: &Path, path: &Path) -> SqlitePool {
        std::fs::write(path, read_entry(archive, DATABASE_ENTRY)).unwrap();
        sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=ro", path.display()))
            .await
            .unwrap()
    }

    /// A database holding the sample content plus a normal deck with a
    /// reviewed card, a soft-deleted card, an archived deck, a session, and
    /// two typed notes, one of them soft-deleted.
    async fn populated(dir: &Path) -> SqlitePool {
        let pool = open_file(&dir.join("synapse.sqlite")).await.unwrap();
        let now = at(NOW);
        let biology = crate::authoring::create_deck(&pool, "Biology", Some("Cells"), now)
            .await
            .unwrap()
            .id;
        for (front, back) in [("A", "a"), ("B", "b")] {
            crate::authoring::create_flashcard(&pool, biology, front, back, now)
                .await
                .unwrap();
        }
        let cards: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM flashcards WHERE deck_id = ?1 ORDER BY id")
                .bind(biology)
                .fetch_all(&pool)
                .await
                .unwrap();

        let crate::study::StartedSession::Started { session_id } =
            crate::study::start_session(&pool, biology, now)
                .await
                .unwrap()
        else {
            panic!("a new card should be due");
        };
        crate::study::record_review(
            &pool,
            session_id,
            cards[0],
            0,
            crate::scheduler::Rating::Good,
            now,
        )
        .await
        .unwrap();
        crate::authoring::delete_flashcard(&pool, cards[1], now)
            .await
            .unwrap();

        let chemistry = crate::authoring::create_deck(&pool, "Chemistry", None, now)
            .await
            .unwrap()
            .id;
        crate::authoring::archive_deck(&pool, chemistry, now)
            .await
            .unwrap();

        crate::notes::create_note(&pool, "Kept note", "still in the library", now)
            .await
            .unwrap();
        let removed = crate::notes::create_note(&pool, "Removed note", "deleted, not lost", now)
            .await
            .unwrap();
        crate::notes::delete_note(&pool, removed.id, now)
            .await
            .unwrap();
        pool
    }

    #[test]
    fn an_export_holds_a_manifest_and_the_database_and_nothing_else() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-contents");
            let pool = populated(&dir).await;
            let archive = dir.join("backup.zip");

            write_package(&pool, &archive, at(NOW), APP_VERSION)
                .await
                .unwrap();

            // Exactly the two documented entries: nothing else can be in an
            // export, so no unrelated file or credential can travel in one.
            assert_eq!(
                entry_names(&archive),
                vec![MANIFEST_ENTRY.to_string(), DATABASE_ENTRY.to_string()]
            );
            // Pinned as literal JSON, not as a round-trip through `Manifest`:
            // these key names are the contract a future import reads, so
            // renaming a field has to fail here.
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&read_entry(&archive, MANIFEST_ENTRY))
                    .unwrap(),
                serde_json::json!({
                    "format": "synapse-export",
                    "format_version": 1,
                    // Every migration in the repository is applied.
                    "schema_version": 9,
                    "app_version": "0.1.0",
                    "exported_at": "2026-09-16T09:30:00.000Z",
                    "contents": { "database": "db/synapse.sqlite" },
                })
            );
            // Only the export is left behind: no snapshot, no partial archive.
            let mut left: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            left.sort();
            assert_eq!(left, vec!["backup.zip", "synapse.sqlite"]);

            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn an_export_keeps_active_history_deleted_cards_and_archived_decks() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-payload");
            let pool = populated(&dir).await;
            let archive = dir.join("backup.zip");

            write_package(&pool, &archive, at(NOW), APP_VERSION)
                .await
                .unwrap();

            let restored = exported_database(&archive, &dir.join("restored.sqlite")).await;

            // The exported file stands on its own: no journal beside it, no
            // half-written page. This is the check an import runs first.
            let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
                .fetch_one(&restored)
                .await
                .unwrap();
            assert_eq!(integrity, "ok");
            // The schema itself travels, so an import gets the same rules.
            assert_eq!(
                count(&restored, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                crate::db::latest_schema_version()
            );
            // Decks: the sample deck, the active deck, and the archived one,
            // which is still marked archived.
            let decks: Vec<(String, bool, Option<String>)> =
                sqlx::query_as("SELECT name, is_sample, archived_at FROM decks ORDER BY id")
                    .fetch_all(&restored)
                    .await
                    .unwrap();
            assert_eq!(
                decks,
                vec![
                    ("Sample deck".to_string(), true, None),
                    ("Biology".to_string(), false, None),
                    (
                        "Chemistry".to_string(),
                        false,
                        Some("2026-09-16T09:30:00.000Z".to_string())
                    ),
                ]
            );
            // Cards: the soft-deleted one is kept, still marked deleted.
            let cards: Vec<(String, Option<String>)> =
                sqlx::query_as("SELECT front, deleted_at FROM flashcards ORDER BY id")
                    .fetch_all(&restored)
                    .await
                    .unwrap();
            assert_eq!(
                cards,
                vec![
                    ("What is active recall?".to_string(), None),
                    ("A".to_string(), None),
                    (
                        "B".to_string(),
                        Some("2026-09-16T09:30:00.000Z".to_string())
                    ),
                ]
            );
            // Notes: the deleted one travels with its deletion time, so a
            // restore brings back a library that hides exactly the same notes.
            let notes: Vec<(String, String, Option<String>)> =
                sqlx::query_as("SELECT title, body, deleted_at FROM notes ORDER BY id")
                    .fetch_all(&restored)
                    .await
                    .unwrap();
            assert_eq!(
                notes,
                vec![
                    (
                        "Kept note".to_string(),
                        "still in the library".to_string(),
                        None
                    ),
                    (
                        "Removed note".to_string(),
                        // The text of a deleted note is kept in full.
                        "deleted, not lost".to_string(),
                        Some("2026-09-16T09:30:00.000Z".to_string())
                    ),
                ]
            );
            // FSRS state, the review log, the session, and the seed marker all
            // survive, so a restore carries on scheduling where it left off.
            let scheduled: (String, Option<f64>, Option<f64>, Option<String>, i64) =
                sqlx::query_as(
                    "SELECT fsrs_state, fsrs_stability, fsrs_difficulty, due, reps
                     FROM flashcards WHERE front = 'A'",
                )
                .fetch_one(&restored)
                .await
                .unwrap();
            assert_eq!(scheduled.0, "Review");
            assert!(scheduled.1.is_some() && scheduled.2.is_some());
            assert!(scheduled.3.is_some());
            assert_eq!(scheduled.4, 1);
            assert_eq!(
                count(&restored, "SELECT COUNT(*) FROM review_logs").await,
                1
            );
            assert_eq!(count(&restored, "SELECT COUNT(*) FROM sessions").await, 1);
            let markers: Vec<String> = sqlx::query_scalar("SELECT name FROM seed_markers")
                .fetch_all(&restored)
                .await
                .unwrap();
            assert_eq!(markers, vec!["sample_card".to_string()]);

            restored.close().await;
            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn an_export_keeps_whether_each_deck_is_archived_or_unarchived() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-unarchived");
            let pool = open_file(&dir.join("synapse.sqlite")).await.unwrap();
            let now = at(NOW);

            // One deck archived and left that way, one archived and brought
            // back, one that was never archived at all.
            let mut ids = Vec::new();
            for name in ["Archived", "Unarchived", "Active"] {
                let deck = crate::authoring::create_deck(&pool, name, None, now)
                    .await
                    .unwrap()
                    .id;
                crate::authoring::create_flashcard(&pool, deck, name, "answer", now)
                    .await
                    .unwrap();
                ids.push(deck);
            }
            crate::authoring::archive_deck(&pool, ids[0], now)
                .await
                .unwrap();
            crate::authoring::archive_deck(&pool, ids[1], now)
                .await
                .unwrap();
            crate::authoring::unarchive_deck(&pool, ids[1])
                .await
                .unwrap();

            let archive = dir.join("backup.zip");
            write_package(&pool, &archive, now, APP_VERSION)
                .await
                .unwrap();
            let restored = exported_database(&archive, &dir.join("restored.sqlite")).await;

            // Each deck travels in the state it was in, and the unarchived one
            // keeps its card rather than coming back empty.
            let decks: Vec<(String, Option<String>)> = sqlx::query_as(
                "SELECT name, archived_at FROM decks WHERE is_sample = 0 ORDER BY id",
            )
            .fetch_all(&restored)
            .await
            .unwrap();
            assert_eq!(
                decks,
                vec![
                    (
                        "Archived".to_string(),
                        Some("2026-09-16T09:30:00.000Z".to_string())
                    ),
                    ("Unarchived".to_string(), None),
                    ("Active".to_string(), None),
                ]
            );
            assert_eq!(
                count(
                    &restored,
                    "SELECT COUNT(*) FROM flashcards WHERE front = 'Unarchived'"
                )
                .await,
                1
            );
            // Nothing opened a session on the way through.
            assert_eq!(
                count(
                    &restored,
                    "SELECT COUNT(*) FROM sessions WHERE ended_at IS NULL"
                )
                .await,
                0
            );

            restored.close().await;
            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn exporting_again_replaces_the_file_only_once_the_new_one_is_complete() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-replace");
            let pool = populated(&dir).await;
            let archive = dir.join("backup.zip");

            write_package(&pool, &archive, at(NOW), APP_VERSION)
                .await
                .unwrap();
            let first = std::fs::read(&archive).unwrap();

            // A second export to the same path (the user confirmed the
            // overwrite in the save dialog) leaves one complete archive.
            let later = at("2026-09-16T10:00:00Z");
            crate::authoring::create_deck(&pool, "Physics", None, later)
                .await
                .unwrap();
            write_package(&pool, &archive, later, APP_VERSION)
                .await
                .unwrap();

            let second = std::fs::read(&archive).unwrap();
            assert_ne!(first, second);
            assert_eq!(
                manifest_of(&archive).exported_at,
                "2026-09-16T10:00:00.000Z"
            );
            let restored = exported_database(&archive, &dir.join("restored.sqlite")).await;
            assert_eq!(count(&restored, "SELECT COUNT(*) FROM decks").await, 4);

            restored.close().await;
            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_failed_export_leaves_no_file_and_no_leftovers() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-failure");
            let pool = populated(&dir).await;

            // A destination inside a folder that doesn't exist: the snapshot
            // can't be written, so the export fails before anything is made.
            let missing = dir.join("no-such-folder").join("backup.zip");
            assert!(write_package(&pool, &missing, at(NOW), APP_VERSION)
                .await
                .is_err());
            assert!(!missing.exists());
            assert!(!missing.parent().unwrap().exists());

            // An existing export is untouched by a later failure, so a good
            // backup is never destroyed by a bad one.
            let archive = dir.join("backup.zip");
            write_package(&pool, &archive, at(NOW), APP_VERSION)
                .await
                .unwrap();
            let good = std::fs::read(&archive).unwrap();
            pool.close().await;

            // Exporting from a closed pool fails at the snapshot step.
            assert!(write_package(&pool, &archive, at(NOW), APP_VERSION)
                .await
                .is_err());
            assert_eq!(std::fs::read(&archive).unwrap(), good);

            // No snapshot or partial archive is left beside it either.
            let mut left: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            left.sort();
            assert_eq!(left, vec!["backup.zip", "synapse.sqlite"]);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn an_untouched_database_still_exports_a_readable_package() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-empty");
            // A real file, as the app always uses: `VACUUM INTO` snapshots the
            // database SQLite has open, and Synapse never runs on an
            // in-memory one.
            let pool = open_file(&dir.join("synapse.sqlite")).await.unwrap();
            let archive = dir.join("backup.zip");

            write_package(&pool, &archive, at(NOW), APP_VERSION)
                .await
                .unwrap();

            assert_eq!(manifest_of(&archive).schema_version, 9);
            let restored = exported_database(&archive, &dir.join("restored.sqlite")).await;
            assert_eq!(count(&restored, "SELECT COUNT(*) FROM decks").await, 1);
            assert_eq!(count(&restored, "SELECT COUNT(*) FROM flashcards").await, 1);

            restored.close().await;
            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn the_manifest_records_the_schema_the_database_actually_has() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-old-schema");
            // A database left by an older Synapse, stopped at migration 3.
            let path = dir.join("synapse.sqlite");
            let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
                .await
                .unwrap();
            migrations_up_to(3).run(&pool).await.unwrap();
            let archive = dir.join("backup.zip");

            write_package(&pool, &archive, at(NOW), APP_VERSION)
                .await
                .unwrap();

            // An import uses this to decide whether it can read the package,
            // so it must describe the exported file, not today's Synapse.
            assert_eq!(manifest_of(&archive).schema_version, 3);
            let restored = exported_database(&archive, &dir.join("restored.sqlite")).await;
            assert_eq!(
                count(&restored, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                3
            );

            restored.close().await;
            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_failure_after_the_snapshot_leaves_no_copy_of_the_database_behind() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-late-failure");
            let pool = populated(&dir).await;

            // A destination that is an existing folder: the snapshot and the
            // archive are both written, and only the final rename fails. This
            // is the path where a full copy of the database already exists on
            // disk and has to be cleaned up.
            let occupied = dir.join("already-a-folder");
            std::fs::create_dir(&occupied).unwrap();
            assert!(write_package(&pool, &occupied, at(NOW), APP_VERSION)
                .await
                .is_err());
            assert!(occupied.is_dir());

            // No snapshot and no partial archive survive next to it.
            let mut left: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            left.sort();
            assert_eq!(left, vec!["already-a-folder", "synapse.sqlite"]);

            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn an_unwritable_destination_is_reported_as_a_destination_problem() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("export-destination-error");
            let pool = populated(&dir).await;

            // A folder that doesn't exist: the user has to pick elsewhere, so
            // this must not be reported as Synapse failing to build a package.
            let missing = dir.join("no-such-folder").join("backup.zip");
            assert!(matches!(
                write_package(&pool, &missing, at(NOW), APP_VERSION).await,
                Err(ExportError::Destination(_))
            ));

            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn temporary_files_stay_in_the_folder_the_user_chose() {
        let destination = Path::new("C:")
            .join("Users")
            .join("someone")
            .join("backup.zip");
        let (snapshot, archive) = temp_paths(&destination, at(NOW));

        for temp in [&snapshot, &archive] {
            assert_eq!(temp.parent(), destination.parent(), "{temp:?}");
            assert_ne!(temp, &destination);
        }
        assert_ne!(snapshot, archive);

        // A name the user typed without an extension works the same way.
        let plain = destination.with_file_name("backup");
        let (snapshot, archive) = temp_paths(&plain, at(NOW));
        assert_eq!(snapshot.parent(), plain.parent());
        assert_eq!(archive.parent(), plain.parent());
    }

    #[test]
    fn the_suggested_name_is_dated_and_ends_in_zip() {
        assert_eq!(
            suggested_file_name(at("2026-09-16T09:30:00Z")),
            "synapse-export-2026-09-16.zip"
        );
    }
}
