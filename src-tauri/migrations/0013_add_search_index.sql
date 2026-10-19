-- V1.8: one search across every card and every note. Forward-only. No
-- existing table, trigger or row changes; this adds a full-text index of the
-- text Synapse already stores, and the triggers that keep it in step.
--
-- `search_index` is an SQLite FTS5 table holding a copy of each card's front
-- and back and each note's title and body. Its rowid says which item a row is
-- for, so cards and notes share one index and one ranking without a lookup
-- column FTS5 couldn't index:
--
--   card  `id`  ->  rowid 2 * id        (even)
--   note  `id`  ->  rowid 2 * id + 1    (odd)
--
-- Cards: `head` is the front, `body` the back. Notes: `head` is the title,
-- `body` the body.
--
-- Every card and note is indexed, deleted or not, in any deck. Whether an item
-- can be shown (a card not deleted, in a deck not archived; a note not
-- deleted) is decided when searching (`search.rs`), from the tables
-- themselves, so deleting, restoring, archiving or unarchiving never has to
-- touch the index and can't leave it out of step.
--
-- `unicode61 remove_diacritics 2` splits words on Unicode letters and digits,
-- folds case, and drops accents, so "ete" finds "Été". The index is derived
-- data: rebuilding it from the tables (as the backfill below does) gives the
-- same rows, and it travels in backups like every other table.

CREATE VIRTUAL TABLE search_index USING fts5(
    head,
    body,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- Backfill: every card and note already stored.
INSERT INTO search_index (rowid, head, body)
SELECT 2 * id, front, back FROM flashcards;

INSERT INTO search_index (rowid, head, body)
SELECT 2 * id + 1, title, body FROM notes;

-- Cards. Only the text is indexed, so only a change to the text is copied;
-- moving, scheduling, deleting or restoring a card leaves its row alone.
CREATE TRIGGER search_index_card_insert
AFTER INSERT ON flashcards
BEGIN
    INSERT INTO search_index (rowid, head, body) VALUES (2 * NEW.id, NEW.front, NEW.back);
END;

CREATE TRIGGER search_index_card_update
AFTER UPDATE OF front, back ON flashcards
BEGIN
    UPDATE search_index SET head = NEW.front, body = NEW.back WHERE rowid = 2 * NEW.id;
END;

-- Cards are never removed (0005's `flashcards_no_hard_delete`), so this never
-- runs today. It keeps the index right if that ever changes.
CREATE TRIGGER search_index_card_delete
AFTER DELETE ON flashcards
BEGIN
    DELETE FROM search_index WHERE rowid = 2 * OLD.id;
END;

-- Notes, the same way.
CREATE TRIGGER search_index_note_insert
AFTER INSERT ON notes
BEGIN
    INSERT INTO search_index (rowid, head, body) VALUES (2 * NEW.id + 1, NEW.title, NEW.body);
END;

CREATE TRIGGER search_index_note_update
AFTER UPDATE OF title, body ON notes
BEGIN
    UPDATE search_index SET head = NEW.title, body = NEW.body WHERE rowid = 2 * NEW.id + 1;
END;

-- Notes are never removed either (0008's `notes_no_hard_delete`).
CREATE TRIGGER search_index_note_delete
AFTER DELETE ON notes
BEGIN
    DELETE FROM search_index WHERE rowid = 2 * OLD.id + 1;
END;
