-- Tags on cards and notes. Forward-only: adds three tables; no existing
-- table, column or row changes.
--
-- A tag is a short label the user gives a card or a note, as Anki's tags
-- are: one word (no spaces or commas), `::` allowed for a hierarchy such as
-- `bio::cells`. Names are unique ignoring letter case, so `Bio` and `bio`
-- are one tag; the case first written is the one shown.
--
-- Tags are part of what the user wrote, like a card's text and pictures
-- (0017), so they follow the row that owns them:
-- - Saving a card's or note's tags replaces its list (the old link rows go,
--   the new ones come in, in one transaction).
-- - While a card or note is deleted its tags are frozen, as its text is
--   (0008, 0010), so restoring it gives them back exactly as they were.
-- - Nothing in an archived deck changes (0006), and that includes its
--   cards' tags. The Sample deck is protected in Rust, as 0010 explains.
-- - Every kind of card takes tags (front/back, cloze, picture cards).
--
-- A tag row is never removed: a tag no active card or note uses is simply
-- not listed, and a deleted card brought back still finds its tags. A tag
-- can be renamed, which renames it everywhere, deleted cards included.
--
-- Backups need no change: the tables travel in the database. An older
-- Synapse refuses a backup made after this migration as newer than it knows.
--
-- Validation happens in Rust (`tags.rs`); the rules below repeat the most
-- important parts inside SQLite, as every migration does.

-- One tag. Up to 60 characters (`tags::TAG_MAX_CHARS`), no whitespace or
-- commas, unique ignoring case.
CREATE TABLE tags (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE
               CHECK (length(name) BETWEEN 1 AND 60
                      AND instr(name, ' ') = 0
                      AND instr(name, ',') = 0
                      AND instr(name, char(9)) = 0
                      AND instr(name, char(10)) = 0
                      AND instr(name, char(13)) = 0),
    created_at TEXT NOT NULL
);

-- Only a tag's name changes (renaming); its id and creation time never do.
CREATE TRIGGER tags_only_name_changes
BEFORE UPDATE OF id, created_at ON tags
BEGIN
    SELECT RAISE(ABORT, 'only a tag''s name changes');
END;

CREATE TRIGGER tags_never_deleted
BEFORE DELETE ON tags
BEGIN
    SELECT RAISE(ABORT, 'a tag is never deleted');
END;

-- The tags of a card. At most 20 a card (`tags::TAGS_MAX`), checked in Rust.
CREATE TABLE card_tags (
    card_id INTEGER NOT NULL REFERENCES flashcards (id),
    tag_id  INTEGER NOT NULL REFERENCES tags (id),
    PRIMARY KEY (card_id, tag_id)
) WITHOUT ROWID;

CREATE INDEX card_tags_by_tag ON card_tags (tag_id);

-- Only an active card in an active deck changes its tags.
CREATE TRIGGER card_tags_insert_changeable_card
BEFORE INSERT ON card_tags
WHEN NOT EXISTS (SELECT 1 FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 WHERE f.id = NEW.card_id AND f.deleted_at IS NULL
                   AND d.archived_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'only an active card in an active deck takes tags');
END;

CREATE TRIGGER card_tags_never_change
BEFORE UPDATE ON card_tags
BEGIN
    SELECT RAISE(ABORT, 'a card''s tag is replaced, never changed');
END;

CREATE TRIGGER card_tags_delete_changeable_card
BEFORE DELETE ON card_tags
WHEN NOT EXISTS (SELECT 1 FROM flashcards f JOIN decks d ON d.id = f.deck_id
                 WHERE f.id = OLD.card_id AND f.deleted_at IS NULL
                   AND d.archived_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'a deleted card, or one in an archived deck, keeps its tags');
END;

-- The tags of a note. At most 20 a note (`tags::TAGS_MAX`), checked in Rust.
CREATE TABLE note_tags (
    note_id INTEGER NOT NULL REFERENCES notes (id),
    tag_id  INTEGER NOT NULL REFERENCES tags (id),
    PRIMARY KEY (note_id, tag_id)
) WITHOUT ROWID;

CREATE INDEX note_tags_by_tag ON note_tags (tag_id);

CREATE TRIGGER note_tags_insert_active_note
BEFORE INSERT ON note_tags
WHEN NOT EXISTS (SELECT 1 FROM notes WHERE id = NEW.note_id AND deleted_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'note is deleted');
END;

CREATE TRIGGER note_tags_never_change
BEFORE UPDATE ON note_tags
BEGIN
    SELECT RAISE(ABORT, 'a note''s tag is replaced, never changed');
END;

CREATE TRIGGER note_tags_delete_active_note
BEFORE DELETE ON note_tags
WHEN NOT EXISTS (SELECT 1 FROM notes WHERE id = OLD.note_id AND deleted_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'note is deleted');
END;
