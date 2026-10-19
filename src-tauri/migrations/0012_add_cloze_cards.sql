-- Cloze cards: a second kind of card, written as one text with deletions like
-- `{{c1::mitochondria}}` or `{{c1::mitochondria::organelle}}` (a hint). Each
-- distinct number `cN` in the text is its own card, with its own FSRS
-- schedule and review logs, exactly like a front/back card. Forward-only:
-- every existing card becomes a 'basic' card and keeps every column and
-- every review log as it was.
--
-- The text is stored once, in `cloze_texts`, and each of its cards is an
-- ordinary `flashcards` row that points at it with its number. Rust
-- (`cloze.rs`) writes the card's `front` (the text with its own deletion
-- shown as `[...]`, or `[hint]`, and the others filled in) and `back` (its
-- answer) from the text, so every path that reads cards (lists, search,
-- counts, reviews, statistics, help with a card, exports) keeps working on
-- cloze cards unchanged. Validation lives in Rust; these rules repeat the
-- most important parts inside SQLite, as every migration here does.
--
-- Editing the text keeps the schedule of every number still in it, deletes
-- (softly, as any card is deleted) the card of a number that disappears,
-- and adds a card for a new number, or brings back the deleted card that
-- number had. Those rules are Rust's; what SQLite holds fixed is below.

-- One cloze text. Its cards are the `flashcards` rows with its id. A text is
-- never removed (its cards are only ever soft-deleted, so they always point
-- at it), and its own times say when it was written and last edited.
CREATE TABLE cloze_texts (
    id         INTEGER PRIMARY KEY,
    text       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TRIGGER cloze_texts_no_hard_delete
BEFORE DELETE ON cloze_texts
BEGIN
    SELECT RAISE(ABORT, 'cloze texts are kept, never removed');
END;

-- As 0004 refuses blank card text.
CREATE TRIGGER cloze_texts_not_blank_insert
BEFORE INSERT ON cloze_texts
WHEN trim(NEW.text, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'cloze text is blank');
END;

CREATE TRIGGER cloze_texts_not_blank_update
BEFORE UPDATE OF text ON cloze_texts
WHEN trim(NEW.text, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'cloze text is blank');
END;

-- Which kind a card is: 'basic' (front and back, as typed) or 'cloze'. A
-- cloze card names its text and its number, and a basic card names neither.
ALTER TABLE flashcards ADD COLUMN kind TEXT NOT NULL DEFAULT 'basic'
    CHECK (kind IN ('basic', 'cloze'));
ALTER TABLE flashcards ADD COLUMN cloze_text_id INTEGER REFERENCES cloze_texts (id);
ALTER TABLE flashcards ADD COLUMN cloze_number INTEGER;

CREATE TRIGGER flashcards_kind_is_complete
BEFORE INSERT ON flashcards
WHEN NOT ((NEW.kind = 'basic' AND NEW.cloze_text_id IS NULL AND NEW.cloze_number IS NULL)
       OR (NEW.kind = 'cloze' AND NEW.cloze_text_id IS NOT NULL
           AND NEW.cloze_number IS NOT NULL AND NEW.cloze_number >= 1))
BEGIN
    SELECT RAISE(ABORT, 'a cloze card needs its text and number, and a basic card neither');
END;

-- A card's kind, text, and number are fixed when it is written: a card never
-- turns into another kind or another deletion.
CREATE TRIGGER flashcards_kind_is_fixed
BEFORE UPDATE OF kind, cloze_text_id, cloze_number ON flashcards
WHEN NEW.kind IS NOT OLD.kind
  OR NEW.cloze_text_id IS NOT OLD.cloze_text_id
  OR NEW.cloze_number IS NOT OLD.cloze_number
BEGIN
    SELECT RAISE(ABORT, 'a card''s kind and cloze number never change');
END;

-- At most one active card per deletion number of a text. A deleted card
-- doesn't count, so a number can come back as a new card when its old one
-- can't be restored (it was left behind in another deck).
CREATE UNIQUE INDEX flashcards_one_active_card_per_cloze
ON flashcards (cloze_text_id, cloze_number)
WHERE cloze_text_id IS NOT NULL AND deleted_at IS NULL;

-- Finds a text's cards, deleted ones included.
CREATE INDEX flashcards_cloze_text ON flashcards (cloze_text_id)
WHERE cloze_text_id IS NOT NULL;

-- Replaces 0010's `flashcards_deleted_are_read_only`, which asks that a column
-- added to `flashcards` later join its list: the same rule, with the three
-- columns above. (The trigger above already fixes them for every card; this
-- keeps the list of what a deleted card freezes complete.)
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
    OR NEW.cloze_number IS NOT OLD.cloze_number)
BEGIN
    SELECT RAISE(ABORT, 'flashcard is deleted');
END;
