//! Searching every card and every note at once, read-only.
//!
//! Uses `search_index`, the FTS5 table migration 0013 creates and keeps in
//! step with `flashcards` and `notes` by triggers. Its rowid says which item a
//! row is for: a card's is `2 * id`, a note's `2 * id + 1`. The index holds
//! every card and note; what may be shown is decided here, from the tables:
//! cards not deleted, in decks not archived, and notes not deleted.
//!
//! What the user types is never handed to FTS5 as query syntax. It is split
//! into words the way the index splits text (letters and digits), each word is
//! quoted, and each matches as a prefix: "mito cell" finds text with a word
//! starting "mito" and a word starting "cell". Case and accents are ignored by
//! the index's tokenizer, for the words typed as for the text. Results are
//! ranked by FTS5's bm25, with a match in a card's front or a note's title
//! counting twice what one in a back or a body does.
//!
//! Results can also be narrowed to a tag (migration 0020), chosen from a list
//! or typed as `#name` among the words. A tag also finds the tags below it
//! (`#bio` finds `bio::cells`), as in Anki. With a tag and no words, every
//! card and note with the tag is listed.

use std::collections::{HashMap, HashSet};

use sqlx::{SqliteConnection, SqlitePool};

use crate::study::StudyError;
use crate::tags;

/// At most this many results are returned; `more` says whether there were
/// others. A search is for finding something, not for listing everything.
pub const RESULT_LIMIT: usize = 50;
/// The longest search accepted, in characters. The message in `commands.rs`
/// says this number.
pub const MAX_QUERY_CHARS: usize = 500;
/// Words after these are ignored, which keeps a pasted paragraph from
/// becoming a huge query.
pub const MAX_TERMS: usize = 10;

/// Snippet markers: control characters, which nothing in Synapse writes into
/// text on purpose. A marker turns highlighting on or off; the screen gets
/// plain pieces of text, never markup.
const MARK_START: char = '\u{2}';
const MARK_END: char = '\u{3}';

/// What to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    All,
    Cards,
    Notes,
}

/// `{ results, more }`: the best matches first, and whether more matched
/// than [`RESULT_LIMIT`].
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct SearchResults {
    pub results: Vec<Hit>,
    pub more: bool,
}

/// One match. A card: `{ kind: "card", cardId, deckId, deckName, front, back,
/// tags }`. A note: `{ kind: "note", noteId, title, body, tags }`. The text
/// fields are snippets as [`Piece`]s; a long side or body is cut around its
/// matches, with "…" where text was left out. `tags` are the item's tags,
/// sorted by name.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Hit {
    #[serde(rename_all = "camelCase")]
    Card {
        card_id: i64,
        deck_id: i64,
        deck_name: String,
        front: Vec<Piece>,
        back: Vec<Piece>,
        tags: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Note {
        note_id: i64,
        title: Vec<Piece>,
        body: Vec<Piece>,
        tags: Vec<String>,
    },
}

/// A run of snippet text, `{ text, hit }`: `hit` is true for a matched word.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Piece {
    pub text: String,
    pub hit: bool,
}

/// The words of `text` as the index splits them: runs of letters and digits
/// (combining accents stay with their letter). Everything else separates
/// words, so no quote, bracket, `*`, `-`, `:` or `^` ever reaches FTS5.
pub fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || ('\u{300}'..='\u{36f}').contains(&c)))
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .take(MAX_TERMS)
        .map(str::to_string)
        .collect()
}

/// The FTS5 query for `terms`: each one quoted, as a prefix, all required.
/// The terms hold only letters, digits and accents, so the quotes can't be
/// broken out of.
fn fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{term}\"*"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// [`search_tagged`] with no tag chosen (a tag can still be typed).
#[cfg(test)]
pub async fn search(
    pool: &SqlitePool,
    text: &str,
    scope: Scope,
    deck: Option<i64>,
) -> Result<SearchResults, StudyError> {
    search_tagged(pool, text, scope, deck, None).await
}

/// Splits what was typed into the words to search for and the tags asked
/// for: each word starting with `#` is a tag (`#bio`), added to `chosen`.
/// Tags come back lowercase, each once, at most [`MAX_TERMS`] of them; a lone
/// `#` is nothing.
pub fn typed_tags(text: &str, chosen: Option<&str>) -> (String, Vec<String>) {
    let mut words = Vec::new();
    let mut wanted: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim().trim_matches(',').to_lowercase();
        if !name.is_empty() && !wanted.contains(&name) && wanted.len() < MAX_TERMS {
            wanted.push(name);
        }
    };
    if let Some(chosen) = chosen {
        add(chosen);
    }
    for word in text.split_whitespace() {
        match word.strip_prefix('#') {
            Some(name) => add(name),
            None => words.push(word),
        }
    }
    (words.join(" "), wanted)
}

/// Whether a tag named `name` is what `wanted` (lowercase) asks for: the tag
/// itself, ignoring case, or one below it in a `::` hierarchy.
fn tag_matches(name: &str, wanted: &str) -> bool {
    let name = name.to_lowercase();
    name == wanted
        || name
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.starts_with("::"))
}

/// The cards and notes that have every tag in `wanted` (each as
/// [`tag_matches`] reads it), deleted ones included: the main query leaves
/// those out. Two JSON arrays of ids, for `json_each`.
async fn tagged(
    conn: &mut SqliteConnection,
    wanted: &[String],
) -> Result<(String, String), sqlx::Error> {
    let all: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM tags")
        .fetch_all(&mut *conn)
        .await?;
    // For each tag asked for, the ids of the tags that answer it.
    let groups: Vec<HashSet<i64>> = wanted
        .iter()
        .map(|wanted| {
            all.iter()
                .filter(|(_, name)| tag_matches(name, wanted))
                .map(|(id, _)| *id)
                .collect()
        })
        .collect();
    if groups.iter().any(HashSet::is_empty) {
        return Ok(("[]".to_string(), "[]".to_string()));
    }
    let ids: Vec<i64> = groups.iter().flatten().copied().collect();
    let ids = serde_json::to_string(&ids).expect("ids serialize");

    let mut lists = Vec::with_capacity(2);
    for sql in [
        "SELECT card_id, tag_id FROM card_tags WHERE tag_id IN (SELECT value FROM json_each(?1))",
        "SELECT note_id, tag_id FROM note_tags WHERE tag_id IN (SELECT value FROM json_each(?1))",
    ] {
        let links: Vec<(i64, i64)> = sqlx::query_as(sql).bind(&ids).fetch_all(&mut *conn).await?;
        let mut owned: HashMap<i64, HashSet<i64>> = HashMap::new();
        for (owner, tag) in links {
            owned.entry(owner).or_default().insert(tag);
        }
        let mut matching: Vec<i64> = owned
            .into_iter()
            .filter(|(_, tags)| groups.iter().all(|group| !group.is_disjoint(tags)))
            .map(|(owner, _)| owner)
            .collect();
        matching.sort_unstable();
        lists.push(serde_json::to_string(&matching).expect("ids serialize"));
    }
    let notes = lists.pop().unwrap_or_default();
    let cards = lists.pop().unwrap_or_default();
    Ok((cards, notes))
}

/// The opening of a text with no match to mark: its first `words` words,
/// with "…" if there were more.
fn opening(text: &str, words: usize) -> Vec<Piece> {
    let mut shown: Vec<&str> = text.split_whitespace().collect();
    let cut = shown.len() > words;
    shown.truncate(words);
    let mut text = shown.join(" ");
    if cut {
        text.push('…');
    }
    if text.is_empty() {
        return Vec::new();
    }
    vec![Piece { text, hit: false }]
}

/// Searches `scope` for `text`; `deck` limits cards to one active deck and
/// may only be given with [`Scope::Cards`]; `tag` (and any `#tag` typed in
/// `text`, see [`typed_tags`]) limits both to what has that tag.
///
/// Refuses a deck that doesn't exist (`NotFound`) or is archived
/// (`DeckArchived`). Text with no words and no tag finds nothing; a tag with
/// no words lists everything that has it, notes first (most recently edited
/// first), then cards by deck. A tag nothing has finds nothing. Writes
/// nothing.
pub async fn search_tagged(
    pool: &SqlitePool,
    text: &str,
    scope: Scope,
    deck: Option<i64>,
    tag: Option<&str>,
) -> Result<SearchResults, StudyError> {
    // Both reads see the same database, even if something is saved between.
    let mut tx = pool.begin().await?;

    if let Some(deck) = deck {
        let archived: Option<Option<String>> =
            sqlx::query_scalar("SELECT archived_at FROM decks WHERE id = ?1")
                .bind(deck)
                .fetch_optional(&mut *tx)
                .await?;
        match archived {
            None => return Err(StudyError::NotFound),
            Some(Some(_)) => return Err(StudyError::DeckArchived),
            Some(None) => {}
        }
    }

    let (text, wanted) = typed_tags(text, tag);
    let terms = terms(&text);
    if terms.is_empty() && wanted.is_empty() {
        return Ok(SearchResults {
            results: Vec::new(),
            more: false,
        });
    }
    let cards = scope != Scope::Notes;
    let notes = scope != Scope::Cards && deck.is_none();
    let (tagged_cards, tagged_notes) = if wanted.is_empty() {
        (None, None)
    } else {
        let (cards, notes) = tagged(&mut tx, &wanted).await?;
        (Some(cards), Some(notes))
    };

    if terms.is_empty() {
        let results = listed_by_tag(
            &mut tx,
            cards,
            notes,
            deck,
            tagged_cards.as_deref().unwrap_or("[]"),
            tagged_notes.as_deref().unwrap_or("[]"),
        )
        .await?;
        tx.commit().await?;
        return Ok(results);
    }
    let query = fts_query(&terms);

    // First the best matches that may be shown, without snippets: building a
    // snippet for every match of a short prefix would be wasted work.
    let ranked: Vec<(i64, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT s.rowid, c.deck_id, d.name
         FROM search_index s
         LEFT JOIN flashcards c ON s.rowid % 2 = 0 AND c.id = s.rowid / 2
         LEFT JOIN decks d ON d.id = c.deck_id
         LEFT JOIN notes n ON s.rowid % 2 = 1 AND n.id = s.rowid / 2
         WHERE search_index MATCH ?1
           AND ((?2 AND c.deleted_at IS NULL AND d.archived_at IS NULL
                 AND d.id IS NOT NULL AND (?3 IS NULL OR c.deck_id = ?3))
             OR (?4 AND n.id IS NOT NULL AND n.deleted_at IS NULL
                 AND (?7 IS NULL OR n.id IN (SELECT value FROM json_each(?7)))))
           AND (?6 IS NULL OR c.id IS NULL OR c.id IN (SELECT value FROM json_each(?6)))
         ORDER BY bm25(search_index, 2.0, 1.0), s.rowid
         LIMIT ?5",
    )
    .bind(&query)
    .bind(cards)
    .bind(deck)
    .bind(notes)
    .bind((RESULT_LIMIT + 1) as i64)
    .bind(tagged_cards.as_deref())
    .bind(tagged_notes.as_deref())
    .fetch_all(&mut *tx)
    .await?;

    let more = ranked.len() > RESULT_LIMIT;
    let ranked = &ranked[..ranked.len().min(RESULT_LIMIT)];
    if ranked.is_empty() {
        return Ok(SearchResults {
            results: Vec::new(),
            more,
        });
    }

    // Then the snippets of just those.
    let ids = serde_json::to_string(&ranked.iter().map(|row| row.0).collect::<Vec<_>>())
        .map_err(|err| StudyError::Internal(err.into()))?;
    let snippets: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT rowid,
                highlight(search_index, 0, ?3, ?4),
                snippet(search_index, 0, ?3, ?4, '…', 12),
                snippet(search_index, 1, ?3, ?4, '…', 24)
         FROM search_index
         WHERE search_index MATCH ?1
           AND rowid IN (SELECT value FROM json_each(?2))",
    )
    .bind(&query)
    .bind(&ids)
    .bind(MARK_START.to_string())
    .bind(MARK_END.to_string())
    .fetch_all(&mut *tx)
    .await?;
    let (card_tags, note_tags) = tags_of(&mut tx, ranked.iter().map(|row| row.0)).await?;
    tx.commit().await?;

    let results = ranked
        .iter()
        .filter_map(|(rowid, deck_id, deck_name)| {
            let (_, whole_head, head, body) = snippets.iter().find(|row| row.0 == *rowid)?;
            Some(if rowid % 2 == 0 {
                Hit::Card {
                    card_id: rowid / 2,
                    deck_id: (*deck_id)?,
                    deck_name: deck_name.clone()?,
                    front: pieces(head),
                    back: pieces(body),
                    tags: card_tags.get(&(rowid / 2)).cloned().unwrap_or_default(),
                }
            } else {
                // A title is at most 200 characters: shown whole.
                Hit::Note {
                    note_id: rowid / 2,
                    title: pieces(whole_head),
                    body: pieces(body),
                    tags: note_tags.get(&(rowid / 2)).cloned().unwrap_or_default(),
                }
            })
        })
        .collect();
    Ok(SearchResults { results, more })
}

/// The tags of the results, by search rowid (`2 * id` a card, `2 * id + 1` a
/// note): the cards' by card id, the notes' by note id.
async fn tags_of(
    conn: &mut SqliteConnection,
    rowids: impl Iterator<Item = i64>,
) -> Result<(HashMap<i64, Vec<String>>, HashMap<i64, Vec<String>>), sqlx::Error> {
    let (cards, notes): (Vec<i64>, Vec<i64>) = rowids.partition(|rowid| rowid % 2 == 0);
    let cards: Vec<i64> = cards.into_iter().map(|rowid| rowid / 2).collect();
    let notes: Vec<i64> = notes.into_iter().map(|rowid| rowid / 2).collect();
    Ok((
        tags::card_tags_of(conn, &cards).await?,
        tags::note_tags_of(conn, &notes).await?,
    ))
}

/// Everything shown that has the tags asked for, when no words were typed:
/// notes first, most recently edited first, then cards by deck name and in
/// the order they were added. Snippets are the opening of each text, nothing
/// marked.
async fn listed_by_tag(
    conn: &mut SqliteConnection,
    cards: bool,
    notes: bool,
    deck: Option<i64>,
    tagged_cards: &str,
    tagged_notes: &str,
) -> Result<SearchResults, sqlx::Error> {
    let limit = (RESULT_LIMIT + 1) as i64;
    let found_notes: Vec<(i64, String, String)> = if notes {
        sqlx::query_as(
            "SELECT id, title, body FROM notes
             WHERE deleted_at IS NULL AND id IN (SELECT value FROM json_each(?1))
             ORDER BY updated_at DESC, id DESC
             LIMIT ?2",
        )
        .bind(tagged_notes)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await?
    } else {
        Vec::new()
    };
    let found_cards: Vec<(i64, i64, String, String, String)> =
        if cards && (found_notes.len() as i64) < limit {
            sqlx::query_as(
                "SELECT c.id, d.id, d.name, c.front, c.back
                 FROM flashcards c JOIN decks d ON d.id = c.deck_id
                 WHERE c.deleted_at IS NULL AND d.archived_at IS NULL
                   AND (?2 IS NULL OR d.id = ?2)
                   AND c.id IN (SELECT value FROM json_each(?1))
                 ORDER BY d.name COLLATE NOCASE, d.id, c.id
                 LIMIT ?3",
            )
            .bind(tagged_cards)
            .bind(deck)
            .bind(limit - found_notes.len() as i64)
            .fetch_all(&mut *conn)
            .await?
        } else {
            Vec::new()
        };

    let more = found_notes.len() + found_cards.len() > RESULT_LIMIT;
    let rowids = found_notes
        .iter()
        .map(|note| note.0 * 2 + 1)
        .chain(found_cards.iter().map(|card| card.0 * 2));
    let (card_tags, note_tags) = tags_of(conn, rowids).await?;
    let results = found_notes
        .into_iter()
        .map(|(note_id, title, body)| Hit::Note {
            note_id,
            title: vec![Piece {
                text: title,
                hit: false,
            }],
            body: opening(&body, 24),
            tags: note_tags.get(&note_id).cloned().unwrap_or_default(),
        })
        .chain(
            found_cards
                .into_iter()
                .map(|(card_id, deck_id, deck_name, front, back)| Hit::Card {
                    card_id,
                    deck_id,
                    deck_name,
                    front: opening(&front, 12),
                    back: opening(&back, 24),
                    tags: card_tags.get(&card_id).cloned().unwrap_or_default(),
                }),
        )
        .take(RESULT_LIMIT)
        .collect();
    Ok(SearchResults { results, more })
}

/// Splits snippet text at its markers into plain and matched pieces.
fn pieces(marked: &str) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut text = String::new();
    let mut hit = false;
    let flush = |text: &mut String, hit: bool, out: &mut Vec<Piece>| {
        if !text.is_empty() {
            out.push(Piece {
                text: std::mem::take(text),
                hit,
            });
        }
    };
    for c in marked.chars() {
        match c {
            MARK_START | MARK_END => {
                flush(&mut text, hit, &mut out);
                hit = c == MARK_START;
            }
            _ => text.push(c),
        }
    }
    flush(&mut text, hit, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring;
    use crate::db::test_support::*;
    use crate::db::{migrate_and_seed, migrations_up_to};
    use crate::notes;

    const NOW: &str = "2026-10-19T12:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    async fn deck(pool: &SqlitePool, name: &str) -> i64 {
        authoring::create_deck(pool, name, None, at(NOW))
            .await
            .unwrap()
            .id
    }

    async fn card(pool: &SqlitePool, deck: i64, front: &str, back: &str) -> i64 {
        authoring::create_flashcard(pool, deck, front, back, at(NOW))
            .await
            .unwrap();
        sqlx::query_scalar("SELECT MAX(id) FROM flashcards")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn note(pool: &SqlitePool, title: &str, body: &str) -> i64 {
        notes::create_note(pool, title, body, at(NOW))
            .await
            .unwrap()
            .id
    }

    /// Each result as a short label: `card 3`, `note 2`.
    async fn found(pool: &SqlitePool, text: &str, scope: Scope, deck: Option<i64>) -> Vec<String> {
        search(pool, text, scope, deck)
            .await
            .unwrap()
            .results
            .iter()
            .map(|hit| match hit {
                Hit::Card { card_id, .. } => format!("card {card_id}"),
                Hit::Note { note_id, .. } => format!("note {note_id}"),
            })
            .collect()
    }

    fn joined(pieces: &[Piece]) -> String {
        pieces
            .iter()
            .map(|piece| {
                if piece.hit {
                    format!("[{}]", piece.text)
                } else {
                    piece.text.clone()
                }
            })
            .collect()
    }

    #[test]
    fn the_bundled_sqlite_has_fts5_with_the_unicode61_tokenizer() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            let enabled: i64 =
                sqlx::query_scalar("SELECT sqlite_compileoption_used('ENABLE_FTS5')")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(enabled, 1);
            sqlx::query(
                "CREATE VIRTUAL TABLE probe USING fts5(text, tokenize = 'unicode61 remove_diacritics 2')",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO probe (text) VALUES ('Été à Noël')")
                .execute(&pool)
                .await
                .unwrap();
            let rows: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM probe WHERE probe MATCH '\"ete\"* \"NOE\"*'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(rows, 1);
        });
    }

    #[test]
    fn words_are_split_like_the_index_and_quoted() {
        assert_eq!(terms("  Mito-chondria  ATP "), ["Mito", "chondria", "ATP"]);
        assert_eq!(terms("l'été"), ["l", "été"]);
        assert_eq!(
            terms("\"quoted\" OR NOT* a:b ^c (d) {e} +f"),
            ["quoted", "OR", "NOT", "a", "b", "c", "d", "e", "f"]
        );
        assert!(terms(" -- *** \"\" ").is_empty());
        assert_eq!(terms(&"word ".repeat(30)).len(), MAX_TERMS);
        assert_eq!(fts_query(&terms("cell ATP")), "\"cell\"* \"ATP\"*");
    }

    #[test]
    fn finds_cards_in_every_deck_and_notes_by_prefix_ignoring_case_and_accents() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let french = deck(&pool, "French").await;
            let cell = card(&pool, biology, "What is a cell?", "The basic unit of life.").await;
            let summer = card(&pool, french, "Summer", "L'été").await;
            let mitosis = note(&pool, "Mitosis", "A CELL divides into two.").await;

            assert_eq!(
                found(&pool, "CEL", Scope::All, None).await,
                [format!("card {cell}"), format!("note {mitosis}")]
            );
            assert_eq!(
                found(&pool, "ete", Scope::All, None).await,
                [format!("card {summer}")]
            );
            assert_eq!(
                found(&pool, "ÉTÉ", Scope::All, None).await,
                [format!("card {summer}")]
            );
            // Every word must match, each as a prefix, anywhere in the item.
            assert_eq!(
                found(&pool, "basic cel", Scope::All, None).await,
                [format!("card {cell}")]
            );
            assert!(found(&pool, "basic summer", Scope::All, None)
                .await
                .is_empty());
            // Only whole-word prefixes: "ell" isn't the start of a word.
            assert!(found(&pool, "ell", Scope::All, None).await.is_empty());
        });
    }

    #[test]
    fn query_syntax_typed_by_the_user_is_only_ever_text() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let cell = card(&pool, biology, "NEAR the cell", "OR AND NOT").await;
            for typed in [
                "\"",
                "\"cell",
                "cell\"",
                "*",
                "cell*",
                "-cell",
                "^cell",
                "cell:",
                "head:cell",
                "(cell",
                "cell)",
                "{head}: cell",
                "NEAR(cell)",
                "cell OR",
                "AND",
                "NOT cell",
                "+",
                "''",
                "\u{2}cell\u{3}",
                "",
            ] {
                let result = search(&pool, typed, Scope::All, None).await;
                assert!(result.is_ok(), "{typed:?}: {result:?}");
            }
            // Operators are ordinary words.
            assert_eq!(
                found(&pool, "or and not", Scope::All, None).await,
                [format!("card {cell}")]
            );
            assert_eq!(
                found(&pool, "near(cell)", Scope::All, None).await,
                [format!("card {cell}")]
            );
            assert!(found(&pool, " \"*\" ", Scope::All, None).await.is_empty());
        });
    }

    fn tag_list(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    async fn tag_card(pool: &SqlitePool, card: i64, list: &[&str]) {
        authoring::set_card_tags(pool, card, &tag_list(list), at(NOW))
            .await
            .unwrap();
    }

    async fn tag_note(pool: &SqlitePool, note: i64, list: &[&str]) {
        notes::set_note_tags(pool, note, &tag_list(list), at(NOW))
            .await
            .unwrap();
    }

    async fn found_tagged(
        pool: &SqlitePool,
        text: &str,
        scope: Scope,
        deck: Option<i64>,
        tag: Option<&str>,
    ) -> Vec<String> {
        let mut found: Vec<String> = search_tagged(pool, text, scope, deck, tag)
            .await
            .unwrap()
            .results
            .iter()
            .map(|hit| match hit {
                Hit::Card { card_id, .. } => format!("card {card_id}"),
                Hit::Note { note_id, .. } => format!("note {note_id}"),
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn hash_words_are_tags_and_the_rest_is_searched() {
        assert_eq!(
            typed_tags("mito #Bio cell #exam, #", Some("Chosen")),
            (
                "mito cell".to_string(),
                vec!["chosen".to_string(), "bio".to_string(), "exam".to_string()]
            )
        );
        assert_eq!(
            typed_tags("#bio #BIO", None),
            (String::new(), vec!["bio".to_string()])
        );
        assert!(tag_matches("Bio", "bio"));
        assert!(tag_matches("bio::Cells", "bio"));
        assert!(tag_matches("bio::cells::x", "bio::cells"));
        assert!(!tag_matches("biology", "bio"));
        assert!(!tag_matches("bio", "bio::cells"));
    }

    #[test]
    fn a_tag_narrows_cards_and_notes_and_finds_the_tags_below_it() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let cells = card(&pool, biology, "Mitosis phases", "Four").await;
            let genes = card(&pool, biology, "Mitosis genes", "Copied").await;
            let other = card(&pool, biology, "Mitosis plain", "None").await;
            let lecture = note(&pool, "Mitosis lecture", "Phases").await;
            let untagged = note(&pool, "Mitosis untagged", "Phases").await;
            tag_card(&pool, cells, &["bio::cells", "exam"]).await;
            tag_card(&pool, genes, &["Bio"]).await;
            tag_note(&pool, lecture, &["bio"]).await;

            // Chosen, or typed as #bio, any case; bio::cells is below bio.
            let expected = vec![
                format!("card {cells}"),
                format!("card {genes}"),
                format!("note {lecture}"),
            ];
            assert_eq!(
                found_tagged(&pool, "mitosis", Scope::All, None, Some("BIO")).await,
                expected
            );
            assert_eq!(
                found_tagged(&pool, "mitosis #bio", Scope::All, None, None).await,
                expected
            );
            // Two tags: both are needed.
            assert_eq!(
                found_tagged(&pool, "#exam mito", Scope::All, None, Some("bio")).await,
                vec![format!("card {cells}")]
            );
            // A tag, or the start of one, that nothing has finds nothing.
            assert!(found_tagged(&pool, "mitosis #bi", Scope::All, None, None)
                .await
                .is_empty());
            assert!(
                found_tagged(&pool, "mitosis", Scope::All, None, Some("nope"))
                    .await
                    .is_empty()
            );
            // Scope and deck still apply.
            assert_eq!(
                found_tagged(&pool, "mitosis", Scope::Notes, None, Some("bio")).await,
                vec![format!("note {lecture}")]
            );
            assert_eq!(
                found_tagged(&pool, "#bio", Scope::Cards, Some(biology), None).await,
                vec![format!("card {cells}"), format!("card {genes}")]
            );
            // Without a tag, nothing changes.
            assert_eq!(
                found(&pool, "mitosis", Scope::All, None).await.len(),
                5,
                "{other} {untagged}"
            );
        });
    }

    #[test]
    fn a_tag_alone_lists_what_has_it_with_its_tags() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let chemistry = deck(&pool, "Chemistry").await;
            let atom = card(&pool, chemistry, "Atom", "Smallest unit").await;
            let long = "word ".repeat(40);
            let cell = card(&pool, biology, "Cell", &long).await;
            let older = note(&pool, "Older", "Body").await;
            let newer = notes::create_note(&pool, "Newer", "Body", at("2026-10-19T13:00:00Z"))
                .await
                .unwrap()
                .id;
            let deleted = note(&pool, "Deleted", "Body").await;
            for id in [atom, cell] {
                tag_card(&pool, id, &["exam", "Z"]).await;
            }
            for id in [older, newer, deleted] {
                tag_note(&pool, id, &["exam"]).await;
            }
            notes::delete_note(&pool, deleted, at(NOW)).await.unwrap();

            let result = search_tagged(&pool, "", Scope::All, None, Some("exam"))
                .await
                .unwrap();
            assert!(!result.more);
            // Notes newest first, then cards by deck name.
            let order: Vec<String> = result
                .results
                .iter()
                .map(|hit| match hit {
                    Hit::Card { card_id, .. } => format!("card {card_id}"),
                    Hit::Note { note_id, .. } => format!("note {note_id}"),
                })
                .collect();
            assert_eq!(
                order,
                [
                    format!("note {newer}"),
                    format!("note {older}"),
                    format!("card {cell}"),
                    format!("card {atom}"),
                ]
            );
            match &result.results[2] {
                Hit::Card {
                    front, back, tags, ..
                } => {
                    assert_eq!(joined(front), "Cell");
                    assert_eq!(joined(back), format!("{}…", "word ".repeat(24).trim_end()));
                    assert_eq!(tags, &tag_list(&["exam", "Z"]));
                }
                other => panic!("expected a card, got {other:?}"),
            }
            match &result.results[0] {
                Hit::Note { tags, .. } => assert_eq!(tags, &tag_list(&["exam"])),
                other => panic!("expected a note, got {other:?}"),
            }

            // Results found by words carry their tags too.
            let result = search(&pool, "atom", Scope::All, None).await.unwrap();
            match &result.results[0] {
                Hit::Card { tags, .. } => assert_eq!(tags, &tag_list(&["exam", "Z"])),
                other => panic!("expected a card, got {other:?}"),
            }
            // Nothing typed and no tag still finds nothing.
            assert!(search(&pool, "  # ", Scope::All, None)
                .await
                .unwrap()
                .results
                .is_empty());
        });
    }

    #[test]
    fn a_tag_alone_stops_at_the_limit() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            for n in 0..=RESULT_LIMIT {
                let id = card(&pool, biology, &format!("Card {n}"), "back").await;
                tag_card(&pool, id, &["many"]).await;
            }
            let result = search_tagged(&pool, "", Scope::All, None, Some("many"))
                .await
                .unwrap();
            assert_eq!(result.results.len(), RESULT_LIMIT);
            assert!(result.more);
        });
    }

    #[test]
    fn deleted_cards_and_notes_and_archived_decks_are_left_out_until_they_return() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let kept = deck(&pool, "Kept").await;
            let deleted = card(&pool, biology, "Ribosome one", "Makes proteins").await;
            let archived = card(&pool, kept, "Ribosome two", "Also proteins").await;
            let gone = note(&pool, "Ribosome notes", "Proteins").await;

            authoring::delete_flashcard(&pool, deleted, at(NOW))
                .await
                .unwrap();
            authoring::archive_deck(&pool, kept, at(NOW)).await.unwrap();
            notes::delete_note(&pool, gone, at(NOW)).await.unwrap();
            assert!(found(&pool, "ribosome", Scope::All, None).await.is_empty());

            authoring::restore_flashcard(&pool, deleted, at(NOW))
                .await
                .unwrap();
            authoring::unarchive_deck(&pool, kept).await.unwrap();
            notes::restore_note(&pool, gone).await.unwrap();
            let mut all = found(&pool, "ribosome", Scope::All, None).await;
            all.sort();
            assert_eq!(
                all,
                [
                    format!("card {deleted}"),
                    format!("card {archived}"),
                    format!("note {gone}")
                ]
            );
        });
    }

    #[test]
    fn edits_and_moves_are_found_at_once() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let chemistry = deck(&pool, "Chemistry").await;
            let id = card(&pool, biology, "Old front", "Old back").await;
            let n = note(&pool, "Old title", "Old body").await;

            authoring::update_flashcard(&pool, id, "Osmosis", "Water moves", at(NOW))
                .await
                .unwrap();
            notes::update_note(&pool, n, "Diffusion", "Particles spread", at(NOW))
                .await
                .unwrap();
            assert!(found(&pool, "old", Scope::All, None).await.is_empty());
            assert_eq!(
                found(&pool, "osmo", Scope::All, None).await,
                [format!("card {id}")]
            );
            assert_eq!(
                found(&pool, "spread", Scope::All, None).await,
                [format!("note {n}")]
            );

            authoring::move_flashcards(&pool, biology, &[id], chemistry, at(NOW))
                .await
                .unwrap();
            let result = search(&pool, "osmosis", Scope::All, None).await.unwrap();
            let Hit::Card {
                deck_id, deck_name, ..
            } = &result.results[0]
            else {
                panic!("expected a card");
            };
            assert_eq!((*deck_id, deck_name.as_str()), (chemistry, "Chemistry"));
        });
    }

    #[test]
    fn scope_and_deck_filters() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let chemistry = deck(&pool, "Chemistry").await;
            let archived = deck(&pool, "Old").await;
            authoring::archive_deck(&pool, archived, at(NOW))
                .await
                .unwrap();
            let in_biology = card(&pool, biology, "Atom", "Smallest unit").await;
            let in_chemistry = card(&pool, chemistry, "Atom", "Has a nucleus").await;
            let n = note(&pool, "Atoms", "Protons and neutrons").await;

            let mut cards = found(&pool, "atom", Scope::Cards, None).await;
            cards.sort();
            assert_eq!(
                cards,
                [format!("card {in_biology}"), format!("card {in_chemistry}")]
            );
            assert_eq!(
                found(&pool, "atom", Scope::Notes, None).await,
                [format!("note {n}")]
            );
            assert_eq!(found(&pool, "atom", Scope::All, None).await.len(), 3);
            assert_eq!(
                found(&pool, "atom", Scope::Cards, Some(chemistry)).await,
                [format!("card {in_chemistry}")]
            );

            assert!(matches!(
                search(&pool, "atom", Scope::Cards, Some(9999)).await,
                Err(StudyError::NotFound)
            ));
            assert!(matches!(
                search(&pool, "atom", Scope::Cards, Some(archived)).await,
                Err(StudyError::DeckArchived)
            ));
        });
    }

    #[test]
    fn ranks_by_bm25_with_fronts_and_titles_weighing_more() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            let in_back = card(&pool, biology, "Which organelle?", "The nucleus holds DNA").await;
            let in_front = card(&pool, biology, "Nucleus", "Holds the DNA").await;
            let twice = note(&pool, "Nucleus", "The nucleus, nucleus and nucleolus.").await;
            assert_eq!(
                found(&pool, "nucleus", Scope::All, None).await,
                [
                    format!("note {twice}"),
                    format!("card {in_front}"),
                    format!("card {in_back}")
                ]
            );
        });
    }

    #[test]
    fn snippets_mark_the_matched_words_as_plain_text() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            card(
                &pool,
                biology,
                "What does <b>mitochondria</b> do?",
                "Makes ATP & heat",
            )
            .await;
            let long = format!(
                "{} Mitosis is here. {}",
                "Filler words. ".repeat(40),
                "More text. ".repeat(40)
            );
            note(&pool, "Cell <division>", &long).await;

            let result = search(&pool, "mito", Scope::All, None).await.unwrap();
            assert_eq!(result.results.len(), 2);
            for hit in &result.results {
                match hit {
                    Hit::Card {
                        front,
                        back,
                        deck_name,
                        ..
                    } => {
                        assert_eq!(deck_name, "Biology");
                        assert_eq!(joined(front), "What does <b>[mitochondria]</b> do?");
                        assert_eq!(joined(back), "Makes ATP & heat");
                    }
                    Hit::Note { title, body, .. } => {
                        assert_eq!(joined(title), "Cell <division>");
                        let body = joined(body);
                        assert!(body.contains("[Mitosis]"), "{body}");
                        assert!(body.starts_with('…') && body.ends_with('…'), "{body}");
                        assert!(body.chars().count() < 200, "{body}");
                    }
                }
            }
        });
    }

    #[test]
    fn at_most_the_limit_is_returned_and_more_says_so() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = deck(&pool, "Biology").await;
            for n in 0..RESULT_LIMIT {
                card(&pool, biology, &format!("Gene {n}"), "Back").await;
            }
            let result = search(&pool, "gene", Scope::All, None).await.unwrap();
            assert_eq!((result.results.len(), result.more), (RESULT_LIMIT, false));
            note(&pool, "Gene", "One more").await;
            let result = search(&pool, "gene", Scope::All, None).await.unwrap();
            assert_eq!((result.results.len(), result.more), (RESULT_LIMIT, true));
        });
    }

    #[test]
    fn the_migration_indexes_what_was_already_there() {
        tauri::async_runtime::block_on(async {
            let pool = memory_pool().await;
            // A v0.7.0 database: schema 11, with a card, a deleted note and
            // an archived deck's card.
            migrations_up_to(11).run(&pool).await.unwrap();
            for sql in [
                "INSERT INTO decks (name, created_at) VALUES ('Biology', ?1), ('Old', ?1)",
                "INSERT INTO flashcards (deck_id, front, back)
                 SELECT id, 'Enzyme', 'Speeds up reactions' FROM decks WHERE name = 'Biology'",
                "INSERT INTO flashcards (deck_id, front, back)
                 SELECT id, 'Enzyme old', 'Archived' FROM decks WHERE name = 'Old'",
                "UPDATE decks SET archived_at = ?1 WHERE name = 'Old'",
                "INSERT INTO notes (title, body, created_at, updated_at)
                 VALUES ('Enzymes', 'Catalysts', ?1, ?1), ('Enzyme draft', 'Gone', ?1, ?1)",
                "UPDATE notes SET deleted_at = ?1 WHERE title = 'Enzyme draft'",
            ] {
                sqlx::query(sql).bind(NOW).execute(&pool).await.unwrap();
            }

            migrate_and_seed(&pool).await.unwrap();
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM search_index").await,
                count(
                    &pool,
                    "SELECT (SELECT COUNT(*) FROM flashcards) + (SELECT COUNT(*) FROM notes)"
                )
                .await
            );
            let mut all = found(&pool, "enzyme", Scope::All, None).await;
            all.sort();
            assert_eq!(all, ["card 1", "note 1"]);
            // The index agrees with FTS5's own check of its content.
            sqlx::query("INSERT INTO search_index (search_index) VALUES ('integrity-check')")
                .execute(&pool)
                .await
                .unwrap();
        });
    }
}
