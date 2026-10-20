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
use crate::notes::{self, Note, NoteError, NoteSummary, NOTE_TITLE_MAX_CHARS};
use crate::search::Piece;
use crate::suggest;

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

/// Why a note couldn't be saved from a source. Nothing was written.
#[derive(Debug)]
pub enum NoteFromSourceError {
    /// The note itself was refused (its title or text).
    Note(NoteError),
    /// The source is missing or deleted.
    Source(SourceError),
}

impl From<sqlx::Error> for NoteFromSourceError {
    fn from(err: sqlx::Error) -> Self {
        NoteFromSourceError::Source(err.into())
    }
}

/// Saves a new note written from the active source `source_id` and records
/// where it came from, in one transaction: the note is never saved without
/// its link, or the link without the note.
pub async fn create_note(
    pool: &SqlitePool,
    source_id: i64,
    title: &str,
    body: &str,
    picture_ids: &[i64],
    now: DateTime<Utc>,
) -> Result<Note, NoteFromSourceError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let source = load(&mut tx, source_id)
        .await
        .map_err(NoteFromSourceError::Source)?;
    if source.deleted_at.is_some() {
        return Err(NoteFromSourceError::Source(SourceError::Deleted));
    }
    let note = notes::insert_note(&mut tx, title, body, picture_ids, now)
        .await
        .map_err(NoteFromSourceError::Note)?;
    sqlx::query("INSERT INTO source_notes (note_id, source_id, created_at) VALUES (?1, ?2, ?3)")
        .bind(note.id)
        .bind(source_id)
        .bind(to_db_time(now))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(note)
}

/// The active notes written from a source, most recently edited first (as
/// the notes library orders them).
pub async fn source_notes(pool: &SqlitePool, id: i64) -> Result<Vec<NoteSummary>, SourceError> {
    source(pool, id).await?;
    Ok(sqlx::query_as(
        "SELECT n.id, n.title, n.created_at, n.updated_at
         FROM source_notes l JOIN notes n ON n.id = l.note_id
         WHERE l.source_id = ?1 AND n.deleted_at IS NULL
         ORDER BY n.updated_at DESC, n.id DESC",
    )
    .bind(id)
    .fetch_all(pool)
    .await?)
}

/// The original file of an active source, read back from the store and
/// checked against its hash. A file that's missing or changed is an
/// internal error: it was written before its row, and is never changed.
pub async fn original(
    pool: &SqlitePool,
    store: &AssetStore,
    id: i64,
) -> Result<(Source, Vec<u8>), SourceError> {
    let source = source(pool, id).await?;
    let missing = || SourceError::Internal(format!("source {id}: its file is missing").into());
    let path = store.path_of(&source.asset.sha256).ok_or_else(missing)?;
    let bytes = std::fs::read(path).map_err(|_| missing())?;
    if bytes.len() as u64 != source.asset.size || assets::sha256_hex(&bytes) != source.asset.sha256
    {
        return Err(missing());
    }
    Ok((source, bytes))
}

/// A name to give a copy of a source's original on disk: the kept name with
/// anything a file system might refuse replaced, and the kind's extension
/// added if the name lacks it, so the computer knows what opens it.
pub fn copy_file_name(source: &Source) -> String {
    let cleaned: String = source
        .file_name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut name = if cleaned.is_empty() {
        "document".to_string()
    } else {
        cleaned.to_string()
    };
    let extension = match source.kind {
        FileKind::Pdf => "pdf",
        FileKind::Text => "txt",
        FileKind::Markdown => "md",
        FileKind::Word => "docx",
        FileKind::PowerPoint => "pptx",
    };
    let has_extension = std::path::Path::new(&name)
        .extension()
        .is_some_and(|found| {
            let found = found.to_string_lossy().to_lowercase();
            found == extension || (source.kind == FileKind::Markdown && found == "markdown")
        });
    if !has_extension {
        name.push('.');
        name.push_str(extension);
    }
    name
}

/// Where a quote is in a source: the page its first word is on, and the page
/// its last word is on (the same page, unless the quote runs over a break).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteSpot {
    pub page: u32,
    pub last_page: u32,
}

/// The source a note came from, and where each of the quotes asked about is
/// in it: `{ sourceId, title, kind, pageCount, spots }`, with a spot (or null, when that quote
/// isn't in the source word for word) per quote, in the order asked.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotedSource {
    pub source_id: i64,
    pub title: String,
    pub kind: FileKind,
    pub page_count: u32,
    pub spots: Vec<Option<QuoteSpot>>,
}

/// One word of a source: its page, where it is in that page's text (bytes),
/// and its comparable form (`suggest::words`).
struct PageWord {
    page: u32,
    start: usize,
    end: usize,
}

/// Every word of every page, in order, and their comparable forms.
fn source_words(pages: &[SourcePage]) -> (Vec<PageWord>, Vec<String>) {
    let mut found = Vec::new();
    let mut words = Vec::new();
    for page in pages {
        let mut start = None;
        for (at, c) in page.text.char_indices().chain([(page.text.len(), ' ')]) {
            match (c.is_alphanumeric(), start) {
                (true, None) => start = Some(at),
                (false, Some(from)) => {
                    found.push(PageWord {
                        page: page.number,
                        start: from,
                        end: at,
                    });
                    words.push(page.text[from..at].to_lowercase());
                    start = None;
                }
                _ => {}
            }
        }
    }
    (found, words)
}

/// Where `quote` is among `words`, by the rule suggested cards and quiz
/// questions were checked with (`suggest::quoted_from`): its pieces (split
/// at "..." or "…") word for word and in order, ignoring spacing,
/// punctuation and case. Each piece's range of word indexes, or None.
fn find_quote(words: &[String], quote: &str) -> Option<Vec<std::ops::Range<usize>>> {
    let pieces: Vec<Vec<String>> = quote
        .split("...")
        .flat_map(|part| part.split('…'))
        .map(suggest::words)
        .filter(|piece| !piece.is_empty())
        .collect();
    if pieces.is_empty() {
        return None;
    }
    let mut from = 0;
    let mut ranges = Vec::with_capacity(pieces.len());
    for piece in &pieces {
        let at = words[from..]
            .windows(piece.len())
            .position(|window| window == piece.as_slice())?;
        ranges.push(from + at..from + at + piece.len());
        from += at + piece.len();
    }
    Some(ranges)
}

/// The most quotes looked for at once: more than a suggestion or quiz shows.
pub const MAX_QUOTES: usize = 100;

/// Where each of `quotes` is in the source the note `note_id` was written
/// from. None when the note didn't come from a source, or its source is
/// deleted: then there's no page to show. A quote the user's edits moved
/// away from the source's wording has no spot.
pub async fn locate_quotes(
    pool: &SqlitePool,
    note_id: i64,
    quotes: &[String],
) -> Result<Option<QuotedSource>, SourceError> {
    let Some(source_id) = note_source(pool, note_id).await? else {
        return Ok(None);
    };
    let found = match source(pool, source_id).await {
        Ok(found) => found,
        Err(SourceError::Deleted | SourceError::NotFound) => return Ok(None),
        Err(err) => return Err(err),
    };
    let pages = source_pages(pool, source_id).await?;
    let (located, words) = source_words(&pages);
    let spots = quotes
        .iter()
        .map(|quote| {
            let ranges = find_quote(&words, quote)?;
            let first = ranges.first()?.start;
            let last = ranges.last()?.end - 1;
            Some(QuoteSpot {
                page: located[first].page,
                last_page: located[last].page,
            })
        })
        .collect();
    Ok(Some(QuotedSource {
        source_id,
        title: found.title,
        kind: found.kind,
        page_count: found.page_count,
        spots,
    }))
}

/// The text of page `number` of an active source as pieces, with the part of
/// `quote` that's on this page marked (`hit`), from its first word to its
/// last, piece by piece. Unmarked when the quote isn't in the source, or is
/// on other pages. None when the source has no such page.
pub async fn quoted_page(
    pool: &SqlitePool,
    id: i64,
    number: u32,
    quote: &str,
) -> Result<Option<Vec<Piece>>, SourceError> {
    let pages = source_pages(pool, id).await?;
    let Some(page) = pages.iter().find(|page| page.number == number) else {
        return Ok(None);
    };
    let (located, words) = source_words(&pages);
    let mut marks: Vec<(usize, usize)> = Vec::new();
    for range in find_quote(&words, quote).unwrap_or_default() {
        let on_page: Vec<&PageWord> = located[range]
            .iter()
            .filter(|word| word.page == number)
            .collect();
        if let (Some(first), Some(last)) = (on_page.first(), on_page.last()) {
            marks.push((first.start, last.end));
        }
    }
    let mut pieces = Vec::new();
    let mut at = 0;
    for (start, end) in marks {
        if start > at {
            pieces.push(Piece {
                text: page.text[at..start].to_string(),
                hit: false,
            });
        }
        pieces.push(Piece {
            text: page.text[start..end].to_string(),
            hit: true,
        });
        at = end;
    }
    if at < page.text.len() || pieces.is_empty() {
        pieces.push(Piece {
            text: page.text[at..].to_string(),
            hit: false,
        });
    }
    Ok(Some(pieces))
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

    #[test]
    fn a_note_from_a_source_is_saved_with_its_link_or_not_at_all() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-note-from");
            let source = create_source(&pool, &store, &pdf_document("x"), at(NOW))
                .await
                .unwrap();

            let note = create_note(&pool, source.id, " Biology ", "Cells", &[], at(NOW))
                .await
                .unwrap();
            assert_eq!(note.title, "Biology");
            assert_eq!(note_source(&pool, note.id).await.unwrap(), Some(source.id));

            // A refused note writes neither the note nor a link.
            assert!(matches!(
                create_note(&pool, source.id, "   ", "Cells", &[], at(NOW)).await,
                Err(NoteFromSourceError::Note(NoteError::Invalid(_)))
            ));
            assert!(matches!(
                create_note(&pool, 999, "Title", "Cells", &[], at(NOW)).await,
                Err(NoteFromSourceError::Source(SourceError::NotFound))
            ));
            delete_source(&pool, source.id, at(LATER)).await.unwrap();
            assert!(matches!(
                create_note(&pool, source.id, "Title", "Cells", &[], at(NOW)).await,
                Err(NoteFromSourceError::Source(SourceError::Deleted))
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM notes").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM source_notes").await, 1);
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_note_from_a_source_keeps_its_pictures_or_saves_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-note-pictures");
            let source = create_source(&pool, &store, &pdf_document("x"), at(NOW))
                .await
                .unwrap();
            let first = crate::assets::add_picture(&pool, &store, &png(1), at(NOW))
                .await
                .unwrap();
            let second = crate::assets::add_picture(&pool, &store, &png(2), at(NOW))
                .await
                .unwrap();

            let note = create_note(
                &pool,
                source.id,
                "Biology",
                "Cells",
                &[second.id, first.id],
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(note.pictures, vec![second.clone(), first.clone()]);
            assert_eq!(
                crate::notes::note(&pool, note.id).await.unwrap().pictures,
                vec![second, first.clone()]
            );
            assert_eq!(note_source(&pool, note.id).await.unwrap(), Some(source.id));

            // A file that isn't a picture refuses the note, and its link.
            assert!(matches!(
                create_note(
                    &pool,
                    source.id,
                    "Other",
                    "Cells",
                    &[first.id, source.asset.id],
                    at(NOW),
                )
                .await,
                Err(NoteFromSourceError::Note(NoteError::Invalid(_)))
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM notes").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM source_notes").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM note_pictures").await, 2);
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_source_lists_the_active_notes_written_from_it() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-notes");
            let source = create_source(&pool, &store, &pdf_document("x"), at(NOW))
                .await
                .unwrap();
            let other = create_source(&pool, &store, &text_document("other"), at(NOW))
                .await
                .unwrap();
            let first = create_note(&pool, source.id, "First", "a", &[], at(NOW))
                .await
                .unwrap();
            let second = create_note(&pool, source.id, "Second", "b", &[], at(LATER))
                .await
                .unwrap();
            create_note(&pool, other.id, "Elsewhere", "c", &[], at(NOW))
                .await
                .unwrap();
            crate::notes::create_note(&pool, "Unlinked", "d", at(NOW))
                .await
                .unwrap();

            let titles = |notes: Vec<NoteSummary>| -> Vec<String> {
                notes.into_iter().map(|note| note.title).collect()
            };
            assert_eq!(
                titles(source_notes(&pool, source.id).await.unwrap()),
                ["Second", "First"]
            );
            crate::notes::delete_note(&pool, second.id, at(LATER))
                .await
                .unwrap();
            assert_eq!(
                titles(source_notes(&pool, source.id).await.unwrap()),
                ["First"]
            );
            assert_eq!(first.title, "First");
            delete_source(&pool, source.id, at(LATER)).await.unwrap();
            assert!(matches!(
                source_notes(&pool, source.id).await,
                Err(SourceError::Deleted)
            ));
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn quotes_are_found_on_the_pages_they_came_from() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-quotes");
            // Pages: "Cells are the unit of life. one", "", "Mitochondria make ATP."
            let source = create_source(&pool, &store, &pdf_document("one"), at(NOW))
                .await
                .unwrap();
            let note = create_note(&pool, source.id, "Biology", "Cells", &[], at(NOW))
                .await
                .unwrap();
            let quotes: Vec<String> = [
                "the UNIT of life",
                "life - one. Mitochondria make",
                "Cells are … make ATP",
                "Ribosomes make proteins",
                "...",
            ]
            .map(String::from)
            .to_vec();

            let found = locate_quotes(&pool, note.id, &quotes)
                .await
                .unwrap()
                .unwrap();
            let spot = |page, last_page| Some(QuoteSpot { page, last_page });
            assert_eq!(found.source_id, source.id);
            assert_eq!((found.title.as_str(), found.page_count), ("Biology one", 3));
            assert_eq!(
                found.spots,
                vec![spot(1, 1), spot(1, 3), spot(1, 3), None, None]
            );

            // A note not written from a source has no pages to show.
            let unlinked = crate::notes::create_note(&pool, "Typed", "Cells", at(NOW))
                .await
                .unwrap();
            assert_eq!(
                locate_quotes(&pool, unlinked.id, &quotes).await.unwrap(),
                None
            );

            // The page with the quote marked, from its first word to its last.
            let marked = |pieces: Vec<Piece>| -> Vec<(String, bool)> {
                pieces.into_iter().map(|p| (p.text, p.hit)).collect()
            };
            let own = |text: &str, hit| (text.to_string(), hit);
            assert_eq!(
                marked(
                    quoted_page(&pool, source.id, 1, "the UNIT of life")
                        .await
                        .unwrap()
                        .unwrap()
                ),
                vec![
                    own("Cells are ", false),
                    own("the unit of life", true),
                    own(". one", false)
                ]
            );
            // A quote that runs over the break is marked on each page.
            assert_eq!(
                marked(
                    quoted_page(&pool, source.id, 3, &quotes[1])
                        .await
                        .unwrap()
                        .unwrap()
                ),
                vec![own("Mitochondria make", true), own(" ATP.", false)]
            );
            // Pieces of a quote with a gap are marked apart.
            assert_eq!(
                marked(
                    quoted_page(&pool, source.id, 1, "Cells ... unit")
                        .await
                        .unwrap()
                        .unwrap()
                ),
                vec![
                    own("Cells", true),
                    own(" are the ", false),
                    own("unit", true),
                    own(" of life. one", false)
                ]
            );
            // Not in the source, or a page without text: nothing is marked.
            assert_eq!(
                marked(
                    quoted_page(&pool, source.id, 1, "Ribosomes")
                        .await
                        .unwrap()
                        .unwrap()
                ),
                vec![own("Cells are the unit of life. one", false)]
            );
            assert_eq!(
                marked(quoted_page(&pool, source.id, 2, "").await.unwrap().unwrap()),
                vec![own("", false)]
            );
            assert_eq!(quoted_page(&pool, source.id, 4, "").await.unwrap(), None);

            // Once the source is deleted there's no page to show.
            delete_source(&pool, source.id, at(LATER)).await.unwrap();
            assert_eq!(locate_quotes(&pool, note.id, &quotes).await.unwrap(), None);
            assert!(matches!(
                quoted_page(&pool, source.id, 1, "").await,
                Err(SourceError::Deleted)
            ));
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn the_original_comes_back_checked_under_a_usable_name() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("sources-original");
            let document = pdf_document("x");
            let source = create_source(&pool, &store, &document, at(NOW))
                .await
                .unwrap();

            let (found, bytes) = original(&pool, &store, source.id).await.unwrap();
            assert_eq!((found.id, bytes), (source.id, document.bytes.clone()));
            assert_eq!(copy_file_name(&source), "Biology x.pdf");

            let named = |kind, file_name: &str| {
                copy_file_name(&Source {
                    kind,
                    file_name: file_name.to_string(),
                    ..source.clone()
                })
            };
            assert_eq!(named(FileKind::Pdf, "../a/b:c.PDF"), "_a_b_c.PDF");
            assert_eq!(named(FileKind::Word, "Lecture"), "Lecture.docx");
            assert_eq!(
                named(FileKind::Markdown, "notes.markdown"),
                "notes.markdown"
            );
            assert_eq!(named(FileKind::Text, " .. "), "document.txt");

            // A kept file that changed on disk isn't handed out.
            let path = store.path_of(&source.asset.sha256).unwrap();
            std::fs::write(&path, b"%PDF-changed").unwrap();
            assert!(matches!(
                original(&pool, &store, source.id).await,
                Err(SourceError::Internal(_))
            ));
            std::fs::remove_file(&path).unwrap();
            assert!(matches!(
                original(&pool, &store, source.id).await,
                Err(SourceError::Internal(_))
            ));
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }
}
