//! Restoring a backup made by Export data (`export.rs`): the import half of
//! the portability contract in `docs/Synapse_Project_Spec.md` §7.
//!
//! Import is **restore, never merge**: the whole database is replaced by the
//! one inside the package. It happens in two separate steps, so nothing is
//! replaced until the user has seen which backup it is and confirmed:
//!
//! 1. [`stage_package`] checks the chosen file completely and prepares a
//!    private copy of its database in the import workspace, a folder beside
//!    the real database. The real database is never read or written here.
//! 2. [`replace_database`] swaps that checked copy in for the real database,
//!    keeping a rollback copy until the new one has opened successfully.
//!
//! Only Synapse's own export format is accepted — `format_version` 1, which
//! holds exactly two entries:
//!
//! ```text
//! manifest.json
//! db/synapse.sqlite
//! ```
//!
//! Anything else is refused: other or extra entries, duplicate names, paths
//! that could climb out of a folder, directories, symlinks, encrypted
//! entries, unusual compression, an archive comment, or data in front of the
//! archive. Nothing is ever extracted to a path taken from the archive.

use std::fs::File;
use std::io::{BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tauri::async_runtime::Mutex;
use zip::{CompressionMethod, ZipArchive};

use crate::db::{self, Database, DbError};
use crate::export::{Manifest, DATABASE_ENTRY, FORMAT, FORMAT_VERSION, MANIFEST_ENTRY};

/// The largest `manifest.json` accepted. A real one is a few hundred bytes.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// The largest database accepted, once decompressed: 1 GiB. Far beyond any
/// real flashcard collection, but it stops a hostile archive from filling
/// the disk. A restore needs up to twice this in free space: the checked copy
/// plus the rollback copy of the current database.
pub const MAX_DATABASE_BYTES: u64 = 1024 * 1024 * 1024;

/// The checked copy of a backup's database, inside the import workspace.
const STAGED_FILE: &str = "staged.sqlite";
/// The current database's safety copy, kept only while a restore runs.
const ROLLBACK_FILE: &str = "rollback.sqlite";

/// Files SQLite can keep beside a database. One left beside a file that is
/// about to change places would be applied to the wrong database.
const SIDECAR_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];

/// Size limits, separate so tests can use tiny ones.
#[derive(Debug, Clone, Copy)]
struct Limits {
    manifest: u64,
    database: u64,
}

const LIMITS: Limits = Limits {
    manifest: MAX_MANIFEST_BYTES,
    database: MAX_DATABASE_BYTES,
};

/// Why a backup was refused or couldn't be prepared. The current database
/// was not touched. Detail is for logs only; commands replace it with a
/// short, user-safe message.
#[derive(Debug)]
pub enum ImportError {
    /// Not a Synapse export: not a zip, the wrong entries, or another format.
    NotAnExport(DbError),
    /// A Synapse export from a newer version than this one can read.
    TooNew(DbError),
    /// A Synapse export whose contents fail a check: a bad checksum, a broken
    /// manifest, or a database that isn't intact or isn't Synapse's schema.
    Damaged(DbError),
    /// Bigger than [`MAX_DATABASE_BYTES`] or [`MAX_MANIFEST_BYTES`].
    TooLarge,
    /// The chosen file couldn't be read at all.
    Unreadable(DbError),
    /// Preparing the private copy failed, e.g. for lack of disk space.
    Internal(DbError),
}

fn not_an_export(detail: impl Into<DbError>) -> ImportError {
    ImportError::NotAnExport(detail.into())
}

fn damaged(detail: impl Into<DbError>) -> ImportError {
    ImportError::Damaged(detail.into())
}

fn internal(detail: impl Into<DbError>) -> ImportError {
    ImportError::Internal(detail.into())
}

/// A backup that passed every check and can replace the database.
#[derive(Debug)]
pub struct StagedImport {
    /// The checked database, already brought up to this build's schema.
    pub database: PathBuf,
    pub manifest: Manifest,
}

/// Checks the package at `archive` and prepares its database in `workspace`.
///
/// The real database is never touched. Checks run in this order, and the
/// first failure stops everything and removes the private copy:
///
/// 1. The file ends in a zip directory record listing exactly two entries and
///    no archive comment. (Read directly, because the zip reader quietly
///    merges entries that share a name.)
/// 2. The zip opens with nothing in front of it and holds exactly
///    `manifest.json` and `db/synapse.sqlite`: plain files with safe names,
///    not encrypted, stored or deflated, and within the size limits.
/// 3. The manifest (its CRC-32 verified) names `format` "synapse-export",
///    then a `format_version` this build reads; then it parses strictly,
///    names the database entry, and has a `schema_version` from 1 up to this
///    build's and a valid `exported_at`.
/// 4. The database is extracted, CRC-32 verified and size-capped, to a fixed
///    name in the workspace.
/// 5. It starts with SQLite's file header.
/// 6. Opened read-only, it passes `PRAGMA integrity_check`.
/// 7. Its migration history is exactly this build's migrations 1 to
///    `schema_version`: all successful, with matching checksums.
/// 8. Its tables, indexes, triggers, and views are exactly what those
///    migrations create — nothing added, removed, or altered.
/// 9. Migrated forward by this build's own migrations (as any older database
///    is at launch), it passes the integrity check again, matches the current
///    schema, has no broken references, and has exactly one sample deck.
pub async fn stage_package(archive: &Path, workspace: &Path) -> Result<StagedImport, ImportError> {
    stage_with_limits(archive, workspace, LIMITS).await
}

async fn stage_with_limits(
    archive: &Path,
    workspace: &Path,
    limits: Limits,
) -> Result<StagedImport, ImportError> {
    // Leftovers from an import that never finished, e.g. the app was closed
    // while it waited for confirmation.
    discard_staged(workspace);
    let staged = workspace.join(STAGED_FILE);

    let result = stage(archive, workspace, &staged, limits).await;
    if result.is_err() {
        discard_staged(workspace);
    }
    result
}

/// The steps of [`stage_package`], so it can clean up after any of them.
async fn stage(
    archive: &Path,
    workspace: &Path,
    staged: &Path,
    limits: Limits,
) -> Result<StagedImport, ImportError> {
    let manifest = unpack(archive, workspace, staged, limits)?;
    check_database(staged, manifest.schema_version).await?;
    Ok(StagedImport {
        database: staged.to_path_buf(),
        manifest,
    })
}

/// Removes a backup's private copy and anything SQLite left beside it, then
/// the workspace folder if nothing else is in it. Only names this module
/// creates are ever removed.
pub fn discard_staged(workspace: &Path) {
    remove_database_files(&workspace.join(STAGED_FILE));
    let _ = std::fs::remove_dir(workspace);
}

fn remove_database_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in SIDECAR_SUFFIXES {
        let _ = std::fs::remove_file(sidecar(path, suffix));
    }
}

/// `path` with `suffix` appended to its file name, e.g. `synapse.sqlite-journal`.
fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Whether SQLite has any file beside `path`. An answer that can't be read is
/// treated as yes, so a doubtful case stops a swap rather than risking one.
fn has_sidecar(path: &Path) -> bool {
    SIDECAR_SUFFIXES
        .iter()
        .any(|suffix| !matches!(sidecar(path, suffix).try_exists(), Ok(false)))
}

/// Checks 1–5: the archive, its manifest, and extracting the database.
fn unpack(
    archive: &Path,
    workspace: &Path,
    staged: &Path,
    limits: Limits,
) -> Result<Manifest, ImportError> {
    let mut file = File::open(archive).map_err(|err| ImportError::Unreadable(Box::new(err)))?;
    check_end_record(&mut file)?;

    let mut zip = ZipArchive::new(file).map_err(|err| ImportError::NotAnExport(Box::new(err)))?;
    if zip.offset() != 0 {
        return Err(not_an_export("there is data in front of the archive"));
    }
    check_entries(&mut zip, limits)?;
    let manifest = read_manifest(&mut zip, limits)?;

    std::fs::create_dir_all(workspace).map_err(internal)?;
    extract_database(&mut zip, staged, limits)?;
    check_sqlite_header(staged)?;
    Ok(manifest)
}

/// The zip "end of central directory" record: 22 bytes, then the archive
/// comment. Synapse writes no comment, so the record ends the file.
const END_RECORD_LEN: u64 = 22;
/// The zip64 locator that sits in front of the end record when an archive
/// needs zip64. A Synapse export never does.
const ZIP64_LOCATOR_LEN: u64 = 20;

/// Check 1: the end record lists exactly two entries and no comment.
fn check_end_record(file: &mut File) -> Result<(), ImportError> {
    let unreadable = |err: std::io::Error| ImportError::Unreadable(Box::new(err));
    let len = file.metadata().map_err(unreadable)?.len();
    if len < END_RECORD_LEN {
        return Err(not_an_export("too short to be a zip archive"));
    }

    // The end record, plus the 20 bytes before it when the file has them.
    let tail_len = len.min(END_RECORD_LEN + ZIP64_LOCATOR_LEN);
    let mut tail = vec![0u8; tail_len as usize];
    file.seek(SeekFrom::End(-(tail_len as i64)))
        .map_err(unreadable)?;
    file.read_exact(&mut tail).map_err(unreadable)?;
    file.rewind().map_err(unreadable)?;

    let record = &tail[(tail_len - END_RECORD_LEN) as usize..];
    let field = |at: usize| u16::from_le_bytes([record[at], record[at + 1]]);
    if record[..4] != *b"PK\x05\x06" || field(20) != 0 {
        return Err(not_an_export(
            "no end record at the end of the file (not a zip, or it has a comment)",
        ));
    }
    if tail_len == END_RECORD_LEN + ZIP64_LOCATOR_LEN && tail[..4] == *b"PK\x06\x07" {
        return Err(not_an_export("the archive uses zip64"));
    }
    // Entries on this disk, and in total.
    if field(8) != 2 || field(10) != 2 {
        return Err(not_an_export(format!(
            "the archive lists {} entries, not 2",
            field(10)
        )));
    }
    Ok(())
}

/// Check 2: exactly the two expected entries, each a plain, safe file.
fn check_entries(zip: &mut ZipArchive<File>, limits: Limits) -> Result<(), ImportError> {
    // The end record said two; the reader agrees only if the names differ.
    if zip.len() != 2 {
        return Err(not_an_export("the archive's entries share a name"));
    }
    // Two entries, each one of two distinct names, means both are present.
    for index in 0..zip.len() {
        let entry = zip
            .by_index_raw(index)
            .map_err(|err| ImportError::Damaged(Box::new(err)))?;
        let limit = match entry.name_raw() {
            name if name == MANIFEST_ENTRY.as_bytes() => limits.manifest,
            name if name == DATABASE_ENTRY.as_bytes() => limits.database,
            name => {
                return Err(not_an_export(format!(
                    "unexpected entry {:?}",
                    String::from_utf8_lossy(name)
                )))
            }
        };
        // The accepted names are fixed and safe, so these can't fail for a
        // real export; checked anyway, so a crafted entry is refused for what
        // it really is.
        if entry.enclosed_name().is_none() || !entry.is_file() {
            return Err(not_an_export(format!(
                "entry {:?} is not a plain file with a safe name",
                entry.name()
            )));
        }
        if entry.encrypted() {
            return Err(not_an_export(format!(
                "entry {:?} is encrypted",
                entry.name()
            )));
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(not_an_export(format!(
                "entry {:?} uses unsupported compression",
                entry.name()
            )));
        }
        if entry.size() > limit {
            return Err(ImportError::TooLarge);
        }
    }
    Ok(())
}

/// Check 3: the manifest describes a package this build can restore.
fn read_manifest(zip: &mut ZipArchive<File>, limits: Limits) -> Result<Manifest, ImportError> {
    let mut bytes = Vec::new();
    let entry = zip
        .by_name(MANIFEST_ENTRY)
        .map_err(|err| ImportError::Damaged(Box::new(err)))?;
    copy_limited(entry, &mut bytes, limits.manifest)?;

    // Identify the package before trusting anything else in it, so an
    // unrelated file or a newer format is refused for the right reason.
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(damaged)?;
    if value.get("format").and_then(|format| format.as_str()) != Some(FORMAT) {
        return Err(not_an_export("the manifest's format isn't synapse-export"));
    }
    match value.get("format_version").and_then(|v| v.as_u64()) {
        Some(version) if version == u64::from(FORMAT_VERSION) => {}
        Some(version) if version > u64::from(FORMAT_VERSION) => {
            return Err(ImportError::TooNew(
                format!("format_version {version}").into(),
            ))
        }
        _ => return Err(damaged("the manifest's format_version isn't valid")),
    }

    // Strict from here: an unknown key means a package this build doesn't
    // fully understand.
    let mut manifest: Manifest = serde_json::from_value(value).map_err(damaged)?;
    if manifest.contents.database != DATABASE_ENTRY {
        return Err(damaged("the manifest names another database entry"));
    }
    let latest = db::latest_schema_version();
    if manifest.schema_version > latest {
        return Err(ImportError::TooNew(
            format!(
                "schema_version {} is newer than {latest}",
                manifest.schema_version
            )
            .into(),
        ));
    }
    if manifest.schema_version < 1 {
        return Err(damaged("the manifest's schema_version isn't valid"));
    }
    let Ok(exported_at) = DateTime::parse_from_rfc3339(&manifest.exported_at) else {
        return Err(damaged("the manifest's exported_at isn't a time"));
    };
    // Rewritten in the one format every stored time uses, so the frontend
    // never receives a valid-but-unusual form it can't display.
    manifest.exported_at = db::to_db_time(exported_at.with_timezone(&Utc));
    Ok(manifest)
}

/// Check 4: the database entry, written to `staged` and flushed to disk.
fn extract_database(
    zip: &mut ZipArchive<File>,
    staged: &Path,
    limits: Limits,
) -> Result<(), ImportError> {
    let entry = zip
        .by_name(DATABASE_ENTRY)
        .map_err(|err| ImportError::Damaged(Box::new(err)))?;
    // `create_new`: never writes over a file, even one this module owns.
    let file = File::create_new(staged).map_err(internal)?;
    let mut out = BufWriter::new(file);
    copy_limited(entry, &mut out, limits.database)?;
    let file = out.into_inner().map_err(|err| internal(err.into_error()))?;
    file.sync_all().map_err(internal)?;
    Ok(())
}

/// Copies `entry` to `out`, refusing more than `limit` bytes. Reading to the
/// end is what makes the zip reader verify the entry's CRC-32, so a mismatch
/// surfaces here as a failed read.
fn copy_limited(entry: impl Read, out: &mut impl Write, limit: u64) -> Result<(), ImportError> {
    let mut reader = entry.take(limit.saturating_add(1));
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(ImportError::Damaged(Box::new(err))),
        };
        total += read as u64;
        if total > limit {
            return Err(ImportError::TooLarge);
        }
        out.write_all(&buffer[..read]).map_err(internal)?;
    }
}

/// Check 5: the first bytes every SQLite 3 database starts with.
fn check_sqlite_header(path: &Path) -> Result<(), ImportError> {
    let mut header = [0u8; 16];
    let read = File::open(path).and_then(|mut file| file.read_exact(&mut header));
    if read.is_err() || header != *b"SQLite format 3\0" {
        return Err(damaged("the database entry isn't an SQLite database"));
    }
    Ok(())
}

/// Checks 6–9 on the extracted copy.
async fn check_database(staged: &Path, schema_version: i64) -> Result<(), ImportError> {
    // As packaged, read-only: nothing in the file changes before it's known
    // to be Synapse's own schema.
    let packaged = open_read_only(staged).await.map_err(damaged)?;
    let result = check_packaged(&packaged, schema_version).await;
    packaged.close().await;
    result?;

    // Brought up to date exactly as an older database is at launch.
    let current = db::open_file(staged).await.map_err(damaged)?;
    let result = check_current(&current).await;
    current.close().await;
    result?;

    if has_sidecar(staged) {
        return Err(internal("the checked copy still has a journal beside it"));
    }
    Ok(())
}

async fn open_read_only(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        // SQLite's advice for files that aren't trusted yet: don't let the
        // schema (views, triggers, defaults) call functions.
        .pragma("trusted_schema", "OFF");
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
}

/// Checks 6–8: the database as it came out of the package.
async fn check_packaged(pool: &SqlitePool, schema_version: i64) -> Result<(), ImportError> {
    integrity_ok(pool).await.map_err(ImportError::Damaged)?;
    check_migrations(pool, schema_version).await?;
    check_schema(pool, schema_version).await
}

/// Check 9: the database after migrating it to this build's schema.
async fn check_current(pool: &SqlitePool) -> Result<(), ImportError> {
    let latest = db::latest_schema_version();
    integrity_ok(pool).await.map_err(ImportError::Damaged)?;
    check_migrations(pool, latest).await?;
    check_schema(pool, latest).await?;

    let broken_reference = sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(pool)
        .await
        .map_err(damaged)?;
    if broken_reference.is_some() {
        return Err(damaged("a row refers to one that doesn't exist"));
    }
    let sample_decks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decks WHERE is_sample = 1")
        .fetch_one(pool)
        .await
        .map_err(damaged)?;
    if sample_decks != 1 {
        return Err(damaged(format!("{sample_decks} sample decks, not 1")));
    }
    Ok(())
}

/// `PRAGMA integrity_check` reports exactly "ok".
async fn integrity_ok(pool: &SqlitePool) -> Result<(), DbError> {
    let report: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(pool)
        .await?;
    if report != ["ok"] {
        return Err(format!("integrity check failed: {}", report.join("; ")).into());
    }
    Ok(())
}

/// The database's migration history is exactly this build's migrations up
/// to `version`: the same versions, all successful, with the same checksums.
async fn check_migrations(pool: &SqlitePool, version: i64) -> Result<(), ImportError> {
    type Applied = (i64, bool, Vec<u8>);
    let applied: Vec<Applied> =
        sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .map_err(damaged)?;
    let expected: Vec<Applied> = db::migrations_up_to(version)
        .iter()
        .map(|migration| (migration.version, true, migration.checksum.to_vec()))
        .collect();
    if applied != expected {
        return Err(damaged(format!(
            "the migration history doesn't match migrations 1 to {version}"
        )));
    }
    Ok(())
}

/// One entry of `sqlite_master`: type, name, table, and its SQL.
type SchemaObject = (String, String, String, Option<String>);

async fn schema_objects(pool: &SqlitePool) -> Result<Vec<SchemaObject>, sqlx::Error> {
    // Left out, and only these: SQLite's own tables (`sqlite_...`), its
    // automatic indexes (a `sqlite_` name and no SQL), and sqlx's bookkeeping
    // table, whose rows `check_migrations` compares instead. Everything else
    // is compared — including any trigger or view, whatever its name, since
    // triggers don't share a namespace with tables and could otherwise hide
    // behind one of those names.
    sqlx::query_as(
        "SELECT type, name, tbl_name, sql FROM sqlite_master
         WHERE NOT (type = 'table'
                    AND (name = '_sqlx_migrations' OR substr(name, 1, 7) = 'sqlite_'))
           AND NOT (type = 'index' AND substr(name, 1, 7) = 'sqlite_' AND sql IS NULL)
         ORDER BY type, name",
    )
    .fetch_all(pool)
    .await
}

/// The database's schema is exactly what migrations 1 to `version` create.
async fn check_schema(pool: &SqlitePool, version: i64) -> Result<(), ImportError> {
    let found = schema_objects(pool).await.map_err(damaged)?;
    let expected = reference_schema(version)
        .await
        .map_err(ImportError::Internal)?;
    if found != expected {
        return Err(damaged(format!(
            "the schema doesn't match migrations 1 to {version}"
        )));
    }
    Ok(())
}

/// The schema this build's migrations create up to `version`, built fresh in
/// memory for comparison.
async fn reference_schema(version: i64) -> Result<Vec<SchemaObject>, DbError> {
    // One connection: each in-memory connection would be its own database.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let result = async {
        db::migrations_up_to(version).run(&pool).await?;
        Ok::<_, DbError>(schema_objects(&pool).await?)
    }
    .await;
    pool.close().await;
    result
}

/// Why a restore didn't complete.
#[derive(Debug)]
pub enum RestoreError {
    /// The current database is still in place, exactly as it was.
    NotRestored(DbError),
    /// The new database couldn't be opened, and putting the previous one back
    /// didn't fully complete either. Synapse must be restarted. If the rename
    /// back failed, the rollback copy stays in the workspace, and a later
    /// restore moves it aside rather than deleting it.
    RollbackFailed(DbError),
}

/// Replaces the database at `db_path` with the checked copy at `staged`.
///
/// Holds the database lock throughout, so no command can read, write, or
/// reopen either file while they change places. In order:
///
/// 1. The pool is closed, which waits for any query still running. From here
///    nothing can commit to the current database.
/// 2. A rollback copy of the now-closed database is taken with `VACUUM INTO`
///    (the same consistent snapshot an export uses), through a private
///    read-only connection. A rollback copy left by an earlier failed restore
///    is moved aside under a dated name first, never deleted. If this fails,
///    stop and reopen the current database.
/// 3. If a journal is beside either file (an interrupted write, which SQLite
///    would replay into whichever database then has that name), stop and
///    reopen the current database.
/// 4. The checked copy is renamed over the database: one step on the same
///    drive, so the file is always either the old database or the new one.
///    If that fails — on Windows, also when another program still has the
///    file open — stop and reopen the current database.
/// 5. The new database is opened and its integrity checked. If that fails,
///    the rollback copy is renamed back and reopened instead.
/// 6. The new pool is installed and the rollback copy deleted.
///
/// Every stop before step 6 leaves the previous data in place.
pub async fn replace_database(
    database: &Database,
    db_path: &Path,
    staged: &Path,
    workspace: &Path,
) -> Result<(), RestoreError> {
    let mut slot = database.slot().lock().await;
    let Some(previous) = slot.take() else {
        return Err(RestoreError::NotRestored("the database isn't open".into()));
    };
    let rollback = workspace.join(ROLLBACK_FILE);

    // 1.
    previous.close().await;

    // 2. The safety copy.
    if let Err(err) = keep_earlier_rollback(workspace, &rollback) {
        *slot = reopen(db_path).await;
        return Err(RestoreError::NotRestored(err));
    }
    if let Err(err) = snapshot(db_path, workspace, &rollback).await {
        remove_database_files(&rollback);
        *slot = reopen(db_path).await;
        return Err(RestoreError::NotRestored(err));
    }

    // 3 and 4. Nothing on disk has changed if this fails.
    if let Err(err) = swap_in(staged, db_path) {
        remove_database_files(&rollback);
        *slot = reopen(db_path).await;
        return Err(RestoreError::NotRestored(err));
    }

    // 5.
    match open_checked(db_path).await {
        Ok(pool) => {
            // 6.
            *slot = Some(pool);
            remove_database_files(&rollback);
            let _ = std::fs::remove_dir(workspace);
            Ok(())
        }
        Err(err) => {
            if let Err(back) = put_back(&rollback, db_path) {
                return Err(RestoreError::RollbackFailed(
                    format!("opening the restored database failed ({err}); putting the previous one back failed ({back})").into(),
                ));
            }
            match db::open_file(db_path).await {
                Ok(pool) => {
                    *slot = Some(pool);
                    Err(RestoreError::NotRestored(err))
                }
                Err(reopen) => Err(RestoreError::RollbackFailed(
                    format!("opening the restored database failed ({err}); reopening the previous one failed ({reopen})").into(),
                )),
            }
        }
    }
}

/// Moves a rollback copy that an earlier, failed restore kept out of the way,
/// under a dated name, so it can never be deleted or overwritten: it may be
/// the only copy of that earlier data.
fn keep_earlier_rollback(workspace: &Path, rollback: &Path) -> Result<(), DbError> {
    if matches!(rollback.try_exists(), Ok(false)) {
        return Ok(());
    }
    let stamp = Utc::now().format("%Y%m%d%H%M%S%3f");
    std::fs::rename(
        rollback,
        workspace.join(format!("rollback-kept-{stamp}.sqlite")),
    )?;
    // Anything SQLite left beside the old name would otherwise attach itself
    // to the next rollback copy.
    for suffix in SIDECAR_SUFFIXES {
        let file = sidecar(rollback, suffix);
        if !matches!(file.try_exists(), Ok(false)) {
            std::fs::remove_file(&file)?;
        }
    }
    Ok(())
}

/// Writes a consistent copy of the closed database at `db_path` to `path`,
/// through a private read-only connection that is closed again afterwards.
async fn snapshot(db_path: &Path, workspace: &Path, path: &Path) -> Result<(), DbError> {
    std::fs::create_dir_all(workspace)?;
    // SQLite takes the path as text; see the same check in `export.rs`.
    let text = path
        .to_str()
        .ok_or("the workspace path isn't valid UTF-8")?;
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    // `VACUUM INTO` refuses to overwrite, so it can only create this copy.
    let result = sqlx::query("VACUUM INTO ?1")
        .bind(text.to_string())
        .execute(&pool)
        .await;
    pool.close().await;
    result?;
    Ok(())
}

/// Steps 3 and 4: renames `staged` over `db_path`, unless SQLite has a
/// journal beside either of them.
fn swap_in(staged: &Path, db_path: &Path) -> Result<(), DbError> {
    if has_sidecar(db_path) || has_sidecar(staged) {
        return Err("a database journal is present beside the database or the checked copy".into());
    }
    std::fs::rename(staged, db_path)?;
    Ok(())
}

/// Opens the restored database, checking it again now it's in place.
async fn open_checked(path: &Path) -> Result<SqlitePool, DbError> {
    let pool = db::open_file(path).await?;
    if let Err(err) = integrity_ok(&pool).await {
        pool.close().await;
        return Err(err);
    }
    Ok(pool)
}

/// Renames the rollback copy back over `db_path`.
fn put_back(rollback: &Path, db_path: &Path) -> Result<(), DbError> {
    // Anything SQLite left beside the database belongs to the one being
    // discarded, and must not be applied to the one being put back.
    for suffix in SIDECAR_SUFFIXES {
        let file = sidecar(db_path, suffix);
        if !matches!(file.try_exists(), Ok(false)) {
            std::fs::remove_file(&file)?;
        }
    }
    std::fs::rename(rollback, db_path)?;
    Ok(())
}

/// Reopens the unchanged database after a stopped restore. If even that
/// fails, the slot stays empty and the next command tries again, exactly as
/// after a failed launch.
async fn reopen(db_path: &Path) -> Option<SqlitePool> {
    match db::open_file(db_path).await {
        Ok(pool) => Some(pool),
        Err(err) => {
            eprintln!("[synapse] reopening the database after a stopped restore failed: {err}");
            None
        }
    }
}

/// The backup waiting for the user's confirmation, shared by the import
/// commands (registered with `.manage()`). At most one waits at a time.
#[derive(Default)]
pub struct ImportState {
    pending: Mutex<Pending>,
}

impl ImportState {
    /// Locked for a whole command, so choosing, confirming, and cancelling
    /// never overlap.
    pub fn pending(&self) -> &Mutex<Pending> {
        &self.pending
    }
}

#[derive(Debug, Default)]
pub struct Pending {
    last_token: u64,
    ready: Option<(u64, PathBuf)>,
}

impl Pending {
    /// Records a checked backup and returns the token its confirmation must
    /// quote, so a confirmation from an out-of-date screen can't restore a
    /// different backup.
    pub fn hold(&mut self, staged: PathBuf) -> u64 {
        self.last_token += 1;
        self.ready = Some((self.last_token, staged));
        self.last_token
    }

    /// The checked backup held under `token`, which can be taken only once.
    /// Any other token leaves the waiting backup where it is.
    pub fn take(&mut self, token: u64) -> Option<PathBuf> {
        match &self.ready {
            Some((held, _)) if *held == token => self.ready.take().map(|(_, staged)| staged),
            _ => None,
        }
    }

    /// Forgets the waiting backup, if any, and removes its private copy.
    pub fn clear(&mut self, workspace: &Path) {
        self.ready = None;
        discard_staged(workspace);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{archive_deck, create_deck, create_flashcard, delete_flashcard};
    use crate::db::open_file;
    use crate::db::test_support::*;
    use crate::export::write_package;
    use crate::scheduler::Rating;
    use crate::study::{self, StartedSession};
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    const NOW: &str = "2026-09-16T09:30:00Z";
    const LATER: &str = "2026-09-16T10:00:00Z";

    /// A fresh folder under the build's `target/`. Removed by the test.
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-dbs")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn card_ids(pool: &SqlitePool, deck: i64) -> Vec<i64> {
        sqlx::query_scalar("SELECT id FROM flashcards WHERE deck_id = ?1 ORDER BY id")
            .bind(deck)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    async fn start(pool: &SqlitePool, deck: i64, now: &str) -> i64 {
        match study::start_session(pool, deck, at(now)).await.unwrap() {
            StartedSession::Started { session_id } => session_id,
            StartedSession::NoneDue => panic!("expected a session to start"),
        }
    }

    /// The backup's source: the sample deck and card, a normal deck with a
    /// reviewed card, a soft-deleted card, a multiline new card and its
    /// unfinished session, an archived deck with a reviewed card and a
    /// finished session, and three typed notes: one plain, one edited, and one
    /// soft-deleted. Every kind of history an import must keep.
    async fn source_database(path: &Path) -> SqlitePool {
        let pool = open_file(path).await.unwrap();
        let biology = create_deck(&pool, "Biology", Some("Cells"), at(NOW))
            .await
            .unwrap()
            .id;
        for (front, back) in [("A", "a"), ("B", "b"), ("Line one\nLine two", "c")] {
            create_flashcard(&pool, biology, front, back, at(NOW))
                .await
                .unwrap();
        }
        let cards = card_ids(&pool, biology).await;
        let session = start(&pool, biology, NOW).await;
        study::record_review(&pool, session, cards[0], 0, Rating::Good, at(NOW))
            .await
            .unwrap();
        delete_flashcard(&pool, cards[1], at(NOW)).await.unwrap();

        let chemistry = create_deck(&pool, "Chemistry", None, at(NOW))
            .await
            .unwrap()
            .id;
        create_flashcard(&pool, chemistry, "X", "x", at(NOW))
            .await
            .unwrap();
        let x = card_ids(&pool, chemistry).await[0];
        let session = start(&pool, chemistry, NOW).await;
        study::record_review(&pool, session, x, 0, Rating::Good, at(NOW))
            .await
            .unwrap();
        archive_deck(&pool, chemistry, at(LATER)).await.unwrap();

        crate::notes::create_note(&pool, "Lecture 1", "Cells\n\n  Genes", at(NOW))
            .await
            .unwrap();
        let draft = crate::notes::create_note(&pool, "Draft", "draft", at(NOW))
            .await
            .unwrap();
        crate::notes::update_note(&pool, draft.id, "Lecture 2", "Proteins", at(LATER))
            .await
            .unwrap();
        let removed = crate::notes::create_note(&pool, "Old lecture", "superseded", at(NOW))
            .await
            .unwrap();
        crate::notes::delete_note(&pool, removed.id, at(LATER))
            .await
            .unwrap();
        pool
    }

    /// The database being replaced: different data from the source.
    async fn target_database(path: &Path) -> SqlitePool {
        let pool = open_file(path).await.unwrap();
        let deck = create_deck(&pool, "Target only", None, at(NOW))
            .await
            .unwrap()
            .id;
        create_flashcard(&pool, deck, "T", "t", at(NOW))
            .await
            .unwrap();
        let card = card_ids(&pool, deck).await[0];
        let session = start(&pool, deck, NOW).await;
        study::record_review(&pool, session, card, 0, Rating::Easy, at(NOW))
            .await
            .unwrap();
        crate::notes::create_note(&pool, "Target note", "only here", at(NOW))
            .await
            .unwrap();
        pool
    }

    /// Every schema object and every row of every table (sqlx's migration
    /// history included), each value rendered by SQLite's `quote()`, so two
    /// databases compare exactly.
    async fn dump(pool: &SqlitePool) -> Vec<(String, Vec<String>)> {
        let schema: Vec<String> = sqlx::query_scalar(
            "SELECT type || ' ' || name || ' ' || COALESCE(sql, '') FROM sqlite_master
             ORDER BY type, name",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        let mut out = vec![("sqlite_master".to_string(), schema)];

        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND substr(name, 1, 7) <> 'sqlite_' ORDER BY name",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        for table in tables {
            let columns: Vec<String> =
                sqlx::query_scalar("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
                    .bind(&table)
                    .fetch_all(pool)
                    .await
                    .unwrap();
            let values = columns
                .iter()
                .map(|column| format!("quote(\"{column}\")"))
                .collect::<Vec<_>>()
                .join(" || ' | ' || ");
            // Test-only SQL built from the database's own table and column
            // names, never from input.
            let sql = format!("SELECT {values} FROM \"{table}\" ORDER BY rowid");
            let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .fetch_all(pool)
                .await
                .unwrap();
            out.push((table, rows));
        }
        out
    }

    /// The pool a restore installed.
    async fn installed(database: &Database) -> SqlitePool {
        database
            .slot()
            .lock()
            .await
            .clone()
            .expect("a pool should be installed")
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn read_entry(archive: &[u8], name: &str) -> Vec<u8> {
        let mut zip = ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut entry = zip.by_name(name).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        bytes
    }

    /// A zip of `entries`, stored uncompressed so tests can edit its bytes.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// A valid manifest for a database at this build's schema, after `edit`.
    fn manifest_with(edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let mut manifest = serde_json::json!({
            "format": "synapse-export",
            "format_version": 1,
            "schema_version": db::latest_schema_version(),
            "app_version": "0.1.0",
            "exported_at": "2026-09-16T09:30:00.000Z",
            "contents": { "database": "db/synapse.sqlite" },
        });
        edit(&mut manifest);
        serde_json::to_vec_pretty(&manifest).unwrap()
    }

    /// Replaces every `from` with `to` (the same length) in `bytes`.
    fn replace_bytes(bytes: &mut [u8], from: &[u8], to: &[u8]) {
        assert_eq!(from.len(), to.len());
        let mut at = 0;
        while let Some(found) = bytes[at..].windows(from.len()).position(|w| w == from) {
            bytes[at + found..at + found + to.len()].copy_from_slice(to);
            at += found + to.len();
        }
    }

    /// For each entry in a stored zip: where its central directory header and
    /// its local header start, and where its data starts.
    fn entry_offsets(bytes: &[u8]) -> Vec<(usize, usize, usize)> {
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
        let u32_at = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
        };
        let end = bytes.len() - END_RECORD_LEN as usize;
        let mut central = u32_at(end + 16);
        let mut entries = Vec::new();
        for _ in 0..u16_at(end + 10) {
            assert_eq!(&bytes[central..central + 4], b"PK\x01\x02");
            let local = u32_at(central + 42);
            let data = local + 30 + u16_at(local + 26) + u16_at(local + 28);
            entries.push((central, local, data));
            central += 46 + u16_at(central + 28) + u16_at(central + 30) + u16_at(central + 32);
        }
        entries
    }

    /// Creates a database at `path`, runs `setup` on one connection (with
    /// foreign keys enforced or not), and returns the file's bytes.
    async fn database_bytes(path: &Path, foreign_keys: bool, setup: &[&'static str]) -> Vec<u8> {
        let pool = open_file(path).await.unwrap();
        {
            let mut conn = pool.acquire().await.unwrap();
            if !foreign_keys {
                sqlx::query("PRAGMA foreign_keys = OFF")
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
            for sql in setup {
                sqlx::query(*sql).execute(&mut *conn).await.unwrap();
            }
        }
        pool.close().await;
        std::fs::read(path).unwrap()
    }

    fn describe(result: &Result<StagedImport, ImportError>) -> &'static str {
        match result {
            Ok(_) => "accepted",
            Err(ImportError::NotAnExport(_)) => "not an export",
            Err(ImportError::TooNew(_)) => "too new",
            Err(ImportError::Damaged(_)) => "damaged",
            Err(ImportError::TooLarge) => "too large",
            Err(ImportError::Unreadable(_)) => "unreadable",
            Err(ImportError::Internal(_)) => "internal",
        }
    }

    #[test]
    fn a_valid_export_replaces_the_current_database_exactly() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-restore");
            let source = source_database(&dir.join("source").join("synapse.sqlite")).await;
            let archive = dir.join("backup.zip");
            write_package(&source, &archive, at(LATER), "0.1.0")
                .await
                .unwrap();
            let source_rows = dump(&source).await;
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let target_rows = dump(&target).await;
            let database = Database::default();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("target").join("import-workspace");

            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(staged.database, workspace.join(STAGED_FILE));
            assert_eq!(staged.manifest.exported_at, "2026-09-16T10:00:00.000Z");
            // Checking and preparing the backup changed nothing yet.
            assert_eq!(dump(&installed(&database).await).await, target_rows);

            replace_database(&database, &target_path, &staged.database, &workspace)
                .await
                .unwrap();

            // Every row of every table is the source's, and nothing of the
            // target's own data is left.
            let restored = installed(&database).await;
            assert_eq!(dump(&restored).await, source_rows);
            assert_eq!(
                count(
                    &restored,
                    "SELECT COUNT(*) FROM decks WHERE name = 'Target only'"
                )
                .await,
                0
            );
            let titles: Vec<String> = sqlx::query_scalar("SELECT title FROM notes ORDER BY id")
                .fetch_all(&restored)
                .await
                .unwrap();
            assert_eq!(titles, vec!["Lecture 1", "Lecture 2", "Old lecture"]);
            // A restored library hides exactly the notes the backup hid: the
            // deleted note is present but not in the library.
            assert_eq!(
                crate::notes::notes(&restored)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|note| note.title)
                    .collect::<Vec<_>>(),
                vec!["Lecture 2".to_string(), "Lecture 1".to_string()]
            );
            assert_eq!(
                crate::notes::deleted_notes(&restored)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|note| (note.title, note.deleted_at))
                    .collect::<Vec<_>>(),
                vec![(
                    "Old lecture".to_string(),
                    "2026-09-16T10:00:00.000Z".to_string()
                )]
            );
            // Nothing is left in the workspace, which is removed, and SQLite
            // has nothing beside the database.
            assert!(!workspace.exists());
            assert_eq!(file_names(&dir.join("target")), vec!["synapse.sqlite"]);

            // The restored history is live: the unfinished session resumes,
            // the archived deck stays off the dashboard, and the deleted card
            // is never offered.
            let biology: i64 = sqlx::query_scalar("SELECT id FROM decks WHERE name = 'Biology'")
                .fetch_one(&restored)
                .await
                .unwrap();
            let open_session: i64 =
                sqlx::query_scalar("SELECT id FROM sessions WHERE ended_at IS NULL")
                    .fetch_one(&restored)
                    .await
                    .unwrap();
            assert_eq!(
                study::start_session(&restored, biology, at(LATER))
                    .await
                    .unwrap(),
                StartedSession::Started {
                    session_id: open_session
                }
            );
            let names: Vec<String> = study::decks(&restored, at(LATER))
                .await
                .unwrap()
                .into_iter()
                .map(|deck| deck.name)
                .collect();
            assert_eq!(names, vec!["Sample deck", "Biology"]);

            // The archived deck came back as archived, not as active, and is
            // unarchivable from here like any other — with its card and its
            // review history, and without opening a session.
            let chemistry: i64 =
                sqlx::query_scalar("SELECT id FROM decks WHERE name = 'Chemistry'")
                    .fetch_one(&restored)
                    .await
                    .unwrap();
            assert_eq!(
                crate::authoring::archived_decks(&restored)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|deck| (deck.id, deck.card_count))
                    .collect::<Vec<_>>(),
                vec![(chemistry, 1)]
            );
            crate::authoring::unarchive_deck(&restored, chemistry)
                .await
                .unwrap();
            let names: Vec<String> = study::decks(&restored, at(LATER))
                .await
                .unwrap()
                .into_iter()
                .map(|deck| deck.name)
                .collect();
            assert_eq!(names, vec!["Sample deck", "Biology", "Chemistry"]);
            assert_eq!(
                count(
                    &restored,
                    "SELECT COUNT(*) FROM review_logs l
                     JOIN flashcards f ON f.id = l.card_id
                     JOIN decks d ON d.id = f.deck_id
                     WHERE d.name = 'Chemistry'"
                )
                .await,
                1
            );
            // Unarchiving opened nothing: the only session still running is
            // Biology's, which was already open in the backup.
            assert_eq!(
                count(
                    &restored,
                    "SELECT COUNT(*) FROM sessions s JOIN decks d ON d.id = s.deck_id
                     WHERE s.ended_at IS NULL AND d.name = 'Chemistry'"
                )
                .await,
                0
            );

            // Reopening it, as a relaunch does, finds the same data and seeds
            // nothing.
            let rows = dump(&restored).await;
            restored.close().await;
            let reopened = open_file(&target_path).await.unwrap();
            assert_eq!(dump(&reopened).await, rows);

            reopened.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn every_invalid_package_is_refused_and_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-refused");
            let source = source_database(&dir.join("source").join("synapse.sqlite")).await;
            let genuine_path = dir.join("genuine.zip");
            write_package(&source, &genuine_path, at(NOW), "0.1.0")
                .await
                .unwrap();
            source.close().await;
            let genuine = std::fs::read(&genuine_path).unwrap();
            let manifest = read_entry(&genuine, MANIFEST_ENTRY);
            let database = read_entry(&genuine, DATABASE_ENTRY);
            let latest = db::latest_schema_version();

            // Databases that are valid SQLite but not what a backup may hold.
            let made = dir.join("made");
            std::fs::create_dir_all(&made).unwrap();
            let extra_trigger = database_bytes(
                &made.join("trigger.sqlite"),
                true,
                &["CREATE TRIGGER sneaky AFTER INSERT ON review_logs BEGIN SELECT 1; END"],
            )
            .await;
            // A trigger may share its name with a table, so it can't hide
            // behind the names the schema check leaves out.
            let hidden_trigger = database_bytes(
                &made.join("hidden.sqlite"),
                true,
                &["CREATE TRIGGER _sqlx_migrations AFTER INSERT ON review_logs
                   BEGIN UPDATE flashcards SET due = NULL; END"],
            )
            .await;
            let forged_trigger = database_bytes(
                &made.join("forged.sqlite"),
                true,
                &[
                    "PRAGMA writable_schema = ON",
                    "INSERT INTO sqlite_master (type, name, tbl_name, rootpage, sql)
                     VALUES ('trigger', 'sqlite_sneaky', 'review_logs', 0,
                             'CREATE TRIGGER sqlite_sneaky AFTER INSERT ON review_logs BEGIN SELECT 1; END')",
                ],
            )
            .await;
            let edited_history = database_bytes(
                &made.join("history.sqlite"),
                true,
                &["UPDATE _sqlx_migrations SET checksum = x'00' WHERE version = 1"],
            )
            .await;
            let broken_reference = database_bytes(
                &made.join("reference.sqlite"),
                false,
                &["INSERT INTO flashcards (deck_id, front, back) VALUES (999, 'q', 'a')"],
            )
            .await;
            let unrelated = {
                let path = made.join("unrelated.sqlite");
                let pool =
                    sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
                        .await
                        .unwrap();
                sqlx::query("CREATE TABLE other_app_data (body TEXT)")
                    .execute(&pool)
                    .await
                    .unwrap();
                pool.close().await;
                std::fs::read(&path).unwrap()
            };
            // An index removed from the schema while its pages stay behind:
            // SQLite opens it happily, and only `PRAGMA integrity_check`
            // reports the orphaned pages.
            let orphaned_pages = database_bytes(
                &made.join("orphaned.sqlite"),
                true,
                &[
                    "PRAGMA writable_schema = ON",
                    "DELETE FROM sqlite_master WHERE name = 'flashcards_deck_due'",
                ],
            )
            .await;
            // Garbage in the middle of a real database: the header survives,
            // so SQLite itself finds the damage when it reads the pages.
            let mut corrupt = database.clone();
            for byte in &mut corrupt[4096..4096 + 512] {
                *byte = 0xA5;
            }

            let mut cases: Vec<(&str, Vec<u8>, &str)> = vec![
                ("too short", b"PK".to_vec(), "not an export"),
                (
                    "plain text",
                    b"not a zip at all\n".repeat(8),
                    "not an export",
                ),
                ("empty file", Vec::new(), "not an export"),
                (
                    "manifest isn't JSON",
                    zip_of(&[(MANIFEST_ENTRY, b"{ nope"), (DATABASE_ENTRY, &database)]),
                    "damaged",
                ),
                (
                    "another app's package",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["format"] = "other-app".into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "not an export",
                ),
                (
                    "newer format version",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["format_version"] = 2.into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "too new",
                ),
                (
                    "format version 0",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["format_version"] = 0.into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "newer schema",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["schema_version"] = (latest + 1).into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "too new",
                ),
                (
                    "schema version 0",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["schema_version"] = 0.into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "manifest schema differs from the database's",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["schema_version"] = (latest - 1).into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "unknown manifest key",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["password"] = "x".into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "manifest names another entry",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| {
                                m["contents"]["database"] = "db/other.sqlite".into()
                            }),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "export time isn't a time",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["exported_at"] = "yesterday".into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "missing database",
                    zip_of(&[(MANIFEST_ENTRY, &manifest)]),
                    "not an export",
                ),
                (
                    "unknown extra entry",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &database),
                        ("notes/extra.txt", b"hello"),
                    ]),
                    "not an export",
                ),
                (
                    "database under another name",
                    zip_of(&[(MANIFEST_ENTRY, &manifest), ("synapse.sqlite", &database)]),
                    "not an export",
                ),
                (
                    "directory entry",
                    {
                        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                        let options = SimpleFileOptions::default();
                        zip.start_file(MANIFEST_ENTRY, options).unwrap();
                        zip.write_all(&manifest).unwrap();
                        zip.add_directory("db/", options).unwrap();
                        zip.finish().unwrap().into_inner()
                    },
                    "not an export",
                ),
                (
                    "symlink in place of the database",
                    {
                        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                        let options = SimpleFileOptions::default();
                        zip.start_file(MANIFEST_ENTRY, options).unwrap();
                        zip.write_all(&manifest).unwrap();
                        zip.add_symlink(DATABASE_ENTRY, "../../synapse.sqlite", options)
                            .unwrap();
                        zip.finish().unwrap().into_inner()
                    },
                    "not an export",
                ),
                (
                    "archive comment",
                    {
                        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                        let options = SimpleFileOptions::default();
                        zip.start_file(MANIFEST_ENTRY, options).unwrap();
                        zip.write_all(&manifest).unwrap();
                        zip.start_file(DATABASE_ENTRY, options).unwrap();
                        zip.write_all(&database).unwrap();
                        zip.set_comment("hello").unwrap();
                        zip.finish().unwrap().into_inner()
                    },
                    "not an export",
                ),
                (
                    "data in front of the archive",
                    {
                        let mut bytes = b"MZ-prepended-bytes".to_vec();
                        bytes.extend(zip_of(&[
                            (MANIFEST_ENTRY, &manifest),
                            (DATABASE_ENTRY, &database),
                        ]));
                        bytes
                    },
                    "not an export",
                ),
                (
                    "duplicate entry names",
                    {
                        let mut bytes = zip_of(&[
                            (MANIFEST_ENTRY, &manifest),
                            (DATABASE_ENTRY, &database),
                            ("db/synapse.sqlitX", &database),
                        ]);
                        replace_bytes(&mut bytes, b"db/synapse.sqlitX", DATABASE_ENTRY.as_bytes());
                        bytes
                    },
                    "not an export",
                ),
                (
                    "encrypted entries",
                    {
                        let mut bytes =
                            zip_of(&[(MANIFEST_ENTRY, &manifest), (DATABASE_ENTRY, &database)]);
                        for (central, local, _) in entry_offsets(&bytes) {
                            bytes[central + 8] |= 1;
                            bytes[local + 6] |= 1;
                        }
                        bytes
                    },
                    "not an export",
                ),
                (
                    "database fails its CRC",
                    {
                        let mut bytes =
                            zip_of(&[(MANIFEST_ENTRY, &manifest), (DATABASE_ENTRY, &database)]);
                        let (_, _, data) = entry_offsets(&bytes)[1];
                        bytes[data + 2000] ^= 0xFF;
                        bytes
                    },
                    "damaged",
                ),
                (
                    "database isn't SQLite",
                    zip_of(&[(MANIFEST_ENTRY, &manifest), (DATABASE_ENTRY, &[7u8; 8192])]),
                    "damaged",
                ),
                (
                    "database pages are malformed",
                    zip_of(&[(MANIFEST_ENTRY, &manifest), (DATABASE_ENTRY, &corrupt)]),
                    "damaged",
                ),
                (
                    "database fails its integrity check",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &orphaned_pages),
                    ]),
                    "damaged",
                ),
                (
                    "SQLite but not a Synapse database",
                    zip_of(&[(MANIFEST_ENTRY, &manifest), (DATABASE_ENTRY, &unrelated)]),
                    "damaged",
                ),
                (
                    "a trigger Synapse doesn't create",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &extra_trigger),
                    ]),
                    "damaged",
                ),
                (
                    "a trigger named like sqlx's table",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &hidden_trigger),
                    ]),
                    "damaged",
                ),
                (
                    "a trigger with SQLite's reserved prefix",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &forged_trigger),
                    ]),
                    "damaged",
                ),
                (
                    "unsupported compression",
                    {
                        let mut bytes =
                            zip_of(&[(MANIFEST_ENTRY, &manifest), (DATABASE_ENTRY, &database)]);
                        // Method 12 (bzip2) in both headers of the database entry.
                        let (central, local, _) = entry_offsets(&bytes)[1];
                        bytes[central + 10] = 12;
                        bytes[local + 8] = 12;
                        bytes
                    },
                    "not an export",
                ),
                (
                    "zip64 records",
                    {
                        let mut bytes =
                            zip_of(&[(MANIFEST_ENTRY, &manifest), (DATABASE_ENTRY, &database)]);
                        let end = bytes.len() - END_RECORD_LEN as usize;
                        let mut locator = b"PK\x06\x07".to_vec();
                        locator.resize(ZIP64_LOCATOR_LEN as usize, 0);
                        bytes.splice(end..end, locator);
                        bytes
                    },
                    "not an export",
                ),
                (
                    "edited migration history",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &edited_history),
                    ]),
                    "damaged",
                ),
                (
                    "a card in a deck that doesn't exist",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &broken_reference),
                    ]),
                    "damaged",
                ),
            ];
            // Unsafe names, each paired with the manifest so there are two entries.
            for name in [
                "../db/synapse.sqlite",
                "/db/synapse.sqlite",
                "db\\synapse.sqlite",
                "db/../db/synapse.sqlite",
                "C:/db/synapse.sqlite",
            ] {
                cases.push((
                    name,
                    zip_of(&[(MANIFEST_ENTRY, &manifest), (name, &database)]),
                    "not an export",
                ));
            }

            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let before = dump(&target).await;
            let workspace = dir.join("target").join("import-workspace");

            for (name, bytes, expected) in &cases {
                let archive = dir.join("candidate.zip");
                std::fs::write(&archive, bytes).unwrap();
                let result = stage_package(&archive, &workspace).await;
                assert_eq!(describe(&result), *expected, "{name}: {result:?}");
                // Some cases must be caught by one particular check, not by
                // whichever check happens to run first.
                let reason = match *name {
                    "database fails its integrity check" => Some("integrity check failed"),
                    "a trigger named like sqlx's table"
                    | "a trigger with SQLite's reserved prefix"
                    | "a trigger Synapse doesn't create" => Some("the schema doesn't match"),
                    _ => None,
                };
                if let Some(reason) = reason {
                    assert!(
                        matches!(&result, Err(ImportError::Damaged(detail))
                            if detail.to_string().starts_with(reason)),
                        "{name}: {result:?}"
                    );
                }
                // No private copy is left, and the current data is untouched.
                assert!(!workspace.exists(), "{name}: the workspace was left behind");
                assert_eq!(dump(&target).await, before, "{name}");
            }

            // And the genuine package passes the same checks.
            assert_eq!(
                describe(&stage_package(&genuine_path, &workspace).await),
                "accepted"
            );
            discard_staged(&workspace);

            target.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn oversized_entries_are_refused() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-too-large");
            let source = open_file(&dir.join("source").join("synapse.sqlite"))
                .await
                .unwrap();
            let archive = dir.join("backup.zip");
            write_package(&source, &archive, at(NOW), "0.1.0")
                .await
                .unwrap();
            source.close().await;
            let workspace = dir.join("import-workspace");

            // Declared sizes over the limit are refused before extracting.
            let tiny = Limits {
                manifest: 16,
                database: MAX_DATABASE_BYTES,
            };
            let result = stage_with_limits(&archive, &workspace, tiny).await;
            assert_eq!(describe(&result), "too large");
            let tiny = Limits {
                manifest: MAX_MANIFEST_BYTES,
                database: 1024,
            };
            let result = stage_with_limits(&archive, &workspace, tiny).await;
            assert_eq!(describe(&result), "too large");
            assert!(!workspace.exists());

            // A size that lies is caught while copying, too.
            let mut out = Vec::new();
            assert!(matches!(
                copy_limited(&[0u8; 100][..], &mut out, 99),
                Err(ImportError::TooLarge)
            ));
            copy_limited(&[0u8; 100][..], &mut out, 100).unwrap();
            assert_eq!(out.len(), 100);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn an_older_backup_is_migrated_forward_before_it_replaces_anything() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-older");
            // A database an older Synapse left: one migration behind, with a
            // reviewed card in a user deck.
            let old_version = db::latest_schema_version() - 1;
            let source_path = dir.join("source").join("synapse.sqlite");
            std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
            let source =
                sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", source_path.display()))
                    .await
                    .unwrap();
            migrations_up_to(old_version).run(&source).await.unwrap();
            for sql in [
                "INSERT INTO seed_markers (name) VALUES ('sample_card')",
                "INSERT INTO decks (name, created_at) VALUES ('Old deck', '2026-09-15T12:00:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'Old', 'card', 'Review', 2.5, 5.0, '2026-09-20T12:00:00.000Z',
                         '2026-09-15T12:00:00.000Z', 1, 0, '2026-09-15T11:00:00.000Z',
                         '2026-09-15T12:00:00.000Z')",
            ] {
                sqlx::query(sql).execute(&source).await.unwrap();
            }
            let archive = dir.join("backup.zip");
            write_package(&source, &archive, at(NOW), "0.1.0")
                .await
                .unwrap();
            source.close().await;

            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, old_version);

            // The checked copy is at this build's schema, with the old rows.
            let copy = open_file(&staged.database).await.unwrap();
            assert_eq!(
                count(&copy, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                db::latest_schema_version()
            );
            let card: (String, String, i64, Option<String>) = sqlx::query_as(
                "SELECT front, fsrs_state, reps, due FROM flashcards WHERE deck_id = 2",
            )
            .fetch_one(&copy)
            .await
            .unwrap();
            assert_eq!(
                card,
                (
                    "Old".to_string(),
                    "Review".to_string(),
                    1,
                    Some("2026-09-20T12:00:00.000Z".to_string())
                )
            );
            // Already seeded (its marker travelled), so no sample card was added.
            assert_eq!(count(&copy, "SELECT COUNT(*) FROM flashcards").await, 1);
            copy.close().await;

            discard_staged(&workspace);
            assert!(!workspace.exists());
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_restored_database_that_fails_to_open_or_check_is_rolled_back() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-rollback");
            let target_path = dir.join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let before = dump(&target).await;
            let database = Database::default();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("import-workspace");

            // "Checked copies" that fail once in place, standing in for
            // anything that goes wrong after the swap: one can't be opened at
            // all, and one opens but fails its integrity check.
            let unopenable = vec![0x42u8; 8192];
            let orphaned_pages = database_bytes(
                &dir.join("made").join("orphaned.sqlite"),
                true,
                &[
                    "PRAGMA writable_schema = ON",
                    "DELETE FROM sqlite_master WHERE name = 'flashcards_deck_due'",
                ],
            )
            .await;

            for bad in [unopenable, orphaned_pages] {
                std::fs::create_dir_all(&workspace).unwrap();
                let staged = workspace.join(STAGED_FILE);
                std::fs::write(&staged, &bad).unwrap();

                let result = replace_database(&database, &target_path, &staged, &workspace).await;
                assert!(
                    matches!(result, Err(RestoreError::NotRestored(_))),
                    "{result:?}"
                );

                // The rollback copy was put back: the same data, open and
                // working, and no copy is left behind.
                let pool = installed(&database).await;
                assert_eq!(dump(&pool).await, before);
                assert_eq!(study::decks(&pool, at(NOW)).await.unwrap().len(), 2);
                assert!(!workspace.join(ROLLBACK_FILE).exists());
                assert!(!staged.exists());
                assert!(!has_sidecar(&target_path));
            }

            installed(&database).await.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_failed_rollback_copy_stops_the_restore_before_anything_changes() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-no-snapshot");
            let target_path = dir.join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let before = dump(&target).await;
            let database = Database::default();
            *database.slot().lock().await = Some(target);

            // The workspace can't be created (a file has its name), so no
            // rollback copy can be taken, and nothing may be swapped.
            let workspace = dir.join("import-workspace");
            std::fs::write(&workspace, b"in the way").unwrap();
            let staged = dir.join("staged.sqlite");
            std::fs::write(&staged, b"never used").unwrap();

            let result = replace_database(&database, &target_path, &staged, &workspace).await;
            assert!(
                matches!(result, Err(RestoreError::NotRestored(_))),
                "{result:?}"
            );
            let pool = installed(&database).await;
            assert_eq!(dump(&pool).await, before);
            assert_eq!(std::fs::read(&staged).unwrap(), b"never used");

            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_rollback_copy_kept_by_an_earlier_failure_is_moved_aside_not_deleted() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-kept-rollback");
            let source = source_database(&dir.join("source").join("synapse.sqlite")).await;
            let archive = dir.join("backup.zip");
            write_package(&source, &archive, at(NOW), "0.1.0")
                .await
                .unwrap();
            let source_rows = dump(&source).await;
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let database = Database::default();
            *database.slot().lock().await = Some(target_database(&target_path).await);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();

            // What a restore whose rollback couldn't be renamed back leaves.
            std::fs::write(workspace.join(ROLLBACK_FILE), b"earlier data").unwrap();

            replace_database(&database, &target_path, &staged.database, &workspace)
                .await
                .unwrap();
            let pool = installed(&database).await;
            assert_eq!(dump(&pool).await, source_rows);

            // The earlier copy survives under a dated name; only this
            // restore's own rollback copy was removed.
            let names = file_names(&workspace);
            assert_eq!(names.len(), 1, "{names:?}");
            assert!(names[0].starts_with("rollback-kept-"), "{names:?}");
            assert_eq!(
                std::fs::read(workspace.join(&names[0])).unwrap(),
                b"earlier data"
            );

            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_failed_swap_leaves_the_current_database_in_place() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-failed-swap");
            let target_path = dir.join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let before = dump(&target).await;
            let database = Database::default();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("import-workspace");

            // The checked copy has vanished, so the rename fails.
            let missing = workspace.join(STAGED_FILE);
            let result = replace_database(&database, &target_path, &missing, &workspace).await;
            assert!(
                matches!(result, Err(RestoreError::NotRestored(_))),
                "{result:?}"
            );

            let pool = installed(&database).await;
            assert_eq!(dump(&pool).await, before);
            assert!(!workspace.join(ROLLBACK_FILE).exists());

            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_database_with_a_journal_beside_it_is_not_replaced() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-in-use");
            let source = open_file(&dir.join("source").join("synapse.sqlite"))
                .await
                .unwrap();
            let archive = dir.join("backup.zip");
            write_package(&source, &archive, at(NOW), "0.1.0")
                .await
                .unwrap();
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let before = dump(&target).await;
            let database = Database::default();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();

            // What an interrupted write leaves, beside the database and then
            // beside the checked copy. Empty, so SQLite itself ignores it.
            for journal in [
                sidecar(&target_path, "-journal"),
                sidecar(&staged.database, "-journal"),
            ] {
                std::fs::write(&journal, b"").unwrap();

                let result =
                    replace_database(&database, &target_path, &staged.database, &workspace).await;
                assert!(
                    matches!(result, Err(RestoreError::NotRestored(_))),
                    "{result:?}"
                );
                std::fs::remove_file(&journal).unwrap();

                let pool = installed(&database).await;
                assert_eq!(dump(&pool).await, before);
                assert!(!workspace.join(ROLLBACK_FILE).exists());
            }
            let pool = installed(&database).await;
            // The checked copy is still waiting; cancelling removes it.
            assert!(staged.database.exists());
            discard_staged(&workspace);
            assert!(!workspace.exists());

            pool.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn cancelling_a_checked_backup_removes_its_copy_and_changes_nothing() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-cancel");
            let source = source_database(&dir.join("source").join("synapse.sqlite")).await;
            let archive = dir.join("backup.zip");
            write_package(&source, &archive, at(NOW), "0.1.0")
                .await
                .unwrap();
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let before = dump(&target).await;
            let workspace = dir.join("target").join("import-workspace");

            // Leftovers from an import that was never finished are cleared
            // before the next one.
            std::fs::create_dir_all(&workspace).unwrap();
            std::fs::write(workspace.join(STAGED_FILE), b"stale").unwrap();
            std::fs::write(sidecar(&workspace.join(STAGED_FILE), "-journal"), b"stale").unwrap();

            let mut pending = Pending::default();
            let staged = stage_package(&archive, &workspace).await.unwrap();
            let token = pending.hold(staged.database);
            pending.clear(&workspace);

            assert!(!workspace.exists());
            assert_eq!(pending.take(token), None);
            assert_eq!(dump(&target).await, before);
            assert_eq!(file_names(&dir.join("target")), vec!["synapse.sqlite"]);

            target.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_checked_backup_is_confirmed_only_with_its_own_token_and_only_once() {
        let mut pending = Pending::default();
        let first = pending.hold(PathBuf::from("first"));
        let second = pending.hold(PathBuf::from("second"));
        assert_ne!(first, second);

        // An out-of-date confirmation doesn't take the newer backup.
        assert_eq!(pending.take(first), None);
        assert_eq!(pending.take(second), Some(PathBuf::from("second")));
        assert_eq!(pending.take(second), None);
    }
}
