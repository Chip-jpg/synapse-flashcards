//! Text recognised in kept files (migration 0019): reading a picture, or the
//! scanned pages of a source, with the engine built into Synapse (`ocr.rs`),
//! and keeping what was read.
//!
//! A recognition belongs to the file (`assets`), not to whatever shows it: a
//! picture is page 0, a PDF page is its number. Rows are never changed or
//! removed; reading a page again adds a row, and the newest row for a file
//! and page is the one in use. What extraction read stays in `source_pages`;
//! a page's recognised text is offered beside it (`sources::source_pages`)
//! and used where the extracted text is blank, marked as recognised wherever
//! it is shown.
//!
//! Reading runs in the helper process and can be stopped ([`Recognising`]).
//! Each page is saved as soon as it is read, so stopping a long PDF keeps the
//! pages already done.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};
use tokio_util::sync::CancellationToken;

use crate::assets::{self, Asset, AssetStore, MediaType};
use crate::db::{to_db_time, DbError};
use crate::extract::FileKind;
use crate::ocr::{self, Job, Line, PageOutcome, PageProblem, Reading, RunError};
use crate::sources::{self, SourceError};

/// The page number a picture is read as.
pub const PICTURE_PAGE: u32 = 0;

/// The engines a recognition can come from, as `recognitions.engine` names
/// them: `"ocrs"` (built in) or `"ollama"` (a vision model in Ollama).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Engine {
    Ocrs,
    Ollama,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Ocrs => ocr::ENGINE,
            Engine::Ollama => "ollama",
        }
    }

    fn parse(name: &str) -> Option<Engine> {
        [Engine::Ocrs, Engine::Ollama]
            .into_iter()
            .find(|engine| engine.as_str() == name)
    }
}

/// What was read from one picture or page: `{ assetId, page, engine, model,
/// text, lines, width, height, createdAt }`. `lines` is empty for an engine
/// that doesn't say where text is; `width` and `height` are null then too.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recognition {
    pub asset_id: i64,
    pub page: u32,
    pub engine: Engine,
    pub model: String,
    pub text: String,
    pub lines: Vec<Line>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub created_at: String,
}

/// Why nothing was recognised. Pages saved before it stopped stay saved.
#[derive(Debug)]
pub enum RecognitionError {
    /// No kept file, or no source, has this id.
    NotFound,
    /// The source is deleted.
    Deleted,
    /// The file isn't a picture, or the source isn't a PDF, or a page asked
    /// for isn't one of its pages.
    NotReadable,
    /// The picture itself couldn't be read (damaged, too large, a kind
    /// Synapse can't decode).
    Picture(PageProblem),
    /// The PDF couldn't be opened. Detail is for logs only.
    Unreadable(DbError),
    /// The user stopped it.
    Stopped,
    /// The helper crashed, couldn't start, or ran too long, or the database
    /// failed. Detail is for logs only.
    Internal(DbError),
}

impl From<sqlx::Error> for RecognitionError {
    fn from(err: sqlx::Error) -> Self {
        RecognitionError::Internal(err.into())
    }
}

impl From<SourceError> for RecognitionError {
    fn from(err: SourceError) -> Self {
        match err {
            SourceError::NotFound => RecognitionError::NotFound,
            SourceError::Deleted => RecognitionError::Deleted,
            SourceError::Internal(detail) => RecognitionError::Internal(detail),
            other => RecognitionError::Internal(format!("{other:?}").into()),
        }
    }
}

type RecognitionRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    String,
);

const FIELDS: &str = "asset_id, page, engine, model, text, lines, width, height, created_at";

fn from_row(row: RecognitionRow) -> Result<Recognition, DbError> {
    let (asset_id, page, engine, model, text, lines, width, height, created_at) = row;
    let size = |value: Option<i64>| value.map(u32::try_from).transpose();
    Ok(Recognition {
        asset_id,
        page: u32::try_from(page)?,
        engine: Engine::parse(&engine).ok_or_else(|| format!("unknown engine {engine:?}"))?,
        model,
        text,
        lines: serde_json::from_str(&lines)?,
        width: size(width)?,
        height: size(height)?,
        created_at,
    })
}

/// Keeps what an engine read from page `page` of the file `asset_id`, on
/// the connection or inside the transaction `conn`. Public to the crate so
/// another engine (a vision model in Ollama) stores its results the same
/// way: `lines` empty and no size when it doesn't say where text is.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert(
    conn: &mut SqliteConnection,
    asset_id: i64,
    page: u32,
    engine: Engine,
    model: &str,
    text: &str,
    lines: &[Line],
    size: Option<(u32, u32)>,
    now: DateTime<Utc>,
) -> Result<Recognition, DbError> {
    let lines = serde_json::to_string(lines)?;
    let row: RecognitionRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO recognitions
             (asset_id, page, engine, model, text, lines, width, height, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         RETURNING {FIELDS}"
    )))
    .bind(asset_id)
    .bind(i64::from(page))
    .bind(engine.as_str())
    .bind(model)
    .bind(text)
    .bind(lines)
    .bind(size.map(|(width, _)| i64::from(width)))
    .bind(size.map(|(_, height)| i64::from(height)))
    .bind(to_db_time(now))
    .fetch_one(conn)
    .await?;
    from_row(row)
}

async fn insert_reading(
    pool: &SqlitePool,
    asset_id: i64,
    page: u32,
    reading: &Reading,
    now: DateTime<Utc>,
) -> Result<Recognition, DbError> {
    let mut conn = pool.acquire().await?;
    insert(
        &mut conn,
        asset_id,
        page,
        Engine::Ocrs,
        ocr::MODEL,
        &reading.text(),
        &reading.lines,
        Some((reading.width, reading.height)),
        now,
    )
    .await
}

/// The recognition in use for one page of a file: the newest.
pub async fn latest(
    pool: &SqlitePool,
    asset_id: i64,
    page: u32,
) -> Result<Option<Recognition>, DbError> {
    let row: Option<RecognitionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {FIELDS} FROM recognitions WHERE asset_id = ?1 AND page = ?2 ORDER BY id DESC LIMIT 1"
    )))
    .bind(asset_id)
    .bind(i64::from(page))
    .fetch_optional(pool)
    .await?;
    row.map(from_row).transpose()
}

/// The newest reading of one page of a file by the built-in engine with
/// today's models, if it has one.
async fn latest_by_builtin(
    pool: &SqlitePool,
    asset_id: i64,
    page: u32,
) -> Result<Option<Recognition>, DbError> {
    let row: Option<RecognitionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {FIELDS} FROM recognitions WHERE asset_id = ?1 AND page = ?2 AND engine = ?3 AND model = ?4
         ORDER BY id DESC LIMIT 1"
    )))
    .bind(asset_id)
    .bind(i64::from(page))
    .bind(ocr::ENGINE)
    .bind(ocr::MODEL)
    .fetch_optional(pool)
    .await?;
    row.map(from_row).transpose()
}

/// The recognition in use for every page of a file that has one, by page.
pub async fn latest_pages(
    conn: &mut SqliteConnection,
    asset_id: i64,
) -> Result<BTreeMap<u32, Recognition>, DbError> {
    let rows: Vec<RecognitionRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {FIELDS} FROM recognitions WHERE id IN (SELECT MAX(id) FROM recognitions WHERE asset_id = ?1 GROUP BY page)
         ORDER BY page"
    )))
    .bind(asset_id)
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| from_row(row).map(|found| (found.page, found)))
        .collect()
}

/// The pages of a file the built-in engine has already read with today's
/// models, so asking again doesn't read them twice.
async fn read_by_builtin(pool: &SqlitePool, asset_id: i64) -> Result<Vec<u32>, DbError> {
    let pages: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT page FROM recognitions WHERE asset_id = ?1 AND engine = ?2 AND model = ?3",
    )
    .bind(asset_id)
    .bind(ocr::ENGINE)
    .bind(ocr::MODEL)
    .fetch_all(pool)
    .await?;
    pages
        .into_iter()
        .map(|page| u32::try_from(page).map_err(Into::into))
        .collect()
}

/// A kept file's bytes, checked against its size and hash. A file that's
/// missing or changed is an internal error: it was written before its row,
/// and is never changed.
pub(crate) fn kept_bytes(store: &AssetStore, asset: &Asset) -> Result<Vec<u8>, RecognitionError> {
    let missing =
        || RecognitionError::Internal(format!("kept file {}: missing or changed", asset.id).into());
    let path = store.path_of(&asset.sha256).ok_or_else(missing)?;
    let bytes = std::fs::read(path).map_err(|_| missing())?;
    if bytes.len() as u64 != asset.size || assets::sha256_hex(&bytes) != asset.sha256 {
        return Err(missing());
    }
    Ok(bytes)
}

/// Where the helper is (normally Synapse itself), and how long it may take
/// on a page. Tests point it at the built binary.
#[derive(Debug, Clone)]
pub struct Helper {
    pub program: PathBuf,
    pub page_timeout: std::time::Duration,
}

impl Helper {
    /// Synapse itself, with [`ocr::PAGE_TIMEOUT`].
    pub fn this_program() -> std::io::Result<Helper> {
        Ok(Helper {
            program: std::env::current_exe()?,
            page_timeout: ocr::PAGE_TIMEOUT,
        })
    }
}

pub(crate) fn run_error(err: RunError) -> RecognitionError {
    match err {
        RunError::Stopped => RecognitionError::Stopped,
        RunError::Unreadable(detail) => RecognitionError::Unreadable(detail.into()),
        RunError::HelperFailed(detail) => RecognitionError::Internal(detail.into()),
    }
}

/// Each page's outcome as the helper answers it, and the helper's run.
type Started = (
    tauri::async_runtime::Receiver<(u32, PageOutcome)>,
    tauri::async_runtime::JoinHandle<Result<(), RunError>>,
);

/// Runs the helper on its own thread, sending each page's outcome back as it
/// comes; the returned handle ends with how the run ended.
fn start(helper: Helper, job: Job, bytes: Vec<u8>, stop: CancellationToken) -> Started {
    let (sender, receiver) = tauri::async_runtime::channel(64);
    let run = tauri::async_runtime::spawn_blocking(move || {
        ocr::run_helper(
            &helper.program,
            &job,
            &bytes,
            &stop,
            helper.page_timeout,
            |page, outcome| {
                let _ = sender.blocking_send((page, outcome));
            },
        )
    });
    (receiver, run)
}

async fn finish(
    run: tauri::async_runtime::JoinHandle<Result<(), RunError>>,
) -> Result<(), RecognitionError> {
    run.await
        .map_err(|err| RecognitionError::Internal(format!("the helper's thread: {err}").into()))?
        .map_err(run_error)
}

/// The text in the kept picture `asset_id`. A picture the built-in engine
/// already read with today's models isn't read again: the result in use is
/// returned. Otherwise it is read and kept.
pub async fn recognise_picture(
    pool: &SqlitePool,
    store: &AssetStore,
    helper: Helper,
    asset_id: i64,
    stop: CancellationToken,
    now: DateTime<Utc>,
) -> Result<Recognition, RecognitionError> {
    let asset = {
        let mut conn = pool.acquire().await?;
        assets::asset_by_id(&mut conn, asset_id)
            .await
            .map_err(RecognitionError::Internal)?
            .ok_or(RecognitionError::NotFound)?
    };
    if !asset.media_type.is_picture() {
        return Err(RecognitionError::NotReadable);
    }
    // The built-in engine's own reading, even when a vision model has read
    // the picture since: it says where the text is, which a model doesn't.
    if let Some(found) = latest_by_builtin(pool, asset_id, PICTURE_PAGE)
        .await
        .map_err(RecognitionError::Internal)?
    {
        return Ok(found);
    }
    let bytes = kept_bytes(store, &asset)?;
    let (mut outcomes, run) = start(helper, Job::Picture, bytes, stop);
    let mut answer = None;
    while let Some((_, outcome)) = outcomes.recv().await {
        answer = Some(outcome);
    }
    finish(run).await?;
    match answer {
        Some(PageOutcome::Read(reading)) => {
            insert_reading(pool, asset_id, PICTURE_PAGE, &reading, now)
                .await
                .map_err(RecognitionError::Internal)
        }
        Some(PageOutcome::Problem(problem)) => Err(RecognitionError::Picture(problem)),
        Some(PageOutcome::Picture(_)) | None => Err(RecognitionError::Internal(
            "the helper answered nothing".into(),
        )),
    }
}

/// Progress through a source's pages, sent as each page finishes:
/// `{ page, done, total, problem }`, `problem` null when the page was read.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageProgress {
    pub page: u32,
    pub done: u32,
    pub total: u32,
    pub problem: Option<PageProblem>,
}

/// A page that couldn't be read, and why: `{ page, problem }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSkipped {
    pub page: u32,
    pub problem: PageProblem,
}

/// How reading a source's pages went: `{ read, skipped, stopped }`. `read`
/// lists the pages read and kept (some may have had no text on them).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecognition {
    pub read: Vec<u32>,
    pub skipped: Vec<PageSkipped>,
    pub stopped: bool,
}

/// The pages of the active PDF source `source_id` that recognition would
/// read when no pages are named: those whose extracted text is blank and
/// that the built-in engine hasn't read with today's models. A page found
/// to draw no picture counts as read (an empty reading is kept for it).
pub async fn pages_to_read(
    pool: &SqlitePool,
    source_id: i64,
) -> Result<Vec<u32>, RecognitionError> {
    let source = sources::source(pool, source_id).await?;
    if source.kind != FileKind::Pdf {
        return Err(RecognitionError::NotReadable);
    }
    let done = read_by_builtin(pool, source.asset.id)
        .await
        .map_err(RecognitionError::Internal)?;
    let blank: Vec<i64> = sqlx::query_scalar(
        "SELECT number FROM source_pages
         WHERE source_id = ?1 AND trim(text, ' ' || char(9, 10, 13)) = ''
         ORDER BY number",
    )
    .bind(source_id)
    .fetch_all(pool)
    .await?;
    Ok(blank
        .into_iter()
        .filter_map(|page| u32::try_from(page).ok())
        .filter(|page| !done.contains(page))
        .collect())
}

/// The pages asked for, each once in the order first asked, if all are pages
/// of a source of `page_count` pages.
pub(crate) fn pages_asked(pages: Vec<u32>, page_count: u32) -> Result<Vec<u32>, RecognitionError> {
    if pages.iter().any(|page| *page == 0 || *page > page_count) {
        return Err(RecognitionError::NotReadable);
    }
    let mut unique = Vec::with_capacity(pages.len());
    for page in pages {
        if !unique.contains(&page) {
            unique.push(page);
        }
    }
    Ok(unique)
}

/// Reads pages of the active PDF source `source_id` and keeps what each
/// says, page by page: `pages` in the order given, or, when None, every page
/// [`pages_to_read`] finds. Each page is saved as it is read and reported to
/// `progress`. Stopping keeps the pages done and returns with `stopped`.
#[allow(clippy::too_many_arguments)]
pub async fn recognise_source(
    pool: &SqlitePool,
    store: &AssetStore,
    helper: Helper,
    source_id: i64,
    pages: Option<Vec<u32>>,
    stop: CancellationToken,
    now: impl Fn() -> DateTime<Utc>,
    progress: impl Fn(PageProgress),
) -> Result<SourceRecognition, RecognitionError> {
    let source = sources::source(pool, source_id).await?;
    if source.kind != FileKind::Pdf || source.asset.media_type != MediaType::Pdf {
        return Err(RecognitionError::NotReadable);
    }
    let pages = match pages {
        Some(pages) => pages_asked(pages, source.page_count)?,
        None => pages_to_read(pool, source_id).await?,
    };
    let mut result = SourceRecognition::default();
    if pages.is_empty() {
        return Ok(result);
    }
    let bytes = kept_bytes(store, &source.asset)?;
    let total = pages.len() as u32;
    let (mut outcomes, run) = start(helper, Job::Pdf { pages }, bytes, stop);
    let mut done = 0;
    let mut saving = Ok(());
    while let Some((page, outcome)) = outcomes.recv().await {
        if saving.is_err() {
            continue;
        }
        done += 1;
        let problem = match outcome {
            PageOutcome::Read(reading) => {
                if let Err(err) = insert_reading(pool, source.asset.id, page, &reading, now()).await
                {
                    saving = Err(RecognitionError::Internal(err));
                    continue;
                }
                result.read.push(page);
                None
            }
            PageOutcome::Problem(problem) => {
                // A page that draws no picture has nothing to read, however
                // often it's asked: keep an empty reading, so it isn't
                // offered again. A picture Synapse couldn't decode is left
                // unread, for another engine or a later version.
                if problem == PageProblem::NoPicture {
                    let empty = Reading {
                        width: 0,
                        height: 0,
                        lines: Vec::new(),
                    };
                    let mut conn = match pool.acquire().await {
                        Ok(conn) => conn,
                        Err(err) => {
                            saving = Err(err.into());
                            continue;
                        }
                    };
                    if let Err(err) = insert(
                        &mut conn,
                        source.asset.id,
                        page,
                        Engine::Ocrs,
                        ocr::MODEL,
                        &empty.text(),
                        &empty.lines,
                        None,
                        now(),
                    )
                    .await
                    {
                        saving = Err(RecognitionError::Internal(err));
                        continue;
                    }
                }
                result.skipped.push(PageSkipped { page, problem });
                Some(problem)
            }
            // Only a job for a vision model answers with a picture.
            PageOutcome::Picture(_) => {
                saving = Err(RecognitionError::Internal(
                    "the helper answered with a picture".into(),
                ));
                continue;
            }
        };
        progress(PageProgress {
            page,
            done,
            total,
            problem,
        });
    }
    let ended = finish(run).await;
    saving?;
    match ended {
        Ok(()) => Ok(result),
        Err(RecognitionError::Stopped) => {
            result.stopped = true;
            Ok(result)
        }
        Err(err) => Err(err),
    }
}

/// The recognition being worked on, so the user can stop it. Only one runs
/// at a time: starting another stops the one before.
#[derive(Default)]
pub struct Recognising {
    state: Mutex<(u64, Option<(u64, CancellationToken)>)>,
}

impl Recognising {
    fn running(&self) -> std::sync::MutexGuard<'_, (u64, Option<(u64, CancellationToken)>)> {
        // Nothing in the lock can panic halfway, so a poisoned lock is safe to use.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts tracking a new recognition, stopping any still running.
    pub fn begin(&self) -> Run<'_> {
        let token = CancellationToken::new();
        let mut running = self.running();
        running.0 += 1;
        let id = running.0;
        if let Some((_, previous)) = running.1.replace((id, token.clone())) {
            previous.cancel();
        }
        Run {
            owner: self,
            id,
            token,
        }
    }

    /// Stops the recognition being worked on, if there is one.
    pub fn stop(&self) {
        if let Some((_, token)) = self.running().1.take() {
            token.cancel();
        }
    }
}

/// One recognition that can be stopped. Dropping it stops tracking it.
pub struct Run<'a> {
    owner: &'a Recognising,
    id: u64,
    token: CancellationToken,
}

impl Run<'_> {
    /// The token that stops this recognition.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        let mut running = self.owner.running();
        if running.1.as_ref().is_some_and(|(id, _)| *id == self.id) {
            running.1 = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::test_support::*;
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::extract::{Document, ExtractedText};
    use crate::ocr::test_support::{self as pages, TestPage, PAGE_TEXT};
    use std::sync::Mutex as StdMutex;

    const NOW: &str = "2026-10-21T09:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    /// The Synapse binary, which `cargo test` builds beside the test binary,
    /// run as the helper.
    fn helper() -> Helper {
        let deps = std::env::current_exe().unwrap();
        let program = deps
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(format!("synapse-scaffold{}", std::env::consts::EXE_SUFFIX));
        assert!(
            program.exists(),
            "run `cargo test` (not `--lib` alone), which builds {program:?}"
        );
        Helper {
            program,
            page_timeout: ocr::PAGE_TIMEOUT,
        }
    }

    use std::path::Path;

    /// A scanned PDF as importing it gives it: page 1 typed text, page 2 the
    /// sample page as a JPEG, page 3 a JBIG2 picture Synapse can't decode.
    fn scanned_pdf() -> Document {
        let pages = vec!["Typed text".to_string(), String::new(), String::new()];
        Document {
            file_name: "Scan.pdf".to_string(),
            bytes: pages::pdf(&[TestPage::TextOnly, TestPage::Jpeg, TestPage::Jbig2]),
            text: ExtractedText {
                kind: FileKind::Pdf,
                title: "Scan".to_string(),
                body: "Typed text".to_string(),
                pages: Some(3),
                pages_without_text: vec![2, 3],
            },
            pages,
        }
    }

    #[test]
    fn recognitions_fit_their_file_and_never_change() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("recognition-rules");
            let picture = assets::add_picture(&pool, &store, &png(1), at(NOW))
                .await
                .unwrap();
            let source = sources::create_source(&pool, &store, &scanned_pdf(), at(NOW))
                .await
                .unwrap();
            let text = sources::create_source(
                &pool,
                &store,
                &sources::test_support::text_document("Plain"),
                at(NOW),
            )
            .await
            .unwrap();
            let mut conn = pool.acquire().await.unwrap();
            let mut add = async |asset: i64, page: u32| {
                insert(
                    &mut conn,
                    asset,
                    page,
                    Engine::Ocrs,
                    ocr::MODEL,
                    "x",
                    &[],
                    None,
                    at(NOW),
                )
                .await
            };
            assert!(add(picture.id, 0).await.is_ok());
            assert!(add(picture.id, 1).await.is_err());
            assert!(add(source.asset.id, 0).await.is_err());
            assert!(add(source.asset.id, 2).await.is_ok());
            assert!(add(text.asset.id, 0).await.is_err());
            assert!(add(text.asset.id, 1).await.is_err());
            drop(conn);

            for sql in [
                "UPDATE recognitions SET text = 'changed'",
                "DELETE FROM recognitions",
            ] {
                assert!(
                    sqlx::query(sqlx::AssertSqlSafe(sql))
                        .execute(&pool)
                        .await
                        .is_err(),
                    "{sql}"
                );
            }
            for lines in ["{}", "not json"] {
                let refused = sqlx::query(
                    "INSERT INTO recognitions (asset_id, page, engine, model, text, lines, created_at)
                     VALUES (?1, 0, 'ocrs', 'm', '', ?2, ?3)",
                )
                .bind(picture.id)
                .bind(lines)
                .bind(NOW)
                .execute(&pool)
                .await;
                assert!(refused.is_err(), "{lines}");
            }
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM recognitions").await, 2);
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_picture_is_read_once_and_kept() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("recognition-picture");
            let picture = assets::add_picture(&pool, &store, &pages::page_png(), at(NOW))
                .await
                .unwrap();
            assert_eq!(latest(&pool, picture.id, 0).await.unwrap(), None);

            let found = recognise_picture(
                &pool,
                &store,
                helper(),
                picture.id,
                CancellationToken::new(),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(found.text, PAGE_TEXT);
            assert_eq!(found.engine, Engine::Ocrs);
            assert_eq!(found.model, ocr::MODEL);
            assert_eq!((found.width, found.height), (Some(1000), Some(330)));
            assert_eq!(found.lines.len(), 3);
            assert_eq!(
                latest(&pool, picture.id, 0).await.unwrap(),
                Some(found.clone())
            );

            // Asking again answers from what was kept: the helper isn't run.
            let broken = Helper {
                program: "no-such-program".into(),
                page_timeout: ocr::PAGE_TIMEOUT,
            };
            let again = recognise_picture(
                &pool,
                &store,
                broken,
                picture.id,
                CancellationToken::new(),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(again, found);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM recognitions").await, 1);
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn only_kept_pictures_are_read_as_pictures() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("recognition-not-pictures");
            let source = sources::create_source(&pool, &store, &scanned_pdf(), at(NOW))
                .await
                .unwrap();
            let stop = CancellationToken::new;
            assert!(matches!(
                recognise_picture(&pool, &store, helper(), 999, stop(), at(NOW)).await,
                Err(RecognitionError::NotFound)
            ));
            assert!(matches!(
                recognise_picture(&pool, &store, helper(), source.asset.id, stop(), at(NOW)).await,
                Err(RecognitionError::NotReadable)
            ));
            // A blank picture holds no text: read, and kept as empty.
            let mut blank = Vec::new();
            image::RgbImage::from_pixel(200, 100, image::Rgb([255, 255, 255]))
                .write_to(
                    &mut std::io::Cursor::new(&mut blank),
                    image::ImageFormat::Png,
                )
                .unwrap();
            let dot = assets::add_picture(&pool, &store, &blank, at(NOW))
                .await
                .unwrap();
            let read = recognise_picture(&pool, &store, helper(), dot.id, stop(), at(NOW))
                .await
                .unwrap();
            assert_eq!(read.text, "");
            assert!(read.lines.is_empty());
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn the_scanned_pages_of_a_source_are_read_page_by_page_and_quoted() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("recognition-source");
            let source = sources::create_source(&pool, &store, &scanned_pdf(), at(NOW))
                .await
                .unwrap();
            assert_eq!(pages_to_read(&pool, source.id).await.unwrap(), vec![2, 3]);

            let heard = StdMutex::new(Vec::new());
            let result = recognise_source(
                &pool,
                &store,
                helper(),
                source.id,
                None,
                CancellationToken::new(),
                || at(NOW),
                |progress| heard.lock().unwrap().push(progress),
            )
            .await
            .unwrap();
            assert_eq!(
                result,
                SourceRecognition {
                    read: vec![2],
                    skipped: vec![PageSkipped {
                        page: 3,
                        problem: PageProblem::UnreadablePicture
                    }],
                    stopped: false,
                }
            );
            assert_eq!(
                heard.into_inner().unwrap(),
                vec![
                    PageProgress {
                        page: 2,
                        done: 1,
                        total: 2,
                        problem: None
                    },
                    PageProgress {
                        page: 3,
                        done: 2,
                        total: 2,
                        problem: Some(PageProblem::UnreadablePicture)
                    },
                ]
            );

            // The page keeps what extraction read (nothing) and offers what
            // was recognised beside it.
            let read = sources::source_pages(&pool, source.id).await.unwrap();
            assert_eq!(read[0].recognised, None);
            assert_eq!(read[1].text, "");
            let recognised = read[1].recognised.as_ref().unwrap();
            assert_eq!(recognised.text, PAGE_TEXT);
            assert_eq!(recognised.engine, Engine::Ocrs);
            assert_eq!(read[1].readable(), PAGE_TEXT);
            assert_eq!(read[2].recognised, None);
            // The page that couldn't be read is still to read; page 2 isn't.
            assert_eq!(pages_to_read(&pool, source.id).await.unwrap(), vec![3]);

            // A quote from the recognised text is found on its page.
            let note =
                sources::create_note(&pool, source.id, "Scan", PAGE_TEXT, &[], None, at(NOW))
                    .await
                    .unwrap();
            let quote = "the Calvin cycle fixes carbon dioxide".to_string();
            let located = sources::locate_quotes(&pool, note.id, std::slice::from_ref(&quote))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(located.spots[0].unwrap().page, 2);
            let marked = sources::quoted_page(&pool, source.id, 2, &quote)
                .await
                .unwrap()
                .unwrap();
            let hit: Vec<&str> = marked
                .iter()
                .filter(|piece| piece.hit)
                .map(|piece| piece.text.as_str())
                .collect();
            assert_eq!(hit, vec!["The Calvin cycle fixes carbon dioxide"]);

            // Naming a page reads it again, adding a row; the newest is used.
            let again = recognise_source(
                &pool,
                &store,
                helper(),
                source.id,
                Some(vec![2, 2]),
                CancellationToken::new(),
                || at("2026-10-21T10:00:00Z"),
                |_| {},
            )
            .await
            .unwrap();
            assert_eq!(again.read, vec![2]);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM recognitions").await, 2);
            let newest = latest(&pool, source.asset.id, 2).await.unwrap().unwrap();
            assert_eq!(newest.created_at, "2026-10-21T10:00:00.000Z");
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_page_without_a_picture_is_kept_as_read_and_not_offered_again() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("recognition-no-picture");
            // Page 1 draws only vector text that extraction didn't find.
            let mut document = scanned_pdf();
            document.bytes = pages::pdf(&[TestPage::TextOnly, TestPage::Jpeg]);
            document.pages = vec![String::new(), String::new()];
            document.text.pages = Some(2);
            let source = sources::create_source(&pool, &store, &document, at(NOW))
                .await
                .unwrap();
            assert_eq!(pages_to_read(&pool, source.id).await.unwrap(), vec![1, 2]);
            let result = recognise_source(
                &pool,
                &store,
                helper(),
                source.id,
                None,
                CancellationToken::new(),
                || at(NOW),
                |_| {},
            )
            .await
            .unwrap();
            assert_eq!(result.read, vec![2]);
            assert_eq!(
                result.skipped,
                vec![PageSkipped {
                    page: 1,
                    problem: PageProblem::NoPicture
                }]
            );
            assert!(pages_to_read(&pool, source.id).await.unwrap().is_empty());
            let empty = latest(&pool, source.asset.id, 1).await.unwrap().unwrap();
            assert_eq!((empty.text.as_str(), empty.width), ("", None));
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn reading_a_source_is_refused_or_stopped_without_writing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("recognition-refused");
            let source = sources::create_source(&pool, &store, &scanned_pdf(), at(NOW))
                .await
                .unwrap();
            let run = |pages: Option<Vec<u32>>, stop: CancellationToken, id: i64| {
                let (pool, store) = (pool.clone(), store.clone());
                async move {
                    recognise_source(&pool, &store, helper(), id, pages, stop, || at(NOW), |_| {})
                        .await
                }
            };
            for pages in [vec![0], vec![4], vec![1, 9]] {
                assert!(matches!(
                    run(Some(pages), CancellationToken::new(), source.id).await,
                    Err(RecognitionError::NotReadable)
                ));
            }
            let text = sources::create_source(
                &pool,
                &store,
                &sources::test_support::text_document("Plain"),
                at(NOW),
            )
            .await
            .unwrap();
            assert!(matches!(
                run(None, CancellationToken::new(), text.id).await,
                Err(RecognitionError::NotReadable)
            ));
            assert!(matches!(
                run(None, CancellationToken::new(), 999).await,
                Err(RecognitionError::NotFound)
            ));

            // Stopped before the first page is done: nothing kept.
            let stop = CancellationToken::new();
            stop.cancel();
            let stopped = run(None, stop, source.id).await.unwrap();
            assert!(stopped.stopped);
            assert!(stopped.read.is_empty());

            sources::delete_source(&pool, source.id, at(NOW))
                .await
                .unwrap();
            assert!(matches!(
                run(None, CancellationToken::new(), source.id).await,
                Err(RecognitionError::Deleted)
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM recognitions").await, 0);
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn starting_a_recognition_stops_the_one_before() {
        let recognising = Recognising::default();
        let first = recognising.begin();
        let token = first.token();
        let second = recognising.begin();
        assert!(token.is_cancelled());
        assert!(!second.token().is_cancelled());
        drop(first);
        recognising.stop();
        assert!(second.token().is_cancelled());
    }
}
