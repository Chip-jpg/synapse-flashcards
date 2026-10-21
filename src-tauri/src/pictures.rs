//! Pictures in notes and on cards (migration 0017).
//!
//! A picture is a kept file (`assets.rs`) of an image type. A note shows a
//! list of them under its text, and each side of a front/back card shows a
//! list under that side's text. The lists live in `note_pictures` and
//! `card_pictures` and refer to files by asset id; a note's or card's text
//! never names a picture, so nothing typed can make Synapse load a file.
//!
//! The frontend adds a picture first (`assets::add_picture`, which returns its
//! `Asset`) and then saves the note or card with the ids of the pictures it
//! should show, in order. Saving replaces the whole list in the same
//! transaction as the text, so a note or card is always saved as one thing.
//! Every id is checked here ([`checked`]): it must be a kept picture, listed
//! once, and the list no longer than its limit.
//!
//! Removing a picture from a list removes only the link row; the file and its
//! `assets` row stay (0016 never removes one), so a deleted note or card, and
//! a restored backup, always find their pictures.

use std::collections::HashMap;

use sqlx::SqliteConnection;

use crate::assets::{Asset, MediaType};
use crate::db::DbError;

/// The most pictures one note shows (the migration repeats it).
pub const NOTE_PICTURES_MAX: usize = 20;
/// The most pictures one side of a card shows (the migration repeats it).
pub const CARD_SIDE_PICTURES_MAX: usize = 4;

/// What was wrong with a list of pictures. Nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureProblem {
    /// More pictures than the list may hold.
    TooMany,
    /// An id that isn't a kept picture (missing, or a document).
    NotAPicture,
    /// The same picture twice in one list.
    Repeated,
}

/// Why checking pictures didn't finish.
#[derive(Debug)]
pub enum PictureError {
    Invalid(PictureProblem),
    Internal(DbError),
}

impl From<sqlx::Error> for PictureError {
    fn from(err: sqlx::Error) -> Self {
        PictureError::Internal(err.into())
    }
}

/// The ids of the pictures on each side of a card, as sent:
/// `{ front: [id], back: [id] }`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct CardPictureIds {
    #[serde(default)]
    pub front: Vec<i64>,
    #[serde(default)]
    pub back: Vec<i64>,
}

/// The pictures on each side of a card: `{ front: [Asset], back: [Asset] }`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct CardPictures {
    pub front: Vec<Asset>,
    pub back: Vec<Asset>,
}

impl CardPictures {
    pub fn is_empty(&self) -> bool {
        self.front.is_empty() && self.back.is_empty()
    }
}

type AssetRow = (i64, String, String, i64, String);

fn asset(row: AssetRow) -> Result<Asset, DbError> {
    let (id, sha256, media_type, size, created_at) = row;
    let media_type = MediaType::parse(&media_type)
        .ok_or_else(|| format!("unknown media type {media_type:?}"))?;
    Ok(Asset {
        id,
        sha256,
        media_type,
        size: u64::try_from(size)?,
        created_at,
    })
}

/// Checks a list of picture ids and returns their assets, in the same order.
/// Each must be a kept picture, none repeated, at most `max` of them.
pub async fn checked(
    conn: &mut SqliteConnection,
    ids: &[i64],
    max: usize,
) -> Result<Vec<Asset>, PictureError> {
    if ids.len() > max {
        return Err(PictureError::Invalid(PictureProblem::TooMany));
    }
    let mut pictures = Vec::with_capacity(ids.len());
    for (index, &id) in ids.iter().enumerate() {
        if ids[..index].contains(&id) {
            return Err(PictureError::Invalid(PictureProblem::Repeated));
        }
        let row: Option<AssetRow> = sqlx::query_as(
            "SELECT id, sha256, media_type, size, created_at FROM assets WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        let picture = row
            .map(asset)
            .transpose()
            .map_err(PictureError::Internal)?
            .filter(|asset| asset.media_type.is_picture())
            .ok_or(PictureError::Invalid(PictureProblem::NotAPicture))?;
        pictures.push(picture);
    }
    Ok(pictures)
}

/// A note's pictures, in order.
pub async fn note_pictures(
    conn: &mut SqliteConnection,
    note_id: i64,
) -> Result<Vec<Asset>, DbError> {
    let rows: Vec<AssetRow> = sqlx::query_as(
        "SELECT a.id, a.sha256, a.media_type, a.size, a.created_at
         FROM note_pictures p JOIN assets a ON a.id = p.asset_id
         WHERE p.note_id = ?1
         ORDER BY p.position",
    )
    .bind(note_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter().map(asset).collect()
}

/// Replaces an active note's pictures with `pictures`, in order. Run in the
/// transaction that saves the note's text, after it has been checked.
pub async fn set_note_pictures(
    conn: &mut SqliteConnection,
    note_id: i64,
    pictures: &[Asset],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM note_pictures WHERE note_id = ?1")
        .bind(note_id)
        .execute(&mut *conn)
        .await?;
    for (index, picture) in pictures.iter().enumerate() {
        sqlx::query("INSERT INTO note_pictures (note_id, position, asset_id) VALUES (?1, ?2, ?3)")
            .bind(note_id)
            .bind(index as i64 + 1)
            .bind(picture.id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// One card's pictures, both sides in order.
pub async fn card_pictures(
    conn: &mut SqliteConnection,
    card_id: i64,
) -> Result<CardPictures, DbError> {
    let mut by_card = pictures_of_cards(
        conn,
        "SELECT p.card_id, p.side, a.id, a.sha256, a.media_type, a.size, a.created_at
         FROM card_pictures p JOIN assets a ON a.id = p.asset_id
         WHERE p.card_id = ?1
         ORDER BY p.side, p.position",
        card_id,
    )
    .await?;
    Ok(by_card.remove(&card_id).unwrap_or_default())
}

/// The pictures of every active card in a deck that has any, by card id.
pub async fn deck_card_pictures(
    conn: &mut SqliteConnection,
    deck_id: i64,
) -> Result<HashMap<i64, CardPictures>, DbError> {
    pictures_of_cards(
        conn,
        "SELECT p.card_id, p.side, a.id, a.sha256, a.media_type, a.size, a.created_at
         FROM card_pictures p
         JOIN flashcards f ON f.id = p.card_id
         JOIN assets a ON a.id = p.asset_id
         WHERE f.deck_id = ?1 AND f.deleted_at IS NULL
         ORDER BY p.card_id, p.side, p.position",
        deck_id,
    )
    .await
}

async fn pictures_of_cards(
    conn: &mut SqliteConnection,
    sql: &'static str,
    id: i64,
) -> Result<HashMap<i64, CardPictures>, DbError> {
    let rows: Vec<(i64, String, i64, String, String, i64, String)> =
        sqlx::query_as(sql).bind(id).fetch_all(&mut *conn).await?;
    let mut by_card: HashMap<i64, CardPictures> = HashMap::new();
    for (card_id, side, asset_id, sha256, media_type, size, created_at) in rows {
        let picture = asset((asset_id, sha256, media_type, size, created_at))?;
        let pictures = by_card.entry(card_id).or_default();
        match side.as_str() {
            "front" => pictures.front.push(picture),
            "back" => pictures.back.push(picture),
            other => return Err(format!("unknown picture side {other:?}").into()),
        }
    }
    Ok(by_card)
}

/// Replaces an active card's pictures. Run in the transaction that saves the
/// card's text, after the card and its deck have been checked.
pub async fn set_card_pictures(
    conn: &mut SqliteConnection,
    card_id: i64,
    pictures: &CardPictures,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM card_pictures WHERE card_id = ?1")
        .bind(card_id)
        .execute(&mut *conn)
        .await?;
    for (side, list) in [("front", &pictures.front), ("back", &pictures.back)] {
        for (index, picture) in list.iter().enumerate() {
            sqlx::query(
                "INSERT INTO card_pictures (card_id, side, position, asset_id)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(card_id)
            .bind(side)
            .bind(index as i64 + 1)
            .bind(picture.id)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::test_support::*;
    use crate::assets::{self, AssetStore};
    use crate::authoring::{self, AuthoringError, InvalidInput};
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::notes::{self, InvalidNote, NoteError};
    use crate::study::{self, SessionCard, StartedSession};
    use sqlx::SqlitePool;

    const NOW: &str = "2026-10-20T12:00:00Z";
    const LATER: &str = "2026-10-20T13:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    async fn pictures(pool: &SqlitePool, store: &AssetStore, count: u8) -> Vec<Asset> {
        let mut added = Vec::new();
        for variant in 0..count {
            added.push(
                assets::add_picture(pool, store, &png(variant), at(NOW))
                    .await
                    .unwrap(),
            );
        }
        added
    }

    async fn pdf(pool: &SqlitePool, store: &AssetStore) -> Asset {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let asset = assets::put(
            &mut tx,
            store,
            b"%PDF-1.7\n%%EOF\n",
            MediaType::Pdf,
            at(NOW),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        asset
    }

    fn ids(pictures: &[Asset]) -> Vec<i64> {
        pictures.iter().map(|picture| picture.id).collect()
    }

    async fn rows(pool: &SqlitePool, sql: &'static str) -> Vec<(i64, String, i64, i64)> {
        sqlx::query_as(sql).fetch_all(pool).await.unwrap()
    }

    async fn note_rows(pool: &SqlitePool) -> Vec<(i64, String, i64, i64)> {
        rows(
            pool,
            "SELECT note_id, '', position, asset_id FROM note_pictures ORDER BY note_id, position",
        )
        .await
    }

    async fn card_rows(pool: &SqlitePool) -> Vec<(i64, String, i64, i64)> {
        rows(
            pool,
            "SELECT card_id, side, position, asset_id FROM card_pictures
             ORDER BY card_id, side, position",
        )
        .await
    }

    fn note_problem<T: std::fmt::Debug>(result: Result<T, NoteError>) -> PictureProblem {
        match result {
            Err(NoteError::Invalid(InvalidNote::Pictures(problem))) => problem,
            other => panic!("expected a picture problem, got {other:?}"),
        }
    }

    fn card_problem<T: std::fmt::Debug>(result: Result<T, AuthoringError>) -> InvalidInput {
        match result {
            Err(AuthoringError::Invalid(problem)) => problem,
            other => panic!("expected invalid input, got {other:?}"),
        }
    }

    #[test]
    fn a_note_keeps_its_pictures_in_order_and_saving_replaces_them() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-note");
            let [a, b, c] = <[Asset; 3]>::try_from(pictures(&pool, &store, 3).await).unwrap();

            let note = notes::create_note_with_pictures(
                &pool,
                "Cells",
                "A diagram",
                &[b.id, a.id],
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(note.pictures, vec![b.clone(), a.clone()]);
            assert_eq!(notes::note(&pool, note.id).await.unwrap(), note);

            // Leaving the pictures out of an edit keeps them.
            let edited = notes::update_note(&pool, note.id, "Cells", "Edited", at(LATER))
                .await
                .unwrap();
            assert_eq!(edited.pictures, vec![b.clone(), a.clone()]);

            // A new list replaces the old one, order included.
            let edited = notes::update_note_with_pictures(
                &pool,
                note.id,
                "Cells",
                "Edited",
                Some(&[a.id, c.id, b.id]),
                at(LATER),
            )
            .await
            .unwrap();
            assert_eq!(edited.pictures, vec![a.clone(), c.clone(), b.clone()]);
            assert_eq!(
                note_rows(&pool).await,
                vec![
                    (note.id, String::new(), 1, a.id),
                    (note.id, String::new(), 2, c.id),
                    (note.id, String::new(), 3, b.id)
                ]
            );

            // An empty list removes them all; the files and their rows stay.
            let edited = notes::update_note_with_pictures(
                &pool,
                note.id,
                "Cells",
                "Edited",
                Some(&[]),
                at(LATER),
            )
            .await
            .unwrap();
            assert!(edited.pictures.is_empty());
            assert!(note_rows(&pool).await.is_empty());
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM assets").await, 3);
            for picture in [&a, &b, &c] {
                assert!(store.verify(&picture.sha256, picture.size));
            }

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_refused_list_of_pictures_saves_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-refused");
            let many = pictures(&pool, &store, 21).await;
            let document = pdf(&pool, &store).await;

            assert_eq!(
                note_problem(
                    notes::create_note_with_pictures(&pool, "T", "B", &ids(&many), at(NOW)).await
                ),
                PictureProblem::TooMany
            );
            assert_eq!(
                note_problem(
                    notes::create_note_with_pictures(
                        &pool,
                        "T",
                        "B",
                        &[many[0].id, many[0].id],
                        at(NOW)
                    )
                    .await
                ),
                PictureProblem::Repeated
            );
            for wrong in [document.id, 9_999] {
                assert_eq!(
                    note_problem(
                        notes::create_note_with_pictures(&pool, "T", "B", &[wrong], at(NOW)).await
                    ),
                    PictureProblem::NotAPicture
                );
            }
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM notes").await,
                0,
                "no note was saved"
            );

            // An edit with a refused list keeps the text and pictures it had.
            let note = notes::create_note_with_pictures(&pool, "T", "B", &[many[0].id], at(NOW))
                .await
                .unwrap();
            assert_eq!(
                note_problem(
                    notes::update_note_with_pictures(
                        &pool,
                        note.id,
                        "New",
                        "New",
                        Some(&ids(&many)),
                        at(LATER)
                    )
                    .await
                ),
                PictureProblem::TooMany
            );
            assert_eq!(notes::note(&pool, note.id).await.unwrap(), note);

            // Up to the limit is fine.
            let full = notes::create_note_with_pictures(
                &pool,
                "Full",
                "B",
                &ids(&many[..NOTE_PICTURES_MAX]),
                at(NOW),
            )
            .await
            .unwrap();
            assert_eq!(full.pictures.len(), NOTE_PICTURES_MAX);

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_deleted_notes_pictures_are_frozen_and_come_back_with_it() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-deleted-note");
            let added = pictures(&pool, &store, 2).await;
            let note = notes::create_note_with_pictures(&pool, "T", "B", &[added[0].id], at(NOW))
                .await
                .unwrap();
            notes::delete_note(&pool, note.id, at(LATER)).await.unwrap();
            let before = note_rows(&pool).await;

            // The migration refuses any change while the note is deleted.
            for sql in [
                "DELETE FROM note_pictures",
                "INSERT INTO note_pictures (note_id, position, asset_id) VALUES (1, 2, 2)",
                "UPDATE note_pictures SET asset_id = 2",
            ] {
                let err = sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(&pool)
                    .await
                    .unwrap_err();
                assert!(err.to_string().contains("note"), "{sql}: {err}");
            }
            assert_eq!(note_rows(&pool).await, before);
            // It can still be read from the history, pictures included.
            assert_eq!(
                notes::deleted_note(&pool, note.id).await.unwrap().pictures,
                vec![added[0].clone()]
            );

            notes::restore_note(&pool, note.id).await.unwrap();
            assert_eq!(
                notes::note(&pool, note.id).await.unwrap().pictures,
                vec![added[0].clone()]
            );

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn the_migration_takes_only_kept_pictures() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-migration");
            let document = pdf(&pool, &store).await;
            let note = notes::create_note(&pool, "T", "B", at(NOW)).await.unwrap();
            for asset_id in [document.id, 9_999] {
                let err = sqlx::query(
                    "INSERT INTO note_pictures (note_id, position, asset_id) VALUES (?1, 1, ?2)",
                )
                .bind(note.id)
                .bind(asset_id)
                .execute(&pool)
                .await
                .unwrap_err();
                assert!(err.to_string().contains("kept pictures"), "{err}");
            }
            let picture = pictures(&pool, &store, 1).await.remove(0);
            for position in [0, 21] {
                let result = sqlx::query(
                    "INSERT INTO note_pictures (note_id, position, asset_id) VALUES (?1, ?2, ?3)",
                )
                .bind(note.id)
                .bind(position)
                .bind(picture.id)
                .execute(&pool)
                .await;
                assert!(result.is_err(), "position {position}");
            }

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_card_shows_its_pictures_in_its_deck_and_in_review() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-card");
            let [a, b, c] = <[Asset; 3]>::try_from(pictures(&pool, &store, 3).await).unwrap();
            let deck = authoring::create_deck(&pool, "Anatomy", None, at(NOW))
                .await
                .unwrap()
                .id;

            let sent = CardPictureIds {
                front: vec![a.id],
                back: vec![b.id, a.id],
            };
            let detail = authoring::create_flashcard_with_pictures(
                &pool,
                deck,
                "Name the bone",
                "Femur",
                &sent,
                at(NOW),
            )
            .await
            .unwrap();
            let card = &detail.cards[0];
            assert_eq!(
                card.pictures,
                CardPictures {
                    front: vec![a.clone()],
                    back: vec![b.clone(), a.clone()],
                }
            );

            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck, at(NOW)).await.unwrap()
            else {
                panic!("a new card is due");
            };
            match study::session_card(&pool, session_id, at(NOW))
                .await
                .unwrap()
            {
                SessionCard::Due { pictures, .. } => assert_eq!(pictures, card.pictures),
                other => panic!("expected the card, got {other:?}"),
            }

            // Editing without pictures keeps them; with a list, replaces them.
            let detail = authoring::update_flashcard(&pool, card.id, "Name it", "Femur", at(LATER))
                .await
                .unwrap();
            assert_eq!(detail.cards[0].pictures, card.pictures);
            let detail = authoring::update_flashcard_with_pictures(
                &pool,
                card.id,
                "Name it",
                "Femur",
                Some(&CardPictureIds {
                    front: vec![],
                    back: vec![c.id],
                }),
                at(LATER),
            )
            .await
            .unwrap();
            assert_eq!(
                detail.cards[0].pictures,
                CardPictures {
                    front: vec![],
                    back: vec![c.clone()],
                }
            );

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_refused_cards_pictures_say_which_side_and_save_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-card-refused");
            let many = pictures(&pool, &store, 5).await;
            let deck = authoring::create_deck(&pool, "Anatomy", None, at(NOW))
                .await
                .unwrap()
                .id;
            let before = count(&pool, "SELECT COUNT(*) FROM flashcards").await;

            let too_many = CardPictureIds {
                front: ids(&many),
                back: vec![],
            };
            assert_eq!(
                card_problem(
                    authoring::create_flashcard_with_pictures(
                        &pool,
                        deck,
                        "F",
                        "B",
                        &too_many,
                        at(NOW)
                    )
                    .await
                ),
                InvalidInput::FrontPictures(PictureProblem::TooMany)
            );
            let repeated = CardPictureIds {
                front: vec![],
                back: vec![many[0].id, many[0].id],
            };
            assert_eq!(
                card_problem(
                    authoring::create_flashcard_with_pictures(
                        &pool,
                        deck,
                        "F",
                        "B",
                        &repeated,
                        at(NOW)
                    )
                    .await
                ),
                InvalidInput::BackPictures(PictureProblem::Repeated)
            );
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM flashcards").await,
                before
            );
            assert!(card_rows(&pool).await.is_empty());

            // The sample deck stays read-only.
            let sample = sample_deck_id(&pool).await;
            let one = CardPictureIds {
                front: vec![many[0].id],
                back: vec![],
            };
            assert!(matches!(
                authoring::create_flashcard_with_pictures(&pool, sample, "F", "B", &one, at(NOW))
                    .await,
                Err(AuthoringError::SampleDeck)
            ));

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_cards_pictures_follow_it_through_delete_restore_move_and_archive() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-card-lifecycle");
            let added = pictures(&pool, &store, 2).await;
            let deck = authoring::create_deck(&pool, "Anatomy", None, at(NOW))
                .await
                .unwrap()
                .id;
            let other = authoring::create_deck(&pool, "Bones", None, at(NOW))
                .await
                .unwrap()
                .id;
            let sent = CardPictureIds {
                front: vec![added[0].id],
                back: vec![added[1].id],
            };
            let card =
                authoring::create_flashcard_with_pictures(&pool, deck, "F", "B", &sent, at(NOW))
                    .await
                    .unwrap()
                    .cards[0]
                    .id;
            let before = card_rows(&pool).await;
            assert_eq!(before.len(), 2);

            // Deleted: frozen.
            authoring::delete_flashcard(&pool, card, at(LATER))
                .await
                .unwrap();
            for sql in [
                "DELETE FROM card_pictures",
                "INSERT INTO card_pictures (card_id, side, position, asset_id)
                 VALUES (2, 'front', 2, 2)",
            ] {
                assert!(
                    sqlx::query(sqlx::AssertSqlSafe(sql))
                        .execute(&pool)
                        .await
                        .is_err(),
                    "{sql}"
                );
            }
            assert_eq!(card_rows(&pool).await, before);

            // Restored: back with the card.
            let detail = authoring::restore_flashcard(&pool, card, at(LATER))
                .await
                .unwrap();
            assert_eq!(detail.cards[0].pictures.front, vec![added[0].clone()]);

            // Moved: they go with it.
            authoring::move_flashcards(&pool, deck, &[card], other, at(LATER))
                .await
                .unwrap();
            let moved = authoring::deck_detail(&pool, other, at(LATER))
                .await
                .unwrap();
            assert_eq!(moved.cards[0].pictures.back, vec![added[1].clone()]);
            assert_eq!(card_rows(&pool).await, before);

            // Archived: nothing changes until it's unarchived.
            authoring::archive_deck(&pool, other, at(LATER))
                .await
                .unwrap();
            assert!(sqlx::query("DELETE FROM card_pictures")
                .execute(&pool)
                .await
                .is_err());
            assert_eq!(card_rows(&pool).await, before);

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }

    #[test]
    fn a_cloze_card_takes_no_pictures() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let store = scratch_store("pictures-cloze");
            let picture = pictures(&pool, &store, 1).await.remove(0);
            let deck = authoring::create_deck(&pool, "Anatomy", None, at(NOW))
                .await
                .unwrap()
                .id;
            let card = crate::cloze::create_cloze(&pool, deck, "The {{c1::femur}}", at(NOW))
                .await
                .unwrap()
                .cards[0]
                .id;
            let err = sqlx::query(
                "INSERT INTO card_pictures (card_id, side, position, asset_id)
                 VALUES (?1, 'front', 1, ?2)",
            )
            .bind(card)
            .bind(picture.id)
            .execute(&pool)
            .await
            .unwrap_err();
            assert!(err.to_string().contains("front/back card"), "{err}");
            let ids = CardPictureIds {
                front: vec![picture.id],
                back: vec![],
            };
            assert!(matches!(
                authoring::update_flashcard_with_pictures(
                    &pool,
                    card,
                    "F",
                    "B",
                    Some(&ids),
                    at(LATER)
                )
                .await,
                Err(AuthoringError::WrongKind)
            ));

            pool.close().await;
            let _ = std::fs::remove_dir_all(store.dir());
        });
    }
}
