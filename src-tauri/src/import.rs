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
//! Only Synapse's own export formats are accepted. Format 2 (since migration
//! 0016) holds:
//!
//! ```text
//! manifest.json
//! db/synapse.sqlite
//! assets/<sha256>      (one per row of the database's `assets` table)
//! ```
//!
//! Format 1 (up to v0.7.3) is exactly the first two entries; it still
//! restores, and its database, brought up to date, simply keeps no files.
//!
//! Anything else is refused: other or extra entries, duplicate names, paths
//! that could climb out of a folder, directories, symlinks, encrypted
//! entries, unusual compression, an archive comment, or data in front of the
//! archive. Nothing is ever extracted to a path taken from the archive: a
//! kept file is written under its hash, after the name was checked to be
//! exactly 64 hex digits.
//!
//! The kept files are restored together with the database. Because each is
//! named by its hash and never changes, they *join* the store beside the
//! database rather than replacing it: at every moment the store holds every
//! file either database needs, so a restore stopped at any point never
//! leaves a database pointing at a missing file. Files that only the
//! replaced database used stay in the store unused (see `assets.rs`).

use std::fs::File;
use std::io::{BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tauri::async_runtime::Mutex;
use zip::{CompressionMethod, ZipArchive};

use crate::assets::{self, AssetStore, HashingWriter, MediaType};
use crate::db::{self, Database, DbError};
use crate::export::{
    self, Manifest, ASSETS_FOLDER, DATABASE_ENTRY, FORMAT, FORMAT_VERSION, MANIFEST_ENTRY,
};

/// The largest `manifest.json` accepted. A real one is a few hundred bytes.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// The largest database accepted, once decompressed: 1 GiB. Far beyond any
/// real flashcard collection, but it stops a hostile archive from filling
/// the disk. A restore needs up to twice this in free space: the checked copy
/// plus the rollback copy of the current database.
pub const MAX_DATABASE_BYTES: u64 = 1024 * 1024 * 1024;

/// The checked copy of a backup's database, inside the import workspace.
const STAGED_FILE: &str = "staged.sqlite";
/// The checked copies of a backup's kept files, inside the import workspace.
const STAGED_ASSETS: &str = "staged-assets";
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
    /// One kept file, and all of them together, and how many.
    asset: u64,
    assets_total: u64,
    asset_count: u64,
}

const LIMITS: Limits = Limits {
    manifest: MAX_MANIFEST_BYTES,
    database: MAX_DATABASE_BYTES,
    asset: assets::MAX_ASSET_BYTES,
    assets_total: assets::MAX_STORE_BYTES,
    asset_count: assets::MAX_STORE_FILES,
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
    /// Bigger than [`MAX_DATABASE_BYTES`] or [`MAX_MANIFEST_BYTES`], or
    /// with more kept files than the store can hold (`assets.rs`).
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
    /// The hashes of the checked kept files, in the workspace's
    /// `staged-assets` folder, in order.
    pub assets: Vec<String>,
    pub manifest: Manifest,
}

/// Checks the package at `archive` and prepares its database in `workspace`.
///
/// The real database is never touched. Checks run in this order, and the
/// first failure stops everything and removes the private copy:
///
/// 1. The file ends in a zip directory record listing at least two entries
///    (and no more than the store can hold) and no archive comment. (Read
///    directly, because the zip reader quietly merges entries that share a
///    name.)
/// 2. The zip opens with nothing in front of it and holds `manifest.json`,
///    `db/synapse.sqlite`, and nothing else but `assets/<sha256>` entries:
///    plain files with safe names, not encrypted, stored or deflated, and
///    within the size limits.
/// 3. The manifest (its CRC-32 verified) names `format` "synapse-export",
///    then a `format_version` this build reads (1 or 2); then it parses
///    strictly, names the entries its format has (format 1 has no kept
///    files), and has a `schema_version` from 1 up to this build's and a
///    valid `exported_at`.
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
/// 10. The package's kept files are exactly the ones the database lists, and
///     each one, extracted to the workspace, has the size its row says, bytes
///     that hash to its name, and the type its row says (by its first bytes).
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
    let (manifest, mut zip, entries) = unpack(archive, workspace, staged, limits)?;
    check_database(staged, manifest.schema_version).await?;
    let assets = stage_assets(&mut zip, staged, &workspace.join(STAGED_ASSETS), entries).await?;
    Ok(StagedImport {
        database: staged.to_path_buf(),
        assets,
        manifest,
    })
}

/// Removes a backup's private copy and anything SQLite left beside it, then
/// the workspace folder if nothing else is in it. Only names this module
/// creates are ever removed.
pub fn discard_staged(workspace: &Path) {
    remove_database_files(&workspace.join(STAGED_FILE));
    remove_staged_assets(workspace);
    let _ = std::fs::remove_dir(workspace);
}

/// Removes the checked copies of a backup's kept files (only names that are
/// a kept file's), then their folder if nothing else is in it.
fn remove_staged_assets(workspace: &Path) {
    let dir = workspace.join(STAGED_ASSETS);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if assets::is_asset_name(&entry.file_name().to_string_lossy()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(&dir);
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

/// What [`unpack`] hands on: the manifest, the open archive, and its kept
/// files' hashes and sizes.
type Unpacked = (Manifest, ZipArchive<File>, Vec<(String, u64)>);

/// Checks 1–5: the archive, its manifest, and extracting the database.
/// Returns the manifest, the open archive, and the hashes of its kept files
/// with their sizes, in order.
fn unpack(
    archive: &Path,
    workspace: &Path,
    staged: &Path,
    limits: Limits,
) -> Result<Unpacked, ImportError> {
    let mut file = File::open(archive).map_err(|err| ImportError::Unreadable(Box::new(err)))?;
    let listed = check_end_record(&mut file, limits)?;

    let mut zip = ZipArchive::new(file).map_err(|err| ImportError::NotAnExport(Box::new(err)))?;
    if zip.offset() != 0 {
        return Err(not_an_export("there is data in front of the archive"));
    }
    let entries = check_entries(&mut zip, listed, limits)?;
    let manifest = read_manifest(&mut zip, limits, !entries.is_empty())?;

    std::fs::create_dir_all(workspace).map_err(internal)?;
    extract_database(&mut zip, staged, limits)?;
    check_sqlite_header(staged)?;
    Ok((manifest, zip, entries))
}

/// The zip "end of central directory" record: 22 bytes, then the archive
/// comment. Synapse writes no comment, so the record ends the file.
const END_RECORD_LEN: u64 = 22;
/// The zip64 locator that sits in front of the end record when an archive
/// needs zip64. A Synapse export never does.
const ZIP64_LOCATOR_LEN: u64 = 20;

/// Check 1: the end record lists at least the two required entries, no more
/// than kept files allow, and no comment. Returns how many it lists.
fn check_end_record(file: &mut File, limits: Limits) -> Result<usize, ImportError> {
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
    let listed = field(10);
    if field(8) != listed || listed < 2 || u64::from(listed) > 2 + limits.asset_count {
        return Err(not_an_export(format!(
            "the archive lists {listed} entries ({} on this disk)",
            field(8)
        )));
    }
    Ok(usize::from(listed))
}

/// Check 2: the manifest, the database, and kept files only, each a plain,
/// safe file. Returns the kept files' hashes and sizes, in order.
fn check_entries(
    zip: &mut ZipArchive<File>,
    listed: usize,
    limits: Limits,
) -> Result<Vec<(String, u64)>, ImportError> {
    // The reader agrees with the end record only if no names repeat.
    if zip.len() != listed {
        return Err(not_an_export("the archive's entries share a name"));
    }
    let mut required = 0;
    let mut kept = Vec::new();
    let mut kept_total: u64 = 0;
    for index in 0..zip.len() {
        let entry = zip
            .by_index_raw(index)
            .map_err(|err| ImportError::Damaged(Box::new(err)))?;
        let hash = entry
            .name_raw()
            .strip_prefix(ASSETS_FOLDER.as_bytes())
            .and_then(|name| std::str::from_utf8(name).ok())
            .filter(|name| assets::is_asset_name(name));
        let limit = match entry.name_raw() {
            name if name == MANIFEST_ENTRY.as_bytes() || name == DATABASE_ENTRY.as_bytes() => {
                required += 1;
                if name == MANIFEST_ENTRY.as_bytes() {
                    limits.manifest
                } else {
                    limits.database
                }
            }
            _ if hash.is_some() => limits.asset,
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
        if let Some(hash) = hash {
            kept_total = kept_total.saturating_add(entry.size());
            kept.push((hash.to_string(), entry.size()));
        }
    }
    // Distinct names, so two required ones means both are there.
    if required != 2 {
        return Err(not_an_export("the manifest or the database is missing"));
    }
    if kept_total > limits.assets_total {
        return Err(ImportError::TooLarge);
    }
    kept.sort();
    Ok(kept)
}

/// Check 3: the manifest describes a package this build can restore.
/// `has_assets` says whether the archive has kept files.
fn read_manifest(
    zip: &mut ZipArchive<File>,
    limits: Limits,
    has_assets: bool,
) -> Result<Manifest, ImportError> {
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
        // Format 1 has no kept files: one there isn't part of this format.
        Some(1) if has_assets => return Err(not_an_export("format 1 holds no kept files")),
        Some(version) if (1..=u64::from(FORMAT_VERSION)).contains(&version) => {}
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
    let expected_assets = (manifest.format_version >= 2).then_some(ASSETS_FOLDER);
    if manifest.contents.assets.as_deref() != expected_assets {
        return Err(damaged("the manifest names another folder of kept files"));
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

/// Check 10: extracts the kept files the checked database lists into
/// `dir`, checking each one, and returns their hashes in order. `entries`
/// are the archive's kept files (from check 2), which must be exactly those.
async fn stage_assets(
    zip: &mut ZipArchive<File>,
    staged: &Path,
    dir: &Path,
    entries: Vec<(String, u64)>,
) -> Result<Vec<String>, ImportError> {
    let listed = export::listed_assets(staged).await.map_err(damaged)?;
    let names: Vec<&str> = listed.iter().map(|(hash, _, _)| hash.as_str()).collect();
    let packed: Vec<&str> = entries.iter().map(|(hash, _)| hash.as_str()).collect();
    if names != packed {
        return Err(damaged(format!(
            "the package has {} kept files, and they aren't the {} its database lists",
            packed.len(),
            names.len()
        )));
    }
    if listed.is_empty() {
        return Ok(Vec::new());
    }

    std::fs::create_dir_all(dir).map_err(internal)?;
    for ((hash, kind, size), (_, packed_size)) in listed.iter().zip(&entries) {
        if packed_size != size {
            return Err(damaged(format!(
                "kept file {hash} isn't the size its row says"
            )));
        }
        let entry = zip
            .by_name(&format!("{ASSETS_FOLDER}{hash}"))
            .map_err(|err| ImportError::Damaged(Box::new(err)))?;
        let path = dir.join(hash);
        // `create_new`: never writes over a file, even one this module owns.
        let file = File::create_new(&path).map_err(internal)?;
        let mut out = HashingWriter::new(BufWriter::new(file));
        copy_limited(entry, &mut out, *size)?;
        let found = out.finish_into();
        let file = found
            .1
            .into_inner()
            .map_err(|err| internal(err.into_error()))?;
        file.sync_all().map_err(internal)?;
        if found.0 != *hash {
            return Err(damaged(format!(
                "kept file {hash} doesn't hash to its name"
            )));
        }
        if !starts_as(&path, *kind) {
            return Err(damaged(format!(
                "kept file {hash} isn't the type its row says"
            )));
        }
    }
    Ok(listed.into_iter().map(|(hash, _, _)| hash).collect())
}

/// Whether the file at `path` starts the way a file of `kind` does.
fn starts_as(path: &Path, kind: MediaType) -> bool {
    let mut head = Vec::with_capacity(16);
    File::open(path)
        .and_then(|file| file.take(16).read_to_end(&mut head))
        .is_ok()
        && assets::matches_type(&head, kind)
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

/// Replaces the database at `db_path` with the checked copy at `staged`, and
/// adds the backup's checked kept files (in the workspace's `staged-assets`
/// folder) to `store`.
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
/// 3. The backup's kept files are moved into the store, each a rename on the
///    same drive. A file already there (same name, same size) is left as it
///    is: the same name means the same bytes. If a move fails, the files
///    moved so far are removed again; stop and reopen the current database.
///    From here the store holds every file either database lists.
///    If a journal is beside either database file (an interrupted write,
///    which SQLite would replay into whichever database then has that name),
///    stop and reopen the current database.
/// 4. The checked copy is renamed over the database: one step on the same
///    drive, so the file is always either the old database or the new one.
///    If that fails — on Windows, also when another program still has the
///    file open — stop and reopen the current database.
/// 5. The new database is opened, its integrity checked, and every file it
///    lists found in the store. If that fails, the rollback copy is renamed
///    back and reopened instead, and the files step 3 added are removed.
/// 6. The new pool is installed and the rollback copy deleted.
///
/// Every stop before step 6 leaves the previous data in place.
pub async fn replace_database(
    database: &Database,
    db_path: &Path,
    store: &AssetStore,
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

    // 3. The store only gains files, which the current database doesn't
    // list, so it is unaffected whatever happens next.
    let added = match add_staged_assets(&workspace.join(STAGED_ASSETS), store) {
        Ok(added) => added,
        Err(err) => {
            remove_database_files(&rollback);
            *slot = reopen(db_path).await;
            return Err(RestoreError::NotRestored(err));
        }
    };

    // 3 and 4. Nothing the current database uses has changed if this fails.
    if let Err(err) = swap_in(staged, db_path) {
        remove_files(&added);
        remove_database_files(&rollback);
        *slot = reopen(db_path).await;
        return Err(RestoreError::NotRestored(err));
    }

    // 5.
    match open_checked(db_path, store).await {
        Ok(pool) => {
            // 6.
            *slot = Some(pool);
            remove_database_files(&rollback);
            remove_staged_assets(workspace);
            let _ = std::fs::remove_dir(workspace);
            Ok(())
        }
        Err(err) => {
            remove_files(&added);
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

/// Step 3: moves every checked kept file in `staged` into `store`, and
/// returns the files that weren't there before. On a failure, those moved so
/// far are removed again, so the store is as it was.
fn add_staged_assets(staged: &Path, store: &AssetStore) -> Result<Vec<PathBuf>, DbError> {
    let mut added = Vec::new();
    let result = (|| -> Result<(), DbError> {
        let entries = match std::fs::read_dir(staged) {
            Ok(entries) => entries,
            // A backup with no kept files has no folder.
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(target) = store.path_of(&name) else {
                continue;
            };
            let size = entry.metadata()?.len();
            if store.has(&name, size) {
                continue;
            }
            std::fs::create_dir_all(store.dir())?;
            let existed = !matches!(target.try_exists(), Ok(false));
            std::fs::rename(entry.path(), &target)?;
            if !existed {
                added.push(target);
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(added),
        Err(err) => {
            remove_files(&added);
            Err(err)
        }
    }
}

fn remove_files(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// Opens the restored database, checking it again now it's in place: its
/// integrity, and every file it lists there in `store`, at its size.
async fn open_checked(path: &Path, store: &AssetStore) -> Result<SqlitePool, DbError> {
    let pool = db::open_file(path).await?;
    let checked = async {
        integrity_ok(&pool).await?;
        let listed: Vec<(String, i64)> = sqlx::query_as("SELECT sha256, size FROM assets")
            .fetch_all(&pool)
            .await?;
        for (hash, size) in listed {
            if !store.has(&hash, u64::try_from(size)?) {
                return Err(format!("kept file {hash} isn't in the store").into());
            }
        }
        Ok::<_, DbError>(())
    }
    .await;
    if let Err(err) = checked {
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
    use crate::assets::test_support::*;
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

    /// The store beside the database at `db_path`, where the app keeps it.
    fn store_beside(db_path: &Path) -> AssetStore {
        AssetStore::new(db_path.parent().unwrap().join(assets::STORE_FOLDER))
    }

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
    /// reviewed card, a card reviewed and then soft-deleted, a multiline new
    /// card and its unfinished session, an archived deck with a reviewed card
    /// and a finished session, and three typed notes: one plain, one edited,
    /// and one soft-deleted. Every kind of history an import must keep.
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
        // Reviewed first, so the deleted card carries a schedule to keep.
        study::record_review(&pool, session, cards[1], 0, Rating::Easy, at(NOW))
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
            // names, never from input. Ordered by the rows themselves: some
            // of FTS5's tables for the search index have no rowid.
            let sql = format!("SELECT {values} FROM \"{table}\" ORDER BY 1");
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
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(LATER),
                "0.1.0",
            )
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

            replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged.database,
                &workspace,
            )
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

            // The deleted card and note travelled as deleted, and restore here
            // as they would have before the backup: the card back into its
            // deck with the schedule its review gave it, the note back into
            // its old place in the library.
            let card_b: i64 = sqlx::query_scalar("SELECT id FROM flashcards WHERE front = 'B'")
                .fetch_one(&restored)
                .await
                .unwrap();
            let schedule_sql = "SELECT quote(fsrs_state) || ' ' || quote(fsrs_stability) || ' '
                                    || quote(fsrs_difficulty) || ' ' || quote(due) || ' '
                                    || quote(last_review) || ' ' || quote(reps) || ' '
                                    || quote(lapses)
                                FROM flashcards WHERE id = ?1";
            let schedule: String = sqlx::query_scalar(schedule_sql)
                .bind(card_b)
                .fetch_one(&restored)
                .await
                .unwrap();
            assert!(schedule.starts_with("'Review'"), "{schedule}");
            let detail = crate::authoring::restore_flashcard(&restored, card_b, at(LATER))
                .await
                .unwrap();
            assert_eq!((detail.id, detail.card_count), (biology, 3));
            assert!(detail.deleted_cards.is_empty());
            let restored_schedule: String = sqlx::query_scalar(schedule_sql)
                .bind(card_b)
                .fetch_one(&restored)
                .await
                .unwrap();
            assert_eq!(restored_schedule, schedule);
            let old_lecture = crate::notes::deleted_notes(&restored).await.unwrap()[0].id;
            crate::notes::restore_note(&restored, old_lecture)
                .await
                .unwrap();
            assert_eq!(
                crate::notes::notes(&restored)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|note| note.title)
                    .collect::<Vec<_>>(),
                vec!["Lecture 2", "Old lecture", "Lecture 1"]
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
            write_package(
                &source,
                &store_of(&source).await,
                &genuine_path,
                at(NOW),
                "0.1.0",
            )
            .await
            .unwrap();
            source.close().await;
            let genuine = std::fs::read(&genuine_path).unwrap();
            let manifest = read_entry(&genuine, MANIFEST_ENTRY);
            let database = read_entry(&genuine, DATABASE_ENTRY);
            let latest = db::latest_schema_version();
            assert_eq!(
                serde_json::from_slice::<Manifest>(&manifest)
                    .unwrap()
                    .format_version,
                2
            );
            let kept_entry = format!("{ASSETS_FOLDER}{}", assets::sha256_hex(&png(0)));

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
                            &manifest_with(|m| m["format_version"] = 3.into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "too new",
                ),
                (
                    "format 2 without its folder of kept files",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["format_version"] = 2.into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "format 1 naming a folder of kept files",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| m["contents"]["assets"] = "assets/".into()),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "format 2 naming another folder",
                    zip_of(&[
                        (
                            MANIFEST_ENTRY,
                            &manifest_with(|m| {
                                m["format_version"] = 2.into();
                                m["contents"]["assets"] = "files/".into();
                            }),
                        ),
                        (DATABASE_ENTRY, &database),
                    ]),
                    "damaged",
                ),
                (
                    "format 1 with a kept file",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest_with(|_| {})),
                        (DATABASE_ENTRY, &database),
                        (&kept_entry, &png(0)),
                    ]),
                    "not an export",
                ),
                (
                    "a kept file the database doesn't list",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &database),
                        (&kept_entry, &png(0)),
                    ]),
                    "damaged",
                ),
                (
                    "a kept file under a name that isn't a hash",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &database),
                        (
                            &kept_entry.to_uppercase().replace("ASSETS/", "assets/"),
                            &png(0),
                        ),
                    ]),
                    "not an export",
                ),
                (
                    "a folder of kept files as an entry",
                    {
                        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                        let options = SimpleFileOptions::default();
                        zip.start_file(MANIFEST_ENTRY, options).unwrap();
                        zip.write_all(&manifest).unwrap();
                        zip.start_file(DATABASE_ENTRY, options).unwrap();
                        zip.write_all(&database).unwrap();
                        zip.add_directory("assets/", options).unwrap();
                        zip.finish().unwrap().into_inner()
                    },
                    "not an export",
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
                "assets/../db/synapse.sqlite",
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
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.1.0",
            )
            .await
            .unwrap();
            source.close().await;
            let workspace = dir.join("import-workspace");

            // Declared sizes over the limit are refused before extracting.
            let tiny = Limits {
                manifest: 16,
                ..LIMITS
            };
            let result = stage_with_limits(&archive, &workspace, tiny).await;
            assert_eq!(describe(&result), "too large");
            let tiny = Limits {
                database: 1024,
                ..LIMITS
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
            let old_version = previous_schema_version();
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
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.1.0",
            )
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
                db::test_support::migration_count()
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
    fn a_backup_from_before_personal_parameters_restores_with_the_defaults() {
        tauri::async_runtime::block_on(async {
            use crate::optimizer::{self, Optimized, Optimizing};

            let dir = scratch("import-pre-0011");
            // A backup from v0.6.0 (schema 10), before FSRS parameters could
            // be fitted: a deck with months of reviews.
            let source_path = dir.join("source").join("synapse.sqlite");
            std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
            let source =
                sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", source_path.display()))
                    .await
                    .unwrap();
            migrations_up_to(10).run(&source).await.unwrap();
            sqlx::query("INSERT INTO seed_markers (name) VALUES ('sample_card')")
                .execute(&source)
                .await
                .unwrap();
            optimizer::tests::add_history(&source, 150).await;
            let archive = dir.join("backup.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.6.0",
            )
            .await
            .unwrap();
            let source_rows = dump(&source).await;
            source.close().await;

            // The database it replaces uses parameters of its own.
            let target_path = dir.join("target").join("synapse.sqlite");
            let database = Database::default();
            let target = target_database(&target_path).await;
            sqlx::query(
                "INSERT INTO fsrs_parameters (parameters, reviews_used, applied_at)
                 VALUES ('[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0.1]', 900, ?1)",
            )
            .bind(NOW)
            .execute(&target)
            .await
            .unwrap();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, 10);
            replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();
            discard_staged(&workspace);
            let restored = installed(&database).await;

            // The backup's history is all there, and its ratings are
            // scheduled with FSRS's defaults, not the replaced database's.
            // Its cards gained only their kind (migration 0012).
            let imported = dump(&restored).await;
            for (name, rows) in &source_rows {
                if name != "sqlite_master" && name != "_sqlx_migrations" {
                    let found = imported.iter().find(|(table, _)| table == name).unwrap();
                    let rows: Vec<String> = if name == "flashcards" {
                        rows.iter()
                            .map(|row| format!("{row} | 'basic' | NULL | NULL"))
                            .collect()
                    } else {
                        rows.clone()
                    };
                    assert_eq!(found.1, rows, "{name}");
                }
            }
            let settings = optimizer::settings(&restored).await.unwrap();
            assert!(!settings.personal);
            assert_eq!(settings.version, 0);
            assert_eq!(settings.usable_reviews, 600);

            // Its history can be optimized, and the parameters applied are
            // kept in the next backup.
            let optimizing = Optimizing::default();
            let Optimized::Proposed { proposal, .. } =
                optimizer::optimize(&restored, &optimizing, optimizer::TIME_LIMIT)
                    .await
                    .unwrap()
            else {
                panic!("expected parameters to be proposed");
            };
            optimizer::apply(&restored, &proposal, at(LATER))
                .await
                .unwrap();
            let next = dir.join("next.zip");
            write_package(
                &restored,
                &store_of(&restored).await,
                &next,
                at(LATER),
                "0.7.0",
            )
            .await
            .unwrap();
            let workspace = dir.join("next-workspace");
            let staged = stage_package(&next, &workspace).await.unwrap();
            let copy = open_file(&staged.database).await.unwrap();
            let kept = optimizer::settings(&copy).await.unwrap();
            assert!(kept.personal);
            assert_eq!(kept.parameters, proposal.proposed);
            copy.close().await;
            discard_staged(&workspace);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_v0_7_0_backup_restores_with_everything_in_it_searchable() {
        tauri::async_runtime::block_on(async {
            use crate::search::{self, Hit, Scope};

            let dir = scratch("import-pre-0013");
            // A backup from v0.7.0 (schema 11), before the search index: a
            // card, a deleted card, an archived deck's card, a note and a
            // deleted note.
            let source_path = dir.join("source").join("synapse.sqlite");
            std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
            let source =
                sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", source_path.display()))
                    .await
                    .unwrap();
            migrations_up_to(11).run(&source).await.unwrap();
            for sql in [
                "INSERT INTO seed_markers (name) VALUES ('sample_card')",
                "INSERT INTO decks (name, created_at) VALUES ('Biology', ?1), ('Old', ?1)",
                "INSERT INTO flashcards (deck_id, front, back) VALUES
                     (2, 'Photosynthèse', 'Light to sugar'),
                     (2, 'Photosynthesis deleted', 'Gone'),
                     (3, 'Photosynthesis archived', 'Put away')",
                "UPDATE flashcards SET deleted_at = ?1 WHERE id = 2",
                "UPDATE decks SET archived_at = ?1 WHERE id = 3",
                "INSERT INTO notes (title, body, created_at, updated_at) VALUES
                     ('Plants', 'Chlorophyll drives photosynthesis.', ?1, ?1),
                     ('Draft', 'Photosynthesis draft', ?1, ?1)",
                "UPDATE notes SET deleted_at = ?1 WHERE id = 2",
            ] {
                sqlx::query(sql).bind(NOW).execute(&source).await.unwrap();
            }
            let archive = dir.join("backup.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.7.0",
            )
            .await
            .unwrap();
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let database = Database::default();
            *database.slot().lock().await = Some(target_database(&target_path).await);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, 11);
            replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();
            discard_staged(&workspace);
            let restored = installed(&database).await;

            // Every card and note was indexed on the way in; only the active
            // ones are found, and what was put away is found once it's back.
            assert_eq!(
                count(&restored, "SELECT COUNT(*) FROM search_index").await,
                5
            );
            let found = |hits: &search::SearchResults| -> Vec<String> {
                let mut labels: Vec<String> = hits
                    .results
                    .iter()
                    .map(|hit| match hit {
                        Hit::Card { card_id, .. } => format!("card {card_id}"),
                        Hit::Note { note_id, .. } => format!("note {note_id}"),
                    })
                    .collect();
                labels.sort();
                labels
            };
            let hits = search::search(&restored, "photosynth", Scope::All, None)
                .await
                .unwrap();
            assert_eq!(found(&hits), ["card 1", "note 1"]);
            crate::authoring::restore_flashcard(&restored, 2, at(LATER))
                .await
                .unwrap();
            crate::notes::restore_note(&restored, 2).await.unwrap();
            crate::authoring::unarchive_deck(&restored, 3)
                .await
                .unwrap();
            let hits = search::search(&restored, "photosynth", Scope::All, None)
                .await
                .unwrap();
            assert_eq!(
                found(&hits),
                ["card 1", "card 2", "card 3", "note 1", "note 2"]
            );

            // A backup made now carries the index, passes every check, and
            // restores to the same rows, index included.
            let next = dir.join("next.zip");
            write_package(
                &restored,
                &store_of(&restored).await,
                &next,
                at(LATER),
                "0.8.0",
            )
            .await
            .unwrap();
            let workspace = dir.join("next-workspace");
            let staged = stage_package(&next, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, db::latest_schema_version());
            let copy = open_file(&staged.database).await.unwrap();
            assert_eq!(dump(&copy).await, dump(&restored).await);
            sqlx::query("INSERT INTO search_index (search_index) VALUES ('integrity-check')")
                .execute(&copy)
                .await
                .unwrap();
            copy.close().await;
            discard_staged(&workspace);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_v0_7_0_backup_restores_aiming_for_ninety_percent() {
        tauri::async_runtime::block_on(async {
            use crate::retention::{self, Change};

            let dir = scratch("import-pre-0014");
            // A backup from v0.7.0 (schema 11), before desired retention could
            // be chosen: a deck with months of reviews and parameters applied.
            let source_path = dir.join("source").join("synapse.sqlite");
            std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
            let source =
                sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", source_path.display()))
                    .await
                    .unwrap();
            migrations_up_to(11).run(&source).await.unwrap();
            sqlx::query("INSERT INTO seed_markers (name) VALUES ('sample_card')")
                .execute(&source)
                .await
                .unwrap();
            let deck = crate::optimizer::tests::add_history(&source, 60).await;
            sqlx::query(
                "INSERT INTO fsrs_parameters (parameters, reviews_used, applied_at)
                 VALUES ('[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0.1]', 900, ?1)",
            )
            .bind(NOW)
            .execute(&source)
            .await
            .unwrap();
            let archive = dir.join("backup.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.7.0",
            )
            .await
            .unwrap();
            let source_rows = dump(&source).await;
            source.close().await;

            // The database it replaces aims for a value of its own.
            let target_path = dir.join("target").join("synapse.sqlite");
            let database = Database::default();
            let target = target_database(&target_path).await;
            retention::set(
                &target,
                0,
                Change {
                    deck_id: None,
                    percent: Some(75),
                },
                at(NOW),
            )
            .await
            .unwrap();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, 11);
            replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();
            discard_staged(&workspace);
            let restored = installed(&database).await;

            // Everything in the backup is there, and it aims for 90% again,
            // not the replaced database's value. Columns later migrations
            // added (cloze cards' on flashcards) come after the old ones.
            let imported = dump(&restored).await;
            for (name, rows) in &source_rows {
                if name != "sqlite_master" && name != "_sqlx_migrations" {
                    let found = imported.iter().find(|(table, _)| table == name).unwrap();
                    assert_eq!(found.1.len(), rows.len(), "{name}");
                    for (now, before) in found.1.iter().zip(rows) {
                        assert!(
                            now == before || now.starts_with(&format!("{before} | ")),
                            "{name}: {now} is not {before}"
                        );
                    }
                }
            }
            let settings = retention::settings(&restored).await.unwrap();
            assert_eq!((settings.version, settings.global), (0, 90));
            let mut conn = restored.acquire().await.unwrap();
            assert_eq!(retention::for_deck(&mut conn, deck).await.unwrap(), 0.9);
            drop(conn);

            // Values chosen afterwards travel in the next backup, which
            // passes the same checks.
            retention::set(
                &restored,
                0,
                Change {
                    deck_id: Some(deck),
                    percent: Some(85),
                },
                at(LATER),
            )
            .await
            .unwrap();
            let next = dir.join("next.zip");
            write_package(
                &restored,
                &store_of(&restored).await,
                &next,
                at(LATER),
                "0.8.0",
            )
            .await
            .unwrap();
            let workspace = dir.join("next-workspace");
            let staged = stage_package(&next, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, db::latest_schema_version());
            let copy = open_file(&staged.database).await.unwrap();
            let kept = retention::settings(&copy).await.unwrap();
            assert_eq!(kept.version, 1);
            assert_eq!(
                kept.decks.iter().find(|d| d.id == deck).unwrap().percent,
                Some(85)
            );
            copy.close().await;
            discard_staged(&workspace);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_v0_7_0_backup_restores_as_basic_cards_and_cloze_cards_survive_the_next_backup() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-pre-0012");
            // A backup from v0.7.0 (schema 11), before cloze cards: a deck
            // with a reviewed card and a deleted one.
            let source_path = dir.join("source").join("synapse.sqlite");
            std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
            let source =
                sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", source_path.display()))
                    .await
                    .unwrap();
            migrations_up_to(11).run(&source).await.unwrap();
            for sql in [
                "INSERT INTO seed_markers (name) VALUES ('sample_card')",
                "INSERT INTO decks (name, created_at) VALUES ('Biology', '2026-09-15T12:00:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'X', 'x', 'Review', 2.5, 5.0, '2026-09-20T12:00:00.000Z',
                         '2026-09-15T12:01:00.000Z', 1, 0, '2026-09-15T12:00:30.000Z',
                         '2026-09-15T12:01:00.000Z'),
                        (2, 'Y', 'y', 'New', NULL, NULL, NULL, NULL, 0, 0,
                         '2026-09-15T12:00:30.000Z', '2026-09-15T12:00:30.000Z')",
                "UPDATE flashcards SET deleted_at = '2026-09-15T12:03:00.000Z',
                                       updated_at = '2026-09-15T12:03:00.000Z'
                 WHERE front = 'Y'",
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (1, 3, 'New', 2.5, 0, 2.5, 5.0, '2026-09-15T12:01:00.000Z')",
            ] {
                sqlx::query(sql).execute(&source).await.unwrap();
            }
            let archive = dir.join("backup.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.7.0",
            )
            .await
            .unwrap();
            let source_rows = dump(&source).await;
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let database = Database::default();
            *database.slot().lock().await = Some(target_database(&target_path).await);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, 11);
            replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();
            discard_staged(&workspace);
            let restored = installed(&database).await;

            // Every card is a basic card, with everything else the backup's.
            let imported = dump(&restored).await;
            for (name, rows) in &source_rows {
                if name == "sqlite_master" || name == "_sqlx_migrations" {
                    continue;
                }
                let found = imported.iter().find(|(table, _)| table == name).unwrap();
                let rows: Vec<String> = if name == "flashcards" {
                    rows.iter()
                        .map(|row| format!("{row} | 'basic' | NULL | NULL"))
                        .collect()
                } else {
                    rows.clone()
                };
                assert_eq!(found.1, rows, "{name}");
            }
            let deck = crate::authoring::restore_flashcard(&restored, 2, at(NOW))
                .await
                .unwrap();
            assert_eq!(deck.card_count, 2);

            // Cloze cards made now, one reviewed and one deleted, pass every
            // check of the next backup and come back exactly.
            let deck = crate::cloze::create_cloze(&restored, 2, "{{c1::A}} {{c2::B}}", at(NOW))
                .await
                .unwrap();
            let session = start(&restored, 2, NOW).await;
            let cloze = deck.cards[2].id;
            study::record_review(&restored, session, cloze, 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            delete_flashcard(&restored, deck.cards[3].id, at(NOW))
                .await
                .unwrap();
            let next = dir.join("next.zip");
            write_package(
                &restored,
                &store_of(&restored).await,
                &next,
                at(LATER),
                "0.8.0",
            )
            .await
            .unwrap();
            let before = dump(&restored).await;

            let other_path = dir.join("other").join("synapse.sqlite");
            let other = Database::default();
            *other.slot().lock().await = Some(target_database(&other_path).await);
            let workspace = dir.join("other").join("import-workspace");
            let staged = stage_package(&next, &workspace).await.unwrap();
            assert_eq!(staged.manifest.schema_version, db::latest_schema_version());
            replace_database(
                &other,
                &other_path,
                &store_beside(&other_path),
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();
            discard_staged(&workspace);
            let copy = installed(&other).await;
            let after = dump(&copy).await;
            for (name, rows) in &before {
                if name != "_sqlx_migrations" {
                    let found = after.iter().find(|(table, _)| table == name).unwrap();
                    assert_eq!(&found.1, rows, "{name}");
                }
            }
            let detail = crate::authoring::deck_detail(&copy, 2, at(NOW))
                .await
                .unwrap();
            assert_eq!(detail.card_count, 3);
            assert_eq!(detail.deleted_cards[0].cloze.as_ref().unwrap().number, 2);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_v0_1_0_backup_is_restored_and_what_it_put_away_can_be_brought_back() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-v0-1-0");
            // A backup from v0.1.0, the first public release: schema 7, when
            // nothing put away could come back yet. An archived deck holding a
            // reviewed card (1) and a card that lapsed and was deleted before
            // the deck was archived (2), their review logs and finished
            // session, and a note.
            let source_path = dir.join("source").join("synapse.sqlite");
            std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
            let source =
                sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", source_path.display()))
                    .await
                    .unwrap();
            migrations_up_to(7).run(&source).await.unwrap();
            for sql in [
                "INSERT INTO seed_markers (name) VALUES ('sample_card')",
                "INSERT INTO decks (name, created_at) VALUES ('Chemistry', '2026-09-15T12:00:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at)
                 VALUES (2, 'X', 'x', 'Review', 2.5, 5.0, '2026-09-20T12:00:00.000Z',
                         '2026-09-15T12:01:00.000Z', 1, 0, '2026-09-15T12:00:30.000Z',
                         '2026-09-15T12:01:00.000Z')",
                "INSERT INTO flashcards (deck_id, front, back, fsrs_state, fsrs_stability,
                                         fsrs_difficulty, due, last_review, reps, lapses,
                                         created_at, updated_at, deleted_at)
                 VALUES (2, 'Y', 'y', 'Relearning', 0.4, 7.2, '2026-09-15T12:12:00.000Z',
                         '2026-09-15T12:02:00.000Z', 2, 1, '2026-09-15T12:00:30.000Z',
                         '2026-09-15T12:03:00.000Z', '2026-09-15T12:03:00.000Z')",
                "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                          elapsed_days, stability_after, difficulty_after, reviewed_at)
                 VALUES (1, 3, 'New', 2.5, 0, 2.5, 5.0, '2026-09-15T12:01:00.000Z'),
                        (2, 3, 'New', 2.3, 0, 2.3, 2.1, '2026-09-15T12:01:30.000Z'),
                        (2, 1, 'Review', 0.0, 0.0, 0.4, 7.2, '2026-09-15T12:02:00.000Z')",
                "INSERT INTO sessions (deck_id, started_at, ended_at, cards_reviewed)
                 VALUES (2, '2026-09-15T12:00:45.000Z', '2026-09-15T12:04:00.000Z', 3)",
                "UPDATE decks SET archived_at = '2026-09-15T12:04:00.000Z' WHERE id = 2",
                "INSERT INTO notes (title, body, created_at, updated_at)
                 VALUES ('Lecture 1', 'Cells', '2026-09-16T09:00:00.000Z',
                         '2026-09-16T09:00:00.000Z')",
            ] {
                sqlx::query(sql).execute(&source).await.unwrap();
            }
            let archive = dir.join("backup.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.1.0",
            )
            .await
            .unwrap();
            let source_rows = dump(&source).await;
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let database = Database::default();
            *database.slot().lock().await = Some(target_database(&target_path).await);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(
                (
                    staged.manifest.schema_version,
                    staged.manifest.app_version.as_str()
                ),
                (7, "0.1.0")
            );
            replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();
            let restored = installed(&database).await;

            // Every deck, card, log, session, and marker is exactly the
            // backup's: the deck still archived, the card still deleted, with
            // its FSRS state and history. The note gained only its (empty)
            // deletion column.
            fn table(rows: &[(String, Vec<String>)], name: &str) -> Vec<String> {
                rows.iter()
                    .find(|(table, _)| table == name)
                    .map(|(_, rows)| rows.clone())
                    .unwrap()
            }
            let imported = dump(&restored).await;
            for name in ["decks", "review_logs", "sessions", "seed_markers"] {
                assert_eq!(table(&imported, name), table(&source_rows, name), "{name}");
            }
            // Its cards gained only their kind (migration 0012).
            let cards: Vec<String> = table(&source_rows, "flashcards")
                .into_iter()
                .map(|row| format!("{row} | 'basic' | NULL | NULL"))
                .collect();
            assert_eq!(table(&imported, "flashcards"), cards);
            let notes: Vec<String> = table(&source_rows, "notes")
                .into_iter()
                .map(|row| format!("{row} | NULL"))
                .collect();
            assert_eq!(table(&imported, "notes"), notes);

            // What v0.1.0 could only put away comes back here: the deck, then
            // its card, and a note deleted and restored after the import.
            crate::authoring::unarchive_deck(&restored, 2)
                .await
                .unwrap();
            let detail = crate::authoring::restore_flashcard(&restored, 2, at(LATER))
                .await
                .unwrap();
            let listed: Vec<i64> = detail.cards.iter().map(|card| card.id).collect();
            assert_eq!(listed, vec![1, 2]);
            crate::notes::delete_note(&restored, 1, at(LATER))
                .await
                .unwrap();
            crate::notes::restore_note(&restored, 1).await.unwrap();

            // Each wrote only its state column, which is the last column of
            // decks and, before the three cloze columns of 0012, of cards:
            // set aside, every row is still the backup's. Nothing else
            // moved, and no session was opened.
            fn without_state(rows: Vec<String>, from_end: usize) -> Vec<String> {
                rows.into_iter()
                    .map(|row| {
                        let mut columns: Vec<&str> = row.split(" | ").collect();
                        columns.remove(columns.len() - 1 - from_end);
                        columns.join(" | ")
                    })
                    .collect()
            }
            let after = dump(&restored).await;
            for (name, from_end) in [("decks", 0), ("flashcards", 3)] {
                assert_eq!(
                    without_state(table(&after, name), from_end),
                    without_state(table(&imported, name), from_end),
                    "{name}"
                );
            }
            for name in ["review_logs", "sessions", "seed_markers", "notes"] {
                assert_eq!(table(&after, name), table(&imported, name), "{name}");
            }
            assert_eq!(
                count(
                    &restored,
                    "SELECT COUNT(*) FROM decks WHERE archived_at IS NOT NULL"
                )
                .await,
                0
            );
            assert_eq!(
                count(
                    &restored,
                    "SELECT COUNT(*) FROM flashcards WHERE deleted_at IS NOT NULL"
                )
                .await,
                0
            );

            restored.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// `source_database`, plus files kept beside it: a picture, and a PDF
    /// kept as a source with a note written from it. Returns the pool and the
    /// kept files' hashes, in order.
    async fn database_with_kept_files(path: &Path) -> (SqlitePool, Vec<String>) {
        let pool = source_database(path).await;
        let store = store_of(&pool).await;
        let picture = assets::add_picture(&pool, &store, &png(10), at(NOW))
            .await
            .unwrap();
        let document = crate::sources::test_support::pdf_document("kept");
        let source = crate::sources::create_source(&pool, &store, &document, at(NOW))
            .await
            .unwrap();
        // The note shows the picture (0017), so a restore must bring both.
        let note = crate::notes::create_note_with_pictures(
            &pool,
            "From the PDF",
            "Cells",
            &[picture.id],
            at(NOW),
        )
        .await
        .unwrap();
        crate::sources::link_note(&pool, note.id, source.id, at(NOW))
            .await
            .unwrap();
        let mut hashes = vec![picture.sha256, source.asset.sha256];
        hashes.sort();
        (pool, hashes)
    }

    /// The bytes of a new database at `path` whose only kept file is listed
    /// with the given hash, type and size.
    async fn database_listing(path: &Path, hash: &str, media: &str, size: usize) -> Vec<u8> {
        let pool = open_file(path).await.unwrap();
        sqlx::query(
            "INSERT INTO assets (sha256, media_type, size, created_at)
             VALUES (?1, ?2, ?3, '2026-10-20T12:00:00.000Z')",
        )
        .bind(hash)
        .bind(media)
        .bind(size as i64)
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        std::fs::read(path).unwrap()
    }

    fn entry_names_of(archive: &[u8]) -> Vec<String> {
        let mut zip = ZipArchive::new(Cursor::new(archive)).unwrap();
        (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn kept_files_travel_in_a_backup_and_join_the_store_on_restore() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-kept-files");
            let (source, hashes) =
                database_with_kept_files(&dir.join("source").join("synapse.sqlite")).await;
            let archive = dir.join("backup.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(LATER),
                "0.8.0",
            )
            .await
            .unwrap();
            let source_rows = dump(&source).await;
            source.close().await;

            // Format 2: the manifest, the database, then each kept file.
            let bytes = std::fs::read(&archive).unwrap();
            let mut expected = vec![MANIFEST_ENTRY.to_string(), DATABASE_ENTRY.to_string()];
            expected.extend(hashes.iter().map(|hash| format!("{ASSETS_FOLDER}{hash}")));
            assert_eq!(entry_names_of(&bytes), expected);

            // The database being replaced keeps a picture of its own.
            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let store = store_beside(&target_path);
            let own = assets::add_picture(&target, &store, &png(11), at(NOW))
                .await
                .unwrap();
            let database = Database::default();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("target").join("import-workspace");

            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(staged.assets, hashes);
            // Checked, but not in the store yet.
            assert_eq!(stored_names(&store), vec![own.sha256.clone()]);

            replace_database(
                &database,
                &target_path,
                &store,
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();

            let restored = installed(&database).await;
            assert_eq!(dump(&restored).await, source_rows);
            // Every file the restored database lists is there, whole; the
            // replaced database's own picture stays, unused.
            for hash in &hashes {
                let kept = assets::asset_by_hash(&restored, hash)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(store.verify(hash, kept.size), "{hash}");
            }
            let mut all = hashes.clone();
            all.push(own.sha256.clone());
            all.sort();
            assert_eq!(stored_names(&store), all);
            assert!(!workspace.exists());

            // The restored source opens with its pages, its picture and PDF
            // are served, and the target's old picture isn't (no row names it).
            let kept_source = crate::sources::sources(&restored).await.unwrap().remove(0);
            assert_eq!(
                crate::sources::source_pages(&restored, kept_source.id)
                    .await
                    .unwrap()
                    .len(),
                3
            );
            for hash in &hashes {
                let response = assets::respond(&restored, &store, &format!("/{hash}")).await;
                assert_eq!(response.status(), 200, "{hash}");
            }
            let response = assets::respond(&restored, &store, &format!("/{}", own.sha256)).await;
            assert_eq!(response.status(), 404);
            // The note still shows its picture.
            let note = crate::notes::notes(&restored)
                .await
                .unwrap()
                .into_iter()
                .find(|note| note.title == "From the PDF")
                .unwrap();
            let pictures = crate::notes::note(&restored, note.id)
                .await
                .unwrap()
                .pictures;
            assert_eq!(pictures.len(), 1);
            assert!(hashes.contains(&pictures[0].sha256));

            // Backed up again, it carries the same files and nothing else.
            let again = dir.join("again.zip");
            write_package(&restored, &store, &again, at(LATER), "0.8.0")
                .await
                .unwrap();
            assert_eq!(entry_names_of(&std::fs::read(&again).unwrap()), expected);

            restored.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_backup_whose_kept_files_dont_match_its_database_is_refused() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-kept-files-refused");
            let (source, hashes) =
                database_with_kept_files(&dir.join("source").join("synapse.sqlite")).await;
            let genuine_path = dir.join("genuine.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &genuine_path,
                at(NOW),
                "0.8.0",
            )
            .await
            .unwrap();
            source.close().await;
            let genuine = std::fs::read(&genuine_path).unwrap();
            let manifest = read_entry(&genuine, MANIFEST_ENTRY);
            let database = read_entry(&genuine, DATABASE_ENTRY);
            let names: Vec<String> = hashes
                .iter()
                .map(|hash| format!("{ASSETS_FOLDER}{hash}"))
                .collect();
            let files: Vec<Vec<u8>> = names
                .iter()
                .map(|name| read_entry(&genuine, name))
                .collect();

            // Rows whose type or size doesn't match their file: a PNG listed
            // as a JPEG, and a PNG listed one byte longer than it is.
            let made = dir.join("made");
            std::fs::create_dir_all(&made).unwrap();
            let picture = png(12);
            let hash = assets::sha256_hex(&picture);
            let wrong_type = database_listing(
                &made.join("type.sqlite"),
                &hash,
                "image/jpeg",
                picture.len(),
            )
            .await;
            let wrong_size = database_listing(
                &made.join("size.sqlite"),
                &hash,
                "image/png",
                picture.len() + 1,
            )
            .await;
            let one_file = format!("{ASSETS_FOLDER}{hash}");

            let mut altered = files[0].clone();
            let last = altered.len() - 1;
            altered[last] ^= 0x01;

            let cases: Vec<(&str, Vec<u8>)> = vec![
                (
                    "a kept file missing",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &database),
                        (&names[0], &files[0]),
                    ]),
                ),
                (
                    "a kept file whose bytes changed",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &database),
                        (&names[0], &altered),
                        (&names[1], &files[1]),
                    ]),
                ),
                (
                    "kept files swapped",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &database),
                        (&names[0], &files[1]),
                        (&names[1], &files[0]),
                    ]),
                ),
                (
                    "a kept file that isn't the type its row says",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &wrong_type),
                        (&one_file, &picture),
                    ]),
                ),
                (
                    "a kept file that isn't the size its row says",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &wrong_size),
                        (&one_file, &picture),
                    ]),
                ),
                (
                    "a kept file too many",
                    zip_of(&[
                        (MANIFEST_ENTRY, &manifest),
                        (DATABASE_ENTRY, &database),
                        (&names[0], &files[0]),
                        (&names[1], &files[1]),
                        (&format!("{ASSETS_FOLDER}{hash}"), &picture),
                    ]),
                ),
            ];

            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let before = dump(&target).await;
            let workspace = dir.join("target").join("import-workspace");
            for (name, bytes) in &cases {
                let archive = dir.join("candidate.zip");
                std::fs::write(&archive, bytes).unwrap();
                let result = stage_package(&archive, &workspace).await;
                assert_eq!(describe(&result), "damaged", "{name}: {result:?}");
                assert!(!workspace.exists(), "{name}: the workspace was left behind");
                assert_eq!(dump(&target).await, before, "{name}");
            }
            // The genuine package is accepted, so the cases above failed for
            // what was changed.
            assert_eq!(
                describe(&stage_package(&genuine_path, &workspace).await),
                "accepted"
            );
            discard_staged(&workspace);
            assert!(!workspace.exists());

            target.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_format_1_backup_from_v0_7_3_restores_with_no_kept_files() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-format-1");
            // What v0.7.3 wrote: schema 15, and a format 1 package of exactly
            // a manifest (with no folder of kept files) and the database.
            let source_path = dir.join("source").join("synapse.sqlite");
            std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
            let source =
                sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", source_path.display()))
                    .await
                    .unwrap();
            migrations_up_to(15).run(&source).await.unwrap();
            for sql in [
                "INSERT INTO seed_markers (name) VALUES ('sample_card')",
                "INSERT INTO decks (name, created_at) VALUES ('Biology', ?1)",
                "INSERT INTO flashcards (deck_id, front, back) VALUES (2, 'Cell', 'Unit of life')",
                "INSERT INTO notes (title, body, created_at, updated_at)
                 VALUES ('Lecture', 'Cells', ?1, ?1)",
            ] {
                sqlx::query(sql).bind(NOW).execute(&source).await.unwrap();
            }
            let made = dir.join("made.zip");
            write_package(&source, &store_of(&source).await, &made, at(NOW), "0.7.3")
                .await
                .unwrap();
            source.close().await;
            let made = std::fs::read(&made).unwrap();
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&read_entry(&made, MANIFEST_ENTRY)).unwrap();
            manifest["format_version"] = 1.into();
            manifest["contents"]
                .as_object_mut()
                .unwrap()
                .remove("assets");
            assert_eq!(
                manifest["contents"],
                serde_json::json!({ "database": "db/synapse.sqlite" })
            );
            let archive = dir.join("v0.7.3.zip");
            std::fs::write(
                &archive,
                zip_of(&[
                    (
                        MANIFEST_ENTRY,
                        &serde_json::to_vec_pretty(&manifest).unwrap(),
                    ),
                    (DATABASE_ENTRY, &read_entry(&made, DATABASE_ENTRY)),
                ]),
            )
            .unwrap();

            // Restored over a database that keeps a picture.
            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let store = store_beside(&target_path);
            let own = assets::add_picture(&target, &store, &png(13), at(NOW))
                .await
                .unwrap();
            let database = Database::default();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("target").join("import-workspace");
            let staged = stage_package(&archive, &workspace).await.unwrap();
            assert_eq!(
                (
                    staged.manifest.format_version,
                    staged.manifest.schema_version
                ),
                (1, 15)
            );
            assert!(staged.assets.is_empty());
            replace_database(
                &database,
                &target_path,
                &store,
                &staged.database,
                &workspace,
            )
            .await
            .unwrap();
            discard_staged(&workspace);

            // Everything it held is back, brought up to date with no kept
            // files and no sources; the store is untouched.
            let restored = installed(&database).await;
            assert_eq!(
                count(&restored, "SELECT COUNT(*) FROM _sqlx_migrations").await,
                migration_count()
            );
            for table in ["assets", "sources", "source_pages", "source_notes"] {
                let sql = format!("SELECT COUNT(*) FROM {table}");
                let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                    .fetch_one(&restored)
                    .await
                    .unwrap();
                assert_eq!(rows, 0, "{table}");
            }
            assert_eq!(
                crate::notes::notes(&restored).await.unwrap()[0].title,
                "Lecture"
            );
            assert_eq!(stored_names(&store), vec![own.sha256.clone()]);
            // It now keeps files like any database.
            let picture = assets::add_picture(&restored, &store, &png(14), at(LATER))
                .await
                .unwrap();
            assert!(store.verify(&picture.sha256, picture.size));

            restored.close().await;
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn a_restore_that_fails_takes_back_the_kept_files_it_added() {
        tauri::async_runtime::block_on(async {
            let dir = scratch("import-kept-files-rollback");
            let (source, hashes) =
                database_with_kept_files(&dir.join("source").join("synapse.sqlite")).await;
            let archive = dir.join("backup.zip");
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.8.0",
            )
            .await
            .unwrap();
            source.close().await;

            let target_path = dir.join("target").join("synapse.sqlite");
            let target = target_database(&target_path).await;
            let store = store_beside(&target_path);
            // The target already keeps one of the backup's files.
            let shared = assets::add_picture(&target, &store, &png(10), at(NOW))
                .await
                .unwrap();
            assert!(hashes.contains(&shared.sha256));
            let before = dump(&target).await;
            let database = Database::default();
            *database.slot().lock().await = Some(target);
            let workspace = dir.join("target").join("import-workspace");

            // Once with the restored database unopenable, once with one of
            // its files gone missing before the swap.
            for fault in ["unopenable", "missing file"] {
                let staged = stage_package(&archive, &workspace).await.unwrap();
                match fault {
                    "unopenable" => std::fs::write(&staged.database, vec![0x42u8; 8192]).unwrap(),
                    _ => {
                        let other = hashes.iter().find(|h| **h != shared.sha256).unwrap();
                        std::fs::remove_file(workspace.join(STAGED_ASSETS).join(other)).unwrap();
                    }
                }
                let result = replace_database(
                    &database,
                    &target_path,
                    &store,
                    &staged.database,
                    &workspace,
                )
                .await;
                assert!(
                    matches!(result, Err(RestoreError::NotRestored(_))),
                    "{fault}: {result:?}"
                );
                discard_staged(&workspace);

                // The previous database is back, and the store holds exactly
                // what it did: the shared file stays, the added one is gone.
                let pool = installed(&database).await;
                assert_eq!(dump(&pool).await, before, "{fault}");
                assert_eq!(stored_names(&store), vec![shared.sha256.clone()], "{fault}");
                assert!(store.verify(&shared.sha256, shared.size));
                assert!(!workspace.exists(), "{fault}");
            }

            installed(&database).await.close().await;
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

                let result = replace_database(
                    &database,
                    &target_path,
                    &store_beside(&target_path),
                    &staged,
                    &workspace,
                )
                .await;
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

            let result = replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged,
                &workspace,
            )
            .await;
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
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.1.0",
            )
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

            replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &staged.database,
                &workspace,
            )
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
            let result = replace_database(
                &database,
                &target_path,
                &store_beside(&target_path),
                &missing,
                &workspace,
            )
            .await;
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
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.1.0",
            )
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

                let result = replace_database(
                    &database,
                    &target_path,
                    &store_beside(&target_path),
                    &staged.database,
                    &workspace,
                )
                .await;
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
            write_package(
                &source,
                &store_of(&source).await,
                &archive,
                at(NOW),
                "0.1.0",
            )
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
