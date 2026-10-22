//! Cloze cards (migration 0012): one text with deletions, where each distinct
//! deletion number is its own card.
//!
//! A deletion is written `{{c1::answer}}`, or `{{c1::answer::hint}}` to show a
//! hint in its place. The number is from 1 to [`CLOZE_NUMBER_MAX`]; the same
//! number may appear more than once, and all of its deletions are hidden
//! together on that number's card. Text that only looks a little like a
//! deletion (`{{c}}`, `{{cx::…}}`) is ordinary text. The text follows a typed
//! card's rules: required, at most `CARD_TEXT_MAX_CHARS` characters after
//! outer whitespace is trimmed, line breaks kept. It must hold at least one
//! deletion; each must be closed with `}}` before the next starts, and its
//! answer (and hint, if given) can't be blank.
//!
//! The text is stored once, in `cloze_texts`; each of its cards is an ordinary
//! `flashcards` row of kind `cloze` with its number. A card's `front` is the
//! text with its own deletions shown as `[...]` (or `[hint]`) and every other
//! deletion filled in, and its `back` is its answer (several answers of one
//! number joined by "; "). They are written from the text whenever it is
//! saved, so every part of Synapse that reads a card's front and back works on
//! a cloze card unchanged. Reviews draw the card from the text itself
//! ([`session_parts`]), so the answer can be shown highlighted in place.
//!
//! Editing the text ([`update_cloze`]):
//! - A number still in the text keeps its card, with its schedule and review
//!   history; only the card's front and back are rewritten.
//! - A number no longer in the text has its card deleted, as deleting a card
//!   does: it moves to *Deleted cards* with its history kept.
//! - A number new to the text gets a card. If that number had a card that was
//!   deleted, in the same deck, that card comes back with its old schedule;
//!   otherwise a new card starts, as an added card does.
//! - A card the user deleted by itself stays deleted while its number stays
//!   in the text.
//!
//! The cards of one text stay in one deck: moving any of them means moving
//! all its active cards together, and a deleted card is restored only if its
//! number is in the text now and its text's active cards are in its deck.

use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};

use crate::authoring::{
    active_normal_deck, changeable_card_deck, checked_tags, load_deck_detail, required_text,
    tag_text_cards, AuthoringError, DeckDetail, InvalidInput, CARD_TEXT_MAX_CHARS,
};
use crate::db::{to_db_time, DbError};
use crate::study;

/// The largest deletion number, `{{c999::…}}`.
pub const CLOZE_NUMBER_MAX: u32 = 999;

/// One piece of a cloze text, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Text(String),
    Deletion {
        number: u32,
        /// As written, inner spaces included.
        answer: String,
        hint: Option<String>,
    },
}

/// A checked cloze text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cloze {
    /// The text as it is stored: trimmed of outer whitespace.
    pub(crate) text: String,
    segments: Vec<Segment>,
}

/// A piece of a cloze card as the review screen shows it:
/// `{ "type": "text", "text" }`, or `{ "type": "gap", "answer", "hint" }` for
/// the card's own deletion, hidden until the answer is shown.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClozePart {
    Text {
        text: String,
    },
    Gap {
        answer: String,
        hint: Option<String>,
    },
}

/// Finds `{{c<digits>::` at the start of `text`: the number's digits and the
/// length of the whole opening.
fn opening(text: &str) -> Option<(&str, usize)> {
    let after = text.strip_prefix("{{c")?;
    let digits = after.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !after[digits..].starts_with("::") {
        return None;
    }
    Some((&after[..digits], 3 + digits + 2))
}

/// Whether `text` has anything that starts a deletion. A cards file uses this
/// to tell a cloze row from a front/back row with its back missing.
pub(crate) fn has_deletion(text: &str) -> bool {
    text.match_indices("{{c")
        .any(|(at, _)| opening(&text[at..]).is_some())
}

fn parse(text: &str) -> Result<Vec<Segment>, InvalidInput> {
    let mut segments = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("{{c") {
        let Some((digits, length)) = opening(&rest[at..]) else {
            // Not a deletion: keep it as text and look further on.
            plain.push_str(&rest[..at + 3]);
            rest = &rest[at + 3..];
            continue;
        };
        let number = digits
            .parse::<u32>()
            .ok()
            .filter(|number| (1..=CLOZE_NUMBER_MAX).contains(number))
            .ok_or(InvalidInput::ClozeNumber)?;
        let body_and_rest = &rest[at + length..];
        let end = body_and_rest
            .find("}}")
            .ok_or(InvalidInput::ClozeUnclosed)?;
        let body = &body_and_rest[..end];
        // Another deletion starting before this one closes.
        if body.contains("{{") {
            return Err(InvalidInput::ClozeUnclosed);
        }
        let (answer, hint) = match body.split_once("::") {
            Some((answer, hint)) => (answer, Some(hint)),
            None => (body, None),
        };
        if answer.trim().is_empty() || hint.is_some_and(|hint| hint.trim().is_empty()) {
            return Err(InvalidInput::ClozeEmpty);
        }

        plain.push_str(&rest[..at]);
        if !plain.is_empty() {
            segments.push(Segment::Text(std::mem::take(&mut plain)));
        }
        segments.push(Segment::Deletion {
            number,
            answer: answer.to_string(),
            hint: hint.map(|hint| hint.trim().to_string()),
        });
        rest = &body_and_rest[end + 2..];
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        segments.push(Segment::Text(plain));
    }
    Ok(segments)
}

/// Checks a cloze text as the user wrote it.
pub(crate) fn validate_cloze(text: &str) -> Result<Cloze, InvalidInput> {
    let text = required_text(
        text,
        CARD_TEXT_MAX_CHARS,
        InvalidInput::ClozeBlank,
        InvalidInput::ClozeTooLong,
    )?;
    let segments = parse(text)?;
    let cloze = Cloze {
        text: text.to_string(),
        segments,
    };
    if cloze.numbers().is_empty() {
        return Err(InvalidInput::ClozeNoDeletion);
    }
    Ok(cloze)
}

impl Cloze {
    /// The text's deletion numbers, each once, smallest first: one card each.
    pub(crate) fn numbers(&self) -> BTreeSet<u32> {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Deletion { number, .. } => Some(*number),
                Segment::Text(_) => None,
            })
            .collect()
    }

    /// The card of `number` as pieces: its own deletions as gaps, and
    /// everything else (other deletions filled in) as text.
    pub(crate) fn parts(&self, number: u32) -> Vec<ClozePart> {
        let mut parts: Vec<ClozePart> = Vec::new();
        for segment in &self.segments {
            let text = match segment {
                Segment::Text(text) => text,
                Segment::Deletion {
                    number: n,
                    answer,
                    hint,
                } if *n == number => {
                    parts.push(ClozePart::Gap {
                        answer: answer.clone(),
                        hint: hint.clone(),
                    });
                    continue;
                }
                Segment::Deletion { answer, .. } => answer,
            };
            match parts.last_mut() {
                Some(ClozePart::Text { text: last }) => last.push_str(text),
                _ => parts.push(ClozePart::Text { text: text.clone() }),
            }
        }
        parts
    }

    /// The card's stored front: its gaps as `[hint]` or `[...]`.
    pub(crate) fn front(&self, number: u32) -> String {
        self.parts(number)
            .into_iter()
            .map(|part| match part {
                ClozePart::Text { text } => text,
                ClozePart::Gap { hint, .. } => format!("[{}]", hint.as_deref().unwrap_or("...")),
            })
            .collect()
    }

    /// The card's stored back: its answers, trimmed, joined by "; ".
    pub(crate) fn back(&self, number: u32) -> String {
        self.parts(number)
            .into_iter()
            .filter_map(|part| match part {
                ClozePart::Gap { answer, .. } => Some(answer.trim().to_string()),
                ClozePart::Text { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Why a deleted cloze card can't be restored right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RestoreBlock {
    /// Its number is no longer in its text.
    NumberGone,
    /// Its text's active cards are in another deck, or its number already has
    /// an active card there.
    Elsewhere,
}

/// A cloze card's row: which text, which number, its deck, and whether it is
/// deleted.
#[derive(Debug, sqlx::FromRow)]
struct ClozeCardRow {
    id: i64,
    deck_id: i64,
    cloze_number: i64,
    deleted: bool,
}

async fn text_of(conn: &mut SqliteConnection, text_id: i64) -> Result<Cloze, AuthoringError> {
    let text: String = sqlx::query_scalar("SELECT text FROM cloze_texts WHERE id = ?1")
        .bind(text_id)
        .fetch_one(&mut *conn)
        .await?;
    // What was saved was checked when it was saved; a stored text that no
    // longer parses was changed outside Synapse.
    validate_cloze(&text).map_err(|problem| {
        AuthoringError::Internal(
            format!("stored cloze text {text_id} is invalid: {problem:?}").into(),
        )
    })
}

/// Every card of a text, deleted ones included, most recently deleted first
/// (active cards, which have no deletion time, come last), then newest first.
async fn cards_of(
    conn: &mut SqliteConnection,
    text_id: i64,
) -> Result<Vec<ClozeCardRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, deck_id, cloze_number, deleted_at IS NOT NULL AS deleted
         FROM flashcards WHERE cloze_text_id = ?1
         ORDER BY deleted_at IS NULL, deleted_at DESC, id DESC",
    )
    .bind(text_id)
    .fetch_all(conn)
    .await
}

/// The text a card was made from, or `None` for a basic card.
async fn text_id_of(conn: &mut SqliteConnection, card_id: i64) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT cloze_text_id FROM flashcards WHERE id = ?1")
        .bind(card_id)
        .fetch_optional(conn)
        .await
        .map(Option::flatten)
}

/// Writes one new cloze text into `deck_id`, with a brand-new card for each
/// of its numbers, smallest first; returns how many cards. The caller has
/// already checked the deck.
pub(crate) async fn insert_cloze(
    conn: &mut SqliteConnection,
    deck_id: i64,
    cloze: &Cloze,
    now: &str,
) -> Result<usize, sqlx::Error> {
    Ok(insert_text(conn, deck_id, cloze, now).await?.len())
}

/// As [`insert_cloze`], returning the new cards' ids.
async fn insert_text(
    conn: &mut SqliteConnection,
    deck_id: i64,
    cloze: &Cloze,
    now: &str,
) -> Result<Vec<i64>, sqlx::Error> {
    let text_id: i64 = sqlx::query_scalar(
        "INSERT INTO cloze_texts (text, created_at, updated_at) VALUES (?1, ?2, ?2)
         RETURNING id",
    )
    .bind(&cloze.text)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    let mut card_ids = Vec::new();
    for number in cloze.numbers() {
        card_ids.push(insert_card(conn, deck_id, text_id, cloze, number, now).await?);
    }
    Ok(card_ids)
}

/// A brand-new card for one number, started as `authoring::insert_new_card`
/// starts a typed one; returns its id.
async fn insert_card(
    conn: &mut SqliteConnection,
    deck_id: i64,
    text_id: i64,
    cloze: &Cloze,
    number: u32,
    now: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO flashcards (deck_id, front, back, fsrs_state, due, reps, lapses,
                                 created_at, updated_at, kind, cloze_text_id, cloze_number)
         VALUES (?1, ?2, ?3, 'New', NULL, 0, 0, ?4, ?4, 'cloze', ?5, ?6)
         RETURNING id",
    )
    .bind(deck_id)
    .bind(cloze.front(number))
    .bind(cloze.back(number))
    .bind(now)
    .bind(text_id)
    .bind(number)
    .fetch_one(conn)
    .await
}

/// Rewrites an active card's front and back from `cloze`, if they changed.
/// Nothing else of the card is written.
async fn rewrite_card(
    conn: &mut SqliteConnection,
    card_id: i64,
    cloze: &Cloze,
    number: u32,
    now: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE flashcards SET front = ?1, back = ?2, updated_at = ?3
         WHERE id = ?4 AND deleted_at IS NULL AND (front IS NOT ?1 OR back IS NOT ?2)",
    )
    .bind(cloze.front(number))
    .bind(cloze.back(number))
    .bind(now)
    .bind(card_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Adds a cloze text to a normal deck, with one new card per number, and
/// returns the deck with updated counts. Refused, with nothing written, as
/// `authoring::create_flashcard` refuses a deck.
#[cfg(test)]
pub async fn create_cloze(
    pool: &SqlitePool,
    deck_id: i64,
    text: &str,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    create_cloze_tagged(pool, deck_id, text, None, now).await
}

/// As [`create_cloze`], giving every card of the text the tags `tag_names`
/// (`None` or an empty list gives none), checked before anything is written.
pub async fn create_cloze_tagged(
    pool: &SqlitePool,
    deck_id: i64,
    text: &str,
    tag_names: Option<&[String]>,
    now: DateTime<Utc>,
) -> Result<DeckDetail, AuthoringError> {
    let cloze = validate_cloze(text)?;
    let tag_names = checked_tags(tag_names)?;
    let at = now;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    active_normal_deck(&mut tx, deck_id).await?;
    let card_ids = insert_text(&mut tx, deck_id, &cloze, &now).await?;
    tag_text_cards(
        &mut tx,
        tag_names.as_deref(),
        None,
        &card_ids,
        &card_ids,
        at,
    )
    .await?;
    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok(deck)
}

/// What saving a cloze text would do, and does: how many cards the text
/// makes, and of them how many keep an existing card, come back from
/// *Deleted cards*, or are new; and how many existing cards are deleted.
/// Sent as `{ cards, kept, restored, added, deleted }`.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ClozeChanges {
    /// The text's active cards after saving.
    pub cards: usize,
    pub kept: usize,
    pub restored: usize,
    pub added: usize,
    pub deleted: usize,
}

/// One change an edit makes to a card.
enum Change {
    Keep { card_id: i64, number: u32 },
    Delete { card_id: i64 },
    Restore { card_id: i64, number: u32 },
    Add { number: u32 },
}

/// Works out what replacing the text of `card_id`'s cloze with `cloze` does,
/// by the rules in this module's header. Writes nothing. The card must be an
/// active cloze card; its deck is returned too.
async fn plan_edit(
    conn: &mut SqliteConnection,
    card_id: i64,
    cloze: &Cloze,
) -> Result<(i64, i64, Vec<Change>), AuthoringError> {
    let deck_id = changeable_card_deck(conn, card_id).await?;
    let text_id = text_id_of(conn, card_id)
        .await?
        .ok_or(AuthoringError::WrongKind)?;
    let old = text_of(conn, text_id).await?.numbers();
    let new = cloze.numbers();
    let cards = cards_of(conn, text_id).await?;

    let mut changes = Vec::new();
    for card in cards.iter().filter(|card| !card.deleted) {
        let number = card.cloze_number as u32;
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
        let active = cards
            .iter()
            .any(|card| !card.deleted && card.cloze_number == i64::from(number));
        if active {
            continue;
        }
        // `cards` lists the most recently deleted first.
        let deleted = cards.iter().find(|card| {
            card.deleted && card.cloze_number == i64::from(number) && card.deck_id == deck_id
        });
        changes.push(match deleted {
            Some(card) => Change::Restore {
                card_id: card.id,
                number,
            },
            None => Change::Add { number },
        });
    }
    Ok((deck_id, text_id, changes))
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

/// What saving `text` would do, for the form's live count: as a new cloze
/// text when `card_id` is `None`, or as the new text of that card's cloze.
/// Writes nothing. An unusable text is refused as saving it would be.
pub async fn preview_cloze(
    pool: &SqlitePool,
    text: &str,
    card_id: Option<i64>,
) -> Result<ClozeChanges, AuthoringError> {
    let cloze = validate_cloze(text)?;
    let Some(card_id) = card_id else {
        let cards = cloze.numbers().len();
        return Ok(ClozeChanges {
            cards,
            added: cards,
            ..ClozeChanges::default()
        });
    };
    let mut conn = pool.acquire().await?;
    let (_, _, changes) = plan_edit(&mut conn, card_id, &cloze).await?;
    Ok(summarize(&changes))
}

/// Replaces the text of `card_id`'s cloze, by the rules in this module's
/// header, and returns the deck with what changed. All in one transaction. A
/// card's schedule, review history, id, and deck are never changed by an
/// edit; only fronts and backs are rewritten, and cards deleted or brought
/// back exactly as deleting and restoring one would.
#[cfg(test)]
pub async fn update_cloze(
    pool: &SqlitePool,
    card_id: i64,
    text: &str,
    now: DateTime<Utc>,
) -> Result<(ClozeChanges, DeckDetail), AuthoringError> {
    update_cloze_tagged(pool, card_id, text, None, now).await
}

/// As [`update_cloze`], also replacing the tags of every active card of the
/// text with `tag_names` when given. Given or not, a card the edit adds or
/// brings back gets the text's tags (those of `card_id`, when none are sent).
pub async fn update_cloze_tagged(
    pool: &SqlitePool,
    card_id: i64,
    text: &str,
    tag_names: Option<&[String]>,
    now: DateTime<Utc>,
) -> Result<(ClozeChanges, DeckDetail), AuthoringError> {
    let cloze = validate_cloze(text)?;
    let tag_names = checked_tags(tag_names)?;
    let at = now;
    let now = to_db_time(now);
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Stops here, before writing anything, if the card can't be edited.
    let (deck_id, text_id, changes) = plan_edit(&mut tx, card_id, &cloze).await?;
    let mut active = Vec::new();
    let mut came = Vec::new();
    for change in &changes {
        match *change {
            Change::Keep { card_id, number } => {
                rewrite_card(&mut tx, card_id, &cloze, number, &now).await?;
                active.push(card_id);
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
                // Clearing `deleted_at` alone first, as migration 0010 asks
                // of every restore; the text is rewritten once it is active.
                sqlx::query(
                    "UPDATE flashcards SET deleted_at = NULL
                     WHERE id = ?1 AND deleted_at IS NOT NULL",
                )
                .bind(card_id)
                .execute(&mut *tx)
                .await?;
                rewrite_card(&mut tx, card_id, &cloze, number, &now).await?;
                active.push(card_id);
                came.push(card_id);
            }
            Change::Add { number } => {
                let new_card = insert_card(&mut tx, deck_id, text_id, &cloze, number, &now).await?;
                active.push(new_card);
                came.push(new_card);
            }
        }
    }
    tag_text_cards(
        &mut tx,
        tag_names.as_deref(),
        Some(card_id),
        &active,
        &came,
        at,
    )
    .await?;
    sqlx::query("UPDATE cloze_texts SET text = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(&cloze.text)
        .bind(&now)
        .bind(text_id)
        .execute(&mut *tx)
        .await?;
    study::finish_session_if_nothing_due(&mut tx, deck_id, &now).await?;

    let deck = load_deck_detail(&mut tx, deck_id, &now).await?;
    tx.commit().await?;
    Ok((summarize(&changes), deck))
}

/// Whether a deleted card can be restored as far as its cloze is concerned:
/// `None` for a basic card, or a cloze card that can come back.
pub(crate) async fn restore_block(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<Option<RestoreBlock>, AuthoringError> {
    let Some(text_id) = text_id_of(conn, card_id).await? else {
        return Ok(None);
    };
    let cards = cards_of(conn, text_id).await?;
    let Some(card) = cards.iter().find(|card| card.id == card_id) else {
        return Err(AuthoringError::CardNotFound);
    };
    let number = card.cloze_number;
    if !text_of(conn, text_id)
        .await?
        .numbers()
        .contains(&(number as u32))
    {
        return Ok(Some(RestoreBlock::NumberGone));
    }
    let elsewhere = cards.iter().any(|other| {
        !other.deleted && (other.deck_id != card.deck_id || other.cloze_number == number)
    });
    Ok(elsewhere.then_some(RestoreBlock::Elsewhere))
}

/// After a cloze card is restored: rewrites its front and back from its
/// text as it is now, which may have changed while it was deleted.
pub(crate) async fn refresh_restored(
    conn: &mut SqliteConnection,
    card_id: i64,
    now: &str,
) -> Result<(), AuthoringError> {
    let Some(text_id) = text_id_of(conn, card_id).await? else {
        return Ok(());
    };
    let number: i64 = sqlx::query_scalar("SELECT cloze_number FROM flashcards WHERE id = ?1")
        .bind(card_id)
        .fetch_one(&mut *conn)
        .await?;
    let cloze = text_of(conn, text_id).await?;
    rewrite_card(conn, card_id, &cloze, number as u32, now).await?;
    Ok(())
}

/// Refuses a move of `card_ids` unless, for every cloze text among them, all
/// of its active cards are selected.
pub(crate) async fn check_whole_texts(
    conn: &mut SqliteConnection,
    card_ids: &[i64],
) -> Result<(), AuthoringError> {
    let selected: HashSet<i64> = card_ids.iter().copied().collect();
    let mut texts = HashSet::new();
    for &card_id in card_ids {
        if let Some(text_id) = text_id_of(conn, card_id).await? {
            texts.insert(text_id);
        }
    }
    for text_id in texts {
        let active = cards_of(conn, text_id).await?;
        if active
            .iter()
            .any(|card| !card.deleted && !selected.contains(&card.id))
        {
            return Err(AuthoringError::ClozeSplit);
        }
    }
    Ok(())
}

/// A review card's pieces, if it is a cloze card.
pub(crate) async fn session_parts(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<Option<Vec<ClozePart>>, DbError> {
    let Some(text_id) = text_id_of(conn, card_id).await? else {
        return Ok(None);
    };
    let number: i64 = sqlx::query_scalar("SELECT cloze_number FROM flashcards WHERE id = ?1")
        .bind(card_id)
        .fetch_one(&mut *conn)
        .await?;
    let cloze = text_of(conn, text_id).await.map_err(|err| match err {
        AuthoringError::Internal(detail) => detail,
        other => format!("{other:?}").into(),
    })?;
    Ok(Some(cloze.parts(number as u32)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{
        archive_deck, create_deck, create_flashcard, delete_flashcard, delete_flashcards,
        move_flashcards, restore_deleted_cards, restore_flashcard, restore_flashcards,
        update_flashcard, DeckCard,
    };
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::scheduler::Rating;
    use crate::study::{self, SessionCard, StartedSession, StudyError};
    use chrono::TimeDelta;

    const NOW: &str = "2026-09-15T12:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    fn cloze(text: &str) -> Cloze {
        validate_cloze(text).unwrap()
    }

    fn problem(text: &str) -> InvalidInput {
        validate_cloze(text).unwrap_err()
    }

    async fn deck(pool: &SqlitePool, name: &str) -> i64 {
        create_deck(pool, name, None, at(NOW)).await.unwrap().id
    }

    /// The deck's cloze cards as `(number, front, back)`, by number.
    fn cloze_cards(cards: &[DeckCard]) -> Vec<(i64, String, String)> {
        let mut found: Vec<_> = cards
            .iter()
            .filter_map(|card| {
                card.cloze
                    .as_ref()
                    .map(|cloze| (cloze.number, card.front.clone(), card.back.clone()))
            })
            .collect();
        found.sort();
        found
    }

    fn card_of(cards: &[DeckCard], number: i64) -> i64 {
        cards
            .iter()
            .find(|card| card.cloze.as_ref().is_some_and(|c| c.number == number))
            .unwrap()
            .id
    }

    type Schedule = (String, Option<f64>, Option<f64>, Option<String>, i64, i64);

    async fn schedule(pool: &SqlitePool, card: i64) -> Schedule {
        sqlx::query_as(
            "SELECT fsrs_state, fsrs_stability, fsrs_difficulty, due, reps, lapses
             FROM flashcards WHERE id = ?1",
        )
        .bind(card)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn logs(pool: &SqlitePool, card: i64) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM review_logs WHERE card_id = ?1")
            .bind(card)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Starts a session in `deck` and rates every due card Good. Only one card
    /// of each text is rated: the rest are buried as its siblings.
    async fn review_all(pool: &SqlitePool, deck: i64) {
        let StartedSession::Started { session_id } =
            study::start_session(pool, deck, at(NOW)).await.unwrap()
        else {
            panic!("expected a session");
        };
        while let SessionCard::Due { card, .. } = study::session_card(pool, session_id, at(NOW))
            .await
            .unwrap()
        {
            study::record_review(pool, session_id, card.id, card.reps, Rating::Good, at(NOW))
                .await
                .unwrap();
        }
    }

    async fn session_in(pool: &SqlitePool, deck: i64, now: DateTime<Utc>) -> i64 {
        match study::start_session(pool, deck, now).await.unwrap() {
            StartedSession::Started { session_id } => session_id,
            StartedSession::NoneDue => panic!("expected a session"),
        }
    }

    /// The session's next card id and its `remaining`, or `None` once it's
    /// completed.
    async fn next_in(pool: &SqlitePool, session: i64, now: DateTime<Utc>) -> Option<(i64, i64)> {
        match study::session_card(pool, session, now).await.unwrap() {
            SessionCard::Due { card, progress, .. } => Some((card.id, progress.remaining)),
            SessionCard::Completed { .. } => None,
        }
    }

    /// Due counts on the Study Desk and on the deck screen, which must agree.
    async fn due_counts(pool: &SqlitePool, deck: i64, now: DateTime<Utc>) -> (i64, i64) {
        let desk = study::decks(pool, now)
            .await
            .unwrap()
            .into_iter()
            .find(|summary| summary.id == deck)
            .unwrap()
            .due_count;
        let detail = crate::authoring::deck_detail(pool, deck, now)
            .await
            .unwrap()
            .due_count;
        (desk, detail)
    }

    #[test]
    fn rating_a_cloze_card_buries_its_siblings_until_the_next_day() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_cloze(&pool, biology, "{{c1::A}}, {{c2::B}}, {{c3::C}}", at(NOW))
                .await
                .unwrap();
            let ids: Vec<i64> = (1..=3).map(|n| card_of(&deck.cards, n)).collect();
            let basic = create_flashcard(&pool, biology, "Q", "A", at(NOW))
                .await
                .unwrap()
                .cards
                .iter()
                .find(|card| card.cloze.is_none())
                .unwrap()
                .id;
            assert_eq!(due_counts(&pool, biology, at(NOW)).await, (4, 4));

            let session = session_in(&pool, biology, at(NOW)).await;
            assert_eq!(next_in(&pool, session, at(NOW)).await, Some((ids[0], 4)));
            study::record_review(&pool, session, ids[0], 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            let (two_before, three_before) =
                (schedule(&pool, ids[1]).await, schedule(&pool, ids[2]).await);

            // 2 and 3 are held back: the session goes on to the other card,
            // and every count agrees with it.
            assert_eq!(next_in(&pool, session, at(NOW)).await, Some((basic, 1)));
            assert_eq!(due_counts(&pool, biology, at(NOW)).await, (1, 1));
            // A held-back card can't be rated, as one that isn't due can't.
            assert!(matches!(
                study::record_review(&pool, session, ids[1], 0, Rating::Good, at(NOW)).await,
                Err(StudyError::Stale)
            ));
            study::record_review(&pool, session, basic, 0, Rating::Good, at(NOW))
                .await
                .unwrap();

            // Nothing else is due, so the session ends without them, and
            // later the same day they're still held back.
            assert_eq!(next_in(&pool, session, at(NOW)).await, None);
            let evening = at(NOW) + TimeDelta::hours(11);
            assert_eq!(due_counts(&pool, biology, evening).await, (0, 0));
            assert_eq!(
                study::start_session(&pool, biology, evening).await.unwrap(),
                StartedSession::NoneDue
            );

            // Their schedules were never touched.
            assert_eq!(schedule(&pool, ids[1]).await, two_before);
            assert_eq!(schedule(&pool, ids[2]).await, three_before);
            assert_eq!(
                (logs(&pool, ids[1]).await, logs(&pool, ids[2]).await),
                (0, 0)
            );

            // From midnight on they are due again; rating 2 holds 3 back.
            let tomorrow = at("2026-09-16T00:00:00Z");
            assert_eq!(due_counts(&pool, biology, tomorrow).await, (2, 2));
            let session = session_in(&pool, biology, tomorrow).await;
            assert_eq!(next_in(&pool, session, tomorrow).await, Some((ids[1], 2)));
            study::record_review(&pool, session, ids[1], 0, Rating::Good, tomorrow)
                .await
                .unwrap();
            assert_eq!(next_in(&pool, session, tomorrow).await, None);
            assert_eq!(due_counts(&pool, biology, tomorrow).await, (0, 0));
        });
    }

    #[test]
    fn a_card_rated_again_comes_back_while_its_siblings_wait() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_cloze(&pool, biology, "{{c1::A}} and {{c2::B}}", at(NOW))
                .await
                .unwrap();
            let (one, two) = (card_of(&deck.cards, 1), card_of(&deck.cards, 2));

            let session = session_in(&pool, biology, at(NOW)).await;
            assert_eq!(next_in(&pool, session, at(NOW)).await, Some((one, 2)));
            study::record_review(&pool, session, one, 0, Rating::Again, at(NOW))
                .await
                .unwrap();

            // Once 1 is due again it comes back, the same day; 2 doesn't.
            let again = at(&schedule(&pool, one).await.3.unwrap());
            assert!(again < at("2026-09-16T00:00:00Z"));
            assert_eq!(next_in(&pool, session, at(NOW)).await, None);
            let session = session_in(&pool, biology, again).await;
            assert_eq!(next_in(&pool, session, again).await, Some((one, 1)));
            assert_eq!(due_counts(&pool, biology, again).await, (1, 1));
            assert_eq!(schedule(&pool, two).await.4, 0);
        });
    }

    #[test]
    fn siblings_rated_the_same_day_dont_hold_each_other_back() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_cloze(&pool, biology, "{{c1::A}} and {{c2::B}}", at(NOW))
                .await
                .unwrap();
            let (one, two) = (card_of(&deck.cards, 1), card_of(&deck.cards, 2));
            // Both rated today, as a session left open across midnight (or an
            // older Synapse) could have done: neither is held back by the other.
            for card in [one, two] {
                sqlx::query(
                    "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                              elapsed_days, stability_after, difficulty_after,
                                              reviewed_at)
                     VALUES (?1, 1, 'New', 0, 0, 1.0, 5.0, ?2)",
                )
                .bind(card)
                .bind(NOW)
                .execute(&pool)
                .await
                .unwrap();
            }
            assert_eq!(due_counts(&pool, biology, at(NOW)).await, (2, 2));
        });
    }

    #[test]
    fn a_text_is_read_into_its_deletions() {
        let text =
            cloze("The {{c1::mitochondria}} makes {{c2::ATP::a molecule}}, in {{c1::cells}}.");
        assert_eq!(text.numbers(), BTreeSet::from([1, 2]));
        assert_eq!(text.front(1), "The [...] makes ATP, in [...].");
        assert_eq!(text.back(1), "mitochondria; cells");
        assert_eq!(
            text.front(2),
            "The mitochondria makes [a molecule], in cells."
        );
        assert_eq!(text.back(2), "ATP");
        assert_eq!(
            text.parts(2),
            vec![
                ClozePart::Text {
                    text: "The mitochondria makes ".into()
                },
                ClozePart::Gap {
                    answer: "ATP".into(),
                    hint: Some("a molecule".into())
                },
                ClozePart::Text {
                    text: ", in cells.".into()
                },
            ]
        );

        // Outer whitespace is trimmed; line breaks and inner spaces kept.
        let text = cloze("  Line one\n  {{c3:: two }}\n");
        assert_eq!(text.text, "Line one\n  {{c3:: two }}");
        assert_eq!(text.front(3), "Line one\n  [...]");
        assert_eq!(text.back(3), "two");

        // What only looks a little like a deletion is text.
        let text = cloze("{{c}} {{cx::y}} {c1::z} {{c1::real}}");
        assert_eq!(text.front(1), "{{c}} {{cx::y}} {c1::z} [...]");
        assert!(has_deletion("a {{c12::b"));
        assert!(!has_deletion("a {{c::b}} {{x1::c}}"));
    }

    #[test]
    fn an_unusable_text_says_why() {
        assert_eq!(problem("  \n "), InvalidInput::ClozeBlank);
        assert_eq!(problem("No deletions here."), InvalidInput::ClozeNoDeletion);
        assert_eq!(problem("The {{c1::cell"), InvalidInput::ClozeUnclosed);
        assert_eq!(
            problem("{{c1::a {{c2::b}} c}}"),
            InvalidInput::ClozeUnclosed
        );
        assert_eq!(problem("The {{c1:: }}"), InvalidInput::ClozeEmpty);
        assert_eq!(problem("The {{c1::cell::}}"), InvalidInput::ClozeEmpty);
        assert_eq!(problem("The {{c0::cell}}"), InvalidInput::ClozeNumber);
        assert_eq!(problem("The {{c1000::cell}}"), InvalidInput::ClozeNumber);
        assert_eq!(problem("{{c99999999999::x}}"), InvalidInput::ClozeNumber);

        // The same 2,000 characters as a typed card, counted after trimming.
        let longest = format!("{{{{c1::a}}}}{}", "x".repeat(CARD_TEXT_MAX_CHARS - 9));
        assert!(validate_cloze(&format!("  {longest}  ")).is_ok());
        assert_eq!(problem(&format!("{longest}x")), InvalidInput::ClozeTooLong);
    }

    #[test]
    fn a_text_adds_one_new_card_per_number() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_cloze(
                &pool,
                biology,
                "{{c2::Ribosomes}} make {{c1::proteins}}.",
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!((deck.card_count, deck.due_count), (2, 2));
            assert_eq!(
                cloze_cards(&deck.cards),
                vec![
                    (1, "Ribosomes make [...].".into(), "proteins".into()),
                    (2, "[...] make proteins.".into(), "Ribosomes".into()),
                ]
            );
            let first = &deck.cards[0];
            let cloze = first.cloze.as_ref().unwrap();
            assert_eq!(cloze.text, "{{c2::Ribosomes}} make {{c1::proteins}}.");
            assert_eq!(
                schedule(&pool, first.id).await,
                ("New".into(), None, None, None, 0, 0)
            );

            // Refused like a typed card, writing nothing.
            let sample = sample_deck_id(&pool).await;
            assert!(matches!(
                create_cloze(&pool, sample, "{{c1::x}}", at(NOW)).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert!(matches!(
                create_cloze(&pool, biology, "no deletion", at(NOW)).await,
                Err(AuthoringError::Invalid(InvalidInput::ClozeNoDeletion))
            ));
            archive_deck(&pool, biology, at(NOW)).await.unwrap();
            assert!(matches!(
                create_cloze(&pool, biology, "{{c1::x}}", at(NOW)).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM cloze_texts").await, 1);
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 3);
        });
    }

    #[test]
    fn editing_keeps_present_numbers_deletes_gone_ones_and_adds_new_ones() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_cloze(&pool, biology, "{{c1::A}} and {{c2::B}}", at(NOW))
                .await
                .unwrap();
            let (one, two) = (card_of(&deck.cards, 1), card_of(&deck.cards, 2));
            review_all(&pool, biology).await;
            let (one_before, two_before) = (schedule(&pool, one).await, schedule(&pool, two).await);

            let text = "{{c1::A}}, then {{c3::C}}";
            let preview = preview_cloze(&pool, text, Some(two)).await.unwrap();
            let later = at(NOW) + TimeDelta::hours(1);
            let (changes, deck) = update_cloze(&pool, two, text, later).await.unwrap();
            assert_eq!(changes, preview);
            assert_eq!(
                changes,
                ClozeChanges {
                    cards: 2,
                    kept: 1,
                    restored: 0,
                    added: 1,
                    deleted: 1
                }
            );

            // 1 keeps its schedule and history, with new text.
            assert_eq!(schedule(&pool, one).await, one_before);
            assert_eq!(logs(&pool, one).await, 1);
            assert_eq!(
                cloze_cards(&deck.cards),
                vec![
                    (1, "[...], then C".into(), "A".into()),
                    (3, "A, then [...]".into(), "C".into()),
                ]
            );
            // 3 is new, but 1 was rated today, so 3 waits until tomorrow as
            // its sibling. 2 is deleted, its schedule and history kept.
            let three = card_of(&deck.cards, 3);
            assert_eq!(schedule(&pool, three).await.0, "New");
            assert_eq!(deck.due_count, 0);
            assert_eq!(deck.deleted_cards.len(), 1);
            let deleted = &deck.deleted_cards[0];
            assert_eq!(deleted.id, two);
            assert_eq!(
                deleted.cloze,
                Some(crate::authoring::DeletedCloze {
                    number: 2,
                    blocked: Some(RestoreBlock::NumberGone)
                })
            );
            assert_eq!(schedule(&pool, two).await, two_before);
            assert_eq!(logs(&pool, two).await, 0);
            // Every card of the text shows the new text.
            assert!(deck
                .cards
                .iter()
                .all(|card| card.cloze.as_ref().unwrap().text == text));

            // While 2 is out of the text it can't be restored, alone or all.
            assert!(matches!(
                restore_flashcard(&pool, two, at(NOW)).await,
                Err(AuthoringError::ClozeNotRestorable(RestoreBlock::NumberGone))
            ));
            assert!(matches!(
                restore_deleted_cards(&pool, biology, 1, at(NOW)).await,
                Err(AuthoringError::DeletedCardsChanged)
            ));

            // When 2 comes back into the text, its own card comes back with
            // its old schedule, rather than a new card.
            let text = "{{c1::A}}, {{c2::B2}}, then {{c3::C}}";
            assert_eq!(
                preview_cloze(&pool, text, Some(one)).await.unwrap(),
                ClozeChanges {
                    cards: 3,
                    kept: 2,
                    restored: 1,
                    added: 0,
                    deleted: 0
                }
            );
            let (_, deck) = update_cloze(&pool, one, text, later).await.unwrap();
            assert!(deck.deleted_cards.is_empty());
            assert_eq!(card_of(&deck.cards, 2), two);
            assert_eq!(schedule(&pool, two).await, two_before);
            assert_eq!(
                cloze_cards(&deck.cards)[1],
                (2, "A, [...], then C".into(), "B2".into())
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM flashcards").await, 4);
        });
    }

    #[test]
    fn a_card_deleted_by_itself_stays_deleted_and_restores_with_the_text_as_it_is() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_cloze(&pool, biology, "{{c1::A}} {{c2::B}}", at(NOW))
                .await
                .unwrap();
            let (one, two) = (card_of(&deck.cards, 1), card_of(&deck.cards, 2));
            delete_flashcard(&pool, two, at(NOW)).await.unwrap();

            // An edit that keeps 2 in the text leaves its card deleted.
            let (changes, deck) = update_cloze(&pool, one, "{{c1::A}} {{c2::Bee}}", at(NOW))
                .await
                .unwrap();
            assert_eq!((changes.cards, changes.kept), (1, 1));
            assert_eq!(deck.deleted_cards[0].id, two);
            assert!(deck.deleted_cards[0].restorable());
            // Its deleted row isn't rewritten: it is frozen while deleted.
            assert_eq!(deck.deleted_cards[0].back, "B");

            // Restored, it takes the text as it is now.
            let deck = restore_flashcard(&pool, two, at(NOW)).await.unwrap();
            assert_eq!(
                cloze_cards(&deck.cards),
                vec![
                    (1, "[...] Bee".into(), "A".into()),
                    (2, "A [...]".into(), "Bee".into()),
                ]
            );

            // *Restore all* counts and restores only the cards that can come
            // back, alongside front/back cards.
            create_flashcard(&pool, biology, "Q", "A", at(NOW))
                .await
                .unwrap();
            let basic = deck.cards.iter().map(|card| card.id).max().unwrap() + 1;
            let all: Vec<i64> = vec![one, two, basic];
            delete_flashcards(&pool, biology, &all, at(NOW))
                .await
                .unwrap();
            refused_as_deleted(&pool, biology).await;
            let restored = restore_deleted_cards(&pool, biology, 3, at(NOW))
                .await
                .unwrap();
            assert_eq!(restored.restored, 3);
            assert_eq!(restored.deck.card_count, 3);
        });
    }

    #[test]
    fn restoring_selected_cloze_cards_follows_the_single_restore_rules() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_cloze(&pool, biology, "{{c1::A}} {{c2::B}} {{c3::C}}", at(NOW))
                .await
                .unwrap();
            let (one, two, three) = (
                card_of(&deck.cards, 1),
                card_of(&deck.cards, 2),
                card_of(&deck.cards, 3),
            );
            delete_flashcards(&pool, biology, &[two, three], at(NOW))
                .await
                .unwrap();
            // 3 leaves the text and 2 is reworded while both are deleted.
            update_cloze(&pool, one, "{{c1::A}} {{c2::Bee}}", at(NOW))
                .await
                .unwrap();
            let rows = |pool: SqlitePool| async move {
                sqlx::query_as::<_, (i64, String, String, Option<String>)>(
                    "SELECT id, front, back, deleted_at FROM flashcards ORDER BY id",
                )
                .fetch_all(&pool)
                .await
                .unwrap()
            };
            let before = rows(pool.clone()).await;

            // 3 can't come back, so neither does 2 alongside it.
            assert!(matches!(
                restore_flashcards(&pool, biology, &[two, three], at(NOW)).await,
                Err(AuthoringError::ClozeNotRestorable(RestoreBlock::NumberGone))
            ));
            assert_eq!(rows(pool.clone()).await, before);

            // 2 alone comes back with the text as it is now.
            let outcome = restore_flashcards(&pool, biology, &[two], at(NOW))
                .await
                .unwrap();
            assert_eq!(outcome.changed, 1);
            assert_eq!(
                cloze_cards(&outcome.deck.cards),
                vec![
                    (1, "[...] Bee".into(), "A".into()),
                    (2, "A [...]".into(), "Bee".into()),
                ]
            );
            assert_eq!(outcome.deck.deleted_cards[0].id, three);
        });
    }

    /// A deleted card of either kind is refused as deleted before its kind
    /// is looked at (the deck's cards are all deleted here).
    async fn refused_as_deleted(pool: &SqlitePool, deck: i64) {
        let detail = crate::authoring::deck_detail(pool, deck, at(NOW))
            .await
            .unwrap();
        assert!(detail.cards.is_empty());
        let basic = detail
            .deleted_cards
            .iter()
            .find(|card| card.cloze.is_none())
            .unwrap()
            .id;
        let cloze = detail
            .deleted_cards
            .iter()
            .find(|card| card.cloze.is_some())
            .unwrap()
            .id;
        // Deleted cards are refused as deleted first, whatever their kind.
        assert!(matches!(
            update_cloze(pool, basic, "{{c1::x}}", at(NOW)).await,
            Err(AuthoringError::CardDeleted)
        ));
        assert!(matches!(
            update_flashcard(pool, cloze, "a", "b", at(NOW)).await,
            Err(AuthoringError::CardDeleted)
        ));
    }

    #[test]
    fn each_kind_is_edited_only_as_itself() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let deck = create_flashcard(&pool, biology, "Q", "A", at(NOW))
                .await
                .unwrap();
            let basic = deck.cards[0].id;
            let deck = create_cloze(&pool, biology, "{{c1::x}}", at(NOW))
                .await
                .unwrap();
            let cloze = card_of(&deck.cards, 1);
            assert!(matches!(
                update_cloze(&pool, basic, "{{c1::y}}", at(NOW)).await,
                Err(AuthoringError::WrongKind)
            ));
            assert!(matches!(
                preview_cloze(&pool, "{{c1::y}}", Some(basic)).await,
                Err(AuthoringError::WrongKind)
            ));
            assert!(matches!(
                update_flashcard(&pool, cloze, "a", "b", at(NOW)).await,
                Err(AuthoringError::WrongKind)
            ));
            // SQLite refuses to change a card's kind or number, too.
            for sql in [
                "UPDATE flashcards SET kind = 'basic', cloze_text_id = NULL, cloze_number = NULL
                 WHERE kind = 'cloze'",
                "UPDATE flashcards SET cloze_number = 2 WHERE kind = 'cloze'",
                "INSERT INTO flashcards (deck_id, front, back, kind) VALUES (2, 'a', 'b', 'cloze')",
                "INSERT INTO flashcards (deck_id, front, back, cloze_number) VALUES (2, 'a', 'b', 1)",
                "DELETE FROM cloze_texts",
                "UPDATE cloze_texts SET text = ' '",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            // And a second active card for the same number.
            assert!(sqlx::query(
                "INSERT INTO flashcards (deck_id, front, back, kind, cloze_text_id, cloze_number)
                 SELECT deck_id, front, back, kind, cloze_text_id, cloze_number
                 FROM flashcards WHERE kind = 'cloze'"
            )
            .execute(&pool)
            .await
            .is_err());

            // An archived deck's cloze can't be edited or previewed as an edit.
            archive_deck(&pool, biology, at(NOW)).await.unwrap();
            assert!(matches!(
                update_cloze(&pool, cloze, "{{c1::y}}", at(NOW)).await,
                Err(AuthoringError::DeckArchived)
            ));
            assert_eq!(
                count(
                    &pool,
                    "SELECT COUNT(*) FROM cloze_texts WHERE text = '{{c1::x}}'"
                )
                .await,
                1
            );
        });
    }

    #[test]
    fn a_texts_cards_move_together_and_one_left_behind_stays_deleted() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let chemistry = deck(&pool, "Chemistry").await;
            let deck = create_cloze(&pool, biology, "{{c1::A}} {{c2::B}} {{c3::C}}", at(NOW))
                .await
                .unwrap();
            let ids: Vec<i64> = (1..=3).map(|n| card_of(&deck.cards, n)).collect();
            review_all(&pool, biology).await;
            delete_flashcard(&pool, ids[2], at(NOW)).await.unwrap();

            // Some of the text's active cards aren't enough.
            assert!(matches!(
                move_flashcards(&pool, biology, &ids[..1], chemistry, at(NOW)).await,
                Err(AuthoringError::ClozeSplit)
            ));
            let before = schedule(&pool, ids[0]).await;
            move_flashcards(&pool, biology, &ids[..2], chemistry, at(NOW))
                .await
                .unwrap();
            assert_eq!(schedule(&pool, ids[0]).await, before);

            // 3 was left behind in Biology: it can't come back there.
            let deck = crate::authoring::deck_detail(&pool, biology, at(NOW))
                .await
                .unwrap();
            assert_eq!(
                deck.deleted_cards[0].cloze.as_ref().unwrap().blocked,
                Some(RestoreBlock::Elsewhere)
            );
            assert!(matches!(
                restore_flashcard(&pool, ids[2], at(NOW)).await,
                Err(AuthoringError::ClozeNotRestorable(RestoreBlock::Elsewhere))
            ));

            // Taking 3 out of the text and putting it back, from Chemistry,
            // gives 3 a new card there, since its old one is elsewhere.
            update_cloze(&pool, ids[0], "{{c1::A}} {{c2::B}}", at(NOW))
                .await
                .unwrap();
            let (changes, deck) =
                update_cloze(&pool, ids[0], "{{c1::A}} {{c2::B}} {{c3::C}}", at(NOW))
                    .await
                    .unwrap();
            assert_eq!((changes.added, changes.restored), (1, 0));
            assert_eq!(deck.id, chemistry);
            assert_eq!(deck.card_count, 3);
            assert_ne!(card_of(&deck.cards, 3), ids[2]);
        });
    }

    #[test]
    fn a_cloze_card_is_reviewed_as_its_text_with_gaps() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            create_cloze(
                &pool,
                biology,
                "{{c1::Paris::city}} is in {{c2::France}}",
                at(NOW),
            )
            .await
            .unwrap();
            let StartedSession::Started { session_id } =
                study::start_session(&pool, biology, at(NOW)).await.unwrap()
            else {
                panic!("expected a session");
            };
            let SessionCard::Due { card, cloze, .. } =
                study::session_card(&pool, session_id, at(NOW))
                    .await
                    .unwrap()
            else {
                panic!("expected a due card");
            };
            assert_eq!(card.front, "[city] is in France");
            assert_eq!(
                cloze,
                Some(vec![
                    ClozePart::Gap {
                        answer: "Paris".into(),
                        hint: Some("city".into())
                    },
                    ClozePart::Text {
                        text: " is in France".into()
                    },
                ])
            );
            study::record_review(
                &pool,
                session_id,
                card.id,
                card.reps,
                Rating::Again,
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(logs(&pool, card.id).await, 1);
            assert_eq!(schedule(&pool, card.id).await.4, 1);

            // A front/back card has no pieces.
            let deck = create_flashcard(&pool, biology, "Q", "A", at(NOW))
                .await
                .unwrap();
            let mut conn = pool.acquire().await.unwrap();
            let basic = deck.cards.last().unwrap().id;
            assert_eq!(session_parts(&mut conn, basic).await.unwrap(), None);
        });
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// Cards' tags by cloze number.
    type TagsByNumber = Vec<(i64, Vec<String>)>;

    /// Each active cloze card's tags by number, and each deleted one's.
    fn cloze_tags(deck: &DeckDetail) -> (TagsByNumber, TagsByNumber) {
        let mut active: Vec<_> = deck
            .cards
            .iter()
            .filter_map(|card| Some((card.cloze.as_ref()?.number, card.tags.clone())))
            .collect();
        active.sort();
        let mut deleted: Vec<_> = deck
            .deleted_cards
            .iter()
            .filter_map(|card| Some((card.cloze.as_ref()?.number, card.tags.clone())))
            .collect();
        deleted.sort();
        (active, deleted)
    }

    #[test]
    fn a_cloze_text_tags_all_its_cards_and_new_ones_get_the_texts_tags() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let text = "{{c1::A}} and {{c2::B}}";

            // A refused tag writes nothing.
            let refused =
                create_cloze_tagged(&pool, biology, text, Some(&names(&["a b"])), at(NOW)).await;
            assert!(matches!(
                refused,
                Err(AuthoringError::Tags(crate::tags::TagProblem::BadCharacter))
            ));
            let deck = create_cloze_tagged(
                &pool,
                biology,
                text,
                Some(&names(&["exam", "bio"])),
                at(NOW),
            )
            .await
            .unwrap();
            let both = names(&["bio", "exam"]);
            assert_eq!(
                cloze_tags(&deck).0,
                vec![(1, both.clone()), (2, both.clone())]
            );
            let one = card_of(&deck.cards, 1);
            // Without tags sent, the cards there keep theirs and a new number
            // gets the edited card's.
            let later = at(NOW) + TimeDelta::hours(1);
            let (_, deck) = update_cloze_tagged(&pool, one, "{{c1::A}} and {{c3::C}}", None, later)
                .await
                .unwrap();
            assert_eq!(
                cloze_tags(&deck).0,
                vec![(1, both.clone()), (3, both.clone())]
            );
            let updated_at = |pool: SqlitePool| async move {
                let at: String =
                    sqlx::query_scalar("SELECT updated_at FROM flashcards WHERE id = ?1")
                        .bind(one)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                at
            };
            let before = updated_at(pool.clone()).await;

            // Tags sent replace every active card's; the deleted 2 keeps its own.
            let chem = names(&["chem"]);
            let (_, deck) =
                update_cloze_tagged(&pool, one, "{{c1::A}} and {{c3::C}}", Some(&chem), later)
                    .await
                    .unwrap();
            assert_eq!(
                cloze_tags(&deck),
                (vec![(1, chem.clone()), (3, chem.clone())], vec![(2, both)])
            );

            // The same text with new tags changes no card's text or `updated_at`.
            assert_eq!(updated_at(pool.clone()).await, before);

            // 2 coming back by an edit takes the text's tags.
            let (changes, deck) = update_cloze_tagged(
                &pool,
                one,
                "{{c1::A}}, {{c2::B}} and {{c3::C}}",
                None,
                later,
            )
            .await
            .unwrap();
            assert_eq!(changes.restored, 1);
            assert_eq!(
                cloze_tags(&deck).0,
                vec![(1, chem.clone()), (2, chem.clone()), (3, chem)]
            );
        });
    }
}
