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

use sqlx::SqlitePool;

use crate::study::StudyError;

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

/// One match. A card: `{ kind: "card", cardId, deckId, deckName, front, back }`.
/// A note: `{ kind: "note", noteId, title, body }`. The text fields are
/// snippets as [`Piece`]s; a long side or body is cut around its matches,
/// with "…" where text was left out.
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
    },
    #[serde(rename_all = "camelCase")]
    Note {
        note_id: i64,
        title: Vec<Piece>,
        body: Vec<Piece>,
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

/// Searches `scope` for `text`; `deck` limits cards to one active deck and
/// may only be given with [`Scope::Cards`].
///
/// Refuses a deck that doesn't exist (`NotFound`) or is archived
/// (`DeckArchived`). Text with no words finds nothing. Writes nothing.
pub async fn search(
    pool: &SqlitePool,
    text: &str,
    scope: Scope,
    deck: Option<i64>,
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

    let terms = terms(text);
    if terms.is_empty() {
        return Ok(SearchResults {
            results: Vec::new(),
            more: false,
        });
    }
    let query = fts_query(&terms);
    let cards = scope != Scope::Notes;
    let notes = scope != Scope::Cards && deck.is_none();

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
             OR (?4 AND n.id IS NOT NULL AND n.deleted_at IS NULL))
         ORDER BY bm25(search_index, 2.0, 1.0), s.rowid
         LIMIT ?5",
    )
    .bind(&query)
    .bind(cards)
    .bind(deck)
    .bind(notes)
    .bind((RESULT_LIMIT + 1) as i64)
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
                }
            } else {
                // A title is at most 200 characters: shown whole.
                Hit::Note {
                    note_id: rowid / 2,
                    title: pieces(whole_head),
                    body: pieces(body),
                }
            })
        })
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
