//! Image occlusion cards (migration 0018): boxes drawn over a kept picture,
//! where each box number is its own card, as each deletion number of a cloze
//! text is (`cloze.rs`).
//!
//! A box is `{ number, left, top, width, height, label }`. Its coordinates
//! are fractions (0 to 1) of the picture's width and height, so they mean
//! the same at any size the picture is shown. Several boxes may share a
//! number: they are covered and uncovered together, on one card. The label
//! is optional; it is the card's answer as text (search, lists, exports),
//! and the review shows it once the box is uncovered.
//!
//! The picture and boxes are stored once, in `occlusions`; each card is an
//! ordinary `flashcards` row with that occlusion and its number. A card's
//! `front` names the header and its box, and its `back` is its label (or its
//! box). They are written from the boxes whenever they are saved, so every
//! part of Synapse that reads a card's front and back works on these cards
//! unchanged. Reviews draw the card from the picture and its boxes
//! ([`session_view`]).
//!
//! Two ways to ask, for all of an occlusion's cards ([`Mode`]): every box
//! covered and the asked one marked (Anki's "hide all, guess one"), or only
//! the asked box covered ("hide one, guess one").
//!
//! Editing the boxes ([`update_occlusion`]) follows cloze's rules exactly: a
//! number still drawn keeps its card and schedule, a number whose boxes are
//! all gone has its card deleted (softly), and a new number gets a card, or
//! brings back the deleted card it had in the same deck. The picture never
//! changes: boxes are about one picture. The cards of one occlusion stay in
//! one deck, and a deleted card is restored only if its number is still
//! drawn and the occlusion's active cards are in its deck.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};

use crate::assets::{self, Asset};
use crate::authoring::{
    active_normal_deck, changeable_card_deck, load_deck_detail, AuthoringError, DeckDetail,
    InvalidInput, CARD_TEXT_MAX_CHARS,
};
use crate::cloze::{ClozeChanges, RestoreBlock};
use crate::db::{to_db_time, DbError};
use crate::study;

/// The longest header, shown above the picture. Migration 0018 repeats it.
pub const HEADER_MAX_CHARS: usize = 500;
/// The longest label of one box.
pub const LABEL_MAX_CHARS: usize = 200;
/// The most boxes on one picture. Migration 0018 repeats it.
pub const BOXES_MAX: usize = 200;
/// The largest box number, as for a cloze deletion.
pub const NUMBER_MAX: u32 = 999;
/// The smallest side a box can have, as a fraction of the picture's: a box
/// thinner than this is a slip of the mouse, not something to hide.
const MIN_SIDE: f64 = 0.002;
/// How far a box may stick out of the picture (rounding in the editor) before
/// it is refused rather than pulled in.
const SLACK: f64 = 0.001;

/// How a card asks: `"hideAll"` or `"hideOne"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// Every box is covered, and the card's own boxes are marked.
    HideAll,
    /// Only the card's own boxes are covered.
    HideOne,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::HideAll => "hideAll",
            Mode::HideOne => "hideOne",
        }
    }

    fn parse(text: &str) -> Option<Mode> {
        match text {
            "hideAll" => Some(Mode::HideAll),
            "hideOne" => Some(Mode::HideOne),
            _ => None,
        }
    }
}

/// One box: `{ number, left, top, width, height, label }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcclusionBox {
    pub number: u32,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub label: String,
}

/// A checked header, way of asking and set of boxes.
#[derive(Debug, Clone, PartialEq)]
pub struct Occlusion {
    pub header: String,
    pub mode: Mode,
    pub boxes: Vec<OcclusionBox>,
}

/// Rounds to four decimals: a tenth of a pixel on a 1,000-pixel picture.
fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// Checks one side of a box (`start` and `size` along it) and returns it
/// pulled inside the picture and rounded.
fn side(start: f64, size: f64) -> Result<(f64, f64), InvalidInput> {
    if !start.is_finite() || !size.is_finite() {
        return Err(InvalidInput::OcclusionBox);
    }
    if start < -SLACK || size <= 0.0 || start + size > 1.0 + SLACK {
        return Err(InvalidInput::OcclusionBox);
    }
    let start = round(start.max(0.0));
    let size = round(size.min(1.0 - start));
    if size < MIN_SIDE {
        return Err(InvalidInput::OcclusionBox);
    }
    Ok((start, size))
}

/// Checks an occlusion as the editor (or an Anki note) sends it.
pub(crate) fn validate_occlusion(
    header: &str,
    mode: Mode,
    boxes: &[OcclusionBox],
) -> Result<Occlusion, InvalidInput> {
    let header = header.trim();
    if header.chars().count() > HEADER_MAX_CHARS {
        return Err(InvalidInput::OcclusionHeaderTooLong);
    }
    if boxes.is_empty() {
        return Err(InvalidInput::OcclusionNoBoxes);
    }
    if boxes.len() > BOXES_MAX {
        return Err(InvalidInput::OcclusionTooManyBoxes);
    }
    let mut checked = Vec::with_capacity(boxes.len());
    for shape in boxes {
        if !(1..=NUMBER_MAX).contains(&shape.number) {
            return Err(InvalidInput::OcclusionBox);
        }
        let (left, width) = side(shape.left, shape.width)?;
        let (top, height) = side(shape.top, shape.height)?;
        let label = shape.label.trim();
        if label.chars().count() > LABEL_MAX_CHARS {
            return Err(InvalidInput::OcclusionLabelTooLong);
        }
        checked.push(OcclusionBox {
            number: shape.number,
            left,
            top,
            width,
            height,
            label: label.to_string(),
        });
    }
    let occlusion = Occlusion {
        header: header.to_string(),
        mode,
        boxes: checked,
    };
    // Labels of one number are joined into its back, which is a card's text.
    if occlusion
        .numbers()
        .into_iter()
        .any(|number| occlusion.back(number).chars().count() > CARD_TEXT_MAX_CHARS)
    {
        return Err(InvalidInput::OcclusionLabelTooLong);
    }
    Ok(occlusion)
}

impl Occlusion {
    /// The box numbers, each once, smallest first: one card each.
    pub(crate) fn numbers(&self) -> BTreeSet<u32> {
        self.boxes.iter().map(|shape| shape.number).collect()
    }

    /// The card's stored front: the header, then which box it asks about.
    pub(crate) fn front(&self, number: u32) -> String {
        let asks = format!("[picture: box {number}]");
        if self.header.is_empty() {
            asks
        } else {
            format!("{}\n{asks}", self.header)
        }
    }

    /// The card's stored back: its boxes' labels, each once, joined by "; ",
    /// or which box it is when none has a label.
    pub(crate) fn back(&self, number: u32) -> String {
        let mut labels: Vec<&str> = Vec::new();
        for shape in self.boxes.iter().filter(|shape| shape.number == number) {
            if !shape.label.is_empty() && !labels.contains(&shape.label.as_str()) {
                labels.push(&shape.label);
            }
        }
        if labels.is_empty() {
            format!("[box {number}]")
        } else {
            labels.join("; ")
        }
    }

    fn boxes_json(&self) -> String {
        // Plain numbers and strings: this can't fail.
        serde_json::to_string(&self.boxes).unwrap_or_else(|_| "[]".to_string())
    }
}

/// An occlusion as stored, with its picture.
struct Stored {
    asset_id: i64,
    occlusion: Occlusion,
}

async fn stored(conn: &mut SqliteConnection, occlusion_id: i64) -> Result<Stored, AuthoringError> {
    let (asset_id, header, mode, boxes): (i64, String, String, String) =
        sqlx::query_as("SELECT asset_id, header, mode, boxes FROM occlusions WHERE id = ?1")
            .bind(occlusion_id)
            .fetch_one(&mut *conn)
            .await?;
    // What was saved was checked when it was saved; anything that no longer
    // reads was changed outside Synapse.
    let broken = |what: &str| {
        AuthoringError::Internal(format!("stored occlusion {occlusion_id}: {what}").into())
    };
    let mode = Mode::parse(&mode).ok_or_else(|| broken("mode"))?;
    let boxes: Vec<OcclusionBox> = serde_json::from_str(&boxes).map_err(|_| broken("boxes"))?;
    let occlusion =
        validate_occlusion(&header, mode, &boxes).map_err(|problem| broken(&format!("{problem:?}")))?;
    Ok(Stored {
        asset_id,
        occlusion,
    })
}

/// A card of an occlusion: its number, deck, and whether it is deleted.
#[derive(Debug, sqlx::FromRow)]
struct CardRow {
    id: i64,
    deck_id: i64,
    occlusion_number: i64,
    deleted: bool,
}

/// Every card of an occlusion, deleted ones included, most recently deleted
/// first (active cards come last), then newest first.
async fn cards_of(
    conn: &mut SqliteConnection,
    occlusion_id: i64,
) -> Result<Vec<CardRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, deck_id, occlusion_number, deleted_at IS NOT NULL AS deleted
         FROM flashcards WHERE occlusion_id = ?1
         ORDER BY deleted_at IS NULL, deleted_at DESC, id DESC",
    )
    .bind(occlusion_id)
    .fetch_all(conn)
    .await
}

/// The occlusion a card was made from and its number, or `None` for any
/// other card.
async fn occlusion_of(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<Option<(i64, u32)>, sqlx::Error> {
    let row: Option<(Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT occlusion_id, occlusion_number FROM flashcards WHERE id = ?1")
            .bind(card_id)
            .fetch_optional(conn)
            .await?;
    Ok(match row {
        Some((Some(id), Some(number))) => Some((id, number as u32)),
        _ => None,
    })
}

/// The kept picture with this id, if it is a picture.
async fn picture(conn: &mut SqliteConnection, asset_id: i64) -> Result<Asset, AuthoringError> {
    match assets::asset_by_id(conn, asset_id).await {
        Ok(Some(asset)) if asset.media_type.is_picture() => Ok(asset),
        Ok(_) => Err(AuthoringError::PictureMissing),
        Err(detail) => Err(AuthoringError::Internal(detail)),
    }
}

/// Writes one new occlusion of the picture `asset_id` into `deck_id`, with a
/// brand-new card for each of its numbers, smallest first; returns how many
/// cards. The caller has already checked the deck and the picture.
pub(crate) async fn insert_occlusion(
    conn: &mut SqliteConnection,
    deck_id: i64,
    asset_id: i64,
    occlusion: &Occlusion,
    now: &str,
) -> Result<usize, sqlx::Error> {
    let occlusion_id: i64 = sqlx::query_scalar(
        "INSERT INTO occlusions (asset_id, header, mode, boxes, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         RETURNING id",
    )
    .bind(asset_id)
    .bind(&occlusion.header)
    .bind(occlusion.mode.as_str())
    .bind(occlusion.boxes_json())
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    let numbers = occlusion.numbers();
    for &number in &numbers {
        insert_card(conn, deck_id, occlusion_id, occlusion, number, now).await?;
    }
    Ok(numbers.len())
}

/// A brand-new card for one number, started as a typed card is.
async fn insert_card(
    conn: &mut SqliteConnection,
    deck_id: i64,
    occlusion_id: i64,
    occlusion: &Occlusion,
    number: u32,
    now: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO flashcards (deck_id, front, back, fsrs_state, due, reps, lapses,
                                 created_at, updated_at, occlusion_id, occlusion_number)
         VALUES (?1, ?2, ?3, 'New', NULL, 0, 0, ?4, ?4, ?5, ?6)",
    )
    .bind(deck_id)
    .bind(occlusion.front(number))
    .bind(occlusion.back(number))
    .bind(now)
    .bind(occlusion_id)
    .bind(number)
    .execute(conn)
    .await?;
    Ok(())
}

/// Rewrites an active card's front and back from `occlusion`, if they
/// changed. Nothing else of the card is written.
async fn rewrite_card(
    conn: &mut SqliteConnection,
    card_id: i64,
    occlusion: &Occlusion,
    number: u32,
    now: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE flashcards SET front = ?1, back = ?2, updated_at = ?3
         WHERE id = ?4 AND deleted_at IS NULL AND (front IS NOT ?1 OR back IS NOT ?2)",
    )
    .bind(occlusion.front(number))
    .bind(occlusion.back(number))
    .bind(now)
    .bind(card_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Adds an occlusion of a kept picture to a normal deck, with one new card
/// per box number, and returns the deck. Refused, with nothing written, as
/// adding a card refuses a deck, or if `asset_id` isn't a kept picture.
pub async fn create_occlusion(
    pool: &SqlitePool,
    deck_id: i64,
    asset_id: i64,
    header: &str,
    mode: Mode,
    boxes: &[OcclusionBox],
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let occlusion = validate_occlusion(header, mode, boxes)?;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    active_normal_deck(&mut tx, deck_id).await?;
    picture(&mut tx, asset_id).await?;
    insert_occlusion(&mut tx, deck_id, asset_id, &occlusion, &now).await?;
    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// An occlusion as the edit form opens it:
/// `{ id, picture, header, mode, boxes, nextNumber }`, where `nextNumber` is
/// above every number its boxes and cards (deleted ones too) ever had, so a
/// new box never takes over an old card's schedule.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcclusionView {
    pub id: i64,
    pub picture: Asset,
    pub header: String,
    pub mode: Mode,
    pub boxes: Vec<OcclusionBox>,
    pub next_number: u32,
}

/// The occlusion an active card was made from, for its edit form.
pub async fn get_occlusion(pool: &SqlitePool, card_id: i64) -> Result<OcclusionView, AuthoringError> {
    let mut conn = pool.acquire().await?;
    changeable_card_deck(&mut conn, card_id).await?;
    let (occlusion_id, _) = occlusion_of(&mut conn, card_id)
        .await?
        .ok_or(AuthoringError::WrongKind)?;
    let Stored {
        asset_id,
        occlusion,
    } = stored(&mut conn, occlusion_id).await?;
    let picture = picture(&mut conn, asset_id).await?;
    let used = cards_of(&mut conn, occlusion_id)
        .await?
        .iter()
        .map(|card| card.occlusion_number as u32)
        .chain(occlusion.numbers())
        .max()
        .unwrap_or(0);
    Ok(OcclusionView {
        id: occlusion_id,
        picture,
        header: occlusion.header,
        mode: occlusion.mode,
        boxes: occlusion.boxes,
        next_number: (used + 1).min(NUMBER_MAX),
    })
}

/// One change an edit makes to a card.
enum Change {
    Keep { card_id: i64, number: u32 },
    Delete { card_id: i64 },
    Restore { card_id: i64, number: u32 },
    Add { number: u32 },
}

fn summarize(changes: &[Change]) -> ClozeChanges {
    let mut summary = ClozeChanges::default();
    for change in changes {
        match change {
            Change::Keep { .. } => summary.kept += 1,
            Change::Delete { .. } => summary.deleted += 1,
            Change::Restore { .. } => summary.restored += 1,
            Change::Add { .. } => summary.added += 1,
        }
    }
    summary.cards = summary.kept + summary.restored + summary.added;
    summary
}

/// Replaces the header, way of asking and boxes of `card_id`'s occlusion, by
/// the rules in this module's header, and returns the deck with what
/// changed. All in one transaction; schedules, review history, ids and decks
/// are never changed, only fronts and backs, and cards deleted or brought
/// back exactly as deleting and restoring one would.
pub async fn update_occlusion(
    pool: &SqlitePool,
    card_id: i64,
    header: &str,
    mode: Mode,
    boxes: &[OcclusionBox],
    now: DateTime<Utc>,
) -> Result<(ClozeChanges, DeckDetail), AuthoringError> {
    let occlusion = validate_occlusion(header, mode, boxes)?;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the card can't be edited.
    let deck_id = changeable_card_deck(&mut tx, card_id).await?;
    let (occlusion_id, _) = occlusion_of(&mut tx, card_id)
        .await?
        .ok_or(AuthoringError::WrongKind)?;
    let old = stored(&mut tx, occlusion_id).await?.occlusion.numbers();
    let new = occlusion.numbers();
    let cards = cards_of(&mut tx, occlusion_id).await?;

    let mut changes = Vec::new();
    for card in cards.iter().filter(|card| !card.deleted) {
        let number = card.occlusion_number as u32;
        changes.push(if new.contains(&number) {
            Change::Keep {
                card_id: card.id,
                number,
            }
        } else {
            Change::Delete { card_id: card.id }
        });
    }
    for &number in new.difference(&old) {
        let number_of = |card: &&CardRow| card.occlusion_number == i64::from(number);
        if cards.iter().filter(number_of).any(|card| !card.deleted) {
            continue;
        }
        // `cards` lists the most recently deleted first.
        let deleted = cards
            .iter()
            .filter(number_of)
            .find(|card| card.deleted && card.deck_id == deck_id);
        changes.push(match deleted {
            Some(card) => Change::Restore {
                card_id: card.id,
                number,
            },
            None => Change::Add { number },
        });
    }

    for change in &changes {
        match *change {
            Change::Keep { card_id, number } => {
                rewrite_card(&mut tx, card_id, &occlusion, number, &now).await?
            }
            Change::Delete { card_id } => {
                sqlx::query(
                    "UPDATE flashcards SET deleted_at = ?1, updated_at = ?1
                     WHERE id = ?2 AND deleted_at IS NULL",
                )
                .bind(&now)
                .bind(card_id)
                .execute(&mut *tx)
                .await?;
            }
            Change::Restore { card_id, number } => {
                // `deleted_at` alone first, as migration 0010 asks of every
                // restore; the text is rewritten once the card is active.
                sqlx::query(
                    "UPDATE flashcards SET deleted_at = NULL
                     WHERE id = ?1 AND deleted_at IS NOT NULL",
                )
                .bind(card_id)
                .execute(&mut *tx)
                .await?;
                rewrite_card(&mut tx, card_id, &occlusion, number, &now).await?;
            }
            Change::Add { number } => {
                insert_card(&mut tx, deck_id, occlusion_id, &occlusion, number, &now).await?
            }
        }
    }
    sqlx::query(
        "UPDATE occlusions SET header = ?1, mode = ?2, boxes = ?3, updated_at = ?4 WHERE id = ?5",
    )
    .bind(&occlusion.header)
    .bind(occlusion.mode.as_str())
    .bind(occlusion.boxes_json())
    .bind(&now)
    .bind(occlusion_id)
    .execute(&mut *tx)
    .await?;
    study::finish_session_if_nothing_due(&mut tx, deck_id, &now).await?;

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok((summarize(&changes), deck))
}

/// Whether a deleted card can be restored as far as its occlusion is
/// concerned: `None` for any other card, or an occlusion card that can come
/// back (its number is still drawn, and its occlusion's active cards are in
/// its deck with none of them taking its number).
pub(crate) async fn restore_block(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<Option<RestoreBlock>, AuthoringError> {
    let Some((occlusion_id, number)) = occlusion_of(conn, card_id).await? else {
        return Ok(None);
    };
    if !stored(conn, occlusion_id)
        .await?
        .occlusion
        .numbers()
        .contains(&number)
    {
        return Ok(Some(RestoreBlock::NumberGone));
    }
    let cards = cards_of(conn, occlusion_id).await?;
    let Some(card) = cards.iter().find(|card| card.id == card_id) else {
        return Err(AuthoringError::CardNotFound);
    };
    let elsewhere = cards.iter().any(|other| {
        !other.deleted
            && (other.deck_id != card.deck_id || other.occlusion_number == card.occlusion_number)
    });
    Ok(elsewhere.then_some(RestoreBlock::Elsewhere))
}

/// After an occlusion card is restored: rewrites its front and back from
/// its boxes as they are now, which may have changed while it was deleted.
pub(crate) async fn refresh_restored(
    conn: &mut SqliteConnection,
    card_id: i64,
    now: &str,
) -> Result<(), AuthoringError> {
    let Some((occlusion_id, number)) = occlusion_of(conn, card_id).await? else {
        return Ok(());
    };
    let occlusion = stored(conn, occlusion_id).await?.occlusion;
    rewrite_card(conn, card_id, &occlusion, number, now).await?;
    Ok(())
}

/// Refuses a move of `card_ids` unless, for every occlusion among them, all
/// of its active cards are selected.
pub(crate) async fn check_whole_occlusions(
    conn: &mut SqliteConnection,
    card_ids: &[i64],
) -> Result<(), AuthoringError> {
    let selected: HashSet<i64> = card_ids.iter().copied().collect();
    let mut occlusions = HashSet::new();
    for &card_id in card_ids {
        if let Some((occlusion_id, _)) = occlusion_of(conn, card_id).await? {
            occlusions.insert(occlusion_id);
        }
    }
    for occlusion_id in occlusions {
        let cards = cards_of(conn, occlusion_id).await?;
        if cards
            .iter()
            .any(|card| !card.deleted && !selected.contains(&card.id))
        {
            return Err(AuthoringError::OcclusionSplit);
        }
    }
    Ok(())
}

/// An occlusion card as the review screen draws it:
/// `{ picture, header, mode, number, boxes }`. The boxes with `number` are
/// the card's own: covered and marked on the question, uncovered on the
/// answer. With `"hideAll"` every other box is covered too.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct SessionOcclusion {
    pub picture: Asset,
    pub header: String,
    pub mode: Mode,
    pub number: u32,
    pub boxes: Vec<OcclusionBox>,
}

/// A review card's picture and boxes, if it is an occlusion card.
pub(crate) async fn session_view(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<Option<SessionOcclusion>, DbError> {
    let Some((occlusion_id, number)) = occlusion_of(conn, card_id).await? else {
        return Ok(None);
    };
    let detail = |err: AuthoringError| -> DbError {
        match err {
            AuthoringError::Internal(detail) => detail,
            other => format!("{other:?}").into(),
        }
    };
    let Stored {
        asset_id,
        occlusion,
    } = stored(conn, occlusion_id).await.map_err(detail)?;
    let picture = picture(conn, asset_id).await.map_err(detail)?;
    Ok(Some(SessionOcclusion {
        picture,
        header: occlusion.header,
        mode: occlusion.mode,
        number,
        boxes: occlusion.boxes,
    }))
}

/// Box numbers renumbered from 1 in the order they first appear, keeping
/// boxes that shared a number together. Anki's numbers can skip (c1, c3), and
/// the editor shows them as they are, so this is only for numbers over
/// [`NUMBER_MAX`].
pub(crate) fn renumber(boxes: &mut [OcclusionBox]) {
    let mut numbers: BTreeMap<u32, u32> = BTreeMap::new();
    for shape in boxes.iter() {
        let next = numbers.len() as u32 + 1;
        numbers.entry(shape.number).or_insert(next);
    }
    for shape in boxes.iter_mut() {
        shape.number = numbers[&shape.number];
    }
}
