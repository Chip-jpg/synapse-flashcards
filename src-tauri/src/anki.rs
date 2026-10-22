//! Importing an Anki package (`.apkg`, or a whole collection as `.colpkg`)
//! as new cards in new decks.
//!
//! The text of front/back and cloze notes travels, with the pictures of
//! front/back notes, and image occlusion notes with their picture. Every
//! imported card starts as a brand-new card, exactly like one the user typed:
//! Anki's schedule and review history are not imported, and neither are
//! sounds, videos, or pictures in cloze notes.
//!
//! Importing has two steps, so nothing is saved before the user has seen it:
//! 1. [`extract_collection`] copies the collection database out of the
//!    package into a scratch folder (with size limits), and
//!    [`read_collection`] reads it, read-only and on its own connection (it is
//!    never attached to Synapse's database), into an [`AnkiPackage`]. Nothing
//!    is written to Synapse.
//! 2. [`import_package`] saves it in one transaction: one new deck per Anki
//!    deck that has cards to add, named as in Anki but never taking the name
//!    of an existing deck (archived ones included), and every card checked as
//!    a typed card is. If anything is refused, nothing is saved.
//!
//! Which collection is read: newer Anki writes `collection.anki21b` (the
//! database compressed with zstd) and adds a `collection.anki2` that only
//! says "please update Anki"; older Anki writes `collection.anki21`, or only
//! `collection.anki2`. The first of those three that the package has is read.
//!
//! Which notes become cards: a note whose note type is front/back shaped, by
//! its card templates rather than its (translatable) name. The first template
//! must show exactly one field on the question (the front) and add exactly one
//! more on the answer (the back); any other template must be the same, or the
//! reverse of it. That is Anki's "Basic", "Basic (and reversed card)", "Basic
//! (optional reversed card)" and "Basic (type in the answer)", and any custom
//! type built the same way. A note becomes one card; when its type has a
//! reverse template and Anki made that card for the note, it is *reversible*,
//! and the user can choose to add the reversed card too.
//!
//! A cloze note (Anki's "Cloze", or any note type of the cloze kind whose one
//! template asks with exactly one `{{cloze:Field}}` and shows no other field
//! on the question) becomes a cloze text, as one typed in Synapse would: its
//! field's HTML becomes text, it is checked as `cloze::validate_cloze` checks
//! a typed text, and it makes one card per deletion number. Anki's syntax is
//! the same (`{{c1::answer}}`, `{{c1::answer::hint}}`). Other fields shown on
//! the answer, such as "Back Extra", are left out, and notes where they held
//! text are counted. A text Synapse wouldn't accept (nested deletions, an
//! empty answer, a number over 999) is left out and counted.
//!
//! An image occlusion note (Anki's "Image Occlusion": a cloze note type
//! whose template sets up `image-occlusion`) becomes an image occlusion of
//! its picture (`occlusion.rs`): the picture is read from the package's
//! media and kept as Synapse keeps any picture, and each of its shapes
//! becomes a box with the shape's number. Anki writes shapes as deletions,
//! `{{c1::image-occlusion:rect:left=.1:top=.2:width=.3:height=.4:oi=1}}`,
//! with coordinates from 0 to 1 of the picture. Rectangles are kept as they
//! are; ellipses and polygons become the rectangles around them, and text
//! labels drawn on the picture are left out (both counted in
//! `occlusion_approximated`). `oi=1` (Anki's "hide all, guess one") on any
//! shape makes the note hide all; otherwise it hides one. The header field
//! is the header; "Back Extra" is left out and counted, as for cloze notes.
//! A note whose picture isn't in the package (Anki leaves media out unless
//! asked), isn't a PNG, JPEG, GIF or WebP, or is over 20 MB, or whose shapes
//! can't be read (pixel coordinates from early versions of Anki's editor),
//! is left out and counted as image occlusion.
//!
//! The pictures (`<img src="…">`) on a front/back note's front and back
//! become the card's pictures on that side (`card_pictures`), in order, each
//! once, as pictures added in Synapse are: read from the package's media,
//! kept once however many cards show them, and saved in the same transaction
//! as the cards. A reversed card shows them the other way round. A side shows
//! at most 4 (`CARD_SIDE_PICTURES_MAX`); pictures past that, or not in the
//! package (or named by a web address), or not a PNG, JPEG, GIF or WebP, or
//! over 20 MB (or past [`ANKI_PICTURES_MAX_BYTES`] in all), are left out and
//! counted by [`PicturesLeftOut`]; the card keeps its text and other
//! pictures. A side must still have text, as a typed card's must: a note
//! whose front is only a picture is left out as empty.
//!
//! Everything else is left out and counted by [`Skipped`]: other note types,
//! notes with an empty side or text, and sides or texts over the
//! 2,000-character limit of a card.
//!
//! A field's HTML becomes plain text by [`html_to_text`]. Nothing here logs a
//! card's text.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{ConnectOptions, SqlitePool};

use crate::assets::{self, Asset, AssetError, AssetStore, MediaType, MAX_PICTURE_BYTES};
use crate::authoring::{
    insert_new_card, validate_card, AuthoringError, InvalidInput, DECK_NAME_MAX_CHARS,
};
use crate::cloze::{insert_cloze, validate_cloze};
use crate::db::to_db_time;
use crate::occlusion::{
    insert_occlusion, renumber, validate_occlusion, Mode, Occlusion, OcclusionBox, NUMBER_MAX,
};
use crate::pictures::{set_card_pictures, CardPictures, CARD_SIDE_PICTURES_MAX};

/// The largest package read. Its media, which are most of a big package, are
/// never read, so this is only a sanity limit.
pub const ANKI_FILE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// The largest collection database copied out of a package, uncompressed.
pub const ANKI_COLLECTION_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// The most cards one import adds, reversed cards included.
pub const ANKI_CARDS_MAX: usize = 50_000;
/// The most decks one import creates.
pub const ANKI_DECKS_MAX: usize = 1_000;
/// How many cards of each deck the preview shows.
pub const EXAMPLES_PER_DECK: usize = 3;
/// The description every imported deck gets.
pub const IMPORTED_DESCRIPTION: &str = "Imported from Anki.";
/// The most picture bytes one import holds in memory for its image occlusion
/// notes and cards, between reading the package and saving it. Image
/// occlusion notes past it are left out as if their picture were missing;
/// cards past it lose the picture (counted as too large).
pub const ANKI_PICTURES_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// The largest list of media names read from a package.
const MEDIA_INDEX_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The first bytes of a zstd frame, which newer packages compress media with.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// The collection files a package may hold, in the order they are preferred.
const COLLECTION_ENTRIES: [&str; 3] = [
    "collection.anki21b",
    "collection.anki21",
    "collection.anki2",
];
/// What marks image occlusion, in its template and in its deletions
/// (`{{c1::image-occlusion:rect:…}}`).
const IMAGE_OCCLUSION: &str = "image-occlusion";
/// Field values in a note are separated by this character.
const FIELD_SEPARATOR: char = '\u{1f}';
/// Anki's names for a subdeck's parents, joined by this in the newer schema.
const DECK_SEPARATOR: char = '\u{1f}';

/// One note that can become cards.
#[derive(Debug, Clone, PartialEq)]
pub struct AnkiNote {
    /// A front/back note's front, or a cloze note's first card's front.
    pub front: String,
    /// A front/back note's back, or a cloze note's first card's back.
    pub back: String,
    /// Anki also made the reversed card (back as the question) for this note.
    pub reversible: bool,
    /// A cloze note's text, checked, as it will be saved.
    pub cloze: Option<String>,
    /// An image occlusion note's picture and boxes, checked.
    pub occlusion: Option<AnkiOcclusion>,
    /// A front/back note's pictures on each side. Once the package is read,
    /// only pictures it holds, at most [`CARD_SIDE_PICTURES_MAX`] a side.
    pub pictures: AnkiCardPictures,
    /// The cards it adds one way: 1, or a cloze text's deletion numbers, or
    /// an image occlusion's box numbers.
    pub cards: usize,
}

/// An image occlusion note: the name of its picture in the package's media,
/// and its header, way of asking and boxes.
#[derive(Debug, Clone, PartialEq)]
pub struct AnkiOcclusion {
    pub picture: String,
    pub occlusion: Occlusion,
}

/// The pictures on each side of a front/back note, by their names in the
/// package's media, in order and each once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnkiCardPictures {
    pub front: Vec<String>,
    pub back: Vec<String>,
}

impl AnkiCardPictures {
    pub fn count(&self) -> usize {
        self.front.len() + self.back.len()
    }
}

/// Pictures on the sides of front/back cards that couldn't be kept, by why.
/// A picture counts once for each side it is on. The card is added without
/// it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PicturesLeftOut {
    /// Not in the package (exported without media), or named by an address.
    pub missing: usize,
    /// Not a PNG, JPEG, GIF or WebP (an SVG, say), or unreadable.
    pub unsupported: usize,
    /// Over 20 MB, or past [`ANKI_PICTURES_MAX_BYTES`] in all.
    pub too_large: usize,
    /// Past the [`CARD_SIDE_PICTURES_MAX`] pictures a side shows.
    pub too_many: usize,
}

/// A picture read from a package, by its type (from its bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct AnkiPicture {
    pub bytes: Arc<[u8]>,
    pub media_type: MediaType,
}

/// An Anki deck with the notes from it that can become cards.
#[derive(Debug, Clone, PartialEq)]
pub struct AnkiDeck {
    /// Its full name in Anki, with `::` between a subdeck and its parents.
    pub anki_name: String,
    pub notes: Vec<AnkiNote>,
}

/// Notes left out, by why.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    /// Image occlusion notes whose picture is missing or unusable, or whose
    /// shapes can't be read.
    pub image_occlusion: usize,
    /// Cloze notes whose text Synapse doesn't accept (nested deletions, an
    /// empty answer or hint, a number over 999).
    pub bad_cloze: usize,
    /// Notes whose type isn't one front and one back.
    pub other_note_type: usize,
    /// A front, back or cloze text with no text (perhaps only an image or
    /// sound).
    pub empty: usize,
    /// A front, back or cloze text over a card's 2,000 characters.
    pub too_long: usize,
    /// Notes Anki made no card for.
    pub no_card: usize,
}

/// A package read and checked. Nothing has been saved.
#[derive(Debug, Clone, PartialEq)]
pub struct AnkiPackage {
    /// Decks with at least one note to add, by name.
    pub decks: Vec<AnkiDeck>,
    /// Every note in the collection.
    pub notes: usize,
    pub skipped: Skipped,
    /// Notes to add that had a sound or video, or a picture in a cloze
    /// note, which is left out. A front/back note's pictures are kept, or
    /// counted in `pictures_left_out`.
    pub media_removed: usize,
    /// Pictures of front/back notes left out, by why.
    pub pictures_left_out: PicturesLeftOut,
    /// Cloze notes to add with text in another field (such as "Back Extra"),
    /// which is left out.
    pub extra_removed: usize,
    /// Notes to add that had been reviewed in Anki; they start as new here.
    pub reviewed: usize,
    /// Image occlusion notes to add whose ellipses or polygons became
    /// rectangles, or whose text labels were left out.
    pub occlusion_approximated: usize,
    /// The pictures of the image occlusion notes and cards to add, by their
    /// name in the package.
    pub pictures: HashMap<String, AnkiPicture>,
}

impl AnkiPackage {
    /// The cards saving would add: one per front/back note and one per
    /// deletion number of a cloze note, plus the reversed ones.
    pub fn card_count(&self, both_directions: bool) -> usize {
        self.decks
            .iter()
            .map(|deck| {
                let forward: usize = deck.notes.iter().map(|note| note.cards).sum();
                let reversed = deck.notes.iter().filter(|note| note.reversible).count();
                forward + if both_directions { reversed } else { 0 }
            })
            .sum()
    }
}

/// A deck saving created.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedDeck {
    pub id: i64,
    pub name: String,
    pub cards: usize,
}

/// What saving did.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnkiImported {
    pub decks: Vec<ImportedDeck>,
    pub cards: usize,
}

/// Why a package couldn't be read or saved. Nothing was written to Synapse.
#[derive(Debug)]
pub enum AnkiError {
    /// The package is larger than [`ANKI_FILE_MAX_BYTES`].
    TooLarge,
    /// Its collection is larger than [`ANKI_COLLECTION_MAX_BYTES`].
    CollectionTooLarge,
    /// The file isn't a zip archive, or holds no Anki collection.
    NotAnki,
    /// The collection couldn't be read: damaged, or a format this doesn't
    /// know. Detail is for logs only.
    Unreadable(String),
    /// More than [`ANKI_CARDS_MAX`] cards to add.
    TooManyCards,
    /// More than [`ANKI_DECKS_MAX`] decks to create.
    TooManyDecks,
    /// There is no card to add.
    NothingToImport,
    /// The scratch folder couldn't be used. Detail is for logs only.
    Io(io::Error),
    /// Saving was refused, or the database failed.
    Authoring(AuthoringError),
    /// A picture couldn't be kept (the store is full, or failed).
    Picture(AssetError),
}

impl From<AuthoringError> for AnkiError {
    fn from(err: AuthoringError) -> Self {
        AnkiError::Authoring(err)
    }
}

impl From<io::Error> for AnkiError {
    fn from(err: io::Error) -> Self {
        AnkiError::Io(err)
    }
}

/// A folder of its own under the system's temporary folder, removed with
/// everything in it when dropped.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new() -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let name = format!(
            "synapse-anki-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        // `create_dir` (not `_all`) fails if it exists, so it's never shared.
        std::fs::create_dir(&path)?;
        Ok(ScratchDir(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Copies the collection database out of the package at `package` into
/// `folder`, decompressing it if needed, and returns its path. Only that one
/// entry is read, with at most [`ANKI_COLLECTION_MAX_BYTES`] written.
pub fn extract_collection(package: &Path, folder: &Path) -> Result<PathBuf, AnkiError> {
    extract_limited(package, folder, ANKI_COLLECTION_MAX_BYTES)
}

/// [`extract_collection`] with a limit of `limit` bytes, so tests can check
/// the limit without writing half a gigabyte.
fn extract_limited(package: &Path, folder: &Path, limit: u64) -> Result<PathBuf, AnkiError> {
    let file = File::open(package)?;
    if file.metadata()?.len() > ANKI_FILE_MAX_BYTES {
        return Err(AnkiError::TooLarge);
    }
    let mut archive = zip::ZipArchive::new(file).map_err(|_| AnkiError::NotAnki)?;
    let Some(name) = COLLECTION_ENTRIES
        .into_iter()
        .find(|name| archive.index_for_name(name).is_some())
    else {
        return Err(AnkiError::NotAnki);
    };
    let mut entry = archive
        .by_name(name)
        .map_err(|err| AnkiError::Unreadable(format!("opening {name}: {err}")))?;
    if entry.size() > limit {
        return Err(AnkiError::CollectionTooLarge);
    }

    let path = folder.join("collection.sqlite");
    let mut out = File::create(&path)?;
    let copied = if name.ends_with('b') {
        // The entry's own size is the compressed size; the limit is checked
        // on what decompressing writes.
        let decoder = ruzstd::decoding::StreamingDecoder::new(&mut entry)
            .map_err(|err| AnkiError::Unreadable(format!("starting zstd: {err}")))?;
        copy_limited(decoder, &mut out, limit)
    } else {
        copy_limited(&mut entry, &mut out, limit)
    }
    .map_err(|err| match err.kind() {
        io::ErrorKind::FileTooLarge => AnkiError::CollectionTooLarge,
        _ => AnkiError::Unreadable(format!("unpacking {name}: {err}")),
    })?;
    if copied < 100 {
        return Err(AnkiError::NotAnki);
    }
    drop(out);
    use_rollback_journal(&path)?;
    Ok(path)
}

/// Copies at most `limit` bytes; more is an error of kind `FileTooLarge`.
fn copy_limited(source: impl Read, out: &mut File, limit: u64) -> io::Result<u64> {
    let mut limited = source.take(limit + 1);
    let copied = io::copy(&mut limited, out)?;
    if copied > limit {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "collection too large",
        ));
    }
    Ok(copied)
}

/// Checks the copied file is an SQLite database, and if it is marked as using
/// a write-ahead log (as newer Anki's collections are), marks it as using a
/// rollback journal instead, so it opens read-only with no `-wal`/`-shm` files
/// beside it. A packaged collection is complete in itself, so nothing is lost.
/// Only this private copy is changed.
fn use_rollback_journal(path: &Path) -> Result<(), AnkiError> {
    let mut file = File::options().read(true).write(true).open(path)?;
    let mut header = [0u8; 20];
    file.read_exact(&mut header)?;
    if &header[..16] != b"SQLite format 3\0" {
        return Err(AnkiError::NotAnki);
    }
    if header[18] == 2 || header[19] == 2 {
        file.seek(SeekFrom::Start(18))?;
        file.write_all(&[1, 1])?;
    }
    Ok(())
}

/// A note type, as much as is needed to tell which notes become cards.
#[derive(Debug, Default)]
struct NoteType {
    cloze: bool,
    /// Field names, in order.
    fields: Vec<String>,
    /// `(ord, question, answer)` of each card template, in order.
    templates: Vec<(i64, String, String)>,
}

/// What a note type's notes become.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    /// Field `front` on the question, field `back` on the answer, and the
    /// template that reverses them, if there is one.
    FrontBack {
        front: usize,
        back: usize,
        reverse_ord: Option<i64>,
    },
    /// A cloze text in field `text`; `extras` are the other fields the answer
    /// shows, which are left out.
    Cloze {
        text: usize,
        extras: BTreeSet<usize>,
    },
    /// Anki's image occlusion: deletions in field `occlusion`, the picture
    /// and header among `shown` (the other fields the question shows), and
    /// `extras` (such as "Back Extra") shown only on the answer. `None` for
    /// a note type laid out some other way, whose notes are left out.
    ImageOcclusion(Option<OcclusionFields>),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OcclusionFields {
    occlusion: usize,
    shown: Vec<usize>,
    extras: BTreeSet<usize>,
}

/// The fields one template puts on its card.
#[derive(Debug, Default)]
struct TemplateFields {
    /// Fields shown as they are.
    shown: BTreeSet<usize>,
    /// Fields the user types in (`{{type:Field}}`).
    typed: BTreeSet<usize>,
    /// Fields with deletions (`{{cloze:Field}}`); also in `shown`.
    cloze_fields: BTreeSet<usize>,
    cloze: bool,
}

/// Names Anki fills in itself, which are not fields.
const SPECIAL_NAMES: [&str; 8] = [
    "FrontSide",
    "Tags",
    "Deck",
    "Subdeck",
    "Card",
    "CardFlag",
    "Type",
    "CardID",
];

fn template_fields(template: &str, fields: &[String]) -> TemplateFields {
    let mut found = TemplateFields::default();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("}}") else { break };
        let tag = rest[..end].trim();
        rest = &rest[end + 2..];
        // Sections (`{{#Field}}`, `{{^Field}}`, `{{/Field}}`) only decide
        // whether Anki makes the card, and Anki's own cards already say that.
        if tag.starts_with(['#', '^', '/', '!']) {
            continue;
        }
        let mut parts = tag.split(':').map(str::trim);
        let name = parts.next_back().unwrap_or_default();
        let filters: Vec<&str> = parts.collect();
        if filters.contains(&"cloze") {
            found.cloze = true;
        }
        if SPECIAL_NAMES.contains(&name) {
            continue;
        }
        if let Some(index) = fields.iter().position(|field| field == name) {
            if filters.contains(&"cloze") {
                found.cloze_fields.insert(index);
                found.shown.insert(index);
            } else if filters.contains(&"type") {
                found.typed.insert(index);
            } else {
                found.shown.insert(index);
            }
        }
    }
    found
}

/// The field a template asks about and the one it answers with, if it shows
/// exactly one field on the question and adds exactly one on the answer.
fn question_and_answer(question: &str, answer: &str, fields: &[String]) -> Option<(usize, usize)> {
    let question = template_fields(question, fields);
    let answer = template_fields(answer, fields);
    if question.cloze || answer.cloze || question.shown.len() != 1 {
        return None;
    }
    let front = *question.shown.first()?;
    let added: Vec<usize> = answer
        .shown
        .union(&answer.typed)
        .copied()
        .filter(|&field| field != front)
        .collect();
    match added[..] {
        [back] => Some((front, back)),
        _ => None,
    }
}

/// What a cloze note type's notes become: a cloze text when its one template
/// asks with one cloze field and shows nothing else on the question.
fn cloze_shape(note_type: &NoteType) -> Shape {
    // Anki's own image occlusion template sets the picture up by this name.
    if note_type.templates.iter().any(|(_, question, answer)| {
        question.contains(IMAGE_OCCLUSION) || answer.contains(IMAGE_OCCLUSION)
    }) {
        let [(_, question, answer)] = &note_type.templates[..] else {
            return Shape::ImageOcclusion(None);
        };
        let question = template_fields(question, &note_type.fields);
        let answer = template_fields(answer, &note_type.fields);
        let [occlusion] = question.cloze_fields.iter().copied().collect::<Vec<_>>()[..] else {
            return Shape::ImageOcclusion(None);
        };
        let shown = question
            .shown
            .iter()
            .copied()
            .filter(|&field| field != occlusion)
            .collect();
        let extras = answer
            .shown
            .union(&answer.typed)
            .copied()
            .filter(|field| !question.shown.contains(field))
            .collect();
        return Shape::ImageOcclusion(Some(OcclusionFields {
            occlusion,
            shown,
            extras,
        }));
    }
    let [(_, question, answer)] = &note_type.templates[..] else {
        return Shape::Other;
    };
    let question = template_fields(question, &note_type.fields);
    let answer = template_fields(answer, &note_type.fields);
    let Some(&text) = question.cloze_fields.first() else {
        return Shape::Other;
    };
    if question.cloze_fields.len() != 1 || question.shown.len() != 1 || !question.typed.is_empty() {
        return Shape::Other;
    }
    let extras = answer
        .shown
        .union(&answer.typed)
        .copied()
        .filter(|&field| field != text)
        .collect();
    Shape::Cloze { text, extras }
}

fn shape(note_type: &NoteType) -> Shape {
    if note_type.cloze {
        return cloze_shape(note_type);
    }
    // Deletions in a normal note type's template: Anki makes one card per
    // template there, not one per number, so it is neither shape.
    let cloze = note_type.templates.iter().any(|(_, question, answer)| {
        template_fields(question, &note_type.fields).cloze
            || template_fields(answer, &note_type.fields).cloze
    });
    if cloze {
        return Shape::Other;
    }
    let mut templates = note_type.templates.iter();
    let Some((_, question, answer)) = templates.next() else {
        return Shape::Other;
    };
    let Some((front, back)) = question_and_answer(question, answer, &note_type.fields) else {
        return Shape::Other;
    };
    let mut reverse_ord = None;
    for (ord, question, answer) in templates {
        match question_and_answer(question, answer, &note_type.fields) {
            Some(pair) if pair == (front, back) => {}
            Some(pair) if pair == (back, front) => {
                reverse_ord = reverse_ord.or(Some(*ord));
            }
            _ => return Shape::Other,
        }
    }
    Shape::FrontBack {
        front,
        back,
        reverse_ord,
    }
}

/// The fields of a protobuf message that are strings or varints, by field
/// number: just enough to read the note type and template settings of a
/// newer collection, which it stores as protobuf. Anything else is skipped.
fn protobuf_fields(bytes: &[u8]) -> Option<HashMap<u64, ProtoValue<'_>>> {
    let mut found = HashMap::new();
    for (number, value) in protobuf_entries(bytes)? {
        found.entry(number).or_insert(value);
    }
    Some(found)
}

/// Every string or varint field of a protobuf message in order, repeated
/// ones included.
fn protobuf_entries(mut bytes: &[u8]) -> Option<Vec<(u64, ProtoValue<'_>)>> {
    fn varint(bytes: &mut &[u8]) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = bytes.split_first()?;
            *bytes = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
    let mut found = Vec::new();
    while !bytes.is_empty() {
        let key = varint(&mut bytes)?;
        let (number, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let value = varint(&mut bytes)?;
                found.push((number, ProtoValue::Varint(value)));
            }
            1 => bytes = bytes.get(8..)?,
            2 => {
                let len = usize::try_from(varint(&mut bytes)?).ok()?;
                let value = bytes.get(..len)?;
                bytes = &bytes[len..];
                found.push((number, ProtoValue::Bytes(value)));
            }
            5 => bytes = bytes.get(4..)?,
            _ => return None,
        }
    }
    Some(found)
}

#[derive(Debug, Clone, Copy)]
enum ProtoValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

fn proto_string(fields: &HashMap<u64, ProtoValue<'_>>, number: u64) -> String {
    match fields.get(&number) {
        Some(ProtoValue::Bytes(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        _ => String::new(),
    }
}

fn unreadable(err: sqlx::Error) -> AnkiError {
    AnkiError::Unreadable(err.to_string())
}

/// Note types and deck names of an older collection (schema 11), kept as JSON
/// in its `col` row.
async fn read_schema_11(
    conn: &mut SqliteConnection,
) -> Result<(HashMap<i64, NoteType>, HashMap<i64, String>), AnkiError> {
    #[derive(serde::Deserialize)]
    struct Field {
        name: String,
        ord: i64,
    }
    #[derive(serde::Deserialize)]
    struct Template {
        ord: i64,
        qfmt: String,
        afmt: String,
    }
    #[derive(serde::Deserialize)]
    struct Model {
        #[serde(rename = "type", default)]
        kind: i64,
        flds: Vec<Field>,
        tmpls: Vec<Template>,
    }
    #[derive(serde::Deserialize)]
    struct Deck {
        name: String,
    }

    let (models, decks): (String, String) = sqlx::query_as("SELECT models, decks FROM col")
        .fetch_one(&mut *conn)
        .await
        .map_err(unreadable)?;
    let models: HashMap<String, Model> = serde_json::from_str(&models)
        .map_err(|err| AnkiError::Unreadable(format!("note types: {err}")))?;
    let decks: HashMap<String, Deck> = serde_json::from_str(&decks)
        .map_err(|err| AnkiError::Unreadable(format!("decks: {err}")))?;

    let note_types = models
        .into_iter()
        .filter_map(|(id, mut model)| {
            model.flds.sort_by_key(|field| field.ord);
            model.tmpls.sort_by_key(|template| template.ord);
            let note_type = NoteType {
                cloze: model.kind == 1,
                fields: model.flds.into_iter().map(|field| field.name).collect(),
                templates: model
                    .tmpls
                    .into_iter()
                    .map(|template| (template.ord, template.qfmt, template.afmt))
                    .collect(),
            };
            Some((id.parse().ok()?, note_type))
        })
        .collect();
    let decks = decks
        .into_iter()
        .filter_map(|(id, deck)| Some((id.parse().ok()?, deck.name)))
        .collect();
    Ok((note_types, decks))
}

/// Note types and deck names of a newer collection (schema 15 and later), in
/// tables of their own with settings as protobuf.
async fn read_schema_18(
    conn: &mut SqliteConnection,
) -> Result<(HashMap<i64, NoteType>, HashMap<i64, String>), AnkiError> {
    let mut note_types: HashMap<i64, NoteType> = HashMap::new();
    let types: Vec<(i64, Vec<u8>)> = sqlx::query_as("SELECT id, config FROM notetypes")
        .fetch_all(&mut *conn)
        .await
        .map_err(unreadable)?;
    for (id, config) in types {
        let config = protobuf_fields(&config)
            .ok_or_else(|| AnkiError::Unreadable("a note type's settings".to_string()))?;
        // Field 1 is the kind: 0 normal, 1 cloze.
        let cloze = matches!(config.get(&1), Some(ProtoValue::Varint(1)));
        note_types.insert(
            id,
            NoteType {
                cloze,
                ..NoteType::default()
            },
        );
    }

    let fields: Vec<(i64, String)> =
        sqlx::query_as("SELECT ntid, name FROM fields ORDER BY ntid, ord")
            .fetch_all(&mut *conn)
            .await
            .map_err(unreadable)?;
    for (note_type, name) in fields {
        if let Some(note_type) = note_types.get_mut(&note_type) {
            note_type.fields.push(name);
        }
    }

    let templates: Vec<(i64, i64, Vec<u8>)> =
        sqlx::query_as("SELECT ntid, ord, config FROM templates ORDER BY ntid, ord")
            .fetch_all(&mut *conn)
            .await
            .map_err(unreadable)?;
    for (note_type, ord, config) in templates {
        let config = protobuf_fields(&config)
            .ok_or_else(|| AnkiError::Unreadable("a card template's settings".to_string()))?;
        // Fields 1 and 2 are the question and answer formats.
        if let Some(note_type) = note_types.get_mut(&note_type) {
            note_type
                .templates
                .push((ord, proto_string(&config, 1), proto_string(&config, 2)));
        }
    }

    let decks: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM decks")
        .fetch_all(&mut *conn)
        .await
        .map_err(unreadable)?;
    let decks = decks
        .into_iter()
        .map(|(id, name)| (id, name.replace(DECK_SEPARATOR, "::")))
        .collect();
    Ok((note_types, decks))
}

/// Reads a collection copied out by [`extract_collection`], read-only, on a
/// connection of its own. Writes nothing anywhere.
pub async fn read_collection(path: &Path) -> Result<AnkiPackage, AnkiError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(true)
        // Newer collections sort names with Anki's own collation. Nothing
        // here sorts by it, but declaring it means no query can fail on it.
        .collation("unicase", |a: &str, b: &str| {
            a.to_lowercase().cmp(&b.to_lowercase())
        });
    let mut conn = options.connect().await.map_err(unreadable)?;

    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&mut conn)
            .await
            .map_err(|_| AnkiError::NotAnki)?;
    let has = |table: &str| tables.iter().any(|name| name == table);
    if !(has("col") && has("notes") && has("cards")) {
        return Err(AnkiError::NotAnki);
    }
    let (note_types, deck_names) = if has("notetypes") {
        read_schema_18(&mut conn).await?
    } else {
        read_schema_11(&mut conn).await?
    };
    let shapes: HashMap<i64, Shape> = note_types
        .iter()
        .map(|(&id, note_type)| (id, shape(note_type)))
        .collect();

    // Each note's cards: the deck of its first card (a card in a filtered
    // deck counts in its home deck), which templates Anki made, and whether
    // any was reviewed.
    struct Cards {
        deck: i64,
        ords: Vec<i64>,
        reviewed: bool,
    }
    let mut cards: HashMap<i64, Cards> = HashMap::new();
    let rows: Vec<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT nid, ord, CASE WHEN odid != 0 THEN odid ELSE did END, reps
         FROM cards ORDER BY nid, ord",
    )
    .fetch_all(&mut conn)
    .await
    .map_err(unreadable)?;
    for (note, ord, deck, reps) in rows {
        let entry = cards.entry(note).or_insert(Cards {
            deck,
            ords: Vec::new(),
            reviewed: false,
        });
        entry.ords.push(ord);
        entry.reviewed |= reps > 0;
    }

    let notes: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT id, mid, flds FROM notes ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .map_err(unreadable)?;
    let mut package = AnkiPackage {
        decks: Vec::new(),
        notes: notes.len(),
        skipped: Skipped::default(),
        media_removed: 0,
        pictures_left_out: PicturesLeftOut::default(),
        extra_removed: 0,
        reviewed: 0,
        occlusion_approximated: 0,
        pictures: HashMap::new(),
    };
    let mut by_deck: HashMap<i64, Vec<AnkiNote>> = HashMap::new();
    for (id, note_type, fields) in notes {
        let shape = match shapes.get(&note_type) {
            Some(Shape::ImageOcclusion(None)) => {
                package.skipped.image_occlusion += 1;
                continue;
            }
            Some(Shape::Other) | None => {
                package.skipped.other_note_type += 1;
                continue;
            }
            Some(shape) => shape,
        };
        let Some(note_cards) = cards.get(&id) else {
            package.skipped.no_card += 1;
            continue;
        };
        let fields: Vec<&str> = fields.split(FIELD_SEPARATOR).collect();
        let field = |index: usize| fields.get(index).copied().unwrap_or_default();
        let (note, media, extra) = match *shape {
            Shape::FrontBack {
                front,
                back,
                reverse_ord,
            } => {
                let (front_html, back_html) = (field(front), field(back));
                let (front, front_media) = text_and_media(front_html);
                let (back, back_media) = text_and_media(back_html);
                let card = match validate_card(&front, &back) {
                    Ok(card) => card,
                    Err(InvalidInput::FrontTooLong | InvalidInput::BackTooLong) => {
                        package.skipped.too_long += 1;
                        continue;
                    }
                    Err(_) => {
                        package.skipped.empty += 1;
                        continue;
                    }
                };
                let (front_pictures, front_unnamed) = picture_names(front_html);
                let (back_pictures, back_unnamed) = picture_names(back_html);
                package.pictures_left_out.missing += front_unnamed + back_unnamed;
                let note = AnkiNote {
                    front: card.front,
                    back: card.back,
                    reversible: reverse_ord.is_some_and(|ord| note_cards.ords.contains(&ord)),
                    cloze: None,
                    occlusion: None,
                    pictures: AnkiCardPictures {
                        front: front_pictures,
                        back: back_pictures,
                    },
                    cards: 1,
                };
                // Its pictures are kept, or counted on their own.
                (note, front_media.other || back_media.other, false)
            }
            Shape::Cloze { text, ref extras } => {
                let (text, media) = html_to_text(field(text));
                if text.contains(IMAGE_OCCLUSION) {
                    package.skipped.image_occlusion += 1;
                    continue;
                }
                let cloze = match validate_cloze(&text) {
                    Ok(cloze) => cloze,
                    Err(InvalidInput::ClozeTooLong) => {
                        package.skipped.too_long += 1;
                        continue;
                    }
                    Err(InvalidInput::ClozeBlank) => {
                        package.skipped.empty += 1;
                        continue;
                    }
                    Err(_) => {
                        package.skipped.bad_cloze += 1;
                        continue;
                    }
                };
                let extra = extras
                    .iter()
                    .any(|&index| !html_to_text(field(index)).0.trim().is_empty());
                let numbers = cloze.numbers();
                let first = numbers.first().copied().unwrap_or(1);
                let note = AnkiNote {
                    front: cloze.front(first),
                    back: cloze.back(first),
                    reversible: false,
                    cards: numbers.len(),
                    cloze: Some(cloze.text),
                    occlusion: None,
                    pictures: AnkiCardPictures::default(),
                };
                (note, media, extra)
            }
            Shape::ImageOcclusion(Some(ref fields)) => {
                let (occlusion, approximated, extra) = match read_image_occlusion(fields, field) {
                    Ok(read) => read,
                    Err(OcclusionProblem::TooLong) => {
                        package.skipped.too_long += 1;
                        continue;
                    }
                    Err(OcclusionProblem::Unreadable) => {
                        package.skipped.image_occlusion += 1;
                        continue;
                    }
                };
                if approximated {
                    package.occlusion_approximated += 1;
                }
                let numbers = occlusion.occlusion.numbers();
                let first = numbers.first().copied().unwrap_or(1);
                let note = AnkiNote {
                    front: occlusion.occlusion.front(first),
                    back: occlusion.occlusion.back(first),
                    reversible: false,
                    cloze: None,
                    cards: numbers.len(),
                    occlusion: Some(occlusion),
                    pictures: AnkiCardPictures::default(),
                };
                // Its picture is used, not left out.
                (note, false, extra)
            }
            // Left out above.
            Shape::ImageOcclusion(None) | Shape::Other => continue,
        };
        if media {
            package.media_removed += 1;
        }
        if extra {
            package.extra_removed += 1;
        }
        if note_cards.reviewed {
            package.reviewed += 1;
        }
        by_deck.entry(note_cards.deck).or_default().push(note);
    }

    package.decks = by_deck
        .into_iter()
        .map(|(id, notes)| AnkiDeck {
            anki_name: deck_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "Anki".to_string()),
            notes,
        })
        .collect();
    package
        .decks
        .sort_by_cached_key(|deck| deck.anki_name.to_lowercase());

    if package.decks.len() > ANKI_DECKS_MAX {
        return Err(AnkiError::TooManyDecks);
    }
    if package.card_count(false) > ANKI_CARDS_MAX {
        return Err(AnkiError::TooManyCards);
    }
    Ok(package)
}

/// Why an image occlusion note is left out.
enum OcclusionProblem {
    /// No picture named, shapes that can't be read, or no box left.
    Unreadable,
    /// The header, or a number's labels, is too long.
    TooLong,
}

/// Reads an image occlusion note: its picture's name, header and boxes;
/// whether a shape was approximated or left out; and whether an answer-only
/// field held text. The picture itself is read later ([`attach_pictures`]).
fn read_image_occlusion<'a>(
    fields: &OcclusionFields,
    field: impl Fn(usize) -> &'a str,
) -> Result<(AnkiOcclusion, bool, bool), OcclusionProblem> {
    let (picture_field, picture) = fields
        .shown
        .iter()
        .find_map(|&index| picture_name(field(index)).map(|name| (index, name)))
        .ok_or(OcclusionProblem::Unreadable)?;
    let header = fields
        .shown
        .iter()
        .filter(|&&index| index != picture_field)
        .map(|&index| html_to_text(field(index)).0)
        .find(|text| !text.trim().is_empty())
        .unwrap_or_default();
    let (mut boxes, hide_all, approximated) =
        read_shapes(field(fields.occlusion)).ok_or(OcclusionProblem::Unreadable)?;
    if boxes.iter().any(|shape| shape.number > NUMBER_MAX) {
        renumber(&mut boxes);
    }
    let mode = if hide_all {
        Mode::HideAll
    } else {
        Mode::HideOne
    };
    let occlusion = validate_occlusion(&header, mode, &boxes).map_err(|problem| match problem {
        InvalidInput::OcclusionHeaderTooLong | InvalidInput::OcclusionLabelTooLong => {
            OcclusionProblem::TooLong
        }
        _ => OcclusionProblem::Unreadable,
    })?;
    let extra = fields
        .extras
        .iter()
        .any(|&index| !html_to_text(field(index)).0.trim().is_empty());
    Ok((AnkiOcclusion { picture, occlusion }, approximated, extra))
}

/// The media name of the first `<img src="…">` in a field, if it names a
/// file in the package (not an address).
fn picture_name(html: &str) -> Option<String> {
    image_sources(html).into_iter().flatten().next()
}

/// The pictures a front/back field shows: the media names of its
/// `<img src="…">`, in order and each once, and how many name no file in
/// the package (an address, or no `src`).
fn picture_names(html: &str) -> (Vec<String>, usize) {
    let mut names: Vec<String> = Vec::new();
    let mut unnamed = 0;
    for source in image_sources(html) {
        match source {
            Some(name) if !names.contains(&name) => names.push(name),
            Some(_) => {}
            None => unnamed += 1,
        }
    }
    (names, unnamed)
}

/// Each `<img>` in a field, in order: the media name its `src` gives, or
/// `None` when it has none or it isn't a file in the package (an address).
fn image_sources(html: &str) -> Vec<Option<String>> {
    let lower = html.to_ascii_lowercase();
    let mut sources = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img") {
        let start = from + at;
        let Some((len, name, _)) = parse_tag(&html[start..]) else {
            break;
        };
        from = start + len;
        // `<imgx>` isn't a picture.
        if name != "img" {
            continue;
        }
        let tag = &html[start..start + len];
        let lower_tag = tag.to_ascii_lowercase();
        let Some(src) = lower_tag
            .match_indices("src")
            .map(|(at, _)| at)
            .find(|&at| {
                tag[at + 3..].trim_start().starts_with('=')
                    && tag[..at].ends_with(char::is_whitespace)
            })
        else {
            sources.push(None);
            continue;
        };
        let value = tag[src + 3..].trim_start()[1..].trim_start();
        let value = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or_default(),
            _ => value
                .split(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')
                .next()
                .unwrap_or_default(),
        };
        let name = html_to_text(value).0;
        let name = name.trim();
        sources
            .push((!name.is_empty() && !name.contains(['/', '\\', ':'])).then(|| name.to_string()));
    }
    sources
}

/// Anki's image occlusion shapes in a note's occlusion field, as boxes;
/// whether any asks to hide all (`oi=1`); and whether any was approximated
/// or left out. `None` if a deletion isn't a shape, a shape can't be read,
/// or no box is left.
fn read_shapes(html: &str) -> Option<(Vec<OcclusionBox>, bool, bool)> {
    let text = html_to_text(html).0;
    let mut boxes = Vec::new();
    let (mut hide_all, mut approximated) = (false, false);
    let mut rest = text.as_str();
    while let Some(at) = rest.find("{{c") {
        let after = &rest[at + 3..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let body_start = after[digits..].strip_prefix("::")?;
        let number: u32 = after[..digits].parse().ok().filter(|&n| n >= 1)?;
        let end = body_start.find("}}")?;
        let body = body_start[..end]
            .strip_prefix(IMAGE_OCCLUSION)?
            .strip_prefix(':')?;
        rest = &body_start[end + 2..];

        let mut parts = body.split(':');
        let kind = parts.next()?;
        let props: HashMap<&str, &str> = parts.filter_map(|part| part.split_once('=')).collect();
        let num = |key: &str| -> Option<f64> {
            props
                .get(key)?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
        };
        if props.get("oi").is_some_and(|value| value.trim() == "1") {
            hide_all = true;
        }
        if num("angle").is_some_and(|angle| angle != 0.0) {
            approximated = true;
        }
        let (left, top, right, bottom) = match kind {
            "rect" => {
                let (left, top) = (num("left")?, num("top")?);
                (left, top, left + num("width")?, top + num("height")?)
            }
            "ellipse" => {
                approximated = true;
                let (left, top) = (num("left")?, num("top")?);
                (left, top, left + 2.0 * num("rx")?, top + 2.0 * num("ry")?)
            }
            "polygon" => {
                approximated = true;
                let points: Vec<(f64, f64)> = props
                    .get("points")?
                    .split_whitespace()
                    .map(|pair| {
                        let (x, y) = pair.split_once(',')?;
                        Some((x.parse().ok()?, y.parse().ok()?))
                    })
                    .collect::<Option<_>>()?;
                let xs = points.iter().map(|point| point.0);
                let ys = points.iter().map(|point| point.1);
                (
                    xs.clone().fold(f64::INFINITY, f64::min),
                    ys.clone().fold(f64::INFINITY, f64::min),
                    xs.fold(f64::NEG_INFINITY, f64::max),
                    ys.fold(f64::NEG_INFINITY, f64::max),
                )
            }
            // A label written on the picture: nothing to hide.
            "text" => {
                approximated = true;
                continue;
            }
            _ => return None,
        };
        // Early versions of Anki's editor stored pixels, not fractions.
        if [left, top, right, bottom]
            .iter()
            .any(|&value| !value.is_finite() || !(-0.5..=1.5).contains(&value))
        {
            return None;
        }
        // A shape partly off the picture is cut to it.
        let (left, top) = (left.max(0.0), top.max(0.0));
        let (right, bottom) = (right.min(1.0), bottom.min(1.0));
        if right - left < 0.002 || bottom - top < 0.002 {
            approximated = true;
            continue;
        }
        boxes.push(OcclusionBox {
            number,
            left,
            top,
            width: right - left,
            height: bottom - top,
            label: String::new(),
        });
    }
    (!boxes.is_empty()).then_some((boxes, hide_all, approximated))
}

/// Reads at most `limit` bytes of `source`; `None` if it holds more.
fn read_limited(source: impl Read, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    source.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// An entry of the package, decompressed if newer Anki compressed it,
/// within `limit` bytes; `None` if it is missing, larger, or unreadable.
fn read_media_entry(
    archive: &mut zip::ZipArchive<File>,
    name: &str,
    limit: u64,
) -> Option<Vec<u8>> {
    media_entry(archive, name, limit).ok()
}

/// Why a media file of the package wasn't read, or isn't a picture to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaProblem {
    Missing,
    TooLarge,
    /// Damaged, or not a PNG, JPEG, GIF or WebP.
    Unsupported,
}

/// [`read_media_entry`], saying why an entry wasn't read.
fn media_entry(
    archive: &mut zip::ZipArchive<File>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, MediaProblem> {
    let entry = archive.by_name(name).map_err(|_| MediaProblem::Missing)?;
    // zstd adds a little to bytes it can't make smaller.
    let raw = read_limited(entry, limit + limit / 64 + 1024)
        .map_err(|_| MediaProblem::Unsupported)?
        .ok_or(MediaProblem::TooLarge)?;
    let bytes = if raw.starts_with(&ZSTD_MAGIC) {
        let decoder = ruzstd::decoding::StreamingDecoder::new(raw.as_slice())
            .map_err(|_| MediaProblem::Unsupported)?;
        read_limited(decoder, limit).map_err(|_| MediaProblem::Unsupported)?
    } else {
        (raw.len() as u64 <= limit).then_some(raw)
    };
    bytes.ok_or(MediaProblem::TooLarge)
}

/// Which entry of the package holds each media file, by the file's name.
/// Older packages list them as JSON (`{"0": "heart.png"}`), newer ones as
/// protobuf (`MediaEntries`: each entry's name, and the entry is its
/// position unless it says otherwise).
fn media_index(archive: &mut zip::ZipArchive<File>) -> HashMap<String, String> {
    let Some(bytes) = read_media_entry(archive, "media", MEDIA_INDEX_MAX_BYTES) else {
        return HashMap::new();
    };
    if let Ok(names) = serde_json::from_slice::<HashMap<String, String>>(&bytes) {
        return names
            .into_iter()
            .map(|(entry, name)| (name, entry))
            .collect();
    }
    let Some(entries) = protobuf_entries(&bytes) else {
        return HashMap::new();
    };
    entries
        .into_iter()
        .filter(|(number, _)| *number == 1)
        .enumerate()
        .filter_map(|(position, (_, value))| {
            let ProtoValue::Bytes(entry) = value else {
                return None;
            };
            let fields = protobuf_fields(entry)?;
            let name = proto_string(&fields, 1);
            let entry = match fields.get(&255) {
                Some(ProtoValue::Varint(entry)) => entry.to_string(),
                _ => position.to_string(),
            };
            (!name.is_empty()).then_some((name, entry))
        })
        .collect()
}

/// Reads the pictures of the image occlusion notes and front/back cards from
/// the package's media. Leaves out (as image occlusion) each image occlusion
/// note whose picture is missing, isn't a PNG, JPEG, GIF or WebP, is over
/// 20 MB, or doesn't fit in [`ANKI_PICTURES_MAX_BYTES`]; takes such pictures
/// off cards, and pictures past [`CARD_SIDE_PICTURES_MAX`] a side, counting
/// each in [`PicturesLeftOut`]. Writes nothing.
fn attach_pictures(mut package: AnkiPackage, path: &Path) -> AnkiPackage {
    let notes = || package.decks.iter().flat_map(|deck| &deck.notes);
    // Image occlusion notes' pictures first: a note can't do without its.
    let for_occlusions: BTreeSet<String> = notes()
        .filter_map(|note| Some(note.occlusion.as_ref()?.picture.clone()))
        .collect();
    let for_cards: BTreeSet<String> = notes()
        .flat_map(|note| note.pictures.front.iter().chain(&note.pictures.back))
        .filter(|name| !for_occlusions.contains(*name))
        .cloned()
        .collect();
    if for_occlusions.is_empty() && for_cards.is_empty() {
        return package;
    }

    let mut problems: HashMap<String, MediaProblem> = HashMap::new();
    let archive = File::open(path)
        .ok()
        .and_then(|file| zip::ZipArchive::new(file).ok());
    if let Some(mut archive) = archive {
        let index = media_index(&mut archive);
        let mut total = 0u64;
        for name in for_occlusions.into_iter().chain(for_cards) {
            let read = match index.get(&name) {
                None => Err(MediaProblem::Missing),
                // Past the budget, nothing more is read.
                Some(_) if total > ANKI_PICTURES_MAX_BYTES => Err(MediaProblem::TooLarge),
                Some(entry) => media_entry(&mut archive, entry, MAX_PICTURE_BYTES),
            };
            let picture = read.and_then(|bytes| {
                let media_type = assets::sniff(&bytes)
                    .filter(|kind| kind.is_picture())
                    .ok_or(MediaProblem::Unsupported)?;
                total += bytes.len() as u64;
                if total > ANKI_PICTURES_MAX_BYTES {
                    return Err(MediaProblem::TooLarge);
                }
                Ok(AnkiPicture {
                    bytes: bytes.into(),
                    media_type,
                })
            });
            match picture {
                Ok(picture) => {
                    package.pictures.insert(name, picture);
                }
                Err(problem) => {
                    problems.insert(name, problem);
                }
            }
        }
    }

    let mut missing = 0;
    let left_out = &mut package.pictures_left_out;
    for deck in &mut package.decks {
        deck.notes.retain(|note| match &note.occlusion {
            Some(occlusion) if !package.pictures.contains_key(&occlusion.picture) => {
                missing += 1;
                false
            }
            _ => true,
        });
        for note in &mut deck.notes {
            for side in [&mut note.pictures.front, &mut note.pictures.back] {
                side.retain(|name| {
                    if package.pictures.contains_key(name) {
                        return true;
                    }
                    match problems.get(name) {
                        // The package couldn't be opened: as if it had none.
                        Some(MediaProblem::Missing) | None => left_out.missing += 1,
                        Some(MediaProblem::TooLarge) => left_out.too_large += 1,
                        Some(MediaProblem::Unsupported) => left_out.unsupported += 1,
                    }
                    false
                });
                if side.len() > CARD_SIDE_PICTURES_MAX {
                    left_out.too_many += side.len() - CARD_SIDE_PICTURES_MAX;
                    side.truncate(CARD_SIDE_PICTURES_MAX);
                }
            }
        }
    }
    package.decks.retain(|deck| !deck.notes.is_empty());
    package.skipped.image_occlusion += missing;

    // Pictures no card or note shows any more aren't held.
    let used: HashSet<String> = package
        .decks
        .iter()
        .flat_map(|deck| &deck.notes)
        .flat_map(|note| {
            let occlusion = note.occlusion.as_ref().map(|o| &o.picture);
            let sides = note.pictures.front.iter().chain(&note.pictures.back);
            occlusion.into_iter().chain(sides).cloned()
        })
        .collect();
    package.pictures.retain(|name, _| used.contains(name));
    package
}

/// Copies the collection out of a package and reads it: both steps of
/// reading, in a scratch folder removed afterwards. Writes nothing to Synapse.
pub async fn read_package(package: &Path) -> Result<AnkiPackage, AnkiError> {
    let scratch = ScratchDir::new()?;
    let package_path = package.to_path_buf();
    let package = package.to_path_buf();
    let folder = scratch.path().to_path_buf();
    let collection =
        tauri::async_runtime::spawn_blocking(move || extract_collection(&package, &folder))
            .await
            .map_err(|err| AnkiError::Unreadable(format!("unpacking stopped: {err}")))??;
    let read = read_collection(&collection).await;
    drop(scratch);
    let package = read?;
    let path = package_path;
    tauri::async_runtime::spawn_blocking(move || Ok(attach_pictures(package, &path)))
        .await
        .map_err(|err| AnkiError::Unreadable(format!("reading pictures stopped: {err}")))?
}

/// A name of at most `max` characters: `text` cut at a character boundary,
/// without trailing spaces.
fn cut(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The names the decks will get: each Anki name, cut to a deck name's 100
/// characters, with " (2)", " (3)"… added when a deck (in `taken`, compared
/// ignoring case) or an earlier one of these already has it.
pub fn deck_names<'a>(taken: impl IntoIterator<Item = &'a str>, wanted: &[&str]) -> Vec<String> {
    let mut taken: HashSet<String> = taken.into_iter().map(str::to_lowercase).collect();
    wanted
        .iter()
        .map(|wanted| {
            let mut base = cut(wanted.trim(), DECK_NAME_MAX_CHARS);
            if base.is_empty() {
                base = "Anki".to_string();
            }
            let mut name = base.clone();
            let mut number = 2;
            while taken.contains(&name.to_lowercase()) {
                let suffix = format!(" ({number})");
                name = format!(
                    "{}{suffix}",
                    cut(&base, DECK_NAME_MAX_CHARS - suffix.chars().count())
                );
                number += 1;
            }
            taken.insert(name.to_lowercase());
            name
        })
        .collect()
}

/// Every deck name in Synapse, archived and sample decks included.
async fn existing_names(conn: &mut SqliteConnection) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM decks")
        .fetch_all(conn)
        .await
}

/// The names the package's decks would get if saved now. Writes nothing.
pub async fn planned_names(
    pool: &SqlitePool,
    package: &AnkiPackage,
) -> Result<Vec<String>, AnkiError> {
    let mut conn = pool.acquire().await.map_err(AuthoringError::from)?;
    let taken = existing_names(&mut conn)
        .await
        .map_err(AuthoringError::from)?;
    let wanted: Vec<&str> = package
        .decks
        .iter()
        .map(|deck| deck.anki_name.as_str())
        .collect();
    Ok(deck_names(taken.iter().map(String::as_str), &wanted))
}

/// Saves a package: a new deck for each of its decks, a new card for each
/// front/back note (with its pictures) and a cloze text with a new card per
/// number for each cloze note, plus the reversed card of each reversible note
/// with `both_directions` (its pictures the other way round). Each picture is
/// kept once. All or nothing, in one transaction. Names are chosen
/// again at this moment, so a deck created since the preview is never
/// clashed with.
pub async fn import_package(
    pool: &SqlitePool,
    store: &AssetStore,
    package: &AnkiPackage,
    both_directions: bool,
    now: DateTime<Utc>,
) -> Result<AnkiImported, AnkiError> {
    let total = package.card_count(both_directions);
    if total == 0 {
        return Err(AnkiError::NothingToImport);
    }
    if total > ANKI_CARDS_MAX {
        return Err(AnkiError::TooManyCards);
    }
    if package.decks.len() > ANKI_DECKS_MAX {
        return Err(AnkiError::TooManyDecks);
    }

    let kept_at = now;
    let now = to_db_time(now);
    // Each picture is kept once, whatever number of notes use it.
    let mut kept = KeptPictures {
        package,
        store,
        now: kept_at,
        kept: HashMap::new(),
    };
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(AuthoringError::from)?;
    let taken = existing_names(&mut tx)
        .await
        .map_err(AuthoringError::from)?;
    let wanted: Vec<&str> = package
        .decks
        .iter()
        .map(|deck| deck.anki_name.as_str())
        .collect();
    let names = deck_names(taken.iter().map(String::as_str), &wanted);

    let mut imported = AnkiImported {
        decks: Vec::new(),
        cards: 0,
    };
    for (deck, name) in package.decks.iter().zip(names) {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO decks (name, description, is_sample, created_at)
             VALUES (?1, ?2, 0, ?3)
             RETURNING id",
        )
        .bind(&name)
        .bind(IMPORTED_DESCRIPTION)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await
        .map_err(AuthoringError::from)?;

        // Every card the one way first (a cloze note's text with all its
        // cards), then the reversed ones, so a note's two cards aren't
        // studied one straight after the other.
        let forward = deck
            .notes
            .iter()
            .map(|note| match (&note.cloze, &note.occlusion) {
                (Some(text), _) => ToSave::Cloze(text),
                (None, Some(occlusion)) => ToSave::Occlusion(occlusion),
                (None, None) => ToSave::Card(
                    &note.front,
                    &note.back,
                    &note.pictures.front,
                    &note.pictures.back,
                ),
            });
        let reversed = deck
            .notes
            .iter()
            .filter(|note| both_directions && note.reversible)
            .map(|note| {
                ToSave::Card(
                    &note.back,
                    &note.front,
                    &note.pictures.back,
                    &note.pictures.front,
                )
            });
        let mut cards = 0;
        for (index, card) in forward.chain(reversed).enumerate() {
            let refused = |problem| AuthoringError::InvalidCard(index, problem);
            match card {
                ToSave::Card(front, back, front_pictures, back_pictures) => {
                    let card = validate_card(front, back).map_err(refused)?;
                    let card_id = insert_new_card(&mut tx, id, &card, &now)
                        .await
                        .map_err(AuthoringError::from)?;
                    if !(front_pictures.is_empty() && back_pictures.is_empty()) {
                        let mut pictures = CardPictures::default();
                        for name in front_pictures {
                            pictures.front.push(kept.keep(&mut tx, name).await?);
                        }
                        for name in back_pictures {
                            pictures.back.push(kept.keep(&mut tx, name).await?);
                        }
                        set_card_pictures(&mut tx, card_id, &pictures)
                            .await
                            .map_err(AuthoringError::from)?;
                    }
                    cards += 1;
                }
                ToSave::Cloze(text) => {
                    let cloze = validate_cloze(text).map_err(refused)?;
                    cards += insert_cloze(&mut tx, id, &cloze, &now)
                        .await
                        .map_err(AuthoringError::from)?;
                }
                ToSave::Occlusion(note) => {
                    let checked = &note.occlusion;
                    let occlusion =
                        validate_occlusion(&checked.header, checked.mode, &checked.boxes)
                            .map_err(refused)?;
                    let asset_id = kept.keep(&mut tx, &note.picture).await?.id;
                    cards += insert_occlusion(&mut tx, id, asset_id, &occlusion, &now)
                        .await
                        .map_err(AuthoringError::from)?;
                }
            }
        }
        imported.cards += cards;
        imported.decks.push(ImportedDeck { id, name, cards });
    }
    tx.commit().await.map_err(AuthoringError::from)?;
    Ok(imported)
}

/// The pictures [`import_package`] has kept so far, so each is kept once,
/// whatever number of notes and cards show it.
struct KeptPictures<'a> {
    package: &'a AnkiPackage,
    store: &'a AssetStore,
    now: DateTime<Utc>,
    kept: HashMap<&'a str, Asset>,
}

impl<'a> KeptPictures<'a> {
    /// The kept file of the package's picture `name`, keeping it if needed.
    async fn keep(
        &mut self,
        conn: &mut SqliteConnection,
        name: &'a str,
    ) -> Result<Asset, AnkiError> {
        if let Some(asset) = self.kept.get(name) {
            return Ok(asset.clone());
        }
        let picture = self.package.pictures.get(name).ok_or_else(|| {
            AuthoringError::Internal("a picture to keep wasn't read from the package".into())
        })?;
        let asset = assets::put(
            conn,
            self.store,
            &picture.bytes,
            picture.media_type,
            self.now,
        )
        .await
        .map_err(AnkiError::Picture)?;
        self.kept.insert(name, asset.clone());
        Ok(asset)
    }
}

/// One thing [`import_package`] saves into a deck.
enum ToSave<'a> {
    /// A card's front and back, and the pictures on each.
    Card(&'a str, &'a str, &'a [String], &'a [String]),
    /// A cloze text, with its cards.
    Cloze(&'a str),
    /// An image occlusion, with its picture and cards.
    Occlusion(&'a AnkiOcclusion),
}

/// A package read and waiting for the user to add or cancel it, under a
/// number the screen sends back. Managed by Tauri, like `import::ImportState`.
#[derive(Default)]
pub struct AnkiImports {
    held: Mutex<(u64, Option<(u64, AnkiPackage)>)>,
}

impl AnkiImports {
    /// Holds `package` in place of any other, and returns its number.
    pub fn hold(&self, package: AnkiPackage) -> u64 {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.0 += 1;
        let token = held.0;
        held.1 = Some((token, package));
        token
    }

    /// A copy of the package held under `token`, if it still is.
    pub fn get(&self, token: u64) -> Option<AnkiPackage> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        match &held.1 {
            Some((held_token, package)) if *held_token == token => Some(package.clone()),
            _ => None,
        }
    }

    /// Forgets the package held under `token`, if it still is.
    pub fn forget(&self, token: u64) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held
            .1
            .as_ref()
            .is_some_and(|(held_token, _)| *held_token == token)
        {
            held.1 = None;
        }
    }
}

/// Tags whose start or end begins a new line.
const BLOCK_TAGS: [&str; 20] = [
    "address",
    "article",
    "blockquote",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "ol",
    "p",
    "pre",
    "ul",
];
/// Tags that are a picture, sound, or video, which is left out.
const MEDIA_TAGS: [&str; 7] = ["img", "audio", "video", "source", "object", "embed", "svg"];
/// Tags whose contents are never text.
const HIDDEN_TAGS: [&str; 3] = ["script", "style", "svg"];

/// A tag at the start of `text` (which starts with `<`): its length in bytes,
/// its name in lowercase, and whether it closes (`</p>`). `None` when `<`
/// doesn't start a tag, so it is text.
fn parse_tag(text: &str) -> Option<(usize, String, bool)> {
    let inner = &text[1..];
    let closing = inner.starts_with('/');
    let name_start = usize::from(closing);
    let name: String = inner[name_start..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if !name.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
        return None;
    }
    let mut quote = None;
    for (index, ch) in inner.char_indices().skip(name_start + name.len()) {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(open), _) if ch == open => quote = None,
            (None, '>') => return Some((index + 2, name.to_ascii_lowercase(), closing)),
            _ => {}
        }
    }
    None
}

/// The character an entity at the start of `text` (which starts with `&`)
/// stands for, and the entity's length. `None` when it isn't one, so `&` is
/// text.
fn parse_entity(text: &str) -> Option<(char, usize)> {
    let end = text[1..]
        .char_indices()
        .take(32)
        .find(|&(_, ch)| ch == ';')?
        .0
        + 1;
    let name = &text[1..end];
    let ch = if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        char::from_u32(code).filter(|&ch| ch != '\0')?
    } else {
        match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => '\u{a0}',
            "ensp" | "emsp" | "thinsp" => ' ',
            "shy" => '\u{ad}',
            "ndash" => '–',
            "mdash" => '—',
            "hellip" => '…',
            "lsquo" => '‘',
            "rsquo" => '’',
            "ldquo" => '“',
            "rdquo" => '”',
            "laquo" => '«',
            "raquo" => '»',
            "bull" => '•',
            "middot" => '·',
            "deg" => '°',
            "times" => '×',
            "divide" => '÷',
            "plusmn" => '±',
            "minus" => '−',
            "le" => '≤',
            "ge" => '≥',
            "ne" => '≠',
            "larr" => '←',
            "rarr" => '→',
            "harr" => '↔',
            "uarr" => '↑',
            "darr" => '↓',
            "copy" => '©',
            "reg" => '®',
            "trade" => '™',
            "euro" => '€',
            "pound" => '£',
            "yen" => '¥',
            "cent" => '¢',
            "sect" => '§',
            "para" => '¶',
            "micro" => 'µ',
            "alpha" => 'α',
            "beta" => 'β',
            "gamma" => 'γ',
            "delta" => 'δ',
            "pi" => 'π',
            "sigma" => 'σ',
            "mu" => 'μ',
            "lambda" => 'λ',
            "theta" => 'θ',
            "omega" => 'ω',
            "Delta" => 'Δ',
            "Sigma" => 'Σ',
            "Omega" => 'Ω',
            _ => return None,
        }
    };
    Some((ch, end + 1))
}

/// Plain text being built from HTML: runs of white space become one space,
/// and line breaks are only where the HTML puts them.
struct PlainText {
    text: String,
}

impl PlainText {
    fn push(&mut self, ch: char) {
        match ch {
            '\u{ad}' => {}
            // A no-break space is a space the HTML meant, so it is kept.
            '\u{a0}' => self.text.push(' '),
            ch if ch.is_whitespace() => {
                if !self.text.is_empty() && !self.text.ends_with([' ', '\n']) {
                    self.text.push(' ');
                }
            }
            ch => self.text.push(ch),
        }
    }

    /// `<br>`: always a new line.
    fn line_break(&mut self) {
        self.text.push('\n');
    }

    /// The edge of a block (`<div>`, `<p>`…): a new line, unless at one.
    fn block_edge(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    /// Each line without outer spaces, at most one blank line in a row, and
    /// no blank lines at either end.
    fn finish(self) -> String {
        let mut out = String::new();
        let mut blank = 0;
        for line in self.text.split('\n').map(str::trim) {
            if line.is_empty() {
                blank += 1;
                continue;
            }
            if !out.is_empty() {
                out.push_str(if blank > 0 { "\n\n" } else { "\n" });
            }
            out.push_str(line);
            blank = 0;
        }
        out
    }
}

/// A field's HTML as plain text, and whether it had a picture, sound, or
/// video (left out). Tags go; `<br>` and the edges of blocks become line
/// breaks; list items start with `- `; table cells are separated by tabs;
/// entities are decoded; other white space is collapsed as a browser would.
/// Anki's `[sound:…]` tags go too.
pub fn html_to_text(html: &str) -> (String, bool) {
    let (text, media) = text_and_media(html);
    (text, media.pictures || media.other)
}

/// What media a field had, beside its text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Media {
    /// An `<img>`.
    pictures: bool,
    /// A sound, video, or other embedded thing (an inline SVG, say).
    other: bool,
}

/// [`html_to_text`], telling pictures apart from other media.
fn text_and_media(html: &str) -> (String, Media) {
    let mut media = Media::default();
    let mut out = PlainText {
        text: String::new(),
    };
    let mut index = 0;
    while index < html.len() {
        let rest = &html[index..];
        if rest.starts_with("[sound:") {
            if let Some(end) = rest.find(']') {
                media.other = true;
                index += end + 1;
                continue;
            }
        }
        if rest.starts_with("<!--") {
            index += rest.find("-->").map_or(rest.len(), |end| end + 3);
            continue;
        }
        if rest.starts_with('<') {
            if let Some((len, name, closing)) = parse_tag(rest) {
                index += len;
                let name = name.as_str();
                if name == "img" {
                    media.pictures = true;
                } else if MEDIA_TAGS.contains(&name) {
                    media.other = true;
                }
                if HIDDEN_TAGS.contains(&name) && !closing && !rest[..len].ends_with("/>") {
                    // Skip to the end of the matching closing tag.
                    let after = html[index..].to_ascii_lowercase();
                    index += after
                        .find(&format!("</{name}"))
                        .and_then(|start| after[start..].find('>').map(|end| start + end + 1))
                        .unwrap_or(after.len());
                    continue;
                }
                match name {
                    "br" => out.line_break(),
                    "li" => {
                        out.block_edge();
                        if !closing {
                            out.text.push_str("- ");
                        }
                    }
                    "tr" | "table" => out.block_edge(),
                    "td" | "th" if closing => out.text.push('\t'),
                    name if BLOCK_TAGS.contains(&name) => out.block_edge(),
                    _ => {}
                }
                continue;
            }
        }
        if rest.starts_with('&') {
            if let Some((ch, len)) = parse_entity(rest) {
                out.push(ch);
                index += len;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or_default();
        out.push(ch);
        index += ch.len_utf8();
    }
    (out.finish(), media)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{archive_deck, create_deck};
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;

    const NOW: &str = "2026-10-19T12:00:00Z";

    /// Packages exported by Anki 25.09 itself from one collection (made by
    /// `testdata/anki/make_fixtures.py`): the newer format (zstd), the legacy
    /// one ("support older Anki versions"), and a whole-collection backup.
    const NEW_APKG: &[u8] = include_bytes!("../testdata/anki/new.apkg");
    const LEGACY_APKG: &[u8] = include_bytes!("../testdata/anki/legacy.apkg");
    const COLPKG: &[u8] = include_bytes!("../testdata/anki/all.colpkg");
    /// Image occlusion notes with their pictures (the second collection in
    /// `make_fixtures.py`), in the newer and the legacy format.
    const OCCLUSION_APKG: &[u8] = include_bytes!("../testdata/anki/occlusion.apkg");
    const OCCLUSION_LEGACY_APKG: &[u8] = include_bytes!("../testdata/anki/occlusion-legacy.apkg");
    /// Front/back notes with pictures (the third collection in
    /// `make_fixtures.py`), in the newer and the legacy format.
    const PICTURES_APKG: &[u8] = include_bytes!("../testdata/anki/pictures.apkg");
    const PICTURES_LEGACY_APKG: &[u8] = include_bytes!("../testdata/anki/pictures-legacy.apkg");

    /// A store folder of its own for each test that saves.
    fn test_store() -> AssetStore {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        crate::assets::test_support::scratch_store(&format!(
            "anki-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    /// Writes `bytes` as a package in a scratch folder and reads it.
    async fn read_async(bytes: &[u8]) -> Result<AnkiPackage, AnkiError> {
        let scratch = ScratchDir::new().unwrap();
        let path = scratch.path().join("deck.apkg");
        std::fs::write(&path, bytes).unwrap();
        read_package(&path).await
    }

    fn read_bytes(bytes: &[u8]) -> Result<AnkiPackage, AnkiError> {
        tauri::async_runtime::block_on(read_async(bytes))
    }

    /// A zip archive of `(name, bytes)` entries, deflated.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    fn note<'a>(package: &'a AnkiPackage, front: &str) -> (&'a str, &'a AnkiNote) {
        package
            .decks
            .iter()
            .find_map(|deck| {
                deck.notes
                    .iter()
                    .find(|note| note.front == front)
                    .map(|note| (deck.anki_name.as_str(), note))
            })
            .unwrap_or_else(|| panic!("no note {front:?}"))
    }

    /// What Anki's own export holds (see `make_fixtures.py`), read the same
    /// from each format.
    fn check_fixture(package: &AnkiPackage) {
        let decks: Vec<(&str, usize)> = package
            .decks
            .iter()
            .map(|deck| (deck.anki_name.as_str(), deck.notes.len()))
            .collect();
        assert_eq!(
            decks,
            vec![("Biology", 3), ("Biology::Cells", 2), ("French", 4)]
        );
        assert_eq!(package.notes, 14);
        assert_eq!(
            package.skipped,
            Skipped {
                image_occlusion: 1,
                bad_cloze: 1,
                other_note_type: 1,
                empty: 1,
                too_long: 1,
                no_card: 0,
            }
        );
        // The sound; the picture beside it isn't in the package (exported
        // without media).
        assert_eq!(package.media_removed, 1);
        assert_eq!(
            package.pictures_left_out,
            PicturesLeftOut {
                missing: 1,
                ..PicturesLeftOut::default()
            }
        );
        assert_eq!(package.extra_removed, 1);
        assert_eq!(package.reviewed, 1);

        let (deck, cell) = note(package, "What is a cell?");
        assert_eq!(deck, "Biology");
        assert_eq!(
            cell.back,
            "The basic unit of life.\nFound in all organisms."
        );
        assert!(!cell.reversible);
        let (_, energy) = note(package, "Mitochondria role");
        assert_eq!(energy.back, "Make energy\n(ATP) & more <3");
        let (_, picture) = note(package, "Picture");
        assert_eq!(picture.back, "A cell");
        // Reversible only when Anki made the reversed card.
        assert!(note(package, "chat").1.reversible);
        assert!(note(package, "chien").1.reversible);
        assert!(!note(package, "oiseau").1.reversible);
        let (_, typed) = note(package, "maison");
        assert_eq!((typed.back.as_str(), typed.reversible), ("house", false));
        assert_eq!(note(package, "chat").1.cloze, None);

        // Cloze notes: the text as plain text, one card per number, and the
        // first card's front and back for the preview.
        let (deck, mitosis) = note(package, "[...] divides cells");
        assert_eq!(deck, "Biology");
        assert_eq!(
            mitosis.cloze.as_deref(),
            Some("{{c1::Mitosis}} divides cells")
        );
        assert_eq!((mitosis.back.as_str(), mitosis.cards), ("Mitosis", 1));
        let (_, ribosomes) = note(package, "[...] make proteins\nin the [...]");
        assert_eq!(
            ribosomes.cloze.as_deref(),
            Some("{{c1::Ribosomes}} make {{c2::proteins::what?}}\nin the {{c1::cytoplasm}}")
        );
        assert_eq!(ribosomes.back, "Ribosomes; cytoplasm");
        assert_eq!((ribosomes.cards, ribosomes.reversible), (2, false));

        assert_eq!(package.card_count(false), 10);
        assert_eq!(package.card_count(true), 12);
    }

    #[test]
    fn anki_packages_are_read_in_every_format() {
        check_fixture(&read_bytes(NEW_APKG).unwrap());
        check_fixture(&read_bytes(LEGACY_APKG).unwrap());
        check_fixture(&read_bytes(COLPKG).unwrap());
    }

    #[test]
    fn the_newest_collection_in_a_package_is_read() {
        // A package with only the "please update Anki" stub isn't preferred
        // over the real collection, whatever the order of its entries.
        let mut archive = zip::ZipArchive::new(io::Cursor::new(NEW_APKG)).unwrap();
        let mut entry = |name: &str| {
            let mut bytes = Vec::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        };
        let stub = entry("collection.anki2");
        let real = entry("collection.anki21b");
        let package = zip_of(&[("collection.anki2", &stub), ("collection.anki21b", &real)]);
        check_fixture(&read_bytes(&package).unwrap());

        // The stub alone reads as what it is: one note with an empty back.
        let only_stub = read_bytes(&zip_of(&[("collection.anki2", &stub)])).unwrap();
        assert!(only_stub.decks.is_empty());
        assert_eq!(only_stub.notes, 1);
    }

    #[test]
    fn files_that_are_not_anki_packages_are_refused() {
        assert!(matches!(read_bytes(b"not a zip"), Err(AnkiError::NotAnki)));
        let no_collection = zip_of(&[("media", b"{}")]);
        assert!(matches!(
            read_bytes(&no_collection),
            Err(AnkiError::NotAnki)
        ));
        let not_sqlite = zip_of(&[("collection.anki2", &[7u8; 4096])]);
        assert!(matches!(read_bytes(&not_sqlite), Err(AnkiError::NotAnki)));
        let not_zstd = zip_of(&[("collection.anki21b", &[7u8; 4096])]);
        assert!(matches!(
            read_bytes(&not_zstd),
            Err(AnkiError::Unreadable(_))
        ));

        // An SQLite database that isn't a collection.
        let scratch = ScratchDir::new().unwrap();
        let other = scratch.path().join("other.sqlite");
        tauri::async_runtime::block_on(async {
            let options = SqliteConnectOptions::new()
                .filename(&other)
                .create_if_missing(true);
            let mut conn = options.connect().await.unwrap();
            sqlx::query("CREATE TABLE things (id INTEGER)")
                .execute(&mut conn)
                .await
                .unwrap();
        });
        let bytes = std::fs::read(&other).unwrap();
        let package = zip_of(&[("collection.anki2", &bytes)]);
        assert!(matches!(read_bytes(&package), Err(AnkiError::NotAnki)));
    }

    #[test]
    fn a_collection_that_unpacks_too_large_is_refused() {
        const LIMIT: u64 = 64 * 1024;
        let scratch = ScratchDir::new().unwrap();
        let path = scratch.path().join("big.apkg");
        let extract = |entries: &[(&str, &[u8])]| {
            std::fs::write(&path, zip_of(entries)).unwrap();
            extract_limited(&path, scratch.path(), LIMIT)
        };
        let zeros = vec![0u8; LIMIT as usize + 1];
        // Zeros compress to almost nothing, so only unpacking finds the size.
        let compressed = ruzstd::encoding::compress_to_vec(
            &zeros[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert!(compressed.len() < 1024);
        assert!(matches!(
            extract(&[("collection.anki21b", &compressed)]),
            Err(AnkiError::CollectionTooLarge)
        ));
        assert!(matches!(
            extract(&[("collection.anki2", &zeros)]),
            Err(AnkiError::CollectionTooLarge)
        ));
        // Up to the limit is unpacked (then found not to be SQLite).
        assert!(matches!(
            extract(&[("collection.anki2", &zeros[1..])]),
            Err(AnkiError::NotAnki)
        ));
    }

    #[test]
    fn the_scratch_folder_is_removed() {
        let scratch = ScratchDir::new().unwrap();
        let path = scratch.path().to_path_buf();
        std::fs::write(path.join("collection.sqlite"), b"x").unwrap();
        assert!(path.exists());
        drop(scratch);
        assert!(!path.exists());
    }

    fn note_type(cloze: bool, fields: &[&str], templates: &[(&str, &str)]) -> NoteType {
        NoteType {
            cloze,
            fields: fields.iter().map(|name| name.to_string()).collect(),
            templates: templates
                .iter()
                .enumerate()
                .map(|(ord, (question, answer))| {
                    (ord as i64, question.to_string(), answer.to_string())
                })
                .collect(),
        }
    }

    #[test]
    fn note_types_are_told_apart_by_their_templates() {
        let basic_answer = "{{FrontSide}}<hr id=answer>{{Back}}";
        let fields = ["Front", "Back"];
        assert_eq!(
            shape(&note_type(false, &fields, &[("{{Front}}", basic_answer)])),
            Shape::FrontBack {
                front: 0,
                back: 1,
                reverse_ord: None
            }
        );
        // Whatever the fields are called, in any order.
        assert_eq!(
            shape(&note_type(
                false,
                &["Antwort", "Frage"],
                &[("{{ Frage }}", "{{FrontSide}}<hr>{{Antwort}}")]
            )),
            Shape::FrontBack {
                front: 1,
                back: 0,
                reverse_ord: None
            }
        );
        // A reverse template, optional or not.
        assert_eq!(
            shape(&note_type(
                false,
                &["Front", "Back", "Add Reverse"],
                &[
                    ("{{Front}}", basic_answer),
                    (
                        "{{#Add Reverse}}{{Back}}{{/Add Reverse}}",
                        "{{FrontSide}}<hr>{{Front}}"
                    ),
                ]
            )),
            Shape::FrontBack {
                front: 0,
                back: 1,
                reverse_ord: Some(1)
            }
        );
        // Typing the answer, and filters such as `text:`.
        assert_eq!(
            shape(&note_type(
                false,
                &fields,
                &[(
                    "{{text:Front}}\n{{type:Back}}",
                    "{{Front}}<hr>{{type:Back}}"
                )]
            )),
            Shape::FrontBack {
                front: 0,
                back: 1,
                reverse_ord: None
            }
        );
        // Cloze, by kind and template; what else the answer shows is left
        // out. Deletions in a normal type, which Anki doesn't number, aren't.
        let cloze_fields = ["Back Extra", "Text"];
        assert_eq!(
            shape(&note_type(
                true,
                &cloze_fields,
                &[("{{cloze:Text}}", "{{cloze:Text}}<br>\n{{Back Extra}}")]
            )),
            Shape::Cloze {
                text: 1,
                extras: BTreeSet::from([0])
            }
        );
        assert_eq!(
            shape(&note_type(
                true,
                &cloze_fields,
                &[("{{#Text}}{{text:cloze:Text}}{{/Text}}", "{{cloze:Text}}")]
            )),
            Shape::Cloze {
                text: 1,
                extras: BTreeSet::new()
            }
        );
        assert_eq!(
            shape(&note_type(
                false,
                &["Text"],
                &[("{{cloze:Text}}", "{{cloze:Text}}")]
            )),
            Shape::Other
        );
        // A cloze type that asks with another field too, with two cloze
        // fields, or with no deletions.
        for question in [
            "{{Back Extra}} {{cloze:Text}}",
            "{{cloze:Text}} {{cloze:Back Extra}}",
            "{{Text}}",
        ] {
            assert_eq!(
                shape(&note_type(
                    true,
                    &cloze_fields,
                    &[(question, "{{cloze:Text}}")]
                )),
                Shape::Other,
                "{question}"
            );
        }
        // Image occlusion, by its template.
        assert_eq!(
            shape(&note_type(
                true,
                &["Occlusion", "Image"],
                &[(
                    "<div>{{cloze:Occlusion}}</div>{{Image}}<script>anki.imageOcclusion.setup();</script><div id=\"image-occlusion-container\">",
                    "{{cloze:Occlusion}}"
                )]
            )),
            Shape::ImageOcclusion(Some(OcclusionFields {
                occlusion: 0,
                shown: vec![1],
                extras: BTreeSet::new(),
            }))
        );
        // Laid out some other way: left out.
        assert_eq!(
            shape(&note_type(
                true,
                &["Occlusion", "Image"],
                &[
                    ("{{cloze:Occlusion}}{{Image}} image-occlusion", "{{Image}}"),
                    ("{{Image}}", "{{cloze:Occlusion}} image-occlusion"),
                ]
            )),
            Shape::ImageOcclusion(None)
        );
        // Two fields on the answer, two on the question, none at all, or a
        // second template that asks something else.
        let vocab = ["Word", "Meaning", "Example"];
        assert_eq!(
            shape(&note_type(
                false,
                &vocab,
                &[("{{Word}}", "{{Meaning}}<br>{{Example}}")]
            )),
            Shape::Other
        );
        assert_eq!(
            shape(&note_type(
                false,
                &vocab,
                &[("{{Word}} {{Example}}", "{{Meaning}}")]
            )),
            Shape::Other
        );
        assert_eq!(
            shape(&note_type(false, &fields, &[("Hi", "{{Back}}")])),
            Shape::Other
        );
        assert_eq!(
            shape(&note_type(
                false,
                &vocab,
                &[("{{Word}}", "{{Meaning}}"), ("{{Example}}", "{{Word}}")]
            )),
            Shape::Other
        );
        assert_eq!(shape(&note_type(false, &fields, &[])), Shape::Other);
    }

    #[test]
    fn html_becomes_plain_text() {
        let text = |html: &str| html_to_text(html).0;
        assert_eq!(text("What is a <b>cell</b>?"), "What is a cell?");
        assert_eq!(text("one<br>two<br/>three"), "one\ntwo\nthree");
        assert_eq!(
            text("line<div>next</div><div><br></div><div>after a gap</div>"),
            "line\nnext\n\nafter a gap"
        );
        assert_eq!(text("<p>One</p><p>Two</p>"), "One\nTwo");
        assert_eq!(text("<ul><li>a</li><li>b</li></ul>"), "- a\n- b");
        assert_eq!(
            text("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>"),
            "a\tb\nc"
        );
        assert_eq!(text("a  \n  b\t c"), "a b c");
        assert_eq!(
            text("&lt;b&gt; &amp; &quot;x&quot; &#39;y&#x27; caf&eacute;"),
            "<b> & \"x\" 'y' caf&eacute;"
        );
        assert_eq!(text("a&nbsp;&nbsp;b &mdash; &rarr; &#8364;"), "a  b — → €");
        assert_eq!(text("1 < 2 and 3 > 2 & so on"), "1 < 2 and 3 > 2 & so on");
        assert_eq!(
            text("<style>.x{}</style>Hi<script>alert(1)</script>!<!-- note -->"),
            "Hi!"
        );
        assert_eq!(text("<span title=\"a > b\">kept</span>"), "kept");
        assert_eq!(text("  <div> padded </div>  "), "padded");
        assert_eq!(text(""), "");
    }

    #[test]
    fn pictures_and_sounds_are_left_out_and_noticed() {
        assert_eq!(
            html_to_text("A <img src=\"a.png\"> cell"),
            ("A cell".to_string(), true)
        );
        assert_eq!(
            html_to_text("Word [sound:word.mp3]"),
            ("Word".to_string(), true)
        );
        assert_eq!(
            html_to_text("<img src=\"only.png\">"),
            (String::new(), true)
        );
        assert_eq!(
            html_to_text("[not a sound]"),
            ("[not a sound]".to_string(), false)
        );
    }

    #[test]
    fn protobuf_settings_are_read() {
        // Field 1 varint 1 (a cloze kind), field 3 a string.
        let fields = protobuf_fields(&[0x08, 0x01, 0x1a, 0x02, b'h', b'i']).unwrap();
        assert!(matches!(fields.get(&1), Some(ProtoValue::Varint(1))));
        assert_eq!(proto_string(&fields, 3), "hi");
        assert_eq!(proto_string(&fields, 4), "");
        // A fixed64 and a fixed32 are skipped; a cut-off message is refused.
        assert!(protobuf_fields(&[0x09, 0, 0, 0, 0, 0, 0, 0, 0, 0x15, 0, 0, 0, 0]).is_some());
        assert!(protobuf_fields(&[0x0a, 0x05, b'a']).is_none());
    }

    #[test]
    fn deck_names_never_clash() {
        let names = deck_names(
            ["Biology", "french (2)"],
            &["Biology", "French", "French", "BIOLOGY", "  "],
        );
        assert_eq!(
            names,
            vec!["Biology (2)", "French", "French (3)", "BIOLOGY (3)", "Anki"]
        );

        let long = "x".repeat(150);
        let names = deck_names([], &[long.as_str(), long.as_str()]);
        assert_eq!(names[0], "x".repeat(100));
        assert_eq!(names[1], format!("{} (2)", "x".repeat(96)));
        assert!(names
            .iter()
            .all(|name| name.chars().count() <= DECK_NAME_MAX_CHARS));
    }

    #[test]
    fn a_package_is_saved_as_new_decks_of_new_cards() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            // An archived deck still holds its name.
            let french = create_deck(&pool, "French", None, at(NOW))
                .await
                .unwrap()
                .id;
            archive_deck(&pool, french, at(NOW)).await.unwrap();
            let package = read_async(NEW_APKG).await.unwrap();
            let before = count(&pool, "SELECT COUNT(*) FROM flashcards").await;

            assert_eq!(
                planned_names(&pool, &package).await.unwrap(),
                vec!["Biology", "Biology::Cells", "French (2)"]
            );
            // Planning wrote nothing.
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM decks").await, 2);

            let imported = import_package(&pool, &test_store(), &package, false, at(NOW))
                .await
                .unwrap();
            let summary: Vec<(&str, usize)> = imported
                .decks
                .iter()
                .map(|deck| (deck.name.as_str(), deck.cards))
                .collect();
            assert_eq!(
                summary,
                vec![("Biology", 4), ("Biology::Cells", 2), ("French (2)", 4)]
            );
            assert_eq!(imported.cards, 10);
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM flashcards").await,
                before + 10
            );
            // The cloze notes are cloze texts, a card per number, in order.
            let cloze: Vec<(String, String, String, i64, String)> = sqlx::query_as(
                "SELECT f.front, f.back, f.kind, f.cloze_number, t.text
                 FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 JOIN cloze_texts t ON t.id = f.cloze_text_id
                 WHERE d.name = 'Biology' ORDER BY f.id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            let ribosomes =
                "{{c1::Ribosomes}} make {{c2::proteins::what?}}\nin the {{c1::cytoplasm}}";
            assert_eq!(
                cloze,
                vec![
                    (
                        "[...] divides cells".to_string(),
                        "Mitosis".to_string(),
                        "cloze".to_string(),
                        1,
                        "{{c1::Mitosis}} divides cells".to_string()
                    ),
                    (
                        "[...] make proteins\nin the [...]".to_string(),
                        "Ribosomes; cytoplasm".to_string(),
                        "cloze".to_string(),
                        1,
                        ribosomes.to_string()
                    ),
                    (
                        "Ribosomes make [what?]\nin the cytoplasm".to_string(),
                        "proteins".to_string(),
                        "cloze".to_string(),
                        2,
                        ribosomes.to_string()
                    ),
                ]
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM cloze_texts").await, 2);
            // New cards, with no schedule or reviews.
            let stored: Vec<(String, String, String, Option<String>, i64, i64)> = sqlx::query_as(
                "SELECT f.front, f.back, f.fsrs_state, f.due, f.reps, d.is_sample
                 FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 WHERE d.name = 'French (2)' ORDER BY f.id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(stored.len(), 4);
            assert!(stored
                .iter()
                .all(|(_, _, state, due, reps, sample)| state == "New"
                    && due.is_none()
                    && *reps == 0
                    && *sample == 0));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM review_logs").await, 0);
            let description: Option<String> =
                sqlx::query_scalar("SELECT description FROM decks WHERE name = 'Biology'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(description.as_deref(), Some(IMPORTED_DESCRIPTION));

            // Importing again makes decks of their own.
            let again = import_package(&pool, &test_store(), &package, false, at(NOW))
                .await
                .unwrap();
            let names: Vec<&str> = again.decks.iter().map(|deck| deck.name.as_str()).collect();
            assert_eq!(
                names,
                vec!["Biology (2)", "Biology::Cells (2)", "French (3)"]
            );
        });
    }

    #[test]
    fn reversed_cards_are_added_only_when_asked() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let package = read_async(NEW_APKG).await.unwrap();
            let imported = import_package(&pool, &test_store(), &package, true, at(NOW))
                .await
                .unwrap();
            assert_eq!(imported.cards, 12);
            let french: Vec<(String, String)> = sqlx::query_as(
                "SELECT f.front, f.back FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 WHERE d.name = 'French' ORDER BY f.id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            let fronts: Vec<&str> = french.iter().map(|(front, _)| front.as_str()).collect();
            // The one way first, then the reversed ones.
            assert_eq!(
                fronts,
                vec!["chat", "chien", "oiseau", "maison", "cat", "dog"]
            );
            assert_eq!(french[4], ("cat".to_string(), "chat".to_string()));
        });
    }

    #[test]
    fn nothing_is_saved_when_anything_is_refused() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let decks_before = count(&pool, "SELECT COUNT(*) FROM decks").await;
            let mut package = read_async(NEW_APKG).await.unwrap();
            // A card that fails a typed card's checks, in the last deck.
            package.decks[2].notes.push(AnkiNote {
                front: "x".repeat(2_001),
                back: "y".to_string(),
                reversible: false,
                cloze: None,
                occlusion: None,
                pictures: AnkiCardPictures::default(),
                cards: 1,
            });
            let refused = import_package(&pool, &test_store(), &package, false, at(NOW)).await;
            assert!(matches!(
                refused,
                Err(AnkiError::Authoring(AuthoringError::InvalidCard(
                    4,
                    InvalidInput::FrontTooLong
                )))
            ));
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM decks").await,
                decks_before
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);

            // A cloze text that fails a typed text's checks, after a cloze
            // text and cards that would have been saved.
            package.decks[2].notes.pop();
            package.decks[2].notes.push(AnkiNote {
                front: "No".to_string(),
                back: "deletion".to_string(),
                reversible: false,
                cloze: Some("No deletion".to_string()),
                occlusion: None,
                pictures: AnkiCardPictures::default(),
                cards: 1,
            });
            let refused = import_package(&pool, &test_store(), &package, false, at(NOW)).await;
            assert!(matches!(
                refused,
                Err(AnkiError::Authoring(AuthoringError::InvalidCard(
                    4,
                    InvalidInput::ClozeNoDeletion
                )))
            ));
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM decks").await,
                decks_before
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM cloze_texts").await, 0);

            let empty = AnkiPackage {
                decks: Vec::new(),
                notes: 3,
                skipped: Skipped::default(),
                media_removed: 0,
                pictures_left_out: PicturesLeftOut::default(),
                extra_removed: 0,
                reviewed: 0,
                occlusion_approximated: 0,
                pictures: HashMap::new(),
            };
            assert!(matches!(
                import_package(&pool, &test_store(), &empty, true, at(NOW)).await,
                Err(AnkiError::NothingToImport)
            ));

            let note = AnkiNote {
                front: "q".to_string(),
                back: "a".to_string(),
                reversible: true,
                cloze: None,
                occlusion: None,
                pictures: AnkiCardPictures::default(),
                cards: 1,
            };
            let many = AnkiPackage {
                decks: vec![AnkiDeck {
                    anki_name: "Big".to_string(),
                    notes: vec![note; ANKI_CARDS_MAX / 2 + 1],
                }],
                ..empty
            };
            assert!(import_package(&pool, &test_store(), &many, false, at(NOW))
                .await
                .is_ok());
            assert!(matches!(
                import_package(&pool, &test_store(), &many, true, at(NOW)).await,
                Err(AnkiError::TooManyCards)
            ));
        });
    }

    #[test]
    fn a_held_package_is_given_back_only_by_its_number() {
        let imports = AnkiImports::default();
        let package = read_bytes(NEW_APKG).unwrap();
        let first = imports.hold(package.clone());
        let second = imports.hold(package.clone());
        assert!(imports.get(first).is_none());
        assert_eq!(imports.get(second), Some(package));
        imports.forget(first);
        assert!(imports.get(second).is_some());
        imports.forget(second);
        assert!(imports.get(second).is_none());
    }

    #[test]
    fn image_occlusion_notes_are_read_with_their_pictures() {
        for bytes in [OCCLUSION_APKG, OCCLUSION_LEGACY_APKG] {
            let package = read_bytes(bytes).unwrap();
            assert_eq!(package.notes, 5);
            // The note whose picture isn't in the package, and the one with
            // pixel coordinates.
            assert_eq!(
                package.skipped,
                Skipped {
                    image_occlusion: 2,
                    ..Skipped::default()
                }
            );
            assert_eq!(package.extra_removed, 1);
            assert_eq!(package.media_removed, 0);
            assert_eq!(package.occlusion_approximated, 1);
            let [deck] = &package.decks[..] else {
                panic!("one deck");
            };
            assert_eq!(deck.anki_name, "Anatomy");
            assert_eq!(deck.notes.len(), 3);
            assert_eq!(package.card_count(false), 2 + 2 + 1);

            let (_, heart) = note(&package, "Heart\n[picture: box 1]");
            let occlusion = heart.occlusion.as_ref().unwrap();
            assert_eq!(occlusion.picture, "heart.png");
            assert_eq!(heart.cards, 2);
            let checked = &occlusion.occlusion;
            assert_eq!(checked.mode, Mode::HideAll);
            let boxes: Vec<(u32, f64, f64, f64, f64)> = checked
                .boxes
                .iter()
                .map(|shape| {
                    (
                        shape.number,
                        shape.left,
                        shape.top,
                        shape.width,
                        shape.height,
                    )
                })
                .collect();
            // The ellipse and polygon as the rectangles around them; the
            // text label left out.
            assert_eq!(
                boxes,
                vec![
                    (1, 0.1, 0.2, 0.3, 0.4),
                    (2, 0.5, 0.1, 0.2, 0.1),
                    (2, 0.6, 0.6, 0.2, 0.3),
                ]
            );

            let (_, plain) = note(&package, "[picture: box 1]");
            let checked = &plain.occlusion.as_ref().unwrap().occlusion;
            assert_eq!(checked.mode, Mode::HideOne);
            assert_eq!(checked.numbers().into_iter().collect::<Vec<_>>(), [1, 3]);
            assert_eq!(note(&package, "Pump").1.occlusion, None);

            let [(name, picture)] = &package.pictures.iter().collect::<Vec<_>>()[..] else {
                panic!("one picture");
            };
            assert_eq!(name.as_str(), "heart.png");
            assert_eq!(picture.media_type, MediaType::Png);
            assert!(picture.bytes.starts_with(b"\x89PNG"));
        }
    }

    #[test]
    fn image_occlusion_notes_are_saved_with_their_picture_kept_once() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = test_store();
            let package = read_async(OCCLUSION_APKG).await.unwrap();
            let imported = import_package(&pool, &store, &package, false, at(NOW))
                .await
                .unwrap();
            assert_eq!(imported.cards, 5);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM occlusions").await, 2);
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM flashcards WHERE occlusion_id IS NOT NULL"
                )
                .await,
                4
            );
            let sha: String = sqlx::query_scalar("SELECT sha256 FROM assets")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(store.verify(&sha, package.pictures["heart.png"].bytes.len() as u64));

            // Reviewed like one drawn in Synapse.
            let deck = imported.decks[0].id;
            let detail = crate::authoring::deck_detail(&pool, deck, at(NOW))
                .await
                .unwrap();
            let card = detail
                .cards
                .iter()
                .find(|card| card.front == "Heart\n[picture: box 2]")
                .unwrap();
            assert_eq!(card.back, "[box 2]");
            let view = crate::occlusion::get_occlusion(&pool, card.id)
                .await
                .unwrap();
            assert_eq!(view.picture.sha256, sha);
            assert_eq!(view.boxes.len(), 3);

            // Importing again reuses the kept picture.
            import_package(&pool, &store, &package, false, at(NOW))
                .await
                .unwrap();
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM occlusions").await, 4);
        });
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn front_back_pictures_are_read_from_the_package() {
        for bytes in [PICTURES_APKG, PICTURES_LEGACY_APKG] {
            let package = read_bytes(bytes).unwrap();
            assert_eq!(package.notes, 8);
            // The note whose front is only a picture has no text there.
            assert_eq!(
                package.skipped,
                Skipped {
                    empty: 1,
                    ..Skipped::default()
                }
            );
            // "Gone" (not in the package, and a web address), "Vector" (an
            // SVG) and "Many"'s fifth.
            assert_eq!(
                package.pictures_left_out,
                PicturesLeftOut {
                    missing: 2,
                    unsupported: 1,
                    too_large: 0,
                    too_many: 1,
                }
            );
            // The sound, and the cloze note's picture.
            assert_eq!(package.media_removed, 2);

            let (_, heart) = note(&package, "Heart");
            assert_eq!(heart.back, "Pumps blood");
            assert_eq!(heart.pictures.front, names(&["heart.png"]));
            assert_eq!(heart.pictures.back, names(&["dot.gif"]));
            let (_, valve) = note(&package, "Valve");
            assert!(valve.reversible);
            assert_eq!(valve.pictures.front, names(&["heart.png"]));
            assert!(valve.pictures.back.is_empty());
            let (_, many) = note(&package, "Many");
            assert_eq!(
                many.pictures.front,
                names(&["p1.png", "p2.png", "p3.png", "p4.png"])
            );
            for front in ["Gone", "Vector"] {
                assert_eq!(note(&package, front).1.pictures.count(), 0);
            }
            assert_eq!(
                note(&package, "Sound").1.pictures.front,
                names(&["heart.png"])
            );
            let (_, cloze) = note(&package, "[...]");
            assert_eq!(cloze.pictures.count(), 0);

            // Only the pictures cards show are held, each once.
            let mut held: Vec<&str> = package.pictures.keys().map(String::as_str).collect();
            held.sort_unstable();
            assert_eq!(
                held,
                [
                    "dot.gif",
                    "heart.png",
                    "p1.png",
                    "p2.png",
                    "p3.png",
                    "p4.png"
                ]
            );
            assert_eq!(package.pictures["dot.gif"].media_type, MediaType::Gif);
            assert_eq!(package.pictures["heart.png"].media_type, MediaType::Png);
        }
    }

    #[test]
    fn front_back_pictures_are_saved_with_their_cards_and_kept_once() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = test_store();
            let package = read_async(PICTURES_APKG).await.unwrap();
            let imported = import_package(&pool, &store, &package, true, at(NOW))
                .await
                .unwrap();
            // Six front/back notes, a reversed card and a cloze card.
            assert_eq!(imported.cards, 8);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 6);
            // Heart 2, Valve 1 and its reversed card 1, Many 4, Sound 1.
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM card_pictures").await, 9);

            let detail = crate::authoring::deck_detail(&pool, imported.decks[0].id, at(NOW))
                .await
                .unwrap();
            let card = |front: &str| {
                detail
                    .cards
                    .iter()
                    .find(|card| card.front == front)
                    .unwrap_or_else(|| panic!("no card {front:?}"))
            };
            let sha = |name: &str| assets::sha256_hex(&package.pictures[name].bytes);
            let shas = |list: &[Asset]| list.iter().map(|a| a.sha256.clone()).collect::<Vec<_>>();
            let heart = card("Heart");
            assert_eq!(shas(&heart.pictures.front), [sha("heart.png")]);
            assert_eq!(shas(&heart.pictures.back), [sha("dot.gif")]);
            assert_eq!(heart.pictures.back[0].media_type, MediaType::Gif);
            // The reversed card shows the front's picture as its answer.
            let reversed = card("Lets blood one way");
            assert!(reversed.pictures.front.is_empty());
            assert_eq!(shas(&reversed.pictures.back), [sha("heart.png")]);
            let many = card("Many");
            assert_eq!(
                shas(&many.pictures.front),
                ["p1.png", "p2.png", "p3.png", "p4.png"].map(sha)
            );
            assert_eq!(card("Gone").pictures, CardPictures::default());
            for name in package.pictures.keys() {
                let bytes = &package.pictures[name].bytes;
                assert!(store.verify(&sha(name), bytes.len() as u64));
            }

            // Shown in review with the card, as a picture added in Synapse.
            let reviewed =
                crate::pictures::card_pictures(&mut pool.acquire().await.unwrap(), heart.id)
                    .await
                    .unwrap();
            assert_eq!(reviewed, heart.pictures);

            // Importing again reuses the kept pictures.
            import_package(&pool, &store, &package, false, at(NOW))
                .await
                .unwrap();
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 6);
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM card_pictures").await,
                9 + 8
            );
        });
    }

    #[test]
    fn card_pictures_too_large_or_not_pictures_are_left_out() {
        let scratch = ScratchDir::new().unwrap();
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let mut huge = png.clone();
        huge.resize(MAX_PICTURE_BYTES as usize + 1, 0);
        let index = br#"{"0": "big.png", "1": "notes.txt", "2": "small.png"}"#;
        let bytes = zip_of(&[
            ("media", index),
            ("0", &huge),
            ("1", b"just text"),
            ("2", &png),
        ]);
        let path = scratch.path().join("pictures.apkg");
        std::fs::write(&path, bytes).unwrap();

        let side = names(&["big.png", "notes.txt", "small.png", "nowhere.png"]);
        let package = AnkiPackage {
            decks: vec![AnkiDeck {
                anki_name: "Deck".to_string(),
                notes: vec![AnkiNote {
                    front: "q".to_string(),
                    back: "a".to_string(),
                    reversible: false,
                    cloze: None,
                    occlusion: None,
                    pictures: AnkiCardPictures {
                        front: side.clone(),
                        back: side,
                    },
                    cards: 1,
                }],
            }],
            notes: 1,
            skipped: Skipped::default(),
            media_removed: 0,
            pictures_left_out: PicturesLeftOut::default(),
            extra_removed: 0,
            reviewed: 0,
            occlusion_approximated: 0,
            pictures: HashMap::new(),
        };
        let package = attach_pictures(package, &path);
        // The card stays, with only the picture that could be kept.
        let note = &package.decks[0].notes[0];
        assert_eq!(note.pictures.front, names(&["small.png"]));
        assert_eq!(note.pictures.back, names(&["small.png"]));
        assert_eq!(
            package.pictures_left_out,
            PicturesLeftOut {
                missing: 2,
                unsupported: 2,
                too_large: 2,
                too_many: 0,
            }
        );
        assert_eq!(package.pictures.len(), 1);
    }

    #[test]
    fn image_occlusion_shapes_and_pictures_are_read_carefully() {
        assert_eq!(
            picture_name(r#"<div><IMG alt="x" SRC='a&amp;b.png'></div>"#),
            Some("a&b.png".to_string())
        );
        assert_eq!(
            picture_name("<img src=heart.png>"),
            Some("heart.png".to_string())
        );
        assert_eq!(
            picture_name(r#"<img srcset="x.png"><img src="y.png">"#),
            Some("y.png".to_string())
        );
        assert_eq!(
            picture_name(r#"<img src="https://example.com/a.png">"#),
            None
        );
        assert_eq!(picture_name(r#"<img src="../a.png">"#), None);
        assert_eq!(picture_name("no picture"), None);
        // Every picture of a side, each once, and those naming no file.
        assert_eq!(
            picture_names(
                r#"<img src="a.png"> x <img src='b.gif'><img src="a.png"><img src="http://x/c.png"><img alt="no source"><imgx src="d.png">"#
            ),
            (names(&["a.png", "b.gif"]), 2)
        );
        assert_eq!(picture_names("no picture"), (Vec::new(), 0));

        // Not a shape, or an unknown one: the note is left out.
        assert_eq!(read_shapes("{{c1::Mitosis}}"), None);
        assert_eq!(read_shapes("{{c1::image-occlusion:star:left=.1}}"), None);
        assert_eq!(
            read_shapes("{{c1::image-occlusion:rect:left=.1:top=.1}}"),
            None
        );
        // Only labels: nothing to hide.
        assert_eq!(
            read_shapes("{{c1::image-occlusion:text:left=.1:top=.1:text=A}}"),
            None
        );
        // A shape partly off the picture is cut to it, and a rotated one
        // counts as approximated.
        let (boxes, hide_all, approximated) = read_shapes(
            "{{c2::image-occlusion:rect:left=-.1:top=.9:width=.3:height=.3:angle=10}}<br>",
        )
        .unwrap();
        assert!(!hide_all && approximated);
        assert_eq!(
            boxes
                .iter()
                .map(|b| (b.number, b.left, b.top))
                .collect::<Vec<_>>(),
            [(2, 0.0, 0.9)]
        );
        assert!((boxes[0].width - 0.2).abs() < 1e-9 && (boxes[0].height - 0.1).abs() < 1e-9);
    }

    #[test]
    fn media_lists_of_both_formats_are_read() {
        let scratch = ScratchDir::new().unwrap();
        let png = crate::assets::test_support::png(0);
        let mut compressed = Vec::new();
        // A stored zstd frame, as newer Anki writes: magic, a frame header
        // with a single segment, then one raw block.
        compressed.extend_from_slice(&ZSTD_MAGIC);
        compressed.push(0x20);
        compressed.push(png.len() as u8);
        let block = ((png.len() as u32) << 3) | 1;
        compressed.extend_from_slice(&block.to_le_bytes()[..3]);
        compressed.extend_from_slice(&png);
        // MediaEntries { entries: [{ name: "a.png" }, { name: "b.png", legacy_zip_filename: 7 }] }
        let mut index = vec![0x0a, 7, 0x0a, 5];
        index.extend_from_slice(b"a.png");
        index.extend_from_slice(&[0x0a, 10, 0x0a, 5]);
        index.extend_from_slice(b"b.png");
        index.extend_from_slice(&[0xf8, 0x0f, 7]);
        let bytes = zip_of(&[("media", &index), ("0", &compressed), ("7", &png)]);
        let path = scratch.path().join("media.zip");
        std::fs::write(&path, bytes).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let found = media_index(&mut archive);
        assert_eq!(found.get("a.png").map(String::as_str), Some("0"));
        assert_eq!(found.get("b.png").map(String::as_str), Some("7"));
        assert_eq!(read_media_entry(&mut archive, "0", 1024), Some(png.clone()));
        assert_eq!(read_media_entry(&mut archive, "7", 1024), Some(png.clone()));
        // Over the limit, or missing: nothing.
        assert_eq!(read_media_entry(&mut archive, "7", 10), None);
        assert_eq!(read_media_entry(&mut archive, "8", 1024), None);
    }
}
