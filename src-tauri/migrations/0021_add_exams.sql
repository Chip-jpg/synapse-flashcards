-- Exams: a date the user gives a deck or a tag, so Synapse can plan daily
-- work and report readiness (`exams.rs`). Forward-only: adds one table; no
-- existing table, column or row changes.
--
-- - `exam_date` is a calendar date on the user's own clock (`YYYY-MM-DD`),
--   not an instant. A date in the past is allowed: the exam is then simply
--   finished.
-- - Exactly one of `deck_id` / `tag_id` says what the exam covers. The scope
--   never changes after creation; only the title and the date do.
-- - Deleting an exam is soft (`deleted_at`) and final: nothing changes in a
--   deleted exam, and rows are never removed.
-- - Nothing here touches cards, schedules or review logs. An archived deck
--   keeps its exam row; its plan just has no cards in scope.
--
-- Backups need no change: the table travels in the database. An older
-- Synapse refuses a backup made after this migration as newer than it knows.
--
-- Validation happens in Rust (`exams.rs`); the checks below repeat the most
-- important rules inside SQLite, as every migration does.

CREATE TABLE exams (
    id         INTEGER PRIMARY KEY,
    -- 1 to 100 characters (`exams::TITLE_MAX_CHARS`), not blank.
    title      TEXT NOT NULL
               CHECK (length(title) BETWEEN 1 AND 100 AND length(trim(title)) >= 1),
    -- `date()` is NULL for an impossible date, and a CHECK that is NULL
    -- passes, so compare with IS (NULL IS 'x' is false), not =.
    exam_date  TEXT NOT NULL
               CHECK (exam_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
                      AND date(exam_date) IS exam_date),
    deck_id    INTEGER REFERENCES decks (id),
    tag_id     INTEGER REFERENCES tags (id),
    created_at TEXT NOT NULL,
    deleted_at TEXT,
    CHECK ((deck_id IS NULL) <> (tag_id IS NULL))
);

CREATE INDEX exams_by_date ON exams (exam_date, id) WHERE deleted_at IS NULL;

-- A new exam is active, and a deck's exam covers an active deck.
CREATE TRIGGER exams_insert_active
BEFORE INSERT ON exams
WHEN NEW.deleted_at IS NOT NULL
  OR (NEW.deck_id IS NOT NULL
      AND NOT EXISTS (SELECT 1 FROM decks
                      WHERE id = NEW.deck_id AND archived_at IS NULL))
BEGIN
    SELECT RAISE(ABORT, 'a new exam is active and covers an active deck');
END;

-- Only the title, the date and the deletion time change.
CREATE TRIGGER exams_only_title_date_deleted_change
BEFORE UPDATE OF id, deck_id, tag_id, created_at ON exams
BEGIN
    SELECT RAISE(ABORT, 'only an exam''s title and date change');
END;

-- A deleted exam is final.
CREATE TRIGGER exams_deleted_are_read_only
BEFORE UPDATE ON exams
WHEN OLD.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'a deleted exam is not changed');
END;

CREATE TRIGGER exams_never_deleted
BEFORE DELETE ON exams
BEGIN
    SELECT RAISE(ABORT, 'an exam is never removed');
END;
