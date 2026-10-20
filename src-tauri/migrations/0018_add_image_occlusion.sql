-- Image occlusion: boxes drawn over a kept picture, where each box number is
-- its own card, the way each deletion number of a cloze text is (0012). The
-- card asks what is under its box(es); the answer uncovers them. Forward-only:
-- adds a table and two columns; no existing row changes.
--
-- The picture is a kept file (`assets`, 0016) and the boxes are stored in
-- `occlusions`, once; each card is an ordinary `flashcards` row that points
-- at it with its number. Rust (`occlusion.rs`) writes the card's `front` (the
-- header and which box) and `back` (the box's label, or which box) whenever
-- the boxes are saved, so every path that reads a card's front and back
-- (lists, search, counts, statistics, exports) keeps working. Reviews draw
-- the card from the picture and its boxes.
--
-- Editing keeps the schedule of every number still drawn, deletes (softly)
-- the card of a number whose boxes were all removed, and adds a card for a
-- new number, or brings back the deleted card that number had: the same
-- rules as editing a cloze text, which are Rust's. What SQLite holds fixed
-- is below.

-- One picture with its boxes. Its cards are the `flashcards` rows with its
-- id. It is never removed (its cards are only ever soft-deleted), and its
-- picture never changes: boxes are about one picture.
CREATE TABLE occlusions (
    id         INTEGER PRIMARY KEY,
    asset_id   INTEGER NOT NULL REFERENCES assets (id),
    -- Shown above the picture on every card; may be empty. Up to 500
    -- characters (`occlusion::HEADER_MAX_CHARS`), trimmed.
    header     TEXT NOT NULL
               CHECK (length(header) <= 500
                      AND trim(header, ' ' || char(9, 10, 13)) = header),
    -- 'hideAll': every box is covered on the question, the asked one marked.
    -- 'hideOne': only the asked box is covered.
    mode       TEXT NOT NULL CHECK (mode IN ('hideAll', 'hideOne')),
    -- A JSON array of boxes, `{ number, left, top, width, height, label }`,
    -- with coordinates from 0 to 1 of the picture's width and height, so they
    -- don't depend on its pixel size. Rust checks each box.
    boxes      TEXT NOT NULL
               CHECK (json_valid(boxes) AND json_type(boxes) = 'array'
                      AND json_array_length(boxes) BETWEEN 1 AND 200),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX occlusions_by_asset ON occlusions (asset_id);

CREATE TRIGGER occlusions_no_hard_delete
BEFORE DELETE ON occlusions
BEGIN
    SELECT RAISE(ABORT, 'image occlusions are kept, never removed');
END;

CREATE TRIGGER occlusions_picture_only
BEFORE INSERT ON occlusions
WHEN NOT EXISTS (SELECT 1 FROM assets
                 WHERE id = NEW.asset_id
                   AND media_type IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp'))
BEGIN
    SELECT RAISE(ABORT, 'an image occlusion needs a kept picture');
END;

CREATE TRIGGER occlusions_fixed
BEFORE UPDATE OF id, asset_id, created_at ON occlusions
WHEN NEW.id IS NOT OLD.id
  OR NEW.asset_id IS NOT OLD.asset_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'an image occlusion''s picture never changes');
END;

-- Which occlusion a card was made from, and its box number. 0012's `kind`
-- column only allows 'basic' and 'cloze', and SQLite can't widen a CHECK
-- without rebuilding `flashcards` (with its triggers, index and search
-- triggers), so an occlusion card is stored as kind 'basic' with these two
-- columns set; Rust tells it apart by `occlusion_id`, never by `kind`.
ALTER TABLE flashcards ADD COLUMN occlusion_id INTEGER REFERENCES occlusions (id);
ALTER TABLE flashcards ADD COLUMN occlusion_number INTEGER;

CREATE TRIGGER flashcards_occlusion_is_complete
BEFORE INSERT ON flashcards
WHEN NOT ((NEW.occlusion_id IS NULL AND NEW.occlusion_number IS NULL)
       OR (NEW.occlusion_id IS NOT NULL AND NEW.occlusion_number IS NOT NULL
           AND NEW.occlusion_number BETWEEN 1 AND 999 AND NEW.kind = 'basic'))
BEGIN
    SELECT RAISE(ABORT, 'an occlusion card needs its occlusion and number, and other cards neither');
END;

-- A card's occlusion and number are fixed when it is written.
CREATE TRIGGER flashcards_occlusion_is_fixed
BEFORE UPDATE OF occlusion_id, occlusion_number ON flashcards
WHEN NEW.occlusion_id IS NOT OLD.occlusion_id
  OR NEW.occlusion_number IS NOT OLD.occlusion_number
BEGIN
    SELECT RAISE(ABORT, 'a card''s occlusion and number never change');
END;

-- At most one active card per box number of an occlusion, as 0012 does for a
-- cloze text's numbers.
CREATE UNIQUE INDEX flashcards_one_active_card_per_occlusion
ON flashcards (occlusion_id, occlusion_number)
WHERE occlusion_id IS NOT NULL AND deleted_at IS NULL;

-- Finds an occlusion's cards, deleted ones included.
CREATE INDEX flashcards_occlusion ON flashcards (occlusion_id)
WHERE occlusion_id IS NOT NULL;

-- Replaces 0012's `flashcards_deleted_are_read_only`, with the two columns
-- above added to its list, as that trigger asks of every later column.
DROP TRIGGER flashcards_deleted_are_read_only;

CREATE TRIGGER flashcards_deleted_are_read_only
BEFORE UPDATE ON flashcards
WHEN OLD.deleted_at IS NOT NULL
  AND (NEW.id IS NOT OLD.id
    OR NEW.deck_id IS NOT OLD.deck_id
    OR NEW.front IS NOT OLD.front
    OR NEW.back IS NOT OLD.back
    OR NEW.fsrs_state IS NOT OLD.fsrs_state
    OR NEW.fsrs_stability IS NOT OLD.fsrs_stability
    OR NEW.fsrs_difficulty IS NOT OLD.fsrs_difficulty
    OR NEW.due IS NOT OLD.due
    OR NEW.last_review IS NOT OLD.last_review
    OR NEW.reps IS NOT OLD.reps
    OR NEW.lapses IS NOT OLD.lapses
    OR NEW.created_at IS NOT OLD.created_at
    OR NEW.updated_at IS NOT OLD.updated_at
    OR NEW.kind IS NOT OLD.kind
    OR NEW.cloze_text_id IS NOT OLD.cloze_text_id
    OR NEW.cloze_number IS NOT OLD.cloze_number
    OR NEW.occlusion_id IS NOT OLD.occlusion_id
    OR NEW.occlusion_number IS NOT OLD.occlusion_number)
BEGIN
    SELECT RAISE(ABORT, 'flashcard is deleted');
END;
