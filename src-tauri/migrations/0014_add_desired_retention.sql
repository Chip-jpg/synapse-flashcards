-- V1.7: the desired retention FSRS schedules with, chosen by the user.
-- Forward-only. Adds one table; no existing table or row changes.
--
-- Desired retention is the chance of remembering a card that FSRS aims for
-- when the card is due. Before this migration it was always 90%.
--
-- Every change is a new row, as with `fsrs_parameters` (0011):
-- - `deck_id` NULL: the global value, which every deck follows unless it has
--   its own. The newest such row is in use; with none, 90%.
-- - `deck_id` set: that deck's own value, or, with `percent` NULL, back to
--   following the global value. The newest row for the deck is in use.
-- Only active, ordinary decks get rows of their own. The sample deck and
-- archived decks follow the global value (an archived deck's earlier row is
-- kept, and applies again if it's unarchived).
--
-- Rows are never changed or removed. A new value only changes how future
-- ratings are scheduled; nothing here (or in Rust) rewrites a stored schedule
-- or any `review_logs` row. Validation happens in Rust (`retention.rs`); the
-- checks below repeat the most important rules inside SQLite.

CREATE TABLE desired_retention (
    id      INTEGER PRIMARY KEY,
    deck_id INTEGER REFERENCES decks (id),
    -- A whole percentage from 70 to 97.
    percent INTEGER CHECK (percent IS NULL OR (typeof(percent) = 'integer'
                                               AND percent BETWEEN 70 AND 97)),
    set_at  TEXT NOT NULL,
    -- Only a deck can go back to following the global value.
    CHECK (deck_id IS NOT NULL OR percent IS NOT NULL)
);

CREATE INDEX desired_retention_by_deck ON desired_retention (deck_id, id);

CREATE TRIGGER desired_retention_active_decks_only
BEFORE INSERT ON desired_retention
WHEN NEW.deck_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM decks
                 WHERE id = NEW.deck_id AND is_sample = 0 AND archived_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'only an active, ordinary deck has its own desired retention');
END;

CREATE TRIGGER desired_retention_no_update
BEFORE UPDATE ON desired_retention
BEGIN
    SELECT RAISE(ABORT, 'desired retention settings are never changed');
END;

CREATE TRIGGER desired_retention_no_delete
BEFORE DELETE ON desired_retention
BEGIN
    SELECT RAISE(ABORT, 'desired retention settings are never removed');
END;
