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
    let occlusion = validate_occlusion(&header, mode, &boxes)
        .map_err(|problem| broken(&format!("{problem:?}")))?;
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
pub async fn get_occlusion(
    pool: &SqlitePool,
    card_id: i64,
) -> Result<OcclusionView, AuthoringError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::test_support::{png, scratch_store};
    use crate::assets::{add_picture, put, MediaType};
    use crate::authoring::{
        create_deck, create_flashcard, delete_flashcard, move_flashcards, restore_deleted_cards,
        restore_flashcard, update_flashcard, DeckCard,
    };
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::scheduler::Rating;
    use crate::study::{SessionCard, StartedSession};

    const NOW: &str = "2026-10-20T12:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    fn rect(number: u32, left: f64, top: f64, label: &str) -> OcclusionBox {
        OcclusionBox {
            number,
            left,
            top,
            width: 0.2,
            height: 0.1,
            label: label.to_string(),
        }
    }

    async fn picture_in(pool: &SqlitePool, name: &str) -> Asset {
        add_picture(pool, &scratch_store(name), &png(0), at(NOW))
            .await
            .unwrap()
    }

    async fn deck(pool: &SqlitePool, name: &str) -> i64 {
        create_deck(pool, name, None, at(NOW)).await.unwrap().id
    }

    /// The deck's occlusion cards as `(number, front, back)`, by number.
    fn occlusion_cards(cards: &[DeckCard]) -> Vec<(i64, String, String)> {
        let mut found: Vec<_> = cards
            .iter()
            .filter_map(|card| {
                let occlusion = card.occlusion.as_ref()?;
                Some((occlusion.number, card.front.clone(), card.back.clone()))
            })
            .collect();
        found.sort();
        found
    }

    fn card_of(cards: &[DeckCard], number: i64) -> i64 {
        cards
            .iter()
            .find(|card| card.occlusion.as_ref().is_some_and(|o| o.number == number))
            .unwrap()
            .id
    }

    #[test]
    fn boxes_are_checked_pulled_inside_and_rounded() {
        let checked = validate_occlusion(
            "  Heart  ",
            Mode::HideAll,
            &[OcclusionBox {
                number: 2,
                left: -0.0004,
                top: 0.123456,
                width: 1.0,
                height: 0.5,
                label: " Aorta ".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(checked.header, "Heart");
        assert_eq!(
            checked.boxes,
            vec![OcclusionBox {
                number: 2,
                left: 0.0,
                top: 0.1235,
                width: 1.0,
                height: 0.5,
                label: "Aorta".to_string(),
            }]
        );

        let problem = |boxes: &[OcclusionBox]| validate_occlusion("", Mode::HideOne, boxes);
        assert_eq!(problem(&[]), Err(InvalidInput::OcclusionNoBoxes));
        let many = vec![rect(1, 0.1, 0.1, ""); BOXES_MAX + 1];
        assert_eq!(problem(&many), Err(InvalidInput::OcclusionTooManyBoxes));
        for bad in [
            rect(0, 0.1, 0.1, ""),
            rect(NUMBER_MAX + 1, 0.1, 0.1, ""),
            rect(1, 0.9, 0.1, ""),
            rect(1, -0.1, 0.1, ""),
            rect(1, f64::NAN, 0.1, ""),
            OcclusionBox {
                width: 0.001,
                ..rect(1, 0.1, 0.1, "")
            },
            OcclusionBox {
                height: -0.1,
                ..rect(1, 0.1, 0.1, "")
            },
        ] {
            assert_eq!(
                problem(std::slice::from_ref(&bad)),
                Err(InvalidInput::OcclusionBox),
                "{bad:?}"
            );
        }
        let long = "x".repeat(LABEL_MAX_CHARS + 1);
        assert_eq!(
            problem(&[rect(1, 0.1, 0.1, &long)]),
            Err(InvalidInput::OcclusionLabelTooLong)
        );
        // Each label fits, but one number's labels together don't fit a card.
        let label = "y".repeat(LABEL_MAX_CHARS);
        let boxes: Vec<_> = (0..11)
            .map(|index| rect(1, 0.1, 0.1, &format!("{index}{}", &label[1..])))
            .collect();
        assert_eq!(problem(&boxes), Err(InvalidInput::OcclusionLabelTooLong));
        assert_eq!(
            validate_occlusion(
                &"h".repeat(HEADER_MAX_CHARS + 1),
                Mode::HideAll,
                &[rect(1, 0.1, 0.1, "")]
            ),
            Err(InvalidInput::OcclusionHeaderTooLong)
        );
        // Unknown fields are refused when read from JSON.
        assert!(serde_json::from_str::<OcclusionBox>(
            r#"{"number":1,"left":0,"top":0,"width":0.5,"height":0.5,"extra":1}"#
        )
        .is_err());
    }

    #[test]
    fn a_card_s_front_names_its_box_and_its_back_is_its_labels() {
        let occlusion = validate_occlusion(
            "Heart",
            Mode::HideAll,
            &[
                rect(1, 0.1, 0.1, "Aorta"),
                rect(2, 0.4, 0.4, ""),
                rect(1, 0.6, 0.6, "Aorta"),
                rect(1, 0.7, 0.2, "Arch"),
            ],
        )
        .unwrap();
        assert_eq!(occlusion.numbers().into_iter().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(occlusion.front(1), "Heart\n[picture: box 1]");
        assert_eq!(occlusion.back(1), "Aorta; Arch");
        assert_eq!(occlusion.back(2), "[box 2]");
        let plain = Occlusion {
            header: String::new(),
            ..occlusion
        };
        assert_eq!(plain.front(2), "[picture: box 2]");
    }

    #[test]
    fn an_occlusion_makes_one_card_per_number_and_shows_its_picture_in_review() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let picture = picture_in(&pool, "occlusion-review").await;
            let deck_id = deck(&pool, "Anatomy").await;
            let boxes = [
                rect(1, 0.1, 0.1, "Aorta"),
                rect(2, 0.5, 0.5, "Ventricle"),
                rect(2, 0.5, 0.7, "Ventricle"),
            ];
            let deck = create_occlusion(
                &pool,
                deck_id,
                picture.id,
                "Heart",
                Mode::HideOne,
                &boxes,
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(
                occlusion_cards(&deck.cards),
                vec![
                    (
                        1,
                        "Heart\n[picture: box 1]".to_string(),
                        "Aorta".to_string()
                    ),
                    (
                        2,
                        "Heart\n[picture: box 2]".to_string(),
                        "Ventricle".to_string()
                    ),
                ]
            );
            assert_eq!(deck.due_count, 2);

            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck_id, at(NOW)).await.unwrap()
            else {
                panic!("expected a session");
            };
            let SessionCard::Due {
                card,
                occlusion,
                cloze,
                offer_help,
                ..
            } = study::session_card(&pool, session_id, at(NOW))
                .await
                .unwrap()
            else {
                panic!("expected a card");
            };
            assert_eq!(cloze, None);
            assert!(!offer_help);
            let occlusion = occlusion.unwrap();
            assert_eq!(occlusion.picture, picture);
            assert_eq!(occlusion.header, "Heart");
            assert_eq!(occlusion.mode, Mode::HideOne);
            assert_eq!(occlusion.number, 1);
            assert_eq!(occlusion.boxes.len(), 3);
            assert_eq!(
                serde_json::to_value(&occlusion).unwrap()["boxes"][1],
                serde_json::json!({
                    "number": 2, "left": 0.5, "top": 0.5, "width": 0.2, "height": 0.1,
                    "label": "Ventricle"
                })
            );

            // After three Agains in a row, help is offered for a picture
            // card too: a vision model sees its picture (`aid.rs`).
            for _ in 0..crate::aid::HELP_AFTER_AGAINS {
                sqlx::query(
                    "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                         elapsed_days, stability_after, difficulty_after, reviewed_at)
                     VALUES (?1, 1, 'Learning', 0, 0, 1, 5, '2026-10-18T10:00:00Z')",
                )
                .bind(card.id)
                .execute(&pool)
                .await
                .unwrap();
            }
            let SessionCard::Due {
                card: again,
                offer_help,
                ..
            } = study::session_card(&pool, session_id, at(NOW))
                .await
                .unwrap()
            else {
                panic!("expected a card");
            };
            assert_eq!(again.id, card.id);
            assert!(offer_help);

            // Once one card is rated, its sibling waits until tomorrow.
            study::record_review(&pool, session_id, card.id, card.reps, Rating::Good, at(NOW))
                .await
                .unwrap();
            assert!(matches!(
                study::session_card(&pool, session_id, at(NOW))
                    .await
                    .unwrap(),
                SessionCard::Completed { .. }
            ));
            let tomorrow = at("2026-10-21T12:00:00Z");
            let deck = crate::authoring::deck_detail(&pool, deck_id, tomorrow)
                .await
                .unwrap();
            assert_eq!(deck.due_count, 1);
        });
    }

    #[test]
    fn only_a_kept_picture_in_a_normal_deck_can_be_occluded() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck_id = deck(&pool, "Anatomy").await;
            let boxes = [rect(1, 0.1, 0.1, "")];
            let create = |asset_id, deck_id| {
                create_occlusion(&pool, deck_id, asset_id, "", Mode::HideAll, &boxes, at(NOW))
            };
            assert!(matches!(
                create(99, deck_id).await,
                Err(AuthoringError::PictureMissing)
            ));
            // A kept file that isn't a picture.
            let store = scratch_store("occlusion-not-picture");
            let mut conn = pool.acquire().await.unwrap();
            let text = put(&mut conn, &store, b"hello", MediaType::Text, at(NOW))
                .await
                .unwrap();
            drop(conn);
            assert!(matches!(
                create(text.id, deck_id).await,
                Err(AuthoringError::PictureMissing)
            ));
            let picture = picture_in(&pool, "occlusion-sample").await;
            assert!(matches!(
                create(picture.id, 1).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM occlusions").await, 0);

            // SQLite refuses the same, and an occlusion card of the wrong
            // shape, whatever Rust would do.
            let refused = sqlx::query(
                "INSERT INTO occlusions (asset_id, header, mode, boxes, created_at, updated_at)
                 VALUES (?1, '', 'hideAll', '[{}]', 'now', 'now')",
            )
            .bind(text.id)
            .execute(&pool)
            .await;
            assert!(refused.is_err());
            create(picture.id, deck_id).await.unwrap();
            for sql in [
                "INSERT INTO flashcards (deck_id, front, back, occlusion_id) VALUES (2, 'a', 'b', 1)",
                "INSERT INTO flashcards (deck_id, front, back, occlusion_number) VALUES (2, 'a', 'b', 1)",
                "INSERT INTO flashcards (deck_id, front, back, occlusion_id, occlusion_number)
                 VALUES (2, 'a', 'b', 1, 1)",
                "UPDATE flashcards SET occlusion_number = 2 WHERE occlusion_id = 1",
                "UPDATE occlusions SET asset_id = asset_id + 1",
                "DELETE FROM occlusions",
                "UPDATE occlusions SET boxes = 'not json'",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
        });
    }

    #[test]
    fn editing_boxes_keeps_schedules_and_deletes_or_brings_back_cards_by_number() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let picture = picture_in(&pool, "occlusion-edit").await;
            let deck_id = deck(&pool, "Anatomy").await;
            let deck = create_occlusion(
                &pool,
                deck_id,
                picture.id,
                "",
                Mode::HideAll,
                &[rect(1, 0.1, 0.1, "A"), rect(2, 0.4, 0.4, "B")],
                at(NOW),
            )
            .await
            .unwrap();
            let first = card_of(&deck.cards, 1);
            let second = card_of(&deck.cards, 2);
            let schedule = |card| {
                let pool = pool.clone();
                async move {
                    sqlx::query_as::<_, (String, Option<String>, i64)>(
                        "SELECT fsrs_state, due, reps FROM flashcards WHERE id = ?1",
                    )
                    .bind(card)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                }
            };
            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck_id, at(NOW)).await.unwrap()
            else {
                panic!("expected a session");
            };
            study::record_review(&pool, session_id, first, 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            let rated = schedule(first).await;

            // Box 2 is gone, box 1 is relabelled, box 3 is new.
            let view = get_occlusion(&pool, first).await.unwrap();
            assert_eq!((view.picture.id, view.next_number), (picture.id, 3));
            let (changes, deck) = update_occlusion(
                &pool,
                first,
                "Heart",
                Mode::HideOne,
                &[rect(1, 0.1, 0.1, "Aorta"), rect(3, 0.6, 0.6, "")],
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(
                changes,
                ClozeChanges {
                    cards: 2,
                    kept: 1,
                    restored: 0,
                    added: 1,
                    deleted: 1,
                }
            );
            assert_eq!(schedule(first).await, rated);
            assert_eq!(
                occlusion_cards(&deck.cards),
                vec![
                    (
                        1,
                        "Heart\n[picture: box 1]".to_string(),
                        "Aorta".to_string()
                    ),
                    (
                        3,
                        "Heart\n[picture: box 3]".to_string(),
                        "[box 3]".to_string()
                    ),
                ]
            );
            let deleted = &deck.deleted_cards[0];
            assert_eq!(deleted.id, second);
            assert_eq!(
                deleted.occlusion,
                Some(crate::authoring::DeletedCloze {
                    number: 2,
                    blocked: Some(RestoreBlock::NumberGone),
                })
            );
            assert!(!deleted.restorable());
            assert!(matches!(
                restore_flashcard(&pool, second, at(NOW)).await,
                Err(AuthoringError::OcclusionNotRestorable(
                    RestoreBlock::NumberGone
                ))
            ));
            // The next number is above every number ever used.
            assert_eq!(get_occlusion(&pool, first).await.unwrap().next_number, 4);

            // Box 2 drawn again brings its card back, with its schedule.
            let (changes, deck) = update_occlusion(
                &pool,
                first,
                "Heart",
                Mode::HideOne,
                &[
                    rect(1, 0.1, 0.1, "Aorta"),
                    rect(2, 0.4, 0.4, "B"),
                    rect(3, 0.6, 0.6, ""),
                ],
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!((changes.restored, changes.cards), (1, 3));
            assert_eq!(card_of(&deck.cards, 2), second);

            // A card deleted by itself can come back while its box is drawn,
            // with its front and back as the boxes are now.
            delete_flashcard(&pool, second, at(NOW)).await.unwrap();
            update_occlusion(
                &pool,
                first,
                "Lungs",
                Mode::HideOne,
                &[
                    rect(1, 0.1, 0.1, "Aorta"),
                    rect(2, 0.4, 0.4, "Bronchus"),
                    rect(3, 0.6, 0.6, ""),
                ],
                at(NOW),
            )
            .await
            .unwrap();
            let restored = restore_deleted_cards(&pool, deck_id, 1, at(NOW))
                .await
                .unwrap();
            assert_eq!(restored.restored, 1);
            let back: (String, String) =
                sqlx::query_as("SELECT front, back FROM flashcards WHERE id = ?1")
                    .bind(second)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                back,
                (
                    "Lungs\n[picture: box 2]".to_string(),
                    "Bronchus".to_string()
                )
            );

            // Never edited as a front and back.
            assert!(matches!(
                update_flashcard(&pool, first, "a", "b", at(NOW)).await,
                Err(AuthoringError::WrongKind)
            ));
            let basic = create_flashcard(&pool, deck_id, "a", "b", at(NOW))
                .await
                .unwrap()
                .cards
                .iter()
                .find(|card| card.occlusion.is_none())
                .unwrap()
                .id;
            assert!(matches!(
                get_occlusion(&pool, basic).await,
                Err(AuthoringError::WrongKind)
            ));
            assert!(matches!(
                update_occlusion(
                    &pool,
                    basic,
                    "",
                    Mode::HideAll,
                    &[rect(1, 0.1, 0.1, "")],
                    at(NOW)
                )
                .await,
                Err(AuthoringError::WrongKind)
            ));
        });
    }

    #[test]
    fn an_occlusion_s_cards_move_together_and_come_back_only_beside_them() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let picture = picture_in(&pool, "occlusion-move").await;
            let from = deck(&pool, "Anatomy").await;
            let to = deck(&pool, "Biology").await;
            let deck = create_occlusion(
                &pool,
                from,
                picture.id,
                "",
                Mode::HideAll,
                &[
                    rect(1, 0.1, 0.1, ""),
                    rect(2, 0.4, 0.4, ""),
                    rect(3, 0.6, 0.6, ""),
                ],
                at(NOW),
            )
            .await
            .unwrap();
            let (one, two, three) = (
                card_of(&deck.cards, 1),
                card_of(&deck.cards, 2),
                card_of(&deck.cards, 3),
            );
            assert!(matches!(
                move_flashcards(&pool, from, &[one, two, three][..2], to, at(NOW)).await,
                Err(AuthoringError::OcclusionSplit)
            ));
            delete_flashcard(&pool, three, at(NOW)).await.unwrap();
            let moved = move_flashcards(&pool, from, &[one, two], to, at(NOW))
                .await
                .unwrap();
            assert_eq!(moved.changed, 2);
            // Its deleted card stayed behind, and can't come back there.
            assert_eq!(
                moved.deck.deleted_cards[0]
                    .occlusion
                    .as_ref()
                    .unwrap()
                    .blocked,
                Some(RestoreBlock::Elsewhere)
            );
            assert!(matches!(
                restore_flashcard(&pool, three, at(NOW)).await,
                Err(AuthoringError::OcclusionNotRestorable(
                    RestoreBlock::Elsewhere
                ))
            ));
        });
    }

    #[test]
    fn a_cards_file_leaves_occlusion_cards_out() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let picture = picture_in(&pool, "occlusion-csv").await;
            let deck_id = deck(&pool, "Anatomy").await;
            create_flashcard(&pool, deck_id, "Pump", "Heart", at(NOW))
                .await
                .unwrap();
            create_occlusion(
                &pool,
                deck_id,
                picture.id,
                "",
                Mode::HideAll,
                &[rect(1, 0.1, 0.1, "")],
                at(NOW),
            )
            .await
            .unwrap();
            let (_, cards) = crate::cards_csv::deck_cards(&pool, deck_id).await.unwrap();
            assert_eq!(cards.len(), 1);
            assert_eq!(cards[0].front, "Pump");
        });
    }

    #[test]
    fn renumbering_keeps_shared_numbers_together_in_order() {
        let mut boxes = vec![
            rect(1500, 0.1, 0.1, ""),
            rect(7, 0.2, 0.1, ""),
            rect(1500, 0.3, 0.1, ""),
        ];
        renumber(&mut boxes);
        let numbers: Vec<u32> = boxes.iter().map(|shape| shape.number).collect();
        assert_eq!(numbers, [1, 2, 1]);
    }
}
