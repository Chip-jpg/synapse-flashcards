-- Sources and pictures, phase 2: pictures in notes and on cards.
-- Forward-only. Adds two tables; no existing table, column or row changes.
--
-- A picture is a kept file (0016's `assets`, an image type). These tables say
-- which pictures a note, or a side of a card, shows and in what order. They
-- refer to the file by `asset_id`, never by path or hash text, and a note's or
-- card's text never names a picture: what is shown comes only from these rows.
--
-- The pictures are part of what the user wrote, like the text beside them, so
-- they follow the row that owns them:
-- - Saving a note or card replaces its list (the old link rows go, the new
--   ones come in, in one transaction). The files themselves stay: `assets`
--   rows are never removed (0016).
-- - While a note or card is deleted its pictures are frozen, as its text is
--   (0008, 0010), so restoring it gives them back exactly as they were.
-- - Nothing in an archived deck changes (0006), and that includes its cards'
--   pictures. The Sample deck is protected in Rust, as 0010 explains.
-- - Only front/back cards hold pictures. A cloze card's sides are written from
--   its text (0012); pictures for those would belong to the text, which is
--   not done here.
--
-- Backups need no change: every `assets` row travels with a format 2 backup,
-- so these rows always point at a file the backup carries.
--
-- Validation happens in Rust (`pictures.rs`, with `notes.rs` and
-- `authoring.rs`); the rules below repeat the most important parts inside
-- SQLite, as every migration does.

-- The pictures of a note, shown under its text, numbered from 1.
-- At most 20 (`pictures::NOTE_PICTURES_MAX`), each picture once.
CREATE TABLE note_pictures (
    note_id  INTEGER NOT NULL REFERENCES notes (id),
    position INTEGER NOT NULL
             CHECK (typeof(position) = 'integer' AND position BETWEEN 1 AND 20),
    asset_id INTEGER NOT NULL REFERENCES assets (id),
    PRIMARY KEY (note_id, position),
    UNIQUE (note_id, asset_id)
) WITHOUT ROWID;

CREATE INDEX note_pictures_by_asset ON note_pictures (asset_id);

CREATE TRIGGER note_pictures_are_pictures
BEFORE INSERT ON note_pictures
WHEN (SELECT media_type FROM assets WHERE id = NEW.asset_id)
     NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')
  OR NOT EXISTS (SELECT 1 FROM assets WHERE id = NEW.asset_id)
BEGIN
    SELECT RAISE(ABORT, 'a note shows only kept pictures');
END;

CREATE TRIGGER note_pictures_insert_active_note
BEFORE INSERT ON note_pictures
WHEN NOT EXISTS (SELECT 1 FROM notes WHERE id = NEW.note_id AND deleted_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'note is deleted');
END;

-- A list is replaced, never edited in place, so a row never changes.
CREATE TRIGGER note_pictures_never_change
BEFORE UPDATE ON note_pictures
BEGIN
    SELECT RAISE(ABORT, 'a note''s picture is replaced, never changed');
END;

CREATE TRIGGER note_pictures_delete_active_note
BEFORE DELETE ON note_pictures
WHEN NOT EXISTS (SELECT 1 FROM notes WHERE id = OLD.note_id AND deleted_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'note is deleted');
END;

-- The pictures on one side of a front/back card, shown under that side's
-- text, numbered from 1. At most 4 a side (`pictures::CARD_SIDE_PICTURES_MAX`),
-- each picture once a side (the same picture may be on both sides).
CREATE TABLE card_pictures (
    card_id  INTEGER NOT NULL REFERENCES flashcards (id),
    side     TEXT NOT NULL CHECK (side IN ('front', 'back')),
    position INTEGER NOT NULL
             CHECK (typeof(position) = 'integer' AND position BETWEEN 1 AND 4),
    asset_id INTEGER NOT NULL REFERENCES assets (id),
    PRIMARY KEY (card_id, side, position),
    UNIQUE (card_id, side, asset_id)
) WITHOUT ROWID;

CREATE INDEX card_pictures_by_asset ON card_pictures (asset_id);

CREATE TRIGGER card_pictures_are_pictures
BEFORE INSERT ON card_pictures
WHEN (SELECT media_type FROM assets WHERE id = NEW.asset_id)
     NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')
  OR NOT EXISTS (SELECT 1 FROM assets WHERE id = NEW.asset_id)
BEGIN
    SELECT RAISE(ABORT, 'a card shows only kept pictures');
END;

-- Only an active front/back card in an active deck takes a picture.
CREATE TRIGGER card_pictures_insert_changeable_card
BEFORE INSERT ON card_pictures
WHEN NOT EXISTS (SELECT 1 FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 WHERE f.id = NEW.card_id AND f.deleted_at IS NULL
                   AND f.kind = 'basic' AND d.archived_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'only an active front/back card in an active deck takes pictures');
END;

CREATE TRIGGER card_pictures_never_change
BEFORE UPDATE ON card_pictures
BEGIN
    SELECT RAISE(ABORT, 'a card''s picture is replaced, never changed');
END;

CREATE TRIGGER card_pictures_delete_changeable_card
BEFORE DELETE ON card_pictures
WHEN NOT EXISTS (SELECT 1 FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 WHERE f.id = OLD.card_id AND f.deleted_at IS NULL
                   AND d.archived_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'a deleted card, or one in an archived deck, keeps its pictures');
END;
