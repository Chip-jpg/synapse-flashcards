//! Sources: imported documents Synapse keeps (migration 0016). A source is
//! the original file, held in the asset store (`assets.rs`), plus the text of
//! each of its pages as extraction read it (`source_pages`). The text is never
//! changed afterwards: it is what the document said. A note saved from a
//! source is the user's own copy, to edit freely, and `source_notes` records
//! which source it came from, so a quote in it can be found on its page.
//!
//! Sources have the lifecycle notes have (`notes.rs`): a rename changes the
//! title and `updated_at`; deleting is a soft delete that freezes the row;
//! restoring clears `deleted_at` and writes nothing else. The kept file is
//! never removed, so a restored source has its original back.
//!
//! This module is the data layer. Choosing a file, the Sources screen, and
//! showing pages belong to the frontend and its commands.

use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};

use crate::assets::{self, Asset, AssetError, AssetStore, MediaType};
use crate::authoring::required_text;
use crate::db::{to_db_time, DbError};
use crate::extract::{Document, FileKind};
use crate::notes::NOTE_TITLE_MAX_CHARS;

/// The longest file name kept with a source, in characters.
pub const FILE_NAME_MAX_CHARS: usize = 255;

/// One source: `{ id, title, kind, fileName, pageCount, asset, createdAt,
/// updatedAt, deletedAt }`. `deletedAt` is null while it is active.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: i64,
    pub title: String,
    pub kind: FileKind,
    pub file_name: String,
    pub page_count: u32,
    /// The original file. Its `sha256` is its address in the `synapse:`
    /// scheme (for a PDF).
    pub asset: Asset,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// The text of one page (or slide) of a source: `{ number, text }`, numbered
/// from 1. A page without text has empty text.
#[derive(Debug, Clone, PartialEq, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SourcePage {
    pub number: u32,
    pub text: String,
}

/// What was wrong with a title the user typed. Nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidTitle {
    Blank,
    TooLong,
}

/// Why a source operation didn't happen. Nothing was changed.
#[derive(Debug)]
pub enum SourceError {
    Invalid(InvalidTitle),
    /// No source (or, for a link, no note) has this id.
    NotFound,
    /// The source (or note) is deleted, so it can't be read or changed.
    Deleted,
    /// The source isn't deleted, so there's nothing to restore.
    NotDeleted,
    /// The note already came from a source.
    AlreadyLinked,
    /// Its file couldn't be kept (too large, not that kind of file, the
    /// store is full).
    Asset(AssetError),
    /// Anything else (a database failure). Detail is for logs only.
    Internal(DbError),
}

impl From<sqlx::Error> for SourceError {
    fn from(err: sqlx::Error) -> Self {
        SourceError::Internal(err.into())
    }
}

impl From<AssetError> for SourceError {
    fn from(err: AssetError) -> Self {
        SourceError::Asset(err)
    }
}

/// How a kind of document is kept.
pub fn media_type(kind: FileKind) -> MediaType {
    match kind {
        FileKind::Pdf => MediaType::Pdf,
        FileKind::Text | FileKind::Markdown => MediaType::Text,
        FileKind::Word => MediaType::Word,
        FileKind::PowerPoint => MediaType::PowerPoint,
    }
}

fn kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Pdf => "pdf",
        FileKind::Text => "text",
        FileKind::Markdown => "markdown",
        FileKind::Word => "word",
        FileKind::PowerPoint => "powerPoint",
    }
}

fn parse_kind(name: &str) -> Option<FileKind> {
    [
        FileKind::Pdf,
        FileKind::Text,
        FileKind::Markdown,
        FileKind::Word,
        FileKind::PowerPoint,
    ]
    .into_iter()
    .find(|kind| kind_name(*kind) == name)
}

fn validate_title(title: &str) -> Result<&str, InvalidTitle> {
    required_text(
        title,
        NOTE_TITLE_MAX_CHARS,
        InvalidTitle::Blank,
        InvalidTitle::TooLong,
    )
}

/// A file name as kept: on one line, at most [`FILE_NAME_MAX_CHARS`].
fn kept_file_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(FILE_NAME_MAX_CHARS)
        .collect();
    if name.is_empty() {
        "document".to_string()
    } else {
        name
    }
}

const SOURCE_COLUMNS: &str = "SELECT s.id, s.title, s.kind, s.file_name, s.page_count,
                                     s.created_at, s.updated_at, s.deleted_at,
                                     a.id, a.sha256, a.media_type, a.size, a.created_at
                              FROM sources s JOIN assets a ON a.id = s.asset_id";

type SourceRow = (
    i64,
    String,
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
    i64,
    String,
    String,
    i64,
    String,
);

fn source_from_row(row: SourceRow) -> Result<Source, SourceError> {
    let (id, title, kind, file_name, page_count, created_at, updated_at, deleted_at) =
        (row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7);
    let bad = |what: &str| SourceError::Internal(format!("source {id}: {what}").into());
    let media_type = MediaType::parse(&row.10).ok_or_else(|| bad("unknown media type"))?;
    Ok(Source {
        id,
        title,
        kind: parse_kind(&kind).ok_or_else(|| bad("unknown kind"))?,
        file_name,
        page_count: u32::try_from(page_count).map_err(|_| bad("page count"))?,
        asset: Asset {
            id: row.8,
            sha256: row.9,
            media_type,
            size: u64::try_from(row.11).map_err(|_| bad("size"))?,
            created_at: row.12,
        },
        created_at,
        updated_at,
        deleted_at,
    })
}

async fn load(conn: &mut SqliteConnection, id: i64) -> Result<Source, SourceError> {
    // Built from constant pieces only.
    let row: Option<SourceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SOURCE_COLUMNS} WHERE s.id = ?1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?;
    source_from_row(row.ok_or(SourceError::NotFound)?)
}

/// Keeps `document` as a new source: its file in `store`, a `sources` row,
/// and every page's text, all in one transaction. Its title is the one
/// extraction found (the document's own, or its file name).
pub async fn create_source(
    pool: &SqlitePool,
    store: &AssetStore,
    document: &Document,
    now: DateTime<Utc>,
) -> Result<Source, SourceError> {
    let title = validate_title(&document.text.title).map_err(SourceError::Invalid)?;
    let kind = document.text.kind;
    let pages = if document.pages.is_empty() {
        vec![document.text.body.clone()]
    } else {
        document.pages.clone()
    };
    let time = to_db_time(now);

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let asset = assets::put(&mut tx, store, &document.bytes, media_type(kind), now).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO sources (asset_id, kind, title, file_name, page_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) RETURNING id",
    )
    .bind(asset.id)
    .bind(kind_name(kind))
    .bind(title)
    .bind(kept_file_name(&document.file_name))
    .bind(pages.len() as i64)
    .bind(&time)
    .fetch_one(&mut *tx)
    .await?;
    for (index, text) in pages.iter().enumerate() {
        sqlx::query("INSERT INTO source_pages (source_id, number, text) VALUES (?1, ?2, ?3)")
            .bind(id)
            .bind(index as i64 + 1)
            .bind(text)
            .execute(&mut *tx)
            .await?;
    }
    let source = load(&mut tx, id).await?;
    tx.commit().await?;
    Ok(source)
}

/// Every active source, most recently changed first (newest id on a tie).
pub async fn sources(pool: &SqlitePool) -> Result<Vec<Source>, SourceError> {
    let rows: Vec<SourceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SOURCE_COLUMNS} WHERE s.deleted_at IS NULL ORDER BY s.updated_at DESC, s.id DESC"
    )))
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(source_from_row).collect()
}

/// Every deleted source, most recently deleted first.
pub async fn deleted_sources(pool: &SqlitePool) -> Result<Vec<Source>, SourceError> {
    let rows: Vec<SourceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SOURCE_COLUMNS} WHERE s.deleted_at IS NOT NULL ORDER BY s.deleted_at DESC, s.id DESC"
    )))
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(source_from_row).collect()
}

/// One active source. A deleted one is refused, as a deleted note is.
pub async fn source(pool: &SqlitePool, id: i64) -> Result<Source, SourceError> {
    let mut conn = pool.acquire().await?;
    let source = load(&mut conn, id).await?;
    if source.deleted_at.is_some() {
        return Err(SourceError::Deleted);
    }
    Ok(source)
}

/// Every page of an active source, in order.
pub async fn source_pages(pool: &SqlitePool, id: i64) -> Result<Vec<SourcePage>, SourceError> {
    source(pool, id).await?;
    Ok(
        sqlx::query_as(
            "SELECT number, text FROM source_pages WHERE source_id = ?1 ORDER BY number",
        )
        .bind(id)
        .fetch_all(pool)
        .await?,
    )
}

/// Renames an active source. The kept file and its pages don't change.
pub async fn rename_source(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    now: DateTime<Utc>,
) -> Result<Source, SourceError> {
    let title = validate_title(title).map_err(SourceError::Invalid)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if load(&mut tx, id).await?.deleted_at.is_some() {
        return Err(SourceError::Deleted);
    }
    sqlx::query(
        "UPDATE sources SET title = ?1, updated_at = ?2 WHERE id = ?3 AND deleted_at IS NULL",
    )
    .bind(title)
    .bind(to_db_time(now))
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let source = load(&mut tx, id).await?;
    tx.commit().await?;
    Ok(source)
}

/// Deletes a source softly: only `deleted_at` is written. Its file, pages,
/// and the notes written from it all stay.
pub async fn delete_source(
    pool: &SqlitePool,
    id: i64,
    now: DateTime<Utc>,
) -> Result<(), SourceError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if load(&mut tx, id).await?.deleted_at.is_some() {
        return Err(SourceError::Deleted);
    }
    sqlx::query("UPDATE sources SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL")
        .bind(to_db_time(now))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Restores a deleted source exactly as it was: only `deleted_at` is cleared.
pub async fn restore_source(pool: &SqlitePool, id: i64) -> Result<(), SourceError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if load(&mut tx, id).await?.deleted_at.is_none() {
        return Err(SourceError::NotDeleted);
    }
    sqlx::query("UPDATE sources SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Records that the note `note_id` was written from the source `source_id`.
/// Both must be active, and a note comes from one source at most, for good.
pub async fn link_note(
    pool: &SqlitePool,
    note_id: i64,
    source_id: i64,
    now: DateTime<Utc>,
) -> Result<(), SourceError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let note: Option<Option<String>> =
        sqlx::query_scalar("SELECT deleted_at FROM notes WHERE id = ?1")
            .bind(note_id)
            .fetch_optional(&mut *tx)
            .await?;
    match note {
        None => return Err(SourceError::NotFound),
        Some(Some(_)) => return Err(SourceError::Deleted),
        Some(None) => {}
    }
    if load(&mut tx, source_id).await?.deleted_at.is_some() {
        return Err(SourceError::Deleted);
    }
    let inserted = sqlx::query(
        "INSERT INTO source_notes (note_id, source_id, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (note_id) DO NOTHING",
    )
    .bind(note_id)
    .bind(source_id)
    .bind(to_db_time(now))
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(SourceError::AlreadyLinked);
    }
    tx.commit().await?;
    Ok(())
}

/// The source a note was written from, if any (whether or not either is
/// deleted now).
pub async fn note_source(pool: &SqlitePool, note_id: i64) -> Result<Option<i64>, SourceError> {
    Ok(
        sqlx::query_scalar("SELECT source_id FROM source_notes WHERE note_id = ?1")
            .bind(note_id)
            .fetch_optional(pool)
            .await?,
    )
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use crate::extract::ExtractedText;

    /// A document as `extract::read_document` returns it, without reading a
    /// file: a three-page "PDF" (only its first bytes are a PDF's), the
    /// second page without text.
    pub fn pdf_document(variant: &str) -> Document {
        let pages = vec![
            format!("Cells are the unit of life. {variant}"),
            String::new(),
            "Mitochondria make ATP.".to_string(),
        ];
        Document {
            file_name: format!("Biology {variant}.pdf"),
            bytes: format!("%PDF-1.7\n{variant}\n%%EOF\n").into_bytes(),
            text: ExtractedText {
                kind: FileKind::Pdf,
                title: format!("Biology {variant}"),
                body: format!("{}\n\n{}", pages[0], pages[2]),
                pages: Some(3),
                pages_without_text: vec![2],
            },
            pages,
        }
    }

    pub fn text_document(text: &str) -> Document {
        Document {
            file_name: "notes.md".to_string(),
            bytes: text.as_bytes().to_vec(),
            text: ExtractedText {
                kind: FileKind::Markdown,
                title: "Notes".to_string(),
                body: text.to_string(),
                pages: None,
                pages_without_text: Vec::new(),
            },
            pages: vec![text.to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::assets::test_support::*;
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;

    const NOW: &str = "2026-10-20T12:00:00Z";
    const LATER: &str = "2026-10-20T13:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    #[test]
    fn a_source_keeps_its_file_and_every_page() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-create");
            let document = pdf_document("one");

            let source = create_source(&pool, &store, &document, at(NOW))
                .await
                .unwrap();
            assert_eq!(source.title, "Biology one");
            assert_eq!(source.kind, FileKind::Pdf);
            assert_eq!(source.file_name, "Biology one.pdf");
            assert_eq!(source.page_count, 3);
            assert_eq!(source.asset.media_type, MediaType::Pdf);
            assert_eq!(source.asset.sha256, assets::sha256_hex(&document.bytes));
            assert_eq!(source.created_at, "2026-10-20T12:00:00.000Z");
            assert_eq!(source.deleted_at, None);
            assert!(store.verify(&source.asset.sha256, source.asset.size));

            let pages = source_pages(&pool, source.id).await.unwrap();
            assert_eq!(
                pages,
                vec![
                    SourcePage {
                        number: 1,
                        text: "Cells are the unit of life. one".into()
                    },
                    SourcePage {
                        number: 2,
                        text: String::new()
                    },
                    SourcePage {
                        number: 3,
                        text: "Mitochondria make ATP.".into()
                    },
                ]
            );

            // The same file imported again is a second source sharing the file.
            let again = create_source(&pool, &store, &document, at(LATER))
                .await
                .unwrap();
            assert_eq!(again.asset, source.asset);
            assert_eq!(stored_names(&store).len(), 1);
            assert_eq!(
                sources(&pool)
                    .await
                    .unwrap()
                    .iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>(),
                vec![again.id, source.id]
            );

            // A text file is one page.
            let text = create_source(&pool, &store, &text_document("# Notes\nhi"), at(NOW))
                .await
                .unwrap();
            assert_eq!((text.kind, text.page_count), (FileKind::Markdown, 1));
            assert_eq!(text.asset.media_type, MediaType::Text);

            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_file_that_isnt_its_kind_is_refused_and_nothing_is_written() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-wrong-kind");
            let mut document = pdf_document("fake");
            document.bytes = png(0);

            assert!(matches!(
                create_source(&pool, &store, &document, at(NOW)).await,
                Err(SourceError::Asset(AssetError::WrongType))
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM sources").await, 0);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 0);

            // SQLite refuses a source whose file is another kind of document.
            let picture = assets::add_picture(&pool, &store, &png(0), at(NOW))
                .await
                .unwrap();
            let err = sqlx::query(
                "INSERT INTO sources (asset_id, kind, title, file_name, page_count,
                                      created_at, updated_at)
                 VALUES (?1, 'pdf', 'T', 'f.pdf', 1, 'x', 'x')",
            )
            .bind(picture.id)
            .execute(&pool)
            .await
            .unwrap_err();
            assert!(err.to_string().contains("kind of document"), "{err}");
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_deleted_source_is_frozen_and_comes_back_exactly() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-lifecycle");
            let source = create_source(&pool, &store, &pdf_document("x"), at(NOW))
                .await
                .unwrap();

            let renamed = rename_source(&pool, source.id, "  Lecture 3  ", at(LATER))
                .await
                .unwrap();
            assert_eq!(renamed.title, "Lecture 3");
            assert_eq!(renamed.updated_at, "2026-10-20T13:00:00.000Z");
            assert!(matches!(
                rename_source(&pool, source.id, "   ", at(LATER)).await,
                Err(SourceError::Invalid(InvalidTitle::Blank))
            ));

            delete_source(&pool, source.id, at(LATER)).await.unwrap();
            assert!(sources(&pool).await.unwrap().is_empty());
            assert!(matches!(
                source_pages(&pool, source.id).await,
                Err(SourceError::Deleted)
            ));
            assert!(matches!(
                rename_source(&pool, source.id, "New", at(LATER)).await,
                Err(SourceError::Deleted)
            ));
            assert!(matches!(
                delete_source(&pool, source.id, at(LATER)).await,
                Err(SourceError::Deleted)
            ));
            let deleted = deleted_sources(&pool).await.unwrap();
            assert_eq!(
                deleted[0].deleted_at.as_deref(),
                Some("2026-10-20T13:00:00.000Z")
            );
            // SQLite holds a deleted source still, whatever the caller.
            for sql in [
                "UPDATE sources SET title = 'Changed'",
                "UPDATE sources SET deleted_at = '2026-10-21T00:00:00.000Z'",
                "DELETE FROM sources",
                "DELETE FROM source_pages",
                "UPDATE source_pages SET text = 'rewritten'",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }

            restore_source(&pool, source.id).await.unwrap();
            assert_eq!(sources(&pool).await.unwrap(), vec![renamed]);
            assert_eq!(source_pages(&pool, source.id).await.unwrap().len(), 3);
            assert!(matches!(
                restore_source(&pool, source.id).await,
                Err(SourceError::NotDeleted)
            ));
            assert!(matches!(
                super::source(&pool, 999).await,
                Err(SourceError::NotFound)
            ));
            // What was imported never changes, even while active.
            for sql in [
                "UPDATE sources SET page_count = 9",
                "UPDATE sources SET file_name = 'other.pdf'",
                "UPDATE sources SET asset_id = 99",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_note_records_the_source_it_came_from_once() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-link");
            let source = create_source(&pool, &store, &pdf_document("x"), at(NOW))
                .await
                .unwrap();
            let note = crate::notes::create_note(&pool, "From the PDF", "Cells", at(NOW))
                .await
                .unwrap();

            assert_eq!(note_source(&pool, note.id).await.unwrap(), None);
            link_note(&pool, note.id, source.id, at(NOW)).await.unwrap();
            assert_eq!(note_source(&pool, note.id).await.unwrap(), Some(source.id));
            assert!(matches!(
                link_note(&pool, note.id, source.id, at(NOW)).await,
                Err(SourceError::AlreadyLinked)
            ));
            assert!(matches!(
                link_note(&pool, 999, source.id, at(NOW)).await,
                Err(SourceError::NotFound)
            ));

            // A deleted source can't be linked; the existing link stays.
            let other = crate::notes::create_note(&pool, "Other", "text", at(NOW))
                .await
                .unwrap();
            delete_source(&pool, source.id, at(LATER)).await.unwrap();
            assert!(matches!(
                link_note(&pool, other.id, source.id, at(NOW)).await,
                Err(SourceError::Deleted)
            ));
            assert_eq!(note_source(&pool, note.id).await.unwrap(), Some(source.id));
            for sql in [
                "UPDATE source_notes SET source_id = 2",
                "DELETE FROM source_notes",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn pages_belong_to_an_active_source_within_its_count() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-pages");
            let source = create_source(&pool, &store, &pdf_document("x"), at(NOW))
                .await
                .unwrap();
            for sql in [
                "INSERT INTO source_pages (source_id, number, text) VALUES (1, 4, 'past the end')",
                "INSERT INTO source_pages (source_id, number, text) VALUES (1, 0, 'zero')",
                "INSERT INTO source_pages (source_id, number, text) VALUES (2, 1, 'no source')",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            assert_eq!(source.id, 1);
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }
}
