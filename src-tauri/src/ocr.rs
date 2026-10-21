//! Reading text in pictures and scanned pages: optical character recognition
//! with the engine built into Synapse.
//!
//! The engine is [ocrs](https://github.com/robertknight/ocrs): pure Rust, run
//! on this computer, with its two neural network models (finding text, then
//! reading it) bundled in `models/` and compiled into the program. Nothing is
//! downloaded and nothing leaves the computer. It reads the Latin alphabet
//! without accents: "è" comes out as "e", and some accented letters as "?".
//! Handwriting is read poorly. Results are always shown as recognised text,
//! for the user to check.
//!
//! What can be read:
//! - A picture Synapse keeps (PNG, JPEG, GIF, WebP; `assets.rs`).
//! - A page of a PDF that is a scan: the largest picture drawn on that page
//!   is read. JPEG, uncompressed or Flate-compressed samples (grey, RGB,
//!   CMYK), and black-and-white CCITT Group 4 scans are understood; JPEG 2000
//!   and JBIG2 are reported as pictures Synapse can't read. Text a PDF draws
//!   as text is extraction's job (`extract.rs`), not this one's.
//!
//! Recognition runs in a helper process, as reading a PDF's text does: Synapse
//! started again with [`OCR_HELPER_ARG`]. A picture built to crash a decoder,
//! or a page that takes too long, stops the helper, not the app; and stopping
//! a recognition the user cancelled is ending that process. The helper reads
//! a job and the file's bytes from standard input and writes one JSON line
//! per page as it finishes, so the app can save each page as it comes and
//! show progress.
//!
//! Nothing here logs the text read.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use tokio_util::sync::CancellationToken;

use crate::extract;

/// The argument that makes Synapse run as the recognition helper (`main.rs`).
pub const OCR_HELPER_ARG: &str = "--recognise-text";

/// The engine's name, as `recognitions.engine` stores it.
pub const ENGINE: &str = "ocrs";

/// Which models read a page, as `recognitions.model` stores it: the engine's
/// version and the start of each bundled model's SHA-256. A later model gives
/// a different name, so its results can be told apart.
pub const MODEL: &str = "ocrs 0.13 (detection f15cfb56, recognition e484866d)";

static DETECTION_MODEL: &[u8] = include_bytes!("../models/text-detection.rten");
static RECOGNITION_MODEL: &[u8] = include_bytes!("../models/text-recognition.rten");

/// The SHA-256 of each bundled model, checked by a test so a changed file
/// can't go unnoticed (and [`MODEL`] must change with it).
pub const DETECTION_MODEL_SHA256: &str =
    "f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca";
pub const RECOGNITION_MODEL_SHA256: &str =
    "e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e";

/// The most bytes the helper reads from standard input: a kept file is at
/// most 64 MiB (`assets::MAX_ASSET_BYTES`), plus the job line.
const HELPER_INPUT_MAX_BYTES: u64 = 64 * 1024 * 1024 + 64 * 1024;

/// The largest picture decoded, in pixels: a 600 dpi A3 scan is about 70
/// million. Anything larger is refused rather than decoded.
pub const MAX_PIXELS: u64 = 80_000_000;

/// A picture whose longer side is over this many pixels is scaled down to it
/// before reading. That keeps a page to a few seconds; text on a page this
/// size is still dozens of pixels high.
pub const MAX_SIDE: u32 = 4_000;

/// The most lines kept from one picture: far more than any page holds.
pub const MAX_LINES: usize = 2_000;

/// How long the helper may spend on one page before it's stopped.
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(90);

/// A rectangle on a picture, in fractions (0 to 1) of its width and height,
/// rounded to 4 decimals: `{ left, top, width, height }`. The same form as an
/// image occlusion box (`occlusion.rs`), so a line can become one.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBox {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// One word read: `{ text, box }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub text: String,
    #[serde(rename = "box")]
    pub bounds: TextBox,
}

/// One line read, in reading order: `{ text, box, words }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub text: String,
    #[serde(rename = "box")]
    pub bounds: TextBox,
    pub words: Vec<Word>,
}

/// Everything read from one picture: its size in pixels as read (after any
/// scaling down), and its lines in reading order.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<Line>,
}

impl Reading {
    /// The text read, as a note would hold it: a blank line where the gap
    /// between two lines is much wider than usual (a new paragraph), and the
    /// lines of a paragraph joined up where they were only broken to fit
    /// (as an imported PDF's are, `extract::unwrap_lines`). Empty when
    /// nothing was read. At most a note body's length.
    pub fn text(&self) -> String {
        let gaps: Vec<f64> = self
            .lines
            .windows(2)
            .map(|pair| pair[1].bounds.top - (pair[0].bounds.top + pair[0].bounds.height))
            .collect();
        let mut usual: Vec<f64> = gaps.iter().copied().filter(|gap| *gap > 0.0).collect();
        usual.sort_by(f64::total_cmp);
        // The lower median: with one paragraph break among a few lines, the
        // break isn't taken for the usual spacing.
        let usual = usual.get(usual.len().saturating_sub(1) / 2).copied();
        let mut heights: Vec<f64> = self.lines.iter().map(|line| line.bounds.height).collect();
        heights.sort_by(f64::total_cmp);
        let height = heights.get(heights.len() / 2).copied().unwrap_or(0.0);

        let mut paragraphs: Vec<Vec<&str>> = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            let breaks = index > 0
                && usual.is_some_and(|usual| {
                    let gap = gaps[index - 1];
                    gap > usual * 1.6 && gap > height * 0.6
                });
            match paragraphs.last_mut() {
                Some(paragraph) if !breaks => paragraph.push(&line.text),
                _ => paragraphs.push(vec![&line.text]),
            }
        }
        let text = paragraphs
            .iter()
            .map(|lines| extract::unwrap_lines(&lines.join("\n")))
            .collect::<Vec<_>>()
            .join("\n\n");
        text.chars()
            .take(crate::notes::NOTE_BODY_MAX_CHARS)
            .collect::<String>()
            .trim()
            .to_string()
    }
}

/// Why a page (or a picture) couldn't be read. Nothing was stored for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageProblem {
    /// The PDF page draws no picture (it may be blank, or vector drawing).
    NoPicture,
    /// The picture is in a form Synapse can't decode (JPEG 2000, JBIG2, an
    /// unusual colour space), or is damaged.
    UnreadablePicture,
    /// The picture is over [`MAX_PIXELS`].
    TooLarge,
    /// The PDF has no such page.
    NoSuchPage,
}

/// What the app asks the helper to read: the first line of its standard
/// input, before the file's bytes.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Job {
    /// The bytes are a picture; it is answered as page 0.
    Picture,
    /// The bytes are a PDF; read these pages (from 1), in this order.
    Pdf { pages: Vec<u32> },
}

/// One line of the helper's answer.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum HelperLine {
    Read {
        page: u32,
        reading: Reading,
    },
    Problem {
        page: u32,
        problem: PageProblem,
    },
    /// The PDF itself couldn't be opened (damaged, password-protected).
    Unreadable {
        detail: String,
    },
}

/// What happened to one page.
#[derive(Debug, Clone, PartialEq)]
pub enum PageOutcome {
    Read(Reading),
    Problem(PageProblem),
}

/// Why a recognition stopped before every page was answered. Pages answered
/// before it stopped were already handed to the caller.
#[derive(Debug)]
pub enum RunError {
    /// The user stopped it.
    Stopped,
    /// The PDF couldn't be opened at all. Detail is for logs only.
    Unreadable(String),
    /// The helper crashed, couldn't start, or spent too long on a page.
    HelperFailed(String),
}

/// The engine, loaded once per process from the bundled models.
fn engine() -> Result<&'static OcrEngine, String> {
    static ENGINE: OnceLock<Result<OcrEngine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let detection = rten::Model::load_static_slice(DETECTION_MODEL)
                .map_err(|err| format!("loading the detection model: {err}"))?;
            let recognition = rten::Model::load_static_slice(RECOGNITION_MODEL)
                .map_err(|err| format!("loading the recognition model: {err}"))?;
            OcrEngine::new(OcrEngineParams {
                detection_model: Some(detection),
                recognition_model: Some(recognition),
                ..Default::default()
            })
            .map_err(|err| format!("starting the engine: {err}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// A fraction of `whole`, between 0 and 1, rounded to 4 decimals.
fn fraction(part: f64, whole: u32) -> f64 {
    let value = (part / f64::from(whole.max(1))).clamp(0.0, 1.0);
    (value * 10_000.0).round() / 10_000.0
}

fn text_box(rect: rten_imageproc::Rect, width: u32, height: u32) -> TextBox {
    let left = f64::from(rect.left().max(0));
    let top = f64::from(rect.top().max(0));
    TextBox {
        left: fraction(left, width),
        top: fraction(top, height),
        width: fraction(f64::from(rect.width().max(0)), width),
        height: fraction(f64::from(rect.height().max(0)), height),
    }
}

/// Reads the text in `picture`, scaling it down first if it is larger than
/// [`MAX_SIDE`]. Runs in this process: the app calls it only from the helper,
/// and tests call it directly.
pub fn read_picture(picture: RgbImage) -> Result<Reading, String> {
    let picture = scaled_down(picture);
    let (width, height) = picture.dimensions();
    let engine = engine()?;
    let source = ImageSource::from_bytes(picture.as_raw(), picture.dimensions())
        .map_err(|err| format!("preparing the picture: {err}"))?;
    let input = engine
        .prepare_input(source)
        .map_err(|err| format!("preparing the picture: {err}"))?;
    let words = engine
        .detect_words(&input)
        .map_err(|err| format!("finding text: {err}"))?;
    let rows = engine.find_text_lines(&input, &words);
    let read = engine
        .recognize_text(&input, &rows)
        .map_err(|err| format!("reading text: {err}"))?;
    let lines = read
        .iter()
        .flatten()
        .filter_map(|line| {
            let text = line.to_string().trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(Line {
                text,
                bounds: text_box(line.bounding_rect(), width, height),
                words: line
                    .words()
                    .map(|word| Word {
                        text: word.to_string(),
                        bounds: text_box(word.bounding_rect(), width, height),
                    })
                    .collect(),
            })
        })
        .take(MAX_LINES)
        .collect();
    Ok(Reading {
        width,
        height,
        lines,
    })
}

fn scaled_down(picture: RgbImage) -> RgbImage {
    let (width, height) = picture.dimensions();
    let longer = width.max(height);
    if longer <= MAX_SIDE {
        return picture;
    }
    let scale = f64::from(MAX_SIDE) / f64::from(longer);
    let to = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    image::imageops::resize(
        &picture,
        to(width),
        to(height),
        image::imageops::FilterType::Triangle,
    )
}

fn too_large(width: u64, height: u64) -> bool {
    width == 0 || height == 0 || width.saturating_mul(height) > MAX_PIXELS
}

/// Decodes a kept picture (PNG, JPEG, GIF's first frame, WebP) to RGB,
/// refusing one over [`MAX_PIXELS`] before decoding it.
pub fn decode_picture(bytes: &[u8]) -> Result<RgbImage, PageProblem> {
    decode_as(bytes, None)
}

fn decode_as(bytes: &[u8], format: Option<ImageFormat>) -> Result<RgbImage, PageProblem> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    match format {
        Some(format) => reader.set_format(format),
        None => {
            reader = reader
                .with_guessed_format()
                .map_err(|_| PageProblem::UnreadablePicture)?;
        }
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| PageProblem::UnreadablePicture)?;
    if too_large(width.into(), height.into()) {
        return Err(PageProblem::TooLarge);
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    match format {
        Some(format) => reader.set_format(format),
        None => {
            reader = reader
                .with_guessed_format()
                .map_err(|_| PageProblem::UnreadablePicture)?;
        }
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let picture = reader
        .decode()
        .map_err(|_| PageProblem::UnreadablePicture)?;
    Ok(picture.into_rgb8())
}

// Reading the picture on a PDF page.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// How deep form XObjects (drawings that contain drawings) are searched for
/// the scanned picture.
const FORM_DEPTH: usize = 3;

/// A name, or the first name of an array of them (a colour space such as
/// `[/ICCBased 5 0 R]`).
fn name_of<'a>(document: &'a Document, object: &'a Object) -> Option<&'a [u8]> {
    match document.dereference(object).ok()?.1 {
        Object::Name(name) => Some(name),
        Object::Array(items) => match document.dereference(items.first()?).ok()?.1 {
            Object::Name(name) => Some(name),
            _ => None,
        },
        _ => None,
    }
}

fn integer(document: &Document, dict: &Dictionary, key: &[u8]) -> Option<i64> {
    let object = dict.get(key).ok()?;
    document.dereference(object).ok()?.1.as_i64().ok()
}

fn dictionary_at<'a>(
    document: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Option<&'a Dictionary> {
    let object = dict.get(key).ok()?;
    match document.dereference(object).ok()?.1 {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

/// Every image stream a set of resources draws, directly or through forms.
fn images_in<'a>(
    document: &'a Document,
    resources: &'a Dictionary,
    depth: usize,
    found: &mut Vec<&'a Stream>,
) {
    let Some(xobjects) = dictionary_at(document, resources, b"XObject") else {
        return;
    };
    for (_, object) in xobjects.iter() {
        let Ok((_, Object::Stream(stream))) = document.dereference(object) else {
            continue;
        };
        match stream.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"Image") => found.push(stream),
            Ok(b"Form") if depth > 0 => {
                if let Some(inner) = dictionary_at(document, &stream.dict, b"Resources") {
                    images_in(document, inner, depth - 1, found);
                }
            }
            _ => {}
        }
    }
}

/// The page's rotation in degrees (0, 90, 180 or 270), inherited from its
/// parents as PDF allows.
fn rotation(document: &Document, page: ObjectId) -> i64 {
    let mut at = document.get_dictionary(page).ok();
    for _ in 0..32 {
        let Some(dict) = at else { break };
        if let Some(degrees) = integer(document, dict, b"Rotate") {
            return degrees.rem_euclid(360) / 90 * 90;
        }
        at = dict
            .get(b"Parent")
            .and_then(Object::as_reference)
            .and_then(|parent| document.get_dictionary(parent))
            .ok();
    }
    0
}

/// The picture drawn on a page: the largest image it uses, decoded, and
/// turned as the page is turned.
fn page_picture(document: &Document, page: ObjectId) -> Result<RgbImage, PageProblem> {
    let (own, inherited) = document
        .get_page_resources(page)
        .map_err(|_| PageProblem::UnreadablePicture)?;
    let mut images = Vec::new();
    if let Some(resources) = own {
        images_in(document, resources, FORM_DEPTH, &mut images);
    }
    for id in inherited {
        if let Ok(resources) = document.get_dictionary(id) {
            images_in(document, resources, FORM_DEPTH, &mut images);
        }
    }
    let area = |stream: &&&Stream| {
        let side = |key: &[u8]| integer(document, &stream.dict, key).unwrap_or(0).max(0);
        side(b"Width").saturating_mul(side(b"Height"))
    };
    let largest = images
        .iter()
        .max_by_key(area)
        .ok_or(PageProblem::NoPicture)?;
    let picture = decode_pdf_image(document, largest)?;
    Ok(match rotation(document, page) {
        90 => image::imageops::rotate90(&picture),
        180 => image::imageops::rotate180(&picture),
        270 => image::imageops::rotate270(&picture),
        _ => picture,
    })
}

fn filters(document: &Document, dict: &Dictionary) -> Vec<Vec<u8>> {
    let Ok(object) = dict.get(b"Filter") else {
        return Vec::new();
    };
    match document.dereference(object).map(|(_, object)| object) {
        Ok(Object::Name(name)) => vec![name.clone()],
        Ok(Object::Array(items)) => items
            .iter()
            .filter_map(|item| match document.dereference(item).ok()?.1 {
                Object::Name(name) => Some(name.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The decode parameters of the last filter.
fn decode_parms<'a>(document: &'a Document, dict: &'a Dictionary) -> Option<&'a Dictionary> {
    let object = dict.get(b"DecodeParms").ok()?;
    match document.dereference(object).ok()?.1 {
        Object::Dictionary(parms) => Some(parms),
        Object::Array(items) => match document.dereference(items.last()?).ok()?.1 {
            Object::Dictionary(parms) => Some(parms),
            _ => None,
        },
        _ => None,
    }
}

/// How many colour components a colour space has, for the ones a scan uses.
fn components(document: &Document, dict: &Dictionary) -> Option<usize> {
    let object = dict.get(b"ColorSpace").ok()?;
    match name_of(document, object)? {
        b"DeviceGray" | b"CalGray" | b"G" => Some(1),
        b"DeviceRGB" | b"CalRGB" | b"RGB" => Some(3),
        b"DeviceCMYK" | b"CMYK" => Some(4),
        b"ICCBased" => {
            let Ok((_, Object::Array(items))) = document.dereference(object) else {
                return None;
            };
            let Ok((_, Object::Stream(profile))) = document.dereference(items.get(1)?) else {
                return None;
            };
            usize::try_from(integer(document, &profile.dict, b"N")?).ok()
        }
        _ => None,
    }
}

/// Whether the image's `Decode` array flips its samples (`[1 0]`).
fn inverted(document: &Document, dict: &Dictionary) -> bool {
    let Ok(object) = dict.get(b"Decode") else {
        return false;
    };
    let Ok((_, Object::Array(items))) = document.dereference(object) else {
        return false;
    };
    let number = |item: &Object| match item {
        Object::Integer(value) => Some(*value as f64),
        Object::Real(value) => Some(f64::from(*value)),
        _ => None,
    };
    matches!(
        (items.first().and_then(number), items.get(1).and_then(number)),
        (Some(first), Some(second)) if first > second
    )
}

fn decode_pdf_image(document: &Document, stream: &Stream) -> Result<RgbImage, PageProblem> {
    let dict = &stream.dict;
    let width = integer(document, dict, b"Width").unwrap_or(0);
    let height = integer(document, dict, b"Height").unwrap_or(0);
    if width <= 0 || height <= 0 {
        return Err(PageProblem::UnreadablePicture);
    }
    if too_large(width as u64, height as u64) {
        return Err(PageProblem::TooLarge);
    }
    let (width, height) = (width as u32, height as u32);
    let filters = filters(document, dict);
    let last = filters.last().map(Vec::as_slice);
    // Any filters before the last are plain compression (Flate around a
    // JPEG, say), which lopdf undoes for every filter it knows.
    let encoded = || -> Result<Vec<u8>, PageProblem> {
        if filters.len() <= 1 {
            return Ok(stream.content.clone());
        }
        let mut outer = stream.clone();
        let rest: Vec<Object> = filters[..filters.len() - 1]
            .iter()
            .map(|name| Object::Name(name.clone()))
            .collect();
        outer.dict.set("Filter", Object::Array(rest));
        outer.dict.remove(b"DecodeParms");
        outer
            .decompressed_content()
            .map_err(|_| PageProblem::UnreadablePicture)
    };
    match last {
        Some(b"DCTDecode" | b"DCT") => {
            let mut picture = decode_as(&encoded()?, Some(ImageFormat::Jpeg))?;
            if inverted(document, dict) {
                image::imageops::invert(&mut picture);
            }
            Ok(picture)
        }
        Some(b"CCITTFaxDecode" | b"CCF") => {
            let parms = decode_parms(document, dict);
            let parm = |key: &[u8]| parms.and_then(|parms| integer(document, parms, key));
            let columns = parm(b"Columns").unwrap_or(1728).max(1) as u32;
            let rows = parm(b"Rows")
                .filter(|rows| *rows > 0)
                .map(|rows| rows as u32);
            let black_is_1 = parms
                .and_then(|parms| parms.get(b"BlackIs1").ok())
                .and_then(|value| value.as_bool().ok())
                .unwrap_or(false);
            let data = encoded()?;
            let mut pixels: Vec<u8> = Vec::with_capacity((columns * height) as usize);
            let mut take = |transitions: &[u32]| {
                for colour in fax::decoder::pels(transitions, columns) {
                    pixels.push(match colour {
                        fax::Color::White => 255,
                        fax::Color::Black => 0,
                    });
                }
            };
            let decoded = match parm(b"K").unwrap_or(0) {
                k if k < 0 => fax::decoder::decode_g4(
                    data.iter().copied(),
                    columns,
                    rows.or(Some(height)),
                    &mut take,
                ),
                0 => fax::decoder::decode_g3(data.iter().copied(), &mut take),
                _ => None,
            };
            if decoded.is_none() || pixels.is_empty() {
                return Err(PageProblem::UnreadablePicture);
            }
            let lines = (pixels.len() / columns as usize) as u32;
            pixels.truncate((lines * columns) as usize);
            let mut grey = GrayImage::from_raw(columns, lines, pixels)
                .ok_or(PageProblem::UnreadablePicture)?;
            if black_is_1 != inverted(document, dict) {
                image::imageops::invert(&mut grey);
            }
            Ok(DynamicImage::ImageLuma8(grey).into_rgb8())
        }
        None
        | Some(b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"RunLengthDecode" | b"RL") => {
            let samples = if filters.is_empty() {
                stream.content.clone()
            } else {
                stream
                    .decompressed_content()
                    .map_err(|_| PageProblem::UnreadablePicture)?
            };
            raw_picture(document, dict, width, height, &samples)
        }
        _ => Err(PageProblem::UnreadablePicture),
    }
}

/// Uncompressed samples: 1-bit (black and white, or an image mask) or 8-bit
/// grey, RGB or CMYK.
fn raw_picture(
    document: &Document,
    dict: &Dictionary,
    width: u32,
    height: u32,
    samples: &[u8],
) -> Result<RgbImage, PageProblem> {
    let mask = dict
        .get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false);
    let bits = if mask {
        1
    } else {
        integer(document, dict, b"BitsPerComponent").unwrap_or(8)
    };
    let components = if mask {
        1
    } else {
        components(document, dict).ok_or(PageProblem::UnreadablePicture)?
    };
    let (w, h) = (width as usize, height as usize);
    let mut grey_or_rgb = match (bits, components) {
        (1, 1) => {
            let row = w.div_ceil(8);
            if samples.len() < row * h {
                return Err(PageProblem::UnreadablePicture);
            }
            let mut pixels = Vec::with_capacity(w * h);
            for y in 0..h {
                for x in 0..w {
                    let bit = samples[y * row + x / 8] >> (7 - x % 8) & 1;
                    // For a mask, 0 paints (black); for an image, 0 is black.
                    pixels.push(if bit == 1 { 255 } else { 0 });
                }
            }
            DynamicImage::ImageLuma8(
                GrayImage::from_raw(width, height, pixels).ok_or(PageProblem::UnreadablePicture)?,
            )
        }
        (8, 1) => DynamicImage::ImageLuma8(
            GrayImage::from_raw(
                width,
                height,
                samples
                    .get(..w * h)
                    .ok_or(PageProblem::UnreadablePicture)?
                    .to_vec(),
            )
            .ok_or(PageProblem::UnreadablePicture)?,
        ),
        (8, 3) => DynamicImage::ImageRgb8(
            RgbImage::from_raw(
                width,
                height,
                samples
                    .get(..w * h * 3)
                    .ok_or(PageProblem::UnreadablePicture)?
                    .to_vec(),
            )
            .ok_or(PageProblem::UnreadablePicture)?,
        ),
        (8, 4) => {
            let cmyk = samples
                .get(..w * h * 4)
                .ok_or(PageProblem::UnreadablePicture)?;
            let rgb = cmyk
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|pixel| {
                    let k = 255 - u16::from(pixel[3]);
                    [0, 1, 2].map(|at| ((255 - u16::from(pixel[at])) * k / 255) as u8)
                })
                .collect();
            DynamicImage::ImageRgb8(
                RgbImage::from_raw(width, height, rgb).ok_or(PageProblem::UnreadablePicture)?,
            )
        }
        _ => return Err(PageProblem::UnreadablePicture),
    }
    .into_rgb8();
    if inverted(document, dict) {
        image::imageops::invert(&mut grey_or_rgb);
    }
    Ok(grey_or_rgb)
}

/// Reads the pictures of `pages` of a PDF, answering each in turn. The PDF
/// failing to open at all is the `Err`.
pub fn read_pdf_pages(
    pdf: &[u8],
    pages: &[u32],
    mut answer: impl FnMut(u32, PageOutcome),
) -> Result<(), String> {
    let document = Document::load_mem(pdf).map_err(|err| format!("opening the PDF: {err}"))?;
    let ids = document.get_pages();
    for &page in pages {
        let outcome = match ids.get(&page) {
            None => PageOutcome::Problem(PageProblem::NoSuchPage),
            Some(id) => match page_picture(&document, *id) {
                Err(problem) => PageOutcome::Problem(problem),
                Ok(picture) => PageOutcome::Read(read_picture(picture)?),
            },
        };
        answer(page, outcome);
    }
    Ok(())
}

// The helper process.

/// Runs as the recognition helper: reads a [`Job`] line and a file from
/// standard input, writes a JSON line per page to standard output, and
/// returns the exit code. A panic in a decoder or the engine ends the
/// process instead, which the app reports as a failed recognition.
pub fn ocr_helper() -> i32 {
    let mut input = BufReader::new(std::io::stdin().take(HELPER_INPUT_MAX_BYTES));
    let mut job_line = String::new();
    if input.read_line(&mut job_line).is_err() {
        return 2;
    }
    let Ok(job) = serde_json::from_str::<Job>(&job_line) else {
        return 2;
    };
    let mut bytes = Vec::new();
    if input.read_to_end(&mut bytes).is_err() {
        return 2;
    }
    let mut out = std::io::stdout().lock();
    let mut write = |line: HelperLine| -> bool {
        serde_json::to_writer(&mut out, &line).is_ok()
            && out.write_all(b"\n").is_ok()
            && out.flush().is_ok()
    };
    let mut written = true;
    match job {
        Job::Picture => {
            let line = match decode_picture(&bytes) {
                Err(problem) => HelperLine::Problem { page: 0, problem },
                Ok(picture) => match read_picture(picture) {
                    Ok(reading) => HelperLine::Read { page: 0, reading },
                    Err(_) => return 4,
                },
            };
            written &= write(line);
        }
        Job::Pdf { pages } => {
            let result = read_pdf_pages(&bytes, &pages, |page, outcome| {
                written &= write(match outcome {
                    PageOutcome::Read(reading) => HelperLine::Read { page, reading },
                    PageOutcome::Problem(problem) => HelperLine::Problem { page, problem },
                });
            });
            if let Err(detail) = result {
                written &= write(HelperLine::Unreadable { detail });
            }
        }
    }
    if written {
        0
    } else {
        3
    }
}

/// Has `helper` (Synapse, run with [`OCR_HELPER_ARG`]) carry out `job` on
/// `bytes`, handing each page's outcome to `answer` as it arrives. Stops the
/// helper when `stop` is cancelled ([`RunError::Stopped`]) or when a page
/// takes longer than `page_timeout`. Blocking: run it off the async runtime.
pub fn run_helper(
    helper: &Path,
    job: &Job,
    bytes: &[u8],
    stop: &CancellationToken,
    page_timeout: Duration,
    mut answer: impl FnMut(u32, PageOutcome),
) -> Result<(), RunError> {
    let failed = |detail: String| RunError::HelperFailed(detail);
    let mut command = Command::new(helper);
    command
        .arg(OCR_HELPER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window flashes up while it works.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|err| failed(format!("starting the helper: {err}")))?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut input = serde_json::to_vec(job).map_err(|err| failed(err.to_string()))?;
    input.push(b'\n');
    input.extend_from_slice(bytes);
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let stdout = child.stdout.take().expect("stdout is piped");
    let (lines, received) = std::sync::mpsc::channel::<std::io::Result<String>>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let failed = line.is_err();
            if lines.send(line).is_err() || failed {
                break;
            }
        }
    });

    let stop_child = |child: &mut std::process::Child| {
        // Its pipes close with it, so both threads finish.
        let _ = child.kill();
        let _ = child.wait();
    };
    let mut deadline = Instant::now() + page_timeout;
    let mut unreadable = None;
    let result = loop {
        if stop.is_cancelled() {
            stop_child(&mut child);
            break Err(RunError::Stopped);
        }
        match received.recv_timeout(Duration::from_millis(20)) {
            Ok(Ok(line)) => {
                deadline = Instant::now() + page_timeout;
                match serde_json::from_str::<HelperLine>(&line) {
                    Ok(HelperLine::Read { page, reading }) => {
                        answer(page, PageOutcome::Read(reading))
                    }
                    Ok(HelperLine::Problem { page, problem }) => {
                        answer(page, PageOutcome::Problem(problem))
                    }
                    Ok(HelperLine::Unreadable { detail }) => unreadable = Some(detail),
                    Err(err) => {
                        stop_child(&mut child);
                        break Err(failed(format!("the helper's answer: {err}")));
                    }
                }
            }
            Ok(Err(err)) => {
                stop_child(&mut child);
                break Err(failed(format!("reading the helper's answer: {err}")));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    stop_child(&mut child);
                    break Err(failed(format!(
                        "the helper spent over {page_timeout:?} on a page"
                    )));
                }
            }
            // Standard output closed: the helper has finished (or died).
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let status = child
                    .wait()
                    .map_err(|err| failed(format!("waiting for the helper: {err}")));
                break match status {
                    Err(err) => Err(err),
                    Ok(status) if !status.success() => {
                        Err(failed(format!("the helper exited with {status}")))
                    }
                    Ok(_) => match unreadable.take() {
                        Some(detail) => Err(RunError::Unreadable(detail)),
                        None => Ok(()),
                    },
                };
            }
        }
    };
    // A helper that exits early may not read all its input; that's reported above.
    let _ = writer.join();
    let _ = reader.join();
    result
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use lopdf::dictionary;

    /// The sample page in `testdata/ocr/page.png`: three lines of English
    /// prose in two paragraphs, black on white, 1000 by 330 pixels.
    pub fn page_png() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/ocr/page.png"
        ))
        .unwrap()
    }

    /// The sample page in grey, as a picture.
    pub fn page_grey() -> GrayImage {
        image::load_from_memory(&page_png()).unwrap().into_luma8()
    }

    /// How a test PDF page draws (or doesn't draw) its picture.
    pub enum TestPage {
        /// The sample page as a JPEG.
        Jpeg,
        /// The sample page as Flate-compressed 8-bit grey samples.
        FlateGrey,
        /// The sample page in black and white, CCITT Group 4.
        Ccitt,
        /// The sample page as a JPEG, stored turned a quarter to the left on
        /// a page that says `/Rotate 90`.
        Rotated,
        /// The sample page as a JPEG, drawn through a form XObject.
        InForm,
        /// A JBIG2 picture (Synapse can't decode those).
        Jbig2,
        /// Only text, no picture.
        TextOnly,
    }

    fn image_stream(dict: Dictionary, content: Vec<u8>) -> Stream {
        let mut stream = Stream::new(dict, content);
        stream.allows_compression = false;
        stream
    }

    fn jpeg(picture: &GrayImage) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(picture)
            .unwrap();
        out
    }

    fn grey_jpeg_stream(picture: &GrayImage) -> Stream {
        image_stream(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => picture.width() as i64,
                "Height" => picture.height() as i64,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            jpeg(picture),
        )
    }

    /// A PDF with one page per entry.
    pub fn pdf(pages: &[TestPage]) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let grey = page_grey();
        let mut kids: Vec<Object> = Vec::new();
        for page in pages {
            let mut page_dict = dictionary! { "Type" => "Page", "Parent" => pages_id };
            let draw = |name: &str| {
                Content {
                    operations: vec![
                        Operation::new("q", vec![]),
                        Operation::new(
                            "cm",
                            vec![
                                500.into(),
                                0.into(),
                                0.into(),
                                165.into(),
                                50.into(),
                                600.into(),
                            ],
                        ),
                        Operation::new("Do", vec![Object::Name(name.as_bytes().to_vec())]),
                        Operation::new("Q", vec![]),
                    ],
                }
                .encode()
                .unwrap()
            };
            let (resources, content) = match page {
                TestPage::Jpeg => {
                    let id = doc.add_object(grey_jpeg_stream(&grey));
                    (
                        dictionary! { "XObject" => dictionary! { "Im1" => id } },
                        draw("Im1"),
                    )
                }
                TestPage::Rotated => {
                    page_dict.set("Rotate", 90);
                    let turned = image::imageops::rotate270(&grey);
                    let id = doc.add_object(grey_jpeg_stream(&turned));
                    (
                        dictionary! { "XObject" => dictionary! { "Im1" => id } },
                        draw("Im1"),
                    )
                }
                TestPage::FlateGrey => {
                    let mut stream = Stream::new(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Image",
                            "Width" => grey.width() as i64,
                            "Height" => grey.height() as i64,
                            "ColorSpace" => "DeviceGray",
                            "BitsPerComponent" => 8,
                        },
                        grey.as_raw().clone(),
                    );
                    stream.compress().unwrap();
                    assert!(stream.dict.get(b"Filter").is_ok());
                    let id = doc.add_object(stream);
                    (
                        dictionary! { "XObject" => dictionary! { "Im1" => id } },
                        draw("Im1"),
                    )
                }
                TestPage::Ccitt => {
                    let mut encoder = fax::encoder::Encoder::new(fax::VecWriter::new());
                    for row in grey.rows() {
                        let pels = row.map(|pixel| {
                            if pixel.0[0] < 128 {
                                fax::Color::Black
                            } else {
                                fax::Color::White
                            }
                        });
                        encoder.encode_line(pels, grey.width()).unwrap();
                    }
                    let data = encoder.finish().unwrap().finish();
                    let id = doc.add_object(image_stream(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Image",
                            "Width" => grey.width() as i64,
                            "Height" => grey.height() as i64,
                            "ColorSpace" => "DeviceGray",
                            "BitsPerComponent" => 1,
                            "Filter" => "CCITTFaxDecode",
                            "DecodeParms" => dictionary! {
                                "K" => -1,
                                "Columns" => grey.width() as i64,
                                "Rows" => grey.height() as i64,
                            },
                        },
                        data,
                    ));
                    (
                        dictionary! { "XObject" => dictionary! { "Im1" => id } },
                        draw("Im1"),
                    )
                }
                TestPage::InForm => {
                    let image_id = doc.add_object(grey_jpeg_stream(&grey));
                    let form = image_stream(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Form",
                            "BBox" => vec![0.into(), 0.into(), 1.into(), 1.into()],
                            "Resources" => dictionary! {
                                "XObject" => dictionary! { "Im0" => image_id },
                            },
                        },
                        b"/Im0 Do".to_vec(),
                    );
                    let form_id = doc.add_object(form);
                    (
                        dictionary! { "XObject" => dictionary! { "Fm1" => form_id } },
                        draw("Fm1"),
                    )
                }
                TestPage::Jbig2 => {
                    let id = doc.add_object(image_stream(
                        dictionary! {
                            "Type" => "XObject",
                            "Subtype" => "Image",
                            "Width" => 100,
                            "Height" => 100,
                            "ColorSpace" => "DeviceGray",
                            "BitsPerComponent" => 1,
                            "Filter" => "JBIG2Decode",
                        },
                        vec![0; 64],
                    ));
                    (
                        dictionary! { "XObject" => dictionary! { "Im1" => id } },
                        draw("Im1"),
                    )
                }
                TestPage::TextOnly => (
                    dictionary! { "Font" => dictionary! { "F1" => font_id } },
                    Content {
                        operations: vec![
                            Operation::new("BT", vec![]),
                            Operation::new("Tf", vec!["F1".into(), 14.into()]),
                            Operation::new("Td", vec![72.into(), 760.into()]),
                            Operation::new("Tj", vec![Object::string_literal("Typed text")]),
                            Operation::new("ET", vec![]),
                        ],
                    }
                    .encode()
                    .unwrap(),
                ),
            };
            let content_id = doc.add_object(Stream::new(dictionary! {}, content));
            page_dict.set("Contents", content_id);
            page_dict.set("Resources", resources);
            kids.push(doc.add_object(page_dict).into());
        }
        let count = kids.len() as i64;
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    pub const PAGE_TEXT: &str = "Photosynthesis converts light energy into chemical energy.\n\
                                 The light reactions take place in the thylakoid membranes.\n\n\
                                 The Calvin cycle fixes carbon dioxide into sugars.";
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn the_bundled_models_are_the_ones_named() {
        for (bytes, expected) in [
            (DETECTION_MODEL, DETECTION_MODEL_SHA256),
            (RECOGNITION_MODEL, RECOGNITION_MODEL_SHA256),
        ] {
            let found: String = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(found, expected);
            assert!(MODEL.contains(&expected[..8]));
        }
    }

    #[test]
    fn a_page_is_read_line_by_line_with_its_paragraphs() {
        let picture = decode_picture(&page_png()).unwrap();
        let reading = read_picture(picture).unwrap();
        assert_eq!((reading.width, reading.height), (1000, 330));
        assert_eq!(reading.lines.len(), 3);
        assert_eq!(reading.text(), PAGE_TEXT);
        let first = &reading.lines[0];
        assert_eq!(first.words.first().unwrap().text, "Photosynthesis");
        // Boxes are fractions of the picture, in reading order.
        assert!(first.bounds.left > 0.02 && first.bounds.left < 0.08);
        assert!(first.bounds.top < reading.lines[1].bounds.top);
        for line in &reading.lines {
            for value in [
                line.bounds.left,
                line.bounds.top,
                line.bounds.width,
                line.bounds.height,
            ] {
                assert!((0.0..=1.0).contains(&value));
            }
        }
    }

    #[test]
    fn a_blank_picture_reads_as_nothing() {
        let reading =
            read_picture(RgbImage::from_pixel(400, 300, image::Rgb([255, 255, 255]))).unwrap();
        assert!(reading.lines.is_empty());
        assert_eq!(reading.text(), "");
    }

    #[test]
    fn a_large_picture_is_scaled_down_before_reading() {
        let picture = RgbImage::from_pixel(MAX_SIDE * 2, 100, image::Rgb([255, 255, 255]));
        let scaled = scaled_down(picture);
        assert_eq!(scaled.dimensions(), (MAX_SIDE, 50));
    }

    #[test]
    fn pictures_that_are_not_pictures_or_too_large_are_refused() {
        assert_eq!(
            decode_picture(b"not a picture"),
            Err(PageProblem::UnreadablePicture)
        );
        // A PNG header claiming 100,000 by 100,000 pixels isn't decoded.
        let mut huge = page_png();
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        // The header's checksum, over its type and data (bytes 12..29).
        let crc = huge[12..29].iter().fold(!0u32, |crc, byte| {
            (0..8).fold(crc ^ u32::from(*byte), |crc, _| {
                if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                }
            })
        });
        huge[29..33].copy_from_slice(&(!crc).to_be_bytes());
        assert_eq!(decode_picture(&huge), Err(PageProblem::TooLarge));
    }

    #[test]
    fn the_picture_on_each_scanned_page_of_a_pdf_is_read() {
        let bytes = pdf(&[
            TestPage::Jpeg,
            TestPage::FlateGrey,
            TestPage::Ccitt,
            TestPage::Rotated,
            TestPage::InForm,
            TestPage::Jbig2,
            TestPage::TextOnly,
        ]);
        let mut answers = Vec::new();
        read_pdf_pages(&bytes, &[1, 2, 3, 4, 5, 6, 7, 9], |page, outcome| {
            answers.push((page, outcome))
        })
        .unwrap();
        let pages: Vec<u32> = answers.iter().map(|(page, _)| *page).collect();
        assert_eq!(pages, vec![1, 2, 3, 4, 5, 6, 7, 9]);
        for (page, outcome) in &answers[..5] {
            let PageOutcome::Read(reading) = outcome else {
                panic!("page {page}: {outcome:?}");
            };
            assert_eq!(reading.text(), PAGE_TEXT, "page {page}");
            assert_eq!((reading.width, reading.height), (1000, 330), "page {page}");
        }
        assert_eq!(
            answers[5].1,
            PageOutcome::Problem(PageProblem::UnreadablePicture)
        );
        assert_eq!(answers[6].1, PageOutcome::Problem(PageProblem::NoPicture));
        assert_eq!(answers[7].1, PageOutcome::Problem(PageProblem::NoSuchPage));
    }

    #[test]
    fn a_damaged_pdf_is_refused_whole() {
        let result = read_pdf_pages(b"%PDF-1.7 not really", &[1], |_, _| {
            panic!("no page should be answered")
        });
        assert!(result.is_err());
    }

    #[test]
    fn paragraphs_follow_wide_gaps_and_wrapped_lines_join() {
        let line = |text: &str, top: f64| Line {
            text: text.to_string(),
            bounds: TextBox {
                left: 0.1,
                top,
                width: 0.8,
                height: 0.04,
            },
            words: Vec::new(),
        };
        let reading = Reading {
            width: 100,
            height: 100,
            lines: vec![
                line("The cell is the basic", 0.10),
                line("unit of life.", 0.16),
                line("Mitochondria make ATP.", 0.22),
                line("A new paragraph", 0.40),
                line("starts here.", 0.46),
            ],
        };
        assert_eq!(
            reading.text(),
            "The cell is the basic unit of life.\nMitochondria make ATP.\n\n\
             A new paragraph starts here."
        );
    }
}
