//! Reading text with a vision model in Ollama: the second engine for text
//! recognition, for what the built-in one (`ocr.rs`) can't read, such as
//! Italian accents and handwriting.
//!
//! Only Ollama on this computer is asked, through `suggest.rs`'s client: the
//! same address, no proxies, no redirects, and only Ollama's local models
//! whose capabilities include "vision", checked again before a picture is
//! sent. Nothing is sent unless the user asks for it.
//!
//! The picture is prepared by the recognition helper (`ocr.rs`), as for the
//! built-in engine: a kept picture, or the picture a PDF page draws, is
//! decoded there and handed back as a PNG no larger than
//! [`ocr::MODEL_MAX_SIDE`], so PDF parsing and picture decoding never run in
//! the app itself.
//!
//! A model can invent text, so what it writes is checked before it is kept:
//! wrapping such as "Here is the text:" or a code fence is taken off, an
//! empty answer or one over a note's length is refused, and where the
//! built-in engine has read the same page, the share of its words the model's
//! text has too is given as a hint ([`agreement`]). The text is kept anyway,
//! as the newest reading, for the user to judge: it is always shown as
//! recognised.
//!
//! Nothing here logs the text read or the model's answer.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::assets::{self, MediaType};
use crate::extract::FileKind;
use crate::notes::NOTE_BODY_MAX_CHARS;
use crate::ocr::{self, Job, PageOutcome, PageProblem};
use crate::recognition::{
    self, Engine, Helper, PageProgress, PageSkipped, Recognition, RecognitionError, PICTURE_PAGE,
};
use crate::sources;
use crate::suggest::{Ollama, SuggestError};

/// The model's working space: the picture (a page at
/// [`ocr::MODEL_MAX_SIDE`] takes a few thousand tokens) and its answer.
const CONTEXT_TOKENS: u64 = 16_384;
/// Room left for the answer: a dense page fits with some to spare.
const ANSWER_TOKENS: u64 = 8_192;
/// How long a model may take over one picture. Vision models are slow
/// without a graphics card, so this is longer than for suggestions.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What the model is told to answer for a picture without text.
const NO_TEXT: &str = "[no text]";

const INSTRUCTIONS: &str = "\
You transcribe the text in pictures of pages, word for word.
Write out all the text you can see, in reading order, exactly as it is written: keep its \
language, spelling, accents, capitals, punctuation and numbers, and put a blank line between \
paragraphs. This includes handwriting.
Do not translate, correct, complete, summarise or explain the text, and do not describe the \
picture. Add nothing of your own: no introduction, no comments, no Markdown, no code fences.
If a word is hard to read, write your best reading of it.
If there is no text in the picture, answer exactly: [no text]";

/// Why a vision model's reading wasn't kept.
#[derive(Debug)]
pub enum VisionError {
    /// The picture or source couldn't be prepared, or the user stopped.
    Recognition(RecognitionError),
    /// Ollama couldn't be asked, or the model couldn't answer.
    Ollama(SuggestError),
    /// The model answered nothing, or only wrapping around nothing.
    Empty,
    /// The answer was cut off, or is longer than a note can be.
    TooLong,
}

impl From<RecognitionError> for VisionError {
    fn from(err: RecognitionError) -> Self {
        VisionError::Recognition(err)
    }
}

impl From<sqlx::Error> for VisionError {
    fn from(err: sqlx::Error) -> Self {
        VisionError::Recognition(err.into())
    }
}

fn internal(detail: crate::db::DbError) -> VisionError {
    VisionError::Recognition(RecognitionError::Internal(detail))
}

/// What a vision model read from a picture: `{ recognition, agreement }`.
/// `agreement` is [`agreement`] with the built-in engine's reading of the
/// same picture, or null when it hasn't read it (or found no words).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReading {
    pub recognition: Recognition,
    pub agreement: Option<u8>,
}

/// One page's [`agreement`]: `{ page, percent }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageAgreement {
    pub page: u32,
    pub percent: u8,
}

/// How reading a source's pages with a model went: `{ read, skipped,
/// stopped, agreement }`, as the built-in engine's `SourceRecognition`, plus
/// the agreement of each page read that the built-in engine had read too.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSourceReading {
    pub read: Vec<u32>,
    pub skipped: Vec<PageSkipped>,
    pub stopped: bool,
    pub agreement: Vec<PageAgreement>,
}

/// The text in a model's answer, without the wrapping models add despite
/// being told not to: a code fence, or a first line such as "Here is the
/// text:". `Some("")` when the model said there is no text; `None` when
/// nothing is left.
pub fn transcription(answer: &str) -> Option<String> {
    let mut text = answer.trim();
    // A fence may come inside an introduction, or the other way round.
    for _ in 0..2 {
        text = without_introduction(without_fence(text));
    }
    let bare = text
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '.' || c.is_whitespace())
        .to_lowercase();
    if bare == NO_TEXT || bare == NO_TEXT.trim_matches(['[', ']']) {
        return Some(String::new());
    }
    (!text.is_empty()).then(|| text.to_string())
}

/// `text` without a code fence around all of it.
fn without_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // The rest of the opening line names a language, if anything.
    let body = rest.split_once('\n').map_or("", |(_, body)| body);
    let body = body.trim_end();
    body.strip_suffix("```").unwrap_or(body).trim()
}

/// `text` without a first line that only introduces it: short, ending in a
/// colon, and saying it is the text or a transcription.
fn without_introduction(text: &str) -> &str {
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let line = first.trim().to_lowercase();
    let introduces = line.ends_with(':')
        && line.chars().count() <= 80
        && [
            "here",
            "transcri",
            "text",
            "sure",
            "certainly",
            "ecco",
            "testo",
        ]
        .iter()
        .any(|word| line.contains(word));
    if introduces {
        rest.trim()
    } else {
        text
    }
}

/// `text`'s words, lowercased and without accents, since the built-in engine
/// reads "è" as "e" (and some accented letters as "?", which splits a word).
fn plain_words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase().chars().map(unaccented).collect())
        .collect()
}

fn unaccented(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

/// The share of the built-in engine's distinct words, as a whole percentage,
/// that the model's text has too, ignoring case and accents: a hint at
/// whether the model read the same page, not a measure of which is right.
/// Words the built-in engine read with a "?" (an accented letter it couldn't
/// read) don't count. None when the built-in engine found no other words.
pub fn agreement(model_text: &str, builtin_text: &str) -> Option<u8> {
    let readable: Vec<&str> = builtin_text
        .split_whitespace()
        .filter(|word| !word.contains('?'))
        .collect();
    let builtin = plain_words(&readable.join(" "));
    if builtin.is_empty() {
        return None;
    }
    let model = plain_words(model_text);
    let shared = builtin.iter().filter(|word| model.contains(*word)).count();
    Some(((shared * 100) as f64 / builtin.len() as f64).round() as u8)
}

/// What the built-in engine last read on a page of a file, if it has.
async fn builtin_text(
    pool: &SqlitePool,
    asset_id: i64,
    page: u32,
) -> Result<Option<String>, VisionError> {
    Ok(sqlx::query_scalar(
        "SELECT text FROM recognitions WHERE asset_id = ?1 AND page = ?2 AND engine = ?3
         ORDER BY id DESC LIMIT 1",
    )
    .bind(asset_id)
    .bind(i64::from(page))
    .bind(Engine::Ocrs.as_str())
    .fetch_optional(pool)
    .await?)
}

async fn agreement_on(
    pool: &SqlitePool,
    asset_id: i64,
    page: u32,
    model_text: &str,
) -> Result<Option<u8>, VisionError> {
    Ok(builtin_text(pool, asset_id, page)
        .await?
        .and_then(|builtin| agreement(model_text, &builtin)))
}

/// Has the helper carry out `job`, which answers one page, and returns that
/// page's outcome.
async fn prepare(
    helper: Helper,
    job: Job,
    bytes: Vec<u8>,
    stop: CancellationToken,
) -> Result<PageOutcome, VisionError> {
    let run = tauri::async_runtime::spawn_blocking(move || {
        let mut answer = None;
        ocr::run_helper(
            &helper.program,
            &job,
            &bytes,
            &stop,
            helper.page_timeout,
            |_, outcome| answer = Some(outcome),
        )
        .map(|()| answer)
    });
    run.await
        .map_err(|err| internal(format!("the helper's thread: {err}").into()))?
        .map_err(|err| VisionError::Recognition(recognition::run_error(err)))?
        .ok_or_else(|| internal("the helper answered nothing".into()))
}

/// Asks `model` to read the PNG `png` and checks its answer, or None if
/// `stop` is cancelled first (which drops the request, and Ollama stops).
async fn ask(
    ollama: &Ollama,
    model: &str,
    png: &[u8],
    stop: &CancellationToken,
) -> Result<Option<String>, VisionError> {
    let asking = ollama.read_picture(
        model,
        INSTRUCTIONS,
        png,
        CONTEXT_TOKENS,
        ANSWER_TOKENS,
        READ_TIMEOUT,
    );
    let Some(written) = stop.run_until_cancelled(asking).await else {
        return Ok(None);
    };
    let written = written.map_err(VisionError::Ollama)?;
    if written.ran_out {
        return Err(VisionError::TooLong);
    }
    let text = transcription(&written.content).ok_or(VisionError::Empty)?;
    if text.chars().count() > NOTE_BODY_MAX_CHARS {
        return Err(VisionError::TooLong);
    }
    Ok(Some(text))
}

async fn keep(
    pool: &SqlitePool,
    asset_id: i64,
    page: u32,
    model: &str,
    text: &str,
    now: DateTime<Utc>,
) -> Result<Recognition, VisionError> {
    let mut conn = pool.acquire().await?;
    recognition::insert(
        &mut conn,
        asset_id,
        page,
        Engine::Ollama,
        model,
        text,
        &[],
        None,
        now,
    )
    .await
    .map_err(internal)
}

/// Reads the kept picture `asset_id` with the vision model `model` and keeps
/// the text as the picture's newest reading. A picture whose reading in use
/// is already this model's isn't sent again: that reading is returned.
#[allow(clippy::too_many_arguments)]
pub async fn read_picture(
    pool: &SqlitePool,
    store: &assets::AssetStore,
    helper: Helper,
    ollama: &Ollama,
    asset_id: i64,
    model: &str,
    stop: CancellationToken,
    now: DateTime<Utc>,
) -> Result<ModelReading, VisionError> {
    let asset = {
        let mut conn = pool.acquire().await?;
        assets::asset_by_id(&mut conn, asset_id)
            .await
            .map_err(internal)?
            .ok_or(RecognitionError::NotFound)?
    };
    if !asset.media_type.is_picture() {
        return Err(RecognitionError::NotReadable.into());
    }
    let in_use = recognition::latest(pool, asset_id, PICTURE_PAGE)
        .await
        .map_err(internal)?;
    let recognition = match in_use {
        Some(found) if found.engine == Engine::Ollama && found.model == model => found,
        _ => {
            let bytes = recognition::kept_bytes(store, &asset)?;
            let png = match prepare(helper, Job::PictureForModel, bytes, stop.clone()).await? {
                PageOutcome::Picture(png) => png,
                PageOutcome::Problem(problem) => {
                    return Err(RecognitionError::Picture(problem).into())
                }
                PageOutcome::Read(_) => {
                    return Err(internal("the helper read the picture itself".into()))
                }
            };
            let text = ask(ollama, model, &png, &stop)
                .await?
                .ok_or(RecognitionError::Stopped)?;
            keep(pool, asset_id, PICTURE_PAGE, model, &text, now).await?
        }
    };
    let agreement = agreement_on(pool, asset_id, PICTURE_PAGE, &recognition.text).await?;
    Ok(ModelReading {
        recognition,
        agreement,
    })
}

/// The pages of the active PDF source `source_id` that reading with `model`
/// would cover when no pages are named: those whose extracted text is blank
/// and whose reading in use isn't already this model's, nor the empty
/// reading kept for a page found to draw no picture.
pub async fn pages_to_read(
    pool: &SqlitePool,
    source_id: i64,
    model: &str,
) -> Result<Vec<u32>, VisionError> {
    let source = sources::source(pool, source_id)
        .await
        .map_err(RecognitionError::from)?;
    if source.kind != FileKind::Pdf {
        return Err(RecognitionError::NotReadable.into());
    }
    let done = {
        let mut conn = pool.acquire().await?;
        recognition::latest_pages(&mut conn, source.asset.id)
            .await
            .map_err(internal)?
    };
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
        .filter(|page| {
            done.get(page).is_none_or(|found| {
                !no_picture(found) && (found.engine != Engine::Ollama || found.model != model)
            })
        })
        .collect())
}

/// Whether `found` is the empty reading kept for a page that draws no
/// picture (`recognition::recognise_source`): no text and no size.
fn no_picture(found: &Recognition) -> bool {
    found.engine == Engine::Ocrs && found.width.is_none() && found.text.is_empty()
}

/// Reads pages of the active PDF source `source_id` with the vision model
/// `model`, page by page: `pages` in the order given, or, when None, every
/// page [`pages_to_read`] finds. Each page is kept as its newest reading as
/// soon as it is read, and reported to `progress`. Stopping keeps the pages
/// done and returns with `stopped`; a failure keeps them too.
#[allow(clippy::too_many_arguments)]
pub async fn read_source(
    pool: &SqlitePool,
    store: &assets::AssetStore,
    helper: Helper,
    ollama: &Ollama,
    source_id: i64,
    pages: Option<Vec<u32>>,
    model: &str,
    stop: CancellationToken,
    now: impl Fn() -> DateTime<Utc>,
    progress: impl Fn(PageProgress),
) -> Result<ModelSourceReading, VisionError> {
    let source = sources::source(pool, source_id)
        .await
        .map_err(RecognitionError::from)?;
    if source.kind != FileKind::Pdf || source.asset.media_type != MediaType::Pdf {
        return Err(RecognitionError::NotReadable.into());
    }
    let pages = match pages {
        Some(pages) => recognition::pages_asked(pages, source.page_count)?,
        None => pages_to_read(pool, source_id, model).await?,
    };
    let mut result = ModelSourceReading::default();
    if pages.is_empty() {
        return Ok(result);
    }
    let bytes = recognition::kept_bytes(store, &source.asset)?;
    let total = pages.len() as u32;
    for (index, page) in pages.into_iter().enumerate() {
        let job = Job::PageForModel { page };
        let outcome = match prepare(helper.clone(), job, bytes.clone(), stop.clone()).await {
            Err(VisionError::Recognition(RecognitionError::Stopped)) => {
                result.stopped = true;
                return Ok(result);
            }
            outcome => outcome?,
        };
        let problem = match outcome {
            PageOutcome::Problem(problem) => {
                // As the built-in engine does: a page that draws no picture
                // has nothing to read, so it isn't offered again.
                if problem == PageProblem::NoPicture {
                    let mut conn = pool.acquire().await?;
                    recognition::insert(
                        &mut conn,
                        source.asset.id,
                        page,
                        Engine::Ocrs,
                        ocr::MODEL,
                        "",
                        &[],
                        None,
                        now(),
                    )
                    .await
                    .map_err(internal)?;
                }
                result.skipped.push(PageSkipped { page, problem });
                Some(problem)
            }
            PageOutcome::Read(_) => return Err(internal("the helper read the page itself".into())),
            PageOutcome::Picture(png) => {
                let Some(text) = ask(ollama, model, &png, &stop).await? else {
                    result.stopped = true;
                    return Ok(result);
                };
                keep(pool, source.asset.id, page, model, &text, now()).await?;
                if let Some(percent) = agreement_on(pool, source.asset.id, page, &text).await? {
                    result.agreement.push(PageAgreement { page, percent });
                }
                result.read.push(page);
                None
            }
        };
        progress(PageProgress {
            page,
            done: index as u32 + 1,
            total,
            problem,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::test_support::*;
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::extract::{Document, ExtractedText};
    use crate::ocr::test_support::{self as pages, TestPage};
    use crate::ocr::PageProblem;
    use crate::suggest::tests::{chat, fake_ollama};
    use serde_json::{json, Value};
    use std::path::Path;
    use std::sync::Mutex;

    const NOW: &str = "2026-10-21T09:00:00Z";
    const MODEL: &str = "qwen2.5vl:3b";
    const ITALIAN: &str =
        "La fotosintesi è il processo più importante.\n\nPerché la luce è energia.";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    /// The Synapse binary, which `cargo test` builds beside the test binary.
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

    fn ollama(base: &str) -> Ollama {
        Ollama::at(base, Duration::from_secs(5)).unwrap()
    }

    /// Ollama's model list: `vision` can read pictures, `text` can't.
    fn tags(vision: &[&str], text: &[&str]) -> String {
        let mut models: Vec<Value> = vision
            .iter()
            .map(|name| json!({ "name": name, "capabilities": ["completion", "vision"] }))
            .collect();
        models.extend(
            text.iter()
                .map(|name| json!({ "name": name, "capabilities": ["completion"] })),
        );
        json!({ "models": models }).to_string()
    }

    fn body_of(request: &str) -> Value {
        serde_json::from_str(request.split_once('\n').unwrap().1).unwrap()
    }

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
    fn wrapping_is_taken_off_the_models_answer() {
        assert_eq!(transcription(ITALIAN).as_deref(), Some(ITALIAN));
        for wrapped in [
            format!("Here is the text from the picture:\n\n{ITALIAN}"),
            format!("```\n{ITALIAN}\n```"),
            format!("```text\n{ITALIAN}\n```"),
            format!("Sure! Here's the transcription:\n```\n{ITALIAN}\n```\n"),
            format!("Ecco il testo:\n{ITALIAN}"),
            format!("  \n{ITALIAN}\n  "),
        ] {
            assert_eq!(
                transcription(&wrapped).as_deref(),
                Some(ITALIAN),
                "{wrapped}"
            );
        }
        // A first line that is the page's own heading stays.
        let heading = "Chapter 2: Light\nPlants use light.";
        assert_eq!(transcription(heading).as_deref(), Some(heading));
        let label = "Text:\nPlants use light.";
        assert_eq!(transcription(label).as_deref(), Some("Plants use light."));

        for nothing in ["[no text]", "[No text].", "\"[no text]\"", "no text"] {
            assert_eq!(transcription(nothing).as_deref(), Some(""), "{nothing}");
        }
        for empty in ["", "   ", "```\n```", "Here is the text:\n"] {
            assert_eq!(transcription(empty), None, "{empty:?}");
        }
    }

    #[test]
    fn agreement_is_the_share_of_the_built_in_words_ignoring_case_and_accents() {
        // The built-in engine reads "è" as "e", and some accents as "?".
        let builtin = "La fotosintesi e il processo piu importante.\n\nPerch? la luce e energia.";
        // "Perch?" doesn't count; every other word is in the model's text.
        assert_eq!(agreement(ITALIAN, builtin), Some(100));
        // One of five words misread.
        assert_eq!(agreement(ITALIAN, "La fotosintesl e il processo"), Some(80));
        assert_eq!(agreement(ITALIAN, "LA FOTOSINTESI"), Some(100));
        assert_eq!(agreement("Something else entirely", builtin), Some(0));
        assert_eq!(agreement(ITALIAN, ""), None);
        assert_eq!(agreement(ITALIAN, " ... "), None);
        assert_eq!(agreement(ITALIAN, "pi? perch?"), None);
    }

    #[test]
    fn a_large_picture_is_scaled_down_before_a_model_sees_it() {
        let picture = image::RgbImage::new(3_200, 1_000);
        let png = ocr::picture_for_model(picture).unwrap();
        let sent = image::load_from_memory(&png).unwrap();
        assert_eq!((sent.width(), sent.height()), (ocr::MODEL_MAX_SIDE, 500));
    }

    #[test]
    fn only_local_models_that_read_pictures_are_offered() {
        tauri::async_runtime::block_on(async {
            let listing = json!({ "models": [
                { "name": "qwen2.5vl:7b", "capabilities": ["completion", "vision"] },
                { "name": "llama3.2:3b", "capabilities": ["completion", "tools"] },
                { "name": "Gemma3:4b", "capabilities": ["completion", "vision"] },
                { "name": "qwen3-vl:235b-cloud", "capabilities": ["completion", "vision"] },
                { "name": "remote-vision", "remote_host": "https://ollama.com",
                  "capabilities": ["vision"] },
                // Older Ollama lists no capabilities: asked with /api/show.
                { "name": "minicpm-v:8b" },
                { "name": "phi3:mini" }
            ]});
            let (base, server) = fake_ollama(vec![
                (200, listing.to_string()),
                (
                    200,
                    json!({ "capabilities": ["completion", "vision"] }).to_string(),
                ),
                (200, json!({ "capabilities": ["completion"] }).to_string()),
            ]);
            assert_eq!(
                ollama(&base).vision_models().await.unwrap(),
                ["Gemma3:4b", "minicpm-v:8b", "qwen2.5vl:7b"]
            );
            let requests = server.join().unwrap();
            assert!(requests[0].starts_with("GET /api/tags"));
            assert!(requests[1].starts_with("POST /api/show"));
            assert_eq!(body_of(&requests[1]), json!({ "model": "minicpm-v:8b" }));
            assert_eq!(body_of(&requests[2]), json!({ "model": "phi3:mini" }));
        });
    }

    #[test]
    fn a_picture_is_sent_to_the_chosen_model_and_its_text_kept_as_the_newest_reading() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("vision-picture");
            let picture = assets::add_picture(&pool, &store, &pages::page_png(), at(NOW))
                .await
                .unwrap();
            // The built-in engine read it first, without accents.
            {
                let mut conn = pool.acquire().await.unwrap();
                recognition::insert(
                    &mut conn,
                    picture.id,
                    PICTURE_PAGE,
                    Engine::Ocrs,
                    ocr::MODEL,
                    "La fotosintesi e il processo piu importante.",
                    &[],
                    None,
                    at(NOW),
                )
                .await
                .unwrap();
            }
            let (base, server) = fake_ollama(vec![
                (200, tags(&[MODEL], &["llama3.2:3b"])),
                (
                    200,
                    chat(&format!("Here is the text:\n```\n{ITALIAN}\n```")),
                ),
            ]);
            let read = read_picture(
                &pool,
                &store,
                helper(),
                &ollama(&base),
                picture.id,
                MODEL,
                CancellationToken::new(),
                at("2026-10-21T09:05:00Z"),
            )
            .await
            .unwrap();
            assert_eq!(read.recognition.engine, Engine::Ollama);
            assert_eq!(read.recognition.model, MODEL);
            assert_eq!(read.recognition.text, ITALIAN);
            assert!(read.recognition.lines.is_empty());
            assert_eq!(read.recognition.width, None);
            assert_eq!(read.agreement, Some(100));
            assert_eq!(
                recognition::latest(&pool, picture.id, PICTURE_PAGE)
                    .await
                    .unwrap(),
                Some(read.recognition.clone())
            );

            let requests = server.join().unwrap();
            assert!(requests[1].starts_with("POST /api/chat"));
            let body = body_of(&requests[1]);
            assert_eq!(body["model"], MODEL);
            assert_eq!(body["stream"], false);
            assert_eq!(body["options"]["temperature"], 0);
            assert!(body.get("format").is_none());
            let images = body["messages"][1]["images"].as_array().unwrap();
            assert_eq!(images.len(), 1);
            use base64::Engine as _;
            let png = base64::engine::general_purpose::STANDARD
                .decode(images[0].as_str().unwrap())
                .unwrap();
            assert!(png.starts_with(b"\x89PNG"));

            // The built-in engine, asked again, still gives its own reading,
            // with the positions a model doesn't give.
            let builtin = recognition::recognise_picture(
                &pool,
                &store,
                helper(),
                picture.id,
                CancellationToken::new(),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(builtin.engine, Engine::Ocrs);
            assert_eq!(builtin.text, "La fotosintesi e il processo piu importante.");

            // Asking again with the same model doesn't send it again.
            let (base, server) = fake_ollama(vec![]);
            let again = read_picture(
                &pool,
                &store,
                helper(),
                &ollama(&base),
                picture.id,
                MODEL,
                CancellationToken::new(),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(again, read);
            assert!(server.join().unwrap().is_empty());
        });
    }

    #[test]
    fn nothing_is_sent_to_a_model_that_cannot_read_pictures_and_nothing_kept_from_a_bad_answer() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("vision-refusals");
            let picture = assets::add_picture(&pool, &store, &pages::page_png(), at(NOW))
                .await
                .unwrap();
            let read = async |replies: Vec<(u16, String)>, model: &str| {
                let (base, server) = fake_ollama(replies);
                let result = read_picture(
                    &pool,
                    &store,
                    helper(),
                    &ollama(&base),
                    picture.id,
                    model,
                    CancellationToken::new(),
                    at(NOW),
                )
                .await;
                (result, server.join().unwrap())
            };

            let (result, requests) =
                read(vec![(200, tags(&[MODEL], &["llama3.2:3b"]))], "llama3.2:3b").await;
            assert!(matches!(
                result,
                Err(VisionError::Ollama(SuggestError::ModelMissing))
            ));
            assert_eq!(requests.len(), 1);

            let (result, _) = read(
                vec![
                    (200, tags(&[MODEL], &[])),
                    (200, chat("Here is the text:\n")),
                ],
                MODEL,
            )
            .await;
            assert!(matches!(result, Err(VisionError::Empty)), "{result:?}");

            let long = "parola ".repeat(NOTE_BODY_MAX_CHARS / 7 + 1);
            let (result, _) =
                read(vec![(200, tags(&[MODEL], &[])), (200, chat(&long))], MODEL).await;
            assert!(matches!(result, Err(VisionError::TooLong)), "{result:?}");

            let cut_off = json!({
                "message": { "role": "assistant", "content": "La fotosintesi" },
                "done_reason": "length",
                "prompt_eval_count": 900
            });
            let (result, _) = read(
                vec![(200, tags(&[MODEL], &[])), (200, cut_off.to_string())],
                MODEL,
            )
            .await;
            assert!(matches!(result, Err(VisionError::TooLong)), "{result:?}");

            assert_eq!(
                recognition::latest(&pool, picture.id, PICTURE_PAGE)
                    .await
                    .unwrap(),
                None
            );

            // A file that isn't a picture is refused before anything is asked.
            let source = sources::create_source(&pool, &store, &scanned_pdf(), at(NOW))
                .await
                .unwrap();
            let (base, server) = fake_ollama(vec![]);
            let result = read_picture(
                &pool,
                &store,
                helper(),
                &ollama(&base),
                source.asset.id,
                MODEL,
                CancellationToken::new(),
                at(NOW),
            )
            .await;
            assert!(matches!(
                result,
                Err(VisionError::Recognition(RecognitionError::NotReadable))
            ));
            assert!(server.join().unwrap().is_empty());
        });
    }

    #[test]
    fn a_models_reading_of_nothing_is_kept_as_empty() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("vision-no-text");
            let mut blank = Vec::new();
            image::RgbImage::from_pixel(200, 100, image::Rgb([255, 255, 255]))
                .write_to(
                    &mut std::io::Cursor::new(&mut blank),
                    image::ImageFormat::Png,
                )
                .unwrap();
            let picture = assets::add_picture(&pool, &store, &blank, at(NOW))
                .await
                .unwrap();
            let (base, _server) =
                fake_ollama(vec![(200, tags(&[MODEL], &[])), (200, chat("[no text]"))]);
            let read = read_picture(
                &pool,
                &store,
                helper(),
                &ollama(&base),
                picture.id,
                MODEL,
                CancellationToken::new(),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(read.recognition.text, "");
            assert_eq!(read.agreement, None);
        });
    }

    #[test]
    fn a_sources_scanned_pages_are_read_one_by_one_and_kept_as_they_are_read() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("vision-source");
            let source = sources::create_source(&pool, &store, &scanned_pdf(), at(NOW))
                .await
                .unwrap();
            assert_eq!(
                pages_to_read(&pool, source.id, MODEL).await.unwrap(),
                [2, 3]
            );

            // Page 3's picture can't be decoded, so only page 2 is sent.
            let (base, server) =
                fake_ollama(vec![(200, tags(&[MODEL], &[])), (200, chat(ITALIAN))]);
            let heard = Mutex::new(Vec::new());
            let result = read_source(
                &pool,
                &store,
                helper(),
                &ollama(&base),
                source.id,
                None,
                MODEL,
                CancellationToken::new(),
                || at(NOW),
                |progress| heard.lock().unwrap().push(progress),
            )
            .await
            .unwrap();
            assert_eq!(
                result,
                ModelSourceReading {
                    read: vec![2],
                    skipped: vec![PageSkipped {
                        page: 3,
                        problem: PageProblem::UnreadablePicture
                    }],
                    stopped: false,
                    agreement: Vec::new(),
                }
            );
            assert_eq!(
                heard.into_inner().unwrap(),
                [
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
            assert_eq!(server.join().unwrap().len(), 2);
            let kept = recognition::latest(&pool, source.asset.id, 2)
                .await
                .unwrap()
                .unwrap();
            assert_eq!((kept.engine, kept.text.as_str()), (Engine::Ollama, ITALIAN));
            // The source's pages now offer the model's text where extraction read none.
            let shown = sources::source_pages(&pool, source.id).await.unwrap();
            assert_eq!(shown[1].readable(), ITALIAN);
            assert_eq!(pages_to_read(&pool, source.id, MODEL).await.unwrap(), [3]);
            assert_eq!(
                pages_to_read(&pool, source.id, "gemma3:4b").await.unwrap(),
                [2, 3]
            );

            // Read again by name, after the built-in engine: agreement is given.
            {
                let mut conn = pool.acquire().await.unwrap();
                recognition::insert(
                    &mut conn,
                    source.asset.id,
                    2,
                    Engine::Ocrs,
                    ocr::MODEL,
                    "Perch? la luce e energia. Clorofilla",
                    &[],
                    None,
                    at(NOW),
                )
                .await
                .unwrap();
            }
            let (base, _server) =
                fake_ollama(vec![(200, tags(&[MODEL], &[])), (200, chat(ITALIAN))]);
            let result = read_source(
                &pool,
                &store,
                helper(),
                &ollama(&base),
                source.id,
                Some(vec![2, 2]),
                MODEL,
                CancellationToken::new(),
                || at(NOW),
                |_| {},
            )
            .await
            .unwrap();
            assert_eq!(result.read, [2]);
            // Four of the five built-in words that count: not "clorofilla".
            assert_eq!(
                result.agreement,
                [PageAgreement {
                    page: 2,
                    percent: 80
                }]
            );

            for pages in [vec![0], vec![4]] {
                let (base, _server) = fake_ollama(vec![]);
                let refused = read_source(
                    &pool,
                    &store,
                    helper(),
                    &ollama(&base),
                    source.id,
                    Some(pages),
                    MODEL,
                    CancellationToken::new(),
                    || at(NOW),
                    |_| {},
                )
                .await;
                assert!(matches!(
                    refused,
                    Err(VisionError::Recognition(RecognitionError::NotReadable))
                ));
            }
        });
    }

    #[test]
    fn a_page_without_a_picture_is_kept_as_read_by_either_engine() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("vision-no-picture");
            // Page 1 draws only vector text that extraction didn't find.
            let mut document = scanned_pdf();
            document.bytes = pages::pdf(&[TestPage::TextOnly, TestPage::Jpeg]);
            document.pages = vec![String::new(), String::new()];
            document.text.pages = Some(2);
            let source = sources::create_source(&pool, &store, &document, at(NOW))
                .await
                .unwrap();
            assert_eq!(
                pages_to_read(&pool, source.id, MODEL).await.unwrap(),
                [1, 2]
            );
            let (base, server) =
                fake_ollama(vec![(200, tags(&[MODEL], &[])), (200, chat(ITALIAN))]);
            let result = read_source(
                &pool,
                &store,
                helper(),
                &ollama(&base),
                source.id,
                None,
                MODEL,
                CancellationToken::new(),
                || at(NOW),
                |_| {},
            )
            .await
            .unwrap();
            assert_eq!(result.read, [2]);
            assert_eq!(
                result.skipped,
                [PageSkipped {
                    page: 1,
                    problem: PageProblem::NoPicture
                }]
            );
            assert_eq!(server.join().unwrap().len(), 2);
            assert!(pages_to_read(&pool, source.id, MODEL)
                .await
                .unwrap()
                .is_empty());
            // Another model is still offered the page with a picture.
            assert_eq!(
                pages_to_read(&pool, source.id, "gemma3:4b").await.unwrap(),
                [2]
            );
            assert_eq!(
                recognition::pages_to_read(&pool, source.id).await.unwrap(),
                [2]
            );
        });
    }

    #[test]
    fn stopping_keeps_the_pages_already_read_and_sends_no_more() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("vision-stop");
            let source = sources::create_source(&pool, &store, &scanned_pdf(), at(NOW))
                .await
                .unwrap();
            let stop = CancellationToken::new();
            // Ollama never answers the chat (status 0 holds it open), and the
            // user stops while it's waiting.
            let (base, server) = fake_ollama(vec![(200, tags(&[MODEL], &[])), (0, String::new())]);
            let stopping = stop.clone();
            let asking = ollama(&base);
            let result = read_source(
                &pool,
                &store,
                helper(),
                &asking,
                source.id,
                Some(vec![2, 3]),
                MODEL,
                stop,
                || at(NOW),
                |_| {},
            );
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                stopping.cancel();
            });
            let result = result.await.unwrap();
            assert!(result.stopped);
            assert!(result.read.is_empty());
            assert_eq!(server.join().unwrap().len(), 2);
            assert_eq!(
                recognition::latest(&pool, source.asset.id, 2)
                    .await
                    .unwrap(),
                None
            );
        });
    }

    /// Reads an Italian page with the built-in engine and with a real vision
    /// model, and prints both, to judge the model's reading. Run with
    /// Ollama running and the model pulled:
    ///
    /// ```text
    /// SYNAPSE_TEST_VISION_MODEL=qwen2.5vl:3b cargo test --locked \
    ///     real_ollama_reads_an_italian_page -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs Ollama running locally, with a vision model"]
    fn real_ollama_reads_an_italian_page() {
        let model = std::env::var("SYNAPSE_TEST_VISION_MODEL")
            .expect("set SYNAPSE_TEST_VISION_MODEL to a vision model installed in Ollama");
        let page = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/ocr/italiano.png"
        ))
        .unwrap();
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("vision-real");
            let picture = assets::add_picture(&pool, &store, &page, at(NOW))
                .await
                .unwrap();
            let builtin = recognition::recognise_picture(
                &pool,
                &store,
                helper(),
                picture.id,
                CancellationToken::new(),
                at(NOW),
            )
            .await
            .unwrap();
            let ollama = Ollama::local().unwrap();
            assert!(ollama.vision_models().await.unwrap().contains(&model));
            let started = std::time::Instant::now();
            let read = read_picture(
                &pool,
                &store,
                helper(),
                &ollama,
                picture.id,
                &model,
                CancellationToken::new(),
                at("2026-10-21T09:05:00Z"),
            )
            .await
            .unwrap();
            eprintln!("built in:\n{}\n", builtin.text);
            eprintln!(
                "{model}, {:.0?}, agreement {:?}:\n{}",
                started.elapsed(),
                read.agreement,
                read.recognition.text
            );
            assert!(!read.recognition.text.is_empty());
        });
    }
}
