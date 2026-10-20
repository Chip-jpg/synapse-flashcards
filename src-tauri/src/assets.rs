//! The files Synapse keeps: imported documents (sources) and pictures.
//!
//! Each file lives in the `assets/` folder beside the database, named by the
//! SHA-256 of its bytes in lowercase hex, and is listed in the `assets` table
//! (migration 0016) with its media type and size. Storing by content means a
//! file is kept once however often it's used, and that a name always means
//! the same bytes: a file is written once and never changed, and its row is
//! never changed or removed.
//!
//! The rules every way in follows:
//! - **The type comes from the bytes.** A picture is a PNG, JPEG, GIF or WebP
//!   because its first bytes say so, never because of a file name or what a
//!   caller claims. SVG is not accepted (it can carry scripts).
//! - **Bounded.** A file is at most [`MAX_ASSET_BYTES`] (pictures
//!   [`MAX_PICTURE_BYTES`]), and the store as a whole at most
//!   [`MAX_STORE_BYTES`] in [`MAX_STORE_FILES`] files, so that everything
//!   kept always fits in one backup (see `export.rs`).
//! - **File first, then row.** The file is written under a temporary name,
//!   flushed, and renamed into place before its row is inserted, so a row
//!   never names a file that isn't there. A failure in between leaves a file
//!   no row names, which is harmless: nothing reads it, no backup carries it,
//!   and adding the same bytes again reuses it.
//! - **Shown only from the store.** The webview gets a kept picture or PDF
//!   through the `synapse:` scheme ([`respond`]), by its hash and nothing
//!   else: never by a path, and never from a URL found in a note's text.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};
use tauri::http::{header, Response, StatusCode};

use crate::db::{to_db_time, DbError};

/// The folder beside the database that holds the kept files.
pub const STORE_FOLDER: &str = "assets";

/// The largest file kept, in bytes: 64 MiB. Migration 0016 repeats it.
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

/// The largest picture kept: 20 MiB, far more than any photo or diagram needs.
pub const MAX_PICTURE_BYTES: u64 = 20 * 1024 * 1024;

/// The most the store holds altogether: 2 GiB in 60,000 files. With the
/// database's own limit (1 GiB) a backup then always stays under 4 GiB and
/// 65,535 entries, so it never needs zip64, which restoring refuses.
pub const MAX_STORE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_STORE_FILES: u64 = 60_000;

/// The URL scheme the webview loads kept files from. In the frontend,
/// `convertFileSrc(sha256, "synapse")` builds the address for this platform
/// (`synapse://localhost/<sha256>`, or `http://synapse.localhost/<sha256>`
/// on Windows).
pub const SCHEME: &str = "synapse";

/// What a kept file is. Stored as its MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/gif")]
    Gif,
    #[serde(rename = "image/webp")]
    Webp,
    #[serde(rename = "application/pdf")]
    Pdf,
    /// A .txt or .md file's bytes, as imported.
    #[serde(rename = "text/plain")]
    Text,
    #[serde(rename = "application/vnd.openxmlformats-officedocument.wordprocessingml.document")]
    Word,
    #[serde(rename = "application/vnd.openxmlformats-officedocument.presentationml.presentation")]
    PowerPoint,
}

impl MediaType {
    const ALL: [MediaType; 8] = [
        MediaType::Png,
        MediaType::Jpeg,
        MediaType::Gif,
        MediaType::Webp,
        MediaType::Pdf,
        MediaType::Text,
        MediaType::Word,
        MediaType::PowerPoint,
    ];

    /// The MIME type, as stored in `assets.media_type`.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Png => "image/png",
            MediaType::Jpeg => "image/jpeg",
            MediaType::Gif => "image/gif",
            MediaType::Webp => "image/webp",
            MediaType::Pdf => "application/pdf",
            MediaType::Text => "text/plain",
            MediaType::Word => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            MediaType::PowerPoint => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            }
        }
    }

    pub fn parse(text: &str) -> Option<MediaType> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    pub fn is_picture(self) -> bool {
        matches!(
            self,
            MediaType::Png | MediaType::Jpeg | MediaType::Gif | MediaType::Webp
        )
    }

    /// Whether the webview may load it through [`SCHEME`]: pictures and PDFs.
    /// Word, PowerPoint and text files are read by Synapse, never shown raw.
    pub fn is_viewable(self) -> bool {
        self.is_picture() || self == MediaType::Pdf
    }

    /// The largest file of this type that is kept.
    pub fn max_bytes(self) -> u64 {
        if self.is_picture() {
            MAX_PICTURE_BYTES
        } else {
            MAX_ASSET_BYTES
        }
    }
}

/// The picture or PDF `bytes` are, by their first bytes (their "magic
/// number"), or `None` for anything else.
pub fn sniff(bytes: &[u8]) -> Option<MediaType> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(MediaType::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(MediaType::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(MediaType::Gif)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(MediaType::Webp)
    } else if bytes.starts_with(b"%PDF-") {
        Some(MediaType::Pdf)
    } else {
        None
    }
}

/// Whether `bytes` can be kept as `kind`. Pictures and PDFs must sniff as
/// exactly that; Word and PowerPoint files must be zip packages (what's in
/// them is checked when their text is read, see `extract.rs`); text must not
/// look like any of those.
pub fn matches_type(bytes: &[u8], kind: MediaType) -> bool {
    const ZIP: &[u8] = b"PK\x03\x04";
    match kind {
        MediaType::Word | MediaType::PowerPoint => bytes.starts_with(ZIP),
        MediaType::Text => sniff(bytes).is_none() && !bytes.starts_with(ZIP),
        _ => sniff(bytes) == Some(kind),
    }
}

/// The lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Passes bytes on to `inner`, hashing them on the way.
pub(crate) struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        HashingWriter {
            inner,
            hasher: Sha256::new(),
        }
    }

    /// The lowercase hex SHA-256 of everything written.
    pub(crate) fn finish(self) -> String {
        self.finish_into().0
    }

    /// [`HashingWriter::finish`], also giving `inner` back.
    pub(crate) fn finish_into(self) -> (String, W) {
        (hex(&self.hasher.finalize()), self.inner)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Whether `name` is a kept file's name: 64 lowercase hex digits.
pub fn is_asset_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The marker in a file's temporary name while it is being written.
const PARTIAL: &str = ".partial-";

/// The `assets/` folder. Only names [`is_asset_name`] accepts are read, and
/// only those and this module's own temporary names are ever written.
#[derive(Debug, Clone)]
pub struct AssetStore {
    dir: PathBuf,
}

impl AssetStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        AssetStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the file named `sha256` is (whether or not it exists), or `None`
    /// for a name that isn't a kept file's.
    pub fn path_of(&self, sha256: &str) -> Option<PathBuf> {
        is_asset_name(sha256).then(|| self.dir.join(sha256))
    }

    /// Whether the file named `sha256` is there with `size` bytes. Its bytes
    /// aren't hashed; see [`AssetStore::verify`].
    pub fn has(&self, sha256: &str, size: u64) -> bool {
        self.path_of(sha256)
            .and_then(|path| std::fs::metadata(path).ok())
            .is_some_and(|meta| meta.is_file() && meta.len() == size)
    }

    /// Whether the file named `sha256` is there, `size` bytes long, and its
    /// bytes really hash to its name.
    #[cfg(test)]
    pub fn verify(&self, sha256: &str, size: u64) -> bool {
        let Some(path) = self.path_of(sha256) else {
            return false;
        };
        let mut hashing = HashingWriter::new(std::io::sink());
        File::open(path)
            .and_then(|mut file| std::io::copy(&mut file, &mut hashing))
            .is_ok_and(|len| len == size && hashing.finish() == sha256)
    }

    /// Writes `bytes` as the file named `sha256` (already worked out from
    /// them), unless it's already there with that size. The file appears
    /// whole or not at all: it's written under a temporary name, flushed,
    /// and renamed into place.
    pub fn write(&self, sha256: &str, bytes: &[u8]) -> std::io::Result<()> {
        let Some(path) = self.path_of(sha256) else {
            return Err(std::io::Error::other("not a kept file's name"));
        };
        if self.has(sha256, bytes.len() as u64) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let stamp = Utc::now().format("%Y%m%d%H%M%S%9f");
        let partial = self
            .dir
            .join(format!("{sha256}{PARTIAL}{}-{stamp}", std::process::id()));
        let written = (|| {
            let mut file = File::create_new(&partial)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&partial, &path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        written
    }

    /// Removes temporary files a write that never finished left behind (the
    /// app closed mid-write). Run at launch, before anything is written.
    pub fn remove_partials(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some((hash, _)) = name.split_once(PARTIAL) {
                if is_asset_name(hash) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// One kept file, as the frontend receives it:
/// `{ id, sha256, mediaType, size, createdAt }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: i64,
    pub sha256: String,
    pub media_type: MediaType,
    pub size: u64,
    pub created_at: String,
}

type AssetRow = (i64, String, String, i64, String);

fn asset_from_row((id, sha256, media_type, size, created_at): AssetRow) -> Result<Asset, DbError> {
    let media_type = MediaType::parse(&media_type)
        .ok_or_else(|| format!("unknown media type {media_type:?}"))?;
    Ok(Asset {
        id,
        sha256,
        media_type,
        size: u64::try_from(size)?,
        created_at,
    })
}

/// Why a file wasn't kept. Nothing was stored.
#[derive(Debug)]
pub enum AssetError {
    /// Larger than its type allows ([`MediaType::max_bytes`]).
    TooLarge,
    /// Empty, or its bytes aren't the type it was offered as (a picture that
    /// isn't PNG, JPEG, GIF or WebP, for one).
    WrongType,
    /// Keeping it would take the store past [`MAX_STORE_BYTES`] or
    /// [`MAX_STORE_FILES`].
    StoreFull,
    /// Writing the file or its row failed. Detail is for logs only.
    Internal(DbError),
}

impl From<sqlx::Error> for AssetError {
    fn from(err: sqlx::Error) -> Self {
        AssetError::Internal(err.into())
    }
}

impl From<std::io::Error> for AssetError {
    fn from(err: std::io::Error) -> Self {
        AssetError::Internal(err.into())
    }
}

/// Keeps `bytes` as a file of type `kind`, on the connection or transaction
/// `conn`, and returns its row. Bytes already kept return the row they
/// already have (writing the file again if it had gone missing).
///
/// Run inside a `BEGIN IMMEDIATE` transaction, so the store's total is
/// checked and the row written without another write in between; if that
/// transaction rolls back, the file stays without a row, which is harmless
/// (see the module comment).
pub async fn put(
    conn: &mut SqliteConnection,
    store: &AssetStore,
    bytes: &[u8],
    kind: MediaType,
    now: DateTime<Utc>,
) -> Result<Asset, AssetError> {
    let size = bytes.len() as u64;
    if size > kind.max_bytes() {
        return Err(AssetError::TooLarge);
    }
    if size == 0 || !matches_type(bytes, kind) {
        return Err(AssetError::WrongType);
    }
    let sha256 = sha256_hex(bytes);

    let existing: Option<AssetRow> = sqlx::query_as(
        "SELECT id, sha256, media_type, size, created_at FROM assets WHERE sha256 = ?1",
    )
    .bind(&sha256)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(row) = existing {
        let asset = asset_from_row(row).map_err(AssetError::Internal)?;
        // The same bytes can't be two kinds of file; text is only ever text.
        if asset.media_type != kind {
            return Err(AssetError::WrongType);
        }
        store.write(&sha256, bytes)?;
        return Ok(asset);
    }

    let (files, total): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM assets")
            .fetch_one(&mut *conn)
            .await?;
    if files as u64 + 1 > MAX_STORE_FILES || total as u64 + size > MAX_STORE_BYTES {
        return Err(AssetError::StoreFull);
    }

    store.write(&sha256, bytes)?;
    let row: AssetRow = sqlx::query_as(
        "INSERT INTO assets (sha256, media_type, size, created_at) VALUES (?1, ?2, ?3, ?4)
         RETURNING id, sha256, media_type, size, created_at",
    )
    .bind(&sha256)
    .bind(kind.as_str())
    .bind(size as i64)
    .bind(to_db_time(now))
    .fetch_one(&mut *conn)
    .await?;
    asset_from_row(row).map_err(AssetError::Internal)
}

/// Keeps a picture: PNG, JPEG, GIF or WebP, by its bytes, up to
/// [`MAX_PICTURE_BYTES`]. Used for pictures chosen from a file and pasted.
pub async fn add_picture(
    pool: &SqlitePool,
    store: &AssetStore,
    bytes: &[u8],
    now: DateTime<Utc>,
) -> Result<Asset, AssetError> {
    if bytes.len() as u64 > MAX_PICTURE_BYTES {
        return Err(AssetError::TooLarge);
    }
    let kind = sniff(bytes)
        .filter(|kind| kind.is_picture())
        .ok_or(AssetError::WrongType)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let asset = put(&mut tx, store, bytes, kind, now).await?;
    tx.commit().await?;
    Ok(asset)
}

/// The kept file with this hash, if there is one.
pub async fn asset_by_hash(pool: &SqlitePool, sha256: &str) -> Result<Option<Asset>, DbError> {
    let row: Option<AssetRow> = sqlx::query_as(
        "SELECT id, sha256, media_type, size, created_at FROM assets WHERE sha256 = ?1",
    )
    .bind(sha256)
    .fetch_optional(pool)
    .await?;
    row.map(asset_from_row).transpose()
}

/// The kept file with this id, if there is one, on a connection or inside a
/// transaction.
pub(crate) async fn asset_by_id(
    conn: &mut SqliteConnection,
    id: i64,
) -> Result<Option<Asset>, DbError> {
    let row: Option<AssetRow> =
        sqlx::query_as("SELECT id, sha256, media_type, size, created_at FROM assets WHERE id = ?1")
            .bind(id)
            .fetch_optional(conn)
            .await?;
    row.map(asset_from_row).transpose()
}

/// The answer to a `synapse:` request for `path`, which must be `/<sha256>`
/// of a kept picture or PDF. Anything else, or a file that is missing or the
/// wrong size, is a plain 404 with no detail.
///
/// The answer can't be reinterpreted: its type is the stored one, with
/// `nosniff`. It never changes (the name is its hash), so it may be cached.
pub async fn respond(pool: &SqlitePool, store: &AssetStore, path: &str) -> Response<Vec<u8>> {
    match lookup(pool, store, path).await {
        Ok(Some((kind, bytes))) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, kind.as_str())
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            )
            // The app's own pages load these from another origin (a custom
            // scheme), e.g. with `fetch` for a PDF viewer.
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(bytes),
        Ok(None) => return not_found(),
        Err(err) => {
            eprintln!("[synapse] showing a kept file failed: {err}");
            return not_found();
        }
    }
    .unwrap_or_else(|_| not_found())
}

/// A plain 404, with nothing about why.
pub fn not_found() -> Response<Vec<u8>> {
    // The page is another origin, so without this a fetch would see a
    // network error rather than a 404.
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Vec::new())
        .expect("a fixed response is valid")
}

async fn lookup(
    pool: &SqlitePool,
    store: &AssetStore,
    path: &str,
) -> Result<Option<(MediaType, Vec<u8>)>, DbError> {
    let Some(sha256) = path.strip_prefix('/').filter(|name| is_asset_name(name)) else {
        return Ok(None);
    };
    let Some(asset) = asset_by_hash(pool, sha256).await? else {
        return Ok(None);
    };
    if !asset.media_type.is_viewable() {
        return Ok(None);
    }
    let Some(file) = store.path_of(sha256) else {
        return Ok(None);
    };
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("[synapse] a kept file is missing from the store");
            return Ok(None);
        }
        Err(err) => return Err(err.into()),
    };
    if bytes.len() as u64 != asset.size {
        eprintln!("[synapse] a kept file isn't the size its row says");
        return Ok(None);
    }
    Ok(Some((asset.media_type, bytes)))
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// A fresh store folder under the build's `target/`. Removed by the caller.
    pub fn scratch_store(name: &str) -> AssetStore {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-dbs")
            .join(name)
            .join(STORE_FOLDER);
        let _ = std::fs::remove_dir_all(&dir);
        AssetStore::new(dir)
    }

    /// The smallest valid PNG: one transparent pixel. `variant` changes one
    /// harmless byte, for a picture with other bytes.
    pub fn png(variant: u8) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[
            0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15,
            0xc4, 0x89, 0, 0, 0, 13, b'I', b'D', b'A', b'T', 0x78, 0x9c, 0x63, 0, 1, 0, 0, 5, 0, 1,
            0x0d, 0x0a, 0x2d, 0xb4, 0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82,
        ]);
        bytes.push(variant);
        bytes
    }

    /// The store beside a file-backed pool's database, where the app keeps it.
    pub async fn store_of(pool: &SqlitePool) -> AssetStore {
        let file: String =
            sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
                .fetch_one(pool)
                .await
                .unwrap();
        AssetStore::new(Path::new(&file).parent().unwrap().join(STORE_FOLDER))
    }

    /// The names in the store folder, sorted.
    pub fn stored_names(store: &AssetStore) -> Vec<String> {
        let mut names: Vec<String> = match std::fs::read_dir(store.dir()) {
            Ok(entries) => entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;

    const NOW: &str = "2026-10-20T12:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    #[test]
    fn types_come_from_the_bytes_not_from_a_name() {
        assert_eq!(sniff(&png(0)), Some(MediaType::Png));
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0rest"), Some(MediaType::Jpeg));
        assert_eq!(sniff(b"GIF89a..."), Some(MediaType::Gif));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some(MediaType::Webp));
        assert_eq!(sniff(b"%PDF-1.7\n"), Some(MediaType::Pdf));
        // An SVG is text that can carry scripts: never a picture here.
        assert_eq!(sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), None);
        assert_eq!(sniff(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b""), None);

        assert!(matches_type(b"PK\x03\x04...", MediaType::Word));
        assert!(!matches_type(b"%PDF-1.7", MediaType::Word));
        assert!(matches_type(b"just words", MediaType::Text));
        assert!(!matches_type(&png(0), MediaType::Text));
        assert!(!matches_type(&png(0), MediaType::Jpeg));

        for kind in MediaType::ALL {
            assert_eq!(MediaType::parse(kind.as_str()), Some(kind));
            // The serde name is the stored name.
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str())
            );
        }
    }

    #[test]
    fn only_lowercase_sha256_hex_is_a_kept_files_name() {
        let hash = sha256_hex(b"abc");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(is_asset_name(&hash));
        assert!(!is_asset_name(&hash.to_uppercase()));
        assert!(!is_asset_name(&hash[1..]));
        assert!(!is_asset_name(&format!("../{}", &hash[3..])));
        let store = AssetStore::new("store");
        assert_eq!(store.path_of("../synapse.sqlite"), None);
        assert_eq!(store.path_of(&hash), Some(Path::new("store").join(&hash)));
    }

    #[test]
    fn a_picture_is_kept_once_by_its_hash_and_its_row_never_changes() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("assets-put");

            let picture = png(1);
            let first = add_picture(&pool, &store, &picture, at(NOW)).await.unwrap();
            assert_eq!(first.sha256, sha256_hex(&picture));
            assert_eq!(first.media_type, MediaType::Png);
            assert_eq!(first.size, picture.len() as u64);
            assert_eq!(first.created_at, "2026-10-20T12:00:00.000Z");
            assert!(store.verify(&first.sha256, first.size));

            // The same bytes again: the same row, no second file.
            let again = add_picture(&pool, &store, &picture, at("2026-10-21T12:00:00Z"))
                .await
                .unwrap();
            assert_eq!(again, first);
            assert_eq!(stored_names(&store), vec![first.sha256.clone()]);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 1);

            // A file that went missing comes back when its bytes are added again.
            std::fs::remove_file(store.path_of(&first.sha256).unwrap()).unwrap();
            add_picture(&pool, &store, &picture, at(NOW)).await.unwrap();
            assert!(store.verify(&first.sha256, first.size));

            // SQLite refuses changing or removing the row.
            for sql in [
                "UPDATE assets SET size = 5",
                "UPDATE assets SET media_type = 'image/gif'",
                "DELETE FROM assets",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            assert_eq!(
                asset_by_hash(&pool, &first.sha256).await.unwrap(),
                Some(first)
            );

            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn what_isnt_a_picture_or_is_too_large_is_refused_and_nothing_is_kept() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("assets-refused");

            for bytes in [
                b"<svg onload=\"alert(1)\"/>".to_vec(),
                b"%PDF-1.7 not a picture".to_vec(),
                Vec::new(),
            ] {
                assert!(matches!(
                    add_picture(&pool, &store, &bytes, at(NOW)).await,
                    Err(AssetError::WrongType)
                ));
            }
            let mut huge = png(0);
            huge.resize(MAX_PICTURE_BYTES as usize + 1, 0);
            assert!(matches!(
                add_picture(&pool, &store, &huge, at(NOW)).await,
                Err(AssetError::TooLarge)
            ));

            // Offered as the wrong type, through the general way in.
            let mut conn = pool.acquire().await.unwrap();
            assert!(matches!(
                put(&mut conn, &store, &png(0), MediaType::Jpeg, at(NOW)).await,
                Err(AssetError::WrongType)
            ));
            drop(conn);

            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 0);
            assert!(stored_names(&store).is_empty());

            // SQLite repeats the type and size rules.
            for sql in [
                "INSERT INTO assets (sha256, media_type, size, created_at)
                 VALUES ('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
                         'image/svg+xml', 10, 'x')",
                "INSERT INTO assets (sha256, media_type, size, created_at)
                 VALUES ('BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD',
                         'image/png', 10, 'x')",
                "INSERT INTO assets (sha256, media_type, size, created_at)
                 VALUES ('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
                         'image/png', 67108865, 'x')",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
        });
    }

    #[test]
    fn the_store_refuses_more_than_one_backup_can_hold() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("assets-full");
            // Rows standing for files already kept: 32 of 64 MiB, the
            // store's 2 GiB exactly.
            for i in 0..32 {
                sqlx::query(
                    "INSERT INTO assets (sha256, media_type, size, created_at)
                     VALUES (?1, 'application/pdf', ?2, 'x')",
                )
                .bind(format!("{i:064x}"))
                .bind(MAX_ASSET_BYTES as i64)
                .execute(&pool)
                .await
                .unwrap();
            }
            assert!(matches!(
                add_picture(&pool, &store, &png(2), at(NOW)).await,
                Err(AssetError::StoreFull)
            ));
            assert!(stored_names(&store).is_empty());
        });
    }

    #[test]
    fn unfinished_writes_are_cleared_and_kept_files_are_not() {
        let store = scratch_store("assets-partials");
        let picture = png(3);
        let hash = sha256_hex(&picture);
        store.write(&hash, &picture).unwrap();
        let partial = store.dir().join(format!("{hash}{PARTIAL}1-2"));
        std::fs::write(&partial, b"half").unwrap();
        let unrelated = store.dir().join("notes.txt");
        std::fs::write(&unrelated, b"not ours").unwrap();

        store.remove_partials();
        assert_eq!(stored_names(&store), vec![hash.clone(), "notes.txt".into()]);
        assert!(store.verify(&hash, picture.len() as u64));
        // A file whose bytes don't match its name fails verification.
        std::fs::write(store.path_of(&hash).unwrap(), png(4)).unwrap();
        assert!(!store.verify(&hash, picture.len() as u64));
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn the_scheme_serves_kept_pictures_by_hash_and_nothing_else() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("assets-respond");
            let picture = png(5);
            let kept = add_picture(&pool, &store, &picture, at(NOW)).await.unwrap();

            let response = respond(&pool, &store, &format!("/{}", kept.sha256)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert_eq!(response.body(), &picture);

            // A kept text file isn't shown raw.
            let mut tx = pool.begin().await.unwrap();
            let text = put(&mut tx, &store, b"plain words", MediaType::Text, at(NOW))
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let missing = sha256_hex(b"never kept");
            for path in [
                format!("/{}", text.sha256),
                format!("/{missing}"),
                "/../synapse.sqlite".to_string(),
                format!("/{}", kept.sha256.to_uppercase()),
                format!("/assets/{}", kept.sha256),
                "/".to_string(),
            ] {
                let response = respond(&pool, &store, &path).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
                assert!(response.body().is_empty());
                // So the page's fetch sees the 404, not a network error.
                assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
            }

            // A file that changed size on disk isn't served.
            std::fs::write(store.path_of(&kept.sha256).unwrap(), b"short").unwrap();
            let response = respond(&pool, &store, &format!("/{}", kept.sha256)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);

            let _ = std::fs::remove_dir_all(store.dir());
        });
    }
}
