-- Slice 4B: soft-deleting cards, and a one-time marker for the sample seed.
-- Forward-only. Existing cards stay active and keep every column unchanged.

-- NULL means the card is active. Deleting a card sets this, once, to the
-- deletion time (ISO-8601 UTC); the row itself, its FSRS state, and its
-- review logs are kept.
ALTER TABLE flashcards ADD COLUMN deleted_at TEXT;

-- Cards are only ever soft-deleted, so removing a row is refused outright.
CREATE TRIGGER flashcards_no_hard_delete
BEFORE DELETE ON flashcards
BEGIN
    SELECT RAISE(ABORT, 'flashcards are soft-deleted, never removed');
END;

-- A deleted card is final: its text, scheduling, and deletion time can't
-- change afterwards (there is no restore).
CREATE TRIGGER flashcards_deleted_are_final
BEFORE UPDATE ON flashcards
WHEN OLD.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'flashcard is deleted');
END;

-- One row per one-time seed that has run in this database. The sample card
-- is seeded only while its marker is missing, so sample data never comes
-- back after the user's cards are deleted.
CREATE TABLE seed_markers (
    name        TEXT PRIMARY KEY,
    recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Databases from before this migration were already seeded: the seed ran on
-- their first launch and cards couldn't be removed, so any card at all means
-- it has run. A brand-new database has no cards yet and is seeded in Rust.
INSERT INTO seed_markers (name)
SELECT 'sample_card'
WHERE EXISTS (SELECT 1 FROM flashcards);
