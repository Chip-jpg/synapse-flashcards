//! Tags on cards and notes (migration 0020).
//!
//! A tag is a short label, one word as in Anki (`bio`, `exam-june`,
//! `bio::cells`), that the user gives cards and notes to find them again
//! across decks. Every kind of card takes tags, and so does every note.
//!
//! Tags are sent and returned by name, never by id: the frontend sends the
//! names a card or note should have, and a name nobody has used yet becomes a
//! new tag in the same transaction. Names are told apart ignoring letter
//! case, compared in Rust (`to_lowercase`) so that `É` and `é` match too,
//! which SQLite's own `NOCASE` doesn't do; the case first written is the one
//! kept and shown.
//!
//! Tags follow the card or note they are on, as its pictures do (0017):
//! saving replaces the list, and a deleted card or note, or a card in an
//! archived deck, keeps its tags frozen. A tag row is never removed, so a
//! card that comes back finds its tags; a tag no active card or note uses is
//! just not listed ([`tags`]).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{to_db_time, DbError};

/// The longest tag name, in characters (the migration repeats it).
pub const TAG_MAX_CHARS: usize = 60;
/// The most tags one card or note has.
pub const TAGS_MAX: usize = 20;

/// What was wrong with the tags sent. Nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagProblem {
    /// A name that is empty once trimmed.
    Blank,
    /// A name over [`TAG_MAX_CHARS`].
    TooLong,
    /// A name with a space (or any whitespace) or a comma in it: a tag is one
    /// word, so `exam june` is two tags.
    BadCharacter,
    /// More than [`TAGS_MAX`] tags on one card or note.
    TooMany,
    /// Another tag already has this name (ignoring letter case), so renaming
    /// onto it would make two tags one.
    NameTaken,
}

/// Why a tag operation didn't happen.
#[derive(Debug)]
pub enum TagError {
    Invalid(TagProblem),
    /// No tag has this id.
    NotFound,
    Internal(DbError),
}

impl From<sqlx::Error> for TagError {
    fn from(err: sqlx::Error) -> Self {
        TagError::Internal(err.into())
    }
}

/// A tag as listed: `{ id, name, cards, notes }`, with how many active cards
/// (in decks that aren't archived) and active notes have it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, sqlx::FromRow)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
    pub cards: i64,
    pub notes: i64,
}

/// Checks one name and returns it trimmed.
pub fn checked_name(name: &str) -> Result<&str, TagProblem> {
    let name = name.trim();
    if name.is_empty() {
        Err(TagProblem::Blank)
    } else if name.chars().count() > TAG_MAX_CHARS {
        Err(TagProblem::TooLong)
    } else if name.chars().any(|c| c.is_whitespace() || c == ',') {
        Err(TagProblem::BadCharacter)
    } else {
        Ok(name)
    }
}

/// Checks a list of names for one card or note: each name as
/// [`checked_name`], a name repeated (ignoring case) kept once, in the order
/// first given, and at most [`TAGS_MAX`] of them after that.
pub fn checked_names(names: &[String]) -> Result<Vec<String>, TagProblem> {
    let mut seen = HashSet::new();
    let mut kept = Vec::new();
    for name in names {
        let name = checked_name(name)?;
        if seen.insert(name.to_lowercase()) {
            kept.push(name.to_string());
        }
    }
    if kept.len() > TAGS_MAX {
        return Err(TagProblem::TooMany);
    }
    Ok(kept)
}

/// Every tag's id by its lowercase name.
async fn ids_by_name(conn: &mut SqliteConnection) -> Result<HashMap<String, i64>, sqlx::Error> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM tags")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| (name.to_lowercase(), id))
        .collect())
}

/// The ids of already-checked names ([`checked_names`]), in the same order,
/// adding a tag for each name not used before. Run in the transaction that
/// links them.
pub async fn tag_ids(
    conn: &mut SqliteConnection,
    names: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<i64>, sqlx::Error> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut known = ids_by_name(conn).await?;
    let now = to_db_time(now);
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        let key = name.to_lowercase();
        let id = match known.get(&key) {
            Some(&id) => id,
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO tags (name, created_at) VALUES (?1, ?2) RETURNING id",
                )
                .bind(name)
                .bind(&now)
                .fetch_one(&mut *conn)
                .await?;
                known.insert(key, id);
                id
            }
        };
        ids.push(id);
    }
    Ok(ids)
}

/// Which table links tags to what.
#[derive(Debug, Clone, Copy)]
enum Owner {
    Card,
    Note,
}

impl Owner {
    fn select_sql(self) -> &'static str {
        match self {
            Owner::Card => "SELECT tag_id FROM card_tags WHERE card_id = ?1",
            Owner::Note => "SELECT tag_id FROM note_tags WHERE note_id = ?1",
        }
    }

    fn delete_sql(self) -> &'static str {
        match self {
            Owner::Card => "DELETE FROM card_tags WHERE card_id = ?1 AND tag_id = ?2",
            Owner::Note => "DELETE FROM note_tags WHERE note_id = ?1 AND tag_id = ?2",
        }
    }

    fn insert_sql(self) -> &'static str {
        match self {
            Owner::Card => "INSERT INTO card_tags (card_id, tag_id) VALUES (?1, ?2)",
            Owner::Note => "INSERT INTO note_tags (note_id, tag_id) VALUES (?1, ?2)",
        }
    }
}

async fn linked_ids(
    conn: &mut SqliteConnection,
    owner: Owner,
    id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(owner.select_sql())
        .bind(id)
        .fetch_all(&mut *conn)
        .await
}

/// Makes `id`'s tags exactly `tag_ids`, writing only the links that change,
/// so saving a card or note with the tags it already has writes nothing.
async fn set_links(
    conn: &mut SqliteConnection,
    owner: Owner,
    id: i64,
    tag_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let current: HashSet<i64> = linked_ids(conn, owner, id).await?.into_iter().collect();
    let wanted: HashSet<i64> = tag_ids.iter().copied().collect();
    for &gone in current.difference(&wanted) {
        sqlx::query(owner.delete_sql())
            .bind(id)
            .bind(gone)
            .execute(&mut *conn)
            .await?;
    }
    for &new in tag_ids.iter().filter(|tag| !current.contains(tag)) {
        sqlx::query(owner.insert_sql())
            .bind(id)
            .bind(new)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Replaces an active card's tags with the already-checked `names`. Run in
/// the transaction that saves the card, after the card and its deck have been
/// checked (an active card of an active normal deck).
pub async fn set_card_tags(
    conn: &mut SqliteConnection,
    card_id: i64,
    names: &[String],
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let ids = tag_ids(conn, names, now).await?;
    set_links(conn, Owner::Card, card_id, &ids).await
}

/// Gives a card just added, which has no tags yet, the tags `tag_ids` (from
/// [`tag_ids`], at most [`TAGS_MAX`], each once). For imports, which look
/// every name up once rather than once a card.
pub async fn link_new_card(
    conn: &mut SqliteConnection,
    card_id: i64,
    tag_ids: &[i64],
) -> Result<(), sqlx::Error> {
    for &tag_id in tag_ids {
        sqlx::query(Owner::Card.insert_sql())
            .bind(card_id)
            .bind(tag_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Replaces an active note's tags with the already-checked `names`. Run in
/// the transaction that saves the note, after it has been checked.
pub async fn set_note_tags(
    conn: &mut SqliteConnection,
    note_id: i64,
    names: &[String],
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let ids = tag_ids(conn, names, now).await?;
    set_links(conn, Owner::Note, note_id, &ids).await
}

/// Adds `add` to each card's tags and takes `remove` off, keeping its other
/// tags (bulk tagging). Names are already checked; a card that already has a
/// tag being added, or lacks one being removed, is simply left as it is. A
/// card that would end up with more than [`TAGS_MAX`] tags refuses the whole
/// change. Returns how many cards' tags changed. The caller has checked every
/// card is an active card of an active normal deck.
pub async fn change_card_tags(
    conn: &mut SqliteConnection,
    card_ids: &[i64],
    add: &[String],
    remove: &[String],
    now: DateTime<Utc>,
) -> Result<Result<usize, TagProblem>, sqlx::Error> {
    let known = ids_by_name(conn).await?;
    let removed: HashSet<i64> = remove
        .iter()
        .filter_map(|name| known.get(&name.to_lowercase()).copied())
        .collect();
    // A name both added and removed is removed: taking a tag off is the
    // safer reading of a contradictory request.
    let removed_names: HashSet<String> = remove.iter().map(|n| n.to_lowercase()).collect();
    let add: Vec<String> = add
        .iter()
        .filter(|name| !removed_names.contains(&name.to_lowercase()))
        .cloned()
        .collect();
    let added = tag_ids(conn, &add, now).await?;

    let mut plans = Vec::with_capacity(card_ids.len());
    for &card_id in card_ids {
        let current = linked_ids(conn, Owner::Card, card_id).await?;
        let mut wanted: Vec<i64> = current
            .iter()
            .copied()
            .filter(|tag| !removed.contains(tag))
            .collect();
        for &tag in &added {
            if !wanted.contains(&tag) {
                wanted.push(tag);
            }
        }
        if wanted.len() > TAGS_MAX {
            return Ok(Err(TagProblem::TooMany));
        }
        let same = wanted.len() == current.len() && wanted.iter().all(|t| current.contains(t));
        if !same {
            plans.push((card_id, wanted));
        }
    }
    for (card_id, wanted) in &plans {
        set_links(conn, Owner::Card, *card_id, wanted).await?;
    }
    Ok(Ok(plans.len()))
}

/// A tag-name query on `card_tags`/`note_tags`, grouped into a map by owner id.
async fn names_by_owner(
    conn: &mut SqliteConnection,
    sql: &'static str,
    id: Option<i64>,
) -> Result<HashMap<i64, Vec<String>>, sqlx::Error> {
    let mut query = sqlx::query_as::<_, (i64, String)>(sql);
    if let Some(id) = id {
        query = query.bind(id);
    }
    let rows = query.fetch_all(&mut *conn).await?;
    let mut by_owner: HashMap<i64, Vec<String>> = HashMap::new();
    for (owner, name) in rows {
        by_owner.entry(owner).or_default().push(name);
    }
    Ok(by_owner)
}

/// Sorts names as they are listed everywhere: ignoring case, then as written.
fn sort_names(names: &mut [String]) {
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
}

fn sorted(mut by_owner: HashMap<i64, Vec<String>>) -> HashMap<i64, Vec<String>> {
    for names in by_owner.values_mut() {
        sort_names(names);
    }
    by_owner
}

/// One card's tags, sorted by name.
#[cfg(test)]
pub async fn card_tags(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let mut by_card = names_by_owner(
        conn,
        "SELECT l.card_id, t.name FROM card_tags l JOIN tags t ON t.id = l.tag_id
         WHERE l.card_id = ?1",
        Some(card_id),
    )
    .await?;
    let mut names = by_card.remove(&card_id).unwrap_or_default();
    sort_names(&mut names);
    Ok(names)
}

/// The tags of every card in a deck that has any, deleted cards included,
/// each list sorted by name, by card id.
pub async fn deck_card_tags(
    conn: &mut SqliteConnection,
    deck_id: i64,
) -> Result<HashMap<i64, Vec<String>>, sqlx::Error> {
    names_by_owner(
        conn,
        "SELECT l.card_id, t.name
         FROM card_tags l
         JOIN flashcards f ON f.id = l.card_id
         JOIN tags t ON t.id = l.tag_id
         WHERE f.deck_id = ?1",
        Some(deck_id),
    )
    .await
    .map(sorted)
}

/// One note's tags, sorted by name.
pub async fn note_tags(
    conn: &mut SqliteConnection,
    note_id: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let mut by_note = names_by_owner(
        conn,
        "SELECT l.note_id, t.name FROM note_tags l JOIN tags t ON t.id = l.tag_id
         WHERE l.note_id = ?1",
        Some(note_id),
    )
    .await?;
    let mut names = by_note.remove(&note_id).unwrap_or_default();
    sort_names(&mut names);
    Ok(names)
}

/// The tags of every note that has any, deleted notes included, each list
/// sorted by name, by note id.
pub async fn all_note_tags(
    conn: &mut SqliteConnection,
) -> Result<HashMap<i64, Vec<String>>, sqlx::Error> {
    names_by_owner(
        conn,
        "SELECT l.note_id, t.name FROM note_tags l JOIN tags t ON t.id = l.tag_id",
        None,
    )
    .await
    .map(sorted)
}

/// Every tag some active card (in a deck that isn't archived) or active note
/// has, with how many of each, sorted by name ignoring case. A tag only
/// deleted or archived things have isn't listed, but it is kept, and comes
/// back with them.
pub async fn tags(pool: &SqlitePool) -> Result<Vec<TagSummary>, sqlx::Error> {
    let mut list = sqlx::query_as::<_, TagSummary>(
        "SELECT t.id, t.name,
                (SELECT COUNT(*) FROM card_tags l
                 JOIN flashcards f ON f.id = l.card_id
                 JOIN decks d ON d.id = f.deck_id
                 WHERE l.tag_id = t.id AND f.deleted_at IS NULL
                   AND d.archived_at IS NULL) AS cards,
                (SELECT COUNT(*) FROM note_tags l
                 JOIN notes n ON n.id = l.note_id
                 WHERE l.tag_id = t.id AND n.deleted_at IS NULL) AS notes
         FROM tags t",
    )
    .fetch_all(pool)
    .await?;
    list.retain(|tag| tag.cards > 0 || tag.notes > 0);
    list.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.name.cmp(&b.name))
    });
    Ok(list)
}

/// Renames a tag everywhere it is used, deleted cards and notes included (a
/// tag is a label, not part of their history). Refused if another tag already
/// has the name, ignoring case; changing only the case of a tag's own name is
/// allowed. Returns the tags as [`tags`] lists them.
pub async fn rename_tag(
    pool: &SqlitePool,
    tag_id: i64,
    name: &str,
) -> Result<Vec<TagSummary>, TagError> {
    let name = checked_name(name).map_err(TagError::Invalid)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let known = ids_by_name(&mut tx).await?;
    if !known.values().any(|&id| id == tag_id) {
        return Err(TagError::NotFound);
    }
    if known
        .get(&name.to_lowercase())
        .is_some_and(|&other| other != tag_id)
    {
        return Err(TagError::Invalid(TagProblem::NameTaken));
    }
    sqlx::query("UPDATE tags SET name = ?1 WHERE id = ?2")
        .bind(name)
        .bind(tag_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(tags(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{self, AuthoringError};
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::notes::{self, NoteError};

    const NOW: &str = "2026-10-22T12:00:00Z";
    const LATER: &str = "2026-10-22T13:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    async fn deck(pool: &SqlitePool, name: &str) -> i64 {
        authoring::create_deck(pool, name, None, at(NOW))
            .await
            .unwrap()
            .id
    }

    async fn card(pool: &SqlitePool, deck_id: i64, front: &str) -> i64 {
        let deck = authoring::create_flashcard(pool, deck_id, front, "back", at(NOW))
            .await
            .unwrap();
        deck.cards.iter().find(|c| c.front == front).unwrap().id
    }

    async fn rows(pool: &SqlitePool, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }

    #[test]
    fn names_are_one_trimmed_word_each_kept_once() {
        assert_eq!(
            checked_names(&names(&[" bio ", "Bio", "exam-june", "bio::cells"])).unwrap(),
            names(&["bio", "exam-june", "bio::cells"])
        );
        assert_eq!(checked_names(&names(&["  "])), Err(TagProblem::Blank));
        assert_eq!(
            checked_names(&names(&["exam june"])),
            Err(TagProblem::BadCharacter)
        );
        assert_eq!(
            checked_names(&names(&["a,b"])),
            Err(TagProblem::BadCharacter)
        );
        assert_eq!(
            checked_names(&names(&["a\u{a0}b"])),
            Err(TagProblem::BadCharacter)
        );
        assert_eq!(
            checked_names(&["x".repeat(TAG_MAX_CHARS + 1)]),
            Err(TagProblem::TooLong)
        );
        assert!(checked_names(&["è".repeat(TAG_MAX_CHARS)]).is_ok());
        let many: Vec<String> = (0..=TAGS_MAX).map(|n| format!("t{n}")).collect();
        assert_eq!(checked_names(&many), Err(TagProblem::TooMany));
        assert!(checked_names(&many[..TAGS_MAX]).is_ok());
    }

    #[test]
    fn a_card_and_a_note_get_tags_saved_with_them() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck_id = deck(&pool, "Bio").await;
            let detail = authoring::create_flashcard_tagged(
                &pool,
                deck_id,
                "Cell",
                "Unit of life",
                &Default::default(),
                Some(&names(&["bio", "Exam"])),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(detail.cards[0].tags, names(&["bio", "Exam"]));

            // The same names in another case are the same tags.
            let note = notes::create_note_tagged(
                &pool,
                "Cells",
                "Body",
                &[],
                Some(&names(&["BIO", "énigme"])),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(note.tags, names(&["bio", "énigme"]));
            assert_eq!(rows(&pool, "SELECT COUNT(*) FROM tags").await, 3);

            // Saving with `None` keeps the tags; with a list, replaces them.
            let detail = authoring::update_flashcard_tagged(
                &pool,
                detail.cards[0].id,
                "Cell",
                "Unit",
                None,
                None,
                at(LATER),
            )
            .await
            .unwrap();
            assert_eq!(detail.cards[0].tags, names(&["bio", "Exam"]));
            let note = notes::update_note_tagged(
                &pool,
                note.id,
                "Cells",
                "Body",
                None,
                Some(&[]),
                at(LATER),
            )
            .await
            .unwrap();
            assert!(note.tags.is_empty());

            let listed = tags(&pool).await.unwrap();
            assert_eq!(
                listed
                    .iter()
                    .map(|t| (t.name.as_str(), t.cards, t.notes))
                    .collect::<Vec<_>>(),
                vec![("bio", 1, 0), ("Exam", 1, 0)]
            );
        });
    }

    #[test]
    fn bad_tags_save_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck_id = deck(&pool, "Bio").await;
            let refused = authoring::create_flashcard_tagged(
                &pool,
                deck_id,
                "F",
                "B",
                &Default::default(),
                Some(&names(&["two words"])),
                at(NOW),
            )
            .await;
            assert!(matches!(
                refused,
                Err(AuthoringError::Tags(TagProblem::BadCharacter))
            ));
            let refused =
                notes::create_note_tagged(&pool, "T", "B", &[], Some(&names(&[""])), at(NOW)).await;
            assert!(matches!(
                refused,
                Err(NoteError::Invalid(notes::InvalidNote::Tags(
                    TagProblem::Blank
                )))
            ));
            assert_eq!(
                rows(&pool, "SELECT COUNT(*) FROM flashcards WHERE deck_id > 1").await,
                0
            );
            assert_eq!(rows(&pool, "SELECT COUNT(*) FROM notes").await, 0);
            assert_eq!(rows(&pool, "SELECT COUNT(*) FROM tags").await, 0);
        });
    }

    #[test]
    fn deleted_and_archived_cards_keep_their_tags() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck_id = deck(&pool, "Bio").await;
            let card_id = card(&pool, deck_id, "F").await;
            authoring::set_card_tags(&pool, card_id, &names(&["bio"]), at(NOW))
                .await
                .unwrap();

            authoring::delete_flashcard(&pool, card_id, at(NOW))
                .await
                .unwrap();
            // Frozen, and refused, while deleted.
            assert!(matches!(
                authoring::set_card_tags(&pool, card_id, &[], at(NOW)).await,
                Err(AuthoringError::CardDeleted)
            ));
            let removed = sqlx::query("DELETE FROM card_tags").execute(&pool).await;
            assert!(
                removed.is_err(),
                "the migration keeps a deleted card's tags"
            );
            // Not listed while only a deleted card has it, but kept.
            assert!(tags(&pool).await.unwrap().is_empty());
            let detail = authoring::deck_detail(&pool, deck_id, at(NOW))
                .await
                .unwrap();
            assert_eq!(detail.deleted_cards[0].tags, names(&["bio"]));

            let detail = authoring::restore_flashcard(&pool, card_id, at(LATER))
                .await
                .unwrap();
            assert_eq!(detail.cards[0].tags, names(&["bio"]));

            authoring::archive_deck(&pool, deck_id, at(LATER))
                .await
                .unwrap();
            let refused = sqlx::query("DELETE FROM card_tags").execute(&pool).await;
            assert!(refused.is_err(), "an archived deck's cards keep their tags");
            assert!(tags(&pool).await.unwrap().is_empty());
            authoring::unarchive_deck(&pool, deck_id).await.unwrap();
            assert_eq!(tags(&pool).await.unwrap()[0].cards, 1);
        });
    }

    #[test]
    fn the_sample_deck_takes_no_tags() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let sample: i64 = rows(
                &pool,
                "SELECT f.id FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 WHERE d.is_sample = 1 LIMIT 1",
            )
            .await;
            assert!(matches!(
                authoring::set_card_tags(&pool, sample, &names(&["x"]), at(NOW)).await,
                Err(AuthoringError::SampleDeck)
            ));
            assert_eq!(rows(&pool, "SELECT COUNT(*) FROM tags").await, 0);
        });
    }

    #[test]
    fn deleted_notes_keep_their_tags() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let note =
                notes::create_note_tagged(&pool, "T", "B", &[], Some(&names(&["a"])), at(NOW))
                    .await
                    .unwrap();
            notes::delete_note(&pool, note.id, at(NOW)).await.unwrap();
            assert!(matches!(
                notes::set_note_tags(&pool, note.id, &[], at(NOW)).await,
                Err(NoteError::Deleted)
            ));
            assert!(sqlx::query("DELETE FROM note_tags")
                .execute(&pool)
                .await
                .is_err());
            assert!(tags(&pool).await.unwrap().is_empty());
            notes::restore_note(&pool, note.id).await.unwrap();
            assert_eq!(
                notes::note(&pool, note.id).await.unwrap().tags,
                names(&["a"])
            );
            let library = notes::notes(&pool).await.unwrap();
            assert_eq!(library[0].tags, names(&["a"]));
        });
    }

    #[test]
    fn bulk_tagging_adds_and_removes_on_the_selected_cards_only() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck_id = deck(&pool, "Bio").await;
            let one = card(&pool, deck_id, "one").await;
            let two = card(&pool, deck_id, "two").await;
            let three = card(&pool, deck_id, "three").await;
            authoring::set_card_tags(&pool, one, &names(&["old", "keep"]), at(NOW))
                .await
                .unwrap();

            let changed = authoring::tag_flashcards(
                &pool,
                deck_id,
                &[one, two],
                &names(&["New"]),
                &names(&["old"]),
                at(LATER),
            )
            .await
            .unwrap();
            assert_eq!(changed.changed, 2);
            let tags_of = |id: i64| {
                changed
                    .deck
                    .cards
                    .iter()
                    .find(|c| c.id == id)
                    .unwrap()
                    .tags
                    .clone()
            };
            assert_eq!(tags_of(one), names(&["keep", "New"]));
            assert_eq!(tags_of(two), names(&["New"]));
            assert!(tags_of(three).is_empty());

            // Asking again changes nothing, and says so.
            let again = authoring::tag_flashcards(
                &pool,
                deck_id,
                &[one, two],
                &names(&["new"]),
                &[],
                at(LATER),
            )
            .await
            .unwrap();
            assert_eq!(again.changed, 0);

            // A card no longer in the deck makes the selection stale.
            authoring::delete_flashcard(&pool, three, at(LATER))
                .await
                .unwrap();
            assert!(matches!(
                authoring::tag_flashcards(
                    &pool,
                    deck_id,
                    &[one, three],
                    &names(&["x"]),
                    &[],
                    at(LATER)
                )
                .await,
                Err(AuthoringError::SelectionChanged)
            ));
            // Nothing to add or remove is refused before anything is read.
            assert!(matches!(
                authoring::tag_flashcards(&pool, deck_id, &[one], &[], &[], at(LATER)).await,
                Err(AuthoringError::NoTagChange)
            ));
            // Going over the limit on any card refuses the whole change.
            let many: Vec<String> = (0..TAGS_MAX).map(|n| format!("t{n}")).collect();
            assert!(matches!(
                authoring::tag_flashcards(&pool, deck_id, &[one, two], &many, &[], at(LATER)).await,
                Err(AuthoringError::Tags(TagProblem::TooMany))
            ));
            assert_eq!(
                authoring::deck_detail(&pool, deck_id, at(LATER))
                    .await
                    .unwrap()
                    .cards
                    .iter()
                    .find(|c| c.id == two)
                    .unwrap()
                    .tags,
                names(&["New"])
            );
            assert_eq!(
                rows(&pool, "SELECT COUNT(*) FROM tags WHERE name = 't0'").await,
                0
            );
        });
    }

    #[test]
    fn renaming_a_tag_renames_it_everywhere_but_never_merges() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck_id = deck(&pool, "Bio").await;
            let one = card(&pool, deck_id, "one").await;
            authoring::set_card_tags(&pool, one, &names(&["bio", "chem"]), at(NOW))
                .await
                .unwrap();
            let bio = tags(&pool).await.unwrap()[0].id;

            let listed = rename_tag(&pool, bio, "Biology").await.unwrap();
            assert_eq!(listed[0].name, "Biology");
            // Changing only its own name's case is fine.
            let listed = rename_tag(&pool, bio, "biology").await.unwrap();
            assert_eq!(listed[0].name, "biology");
            assert!(matches!(
                rename_tag(&pool, bio, "CHEM").await,
                Err(TagError::Invalid(TagProblem::NameTaken))
            ));
            assert!(matches!(
                rename_tag(&pool, bio, "two words").await,
                Err(TagError::Invalid(TagProblem::BadCharacter))
            ));
            assert!(matches!(
                rename_tag(&pool, 999, "x").await,
                Err(TagError::NotFound)
            ));
            assert_eq!(
                card_tags(&mut pool.acquire().await.unwrap(), one)
                    .await
                    .unwrap(),
                names(&["biology", "chem"])
            );
        });
    }

    #[test]
    fn tags_never_go_away() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck_id = deck(&pool, "Bio").await;
            let one = card(&pool, deck_id, "one").await;
            authoring::set_card_tags(&pool, one, &names(&["bio"]), at(NOW))
                .await
                .unwrap();
            authoring::set_card_tags(&pool, one, &[], at(NOW))
                .await
                .unwrap();
            assert!(sqlx::query("DELETE FROM tags")
                .execute(&pool)
                .await
                .is_err());
            assert!(sqlx::query("UPDATE tags SET created_at = 'x'")
                .execute(&pool)
                .await
                .is_err());
            assert_eq!(rows(&pool, "SELECT COUNT(*) FROM tags").await, 1);
            // Unused, so not listed; a later card with that name reuses it.
            assert!(tags(&pool).await.unwrap().is_empty());
            authoring::set_card_tags(&pool, one, &names(&["BIO"]), at(NOW))
                .await
                .unwrap();
            assert_eq!(rows(&pool, "SELECT COUNT(*) FROM tags").await, 1);
            assert_eq!(tags(&pool).await.unwrap()[0].name, "bio");
        });
    }
}
