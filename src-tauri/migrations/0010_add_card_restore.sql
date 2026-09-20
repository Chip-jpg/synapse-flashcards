-- V1.5: restoring a soft-deleted card, so deleting one is no longer a
-- one-way door. Forward-only. No column is added, no row changes, and every
-- card keeps its id, deck, text, FSRS state, due date, reps, lapses,
-- last_review, created/updated times, and every one of its review logs.
--
-- Migration 0005 made a deleted card final by refusing every update to it,
-- which also refused the one update a restore needs: clearing `deleted_at`.
-- This narrows that rule exactly as 0008 did for deleted notes and 0009 for
-- archived decks — freeze the row, but allow the one column whose whole
-- purpose is to say which state the row is in. (0009's header says a deleted
-- card stays deleted because 0005 "has no restore"; that is what this
-- migration changes. Unarchiving a deck still restores no cards by itself:
-- the two are separate actions, and a card deleted before its deck was
-- archived comes back only when the user restores that card.)
--
-- Every other deletion rule stays in place: cards are still never removed
-- (0005's `flashcards_no_hard_delete`), a deleted card is still invisible to
-- lists, counts, sessions, and reviews while it is deleted, and nothing in an
-- archived deck may change at all (0006's `flashcards_archived_deck_update`
-- refuses every update to a card whose deck is archived, which is what stops
-- a card being restored into an archived deck).
--
-- Restoring writes nothing but `deleted_at`, so a restored card returns with
-- the exact schedule it had: same `due`, `fsrs_state`, stability, difficulty,
-- `reps`, `lapses`, and `last_review`, and the same `review_logs` rows. It is
-- not reset to a new card and does not become due because it was restored —
-- it is due again only if the `due` date it already had has passed. No review
-- session is started, resumed, or reopened by a restore. Validation lives in
-- Rust (`authoring.rs`); these rules repeat the most important parts inside
-- SQLite, as every migration here does.
--
-- The sample deck's cards are protected in Rust, not here, which is where
-- 0005 and 0006 left that rule too: no SQL trigger refuses edits to a sample
-- card, because nothing in Synapse ever writes one. A sample card can only
-- carry a `deleted_at` if it was set from outside Synapse, and `authoring.rs`
-- refuses to restore it by name.

-- Replaces 0005's `flashcards_deleted_are_final`.
DROP TRIGGER flashcards_deleted_are_final;

-- A deleted card is read-only: every column it has except `deleted_at` is
-- frozen while it is deleted, so restoring one gives back exactly what was
-- deleted. Clearing `deleted_at` (the restore) is the one allowed change, and
-- it is the only reason this lists the columns instead of refusing every
-- update the way 0005 did. A column added to `flashcards` later belongs in
-- this list too. `IS NOT` compares like `<>` but is also correct for NULLs.
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
    OR NEW.updated_at IS NOT OLD.updated_at)
BEGIN
    SELECT RAISE(ABORT, 'flashcard is deleted');
END;

-- A card is always written as an active one. Inserting a row that is already
-- deleted would create a card nobody ever wrote, appearing straight into a
-- deck's deleted history with a date it was never studied on, so it is
-- refused at the door — as 0008 refuses a note that is born deleted and 0009
-- a deck that is born archived.
CREATE TRIGGER flashcards_are_written_active
BEFORE INSERT ON flashcards
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'a new flashcard cannot already be deleted');
END;

-- While a card is deleted its deletion time is fixed: deleting again can
-- never move the date the card has been showing under *Deleted cards*.
-- (Restoring clears it, so a card deleted, brought back, and deleted again is
-- deleted anew and records that later time — it is a fresh deletion, not a
-- moved one.) Without this, a second delete would pass the trigger above,
-- which only freezes the columns that aren't `deleted_at`.
CREATE TRIGGER flashcards_deleted_at_written_once
BEFORE UPDATE OF deleted_at ON flashcards
WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'flashcard is already deleted');
END;
