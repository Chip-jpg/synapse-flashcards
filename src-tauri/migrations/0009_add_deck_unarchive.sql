-- V1.4: unarchiving a normal deck, so archiving is no longer a one-way door.
-- Forward-only. No column is added, no row changes, and every deck keeps its
-- id, name, description, cards, scheduling, review logs, and sessions.
--
-- Migration 0006 made an archived deck final by refusing every update to it,
-- which also refused the one update an unarchive needs: clearing
-- `archived_at`. This narrows that rule the way 0008 did for deleted notes —
-- freeze the row, but allow the one column whose whole purpose is to say
-- which state the row is in — and leaves every other archiving rule in place:
-- the sample deck is still never archived (so it can never be unarchived),
-- archived decks are still never removed, and nothing new still happens
-- inside a deck while it is archived.
--
-- Unarchiving restores a deck to exactly what it was before: an active normal
-- deck. It writes no time of its own, adds no "unarchived" history, and
-- touches nothing but `archived_at`, so cards, card states, review logs, and
-- sessions come back untouched. A card that was soft-deleted before the deck
-- was archived stays deleted (0005 is unchanged and has no restore), and an
-- active card becomes due again only according to the `due` date it already
-- had. Validation lives in Rust (`authoring.rs`); these rules repeat the most
-- important parts inside SQLite, as every migration here does.

-- Replaces 0006's `decks_archived_are_final`.
DROP TRIGGER decks_archived_are_final;

-- An archived deck is read-only: every column it has except `archived_at` is
-- frozen while it is archived, so unarchiving one gives back exactly what was
-- archived. Clearing `archived_at` (the unarchive) is the one allowed change,
-- and it is the only reason this lists the columns instead of refusing every
-- update the way 0006 did. A column added to `decks` later belongs in this
-- list too. `IS NOT` compares like `<>` but is also correct for NULLs.
CREATE TRIGGER decks_archived_are_read_only
BEFORE UPDATE ON decks
WHEN OLD.archived_at IS NOT NULL
  AND (NEW.id IS NOT OLD.id
    OR NEW.name IS NOT OLD.name
    OR NEW.description IS NOT OLD.description
    OR NEW.is_sample IS NOT OLD.is_sample
    OR NEW.created_at IS NOT OLD.created_at)
BEGIN
    SELECT RAISE(ABORT, 'deck is archived');
END;

-- A deck is always created as an active one. Inserting a row that is already
-- archived would create a deck nobody ever made, appearing straight into the
-- archive history with a date it was never used on, so it is refused at the
-- door — as 0008 refuses a note that is born deleted.
CREATE TRIGGER decks_are_created_active
BEFORE INSERT ON decks
WHEN NEW.archived_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'a new deck cannot already be archived');
END;

-- While a deck is archived its archive time is fixed: archiving again can
-- never move the date the deck has been showing. (Unarchiving clears it, so a
-- deck archived, brought back, and archived again is archived anew and records
-- that later time — it is a fresh archiving, not a moved one.)
CREATE TRIGGER decks_archived_at_written_once
BEFORE UPDATE OF archived_at ON decks
WHEN OLD.archived_at IS NOT NULL AND NEW.archived_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'deck is already archived');
END;

-- Unarchiving never brings an abandoned review session back to life. A deck
-- can only be archived once its unfinished session has ended (0006's
-- `decks_archive_needs_no_open_session`), so an archived deck never has one
-- and this can only fire if that invariant was broken from outside Synapse.
-- It mirrors the archiving side, so a session is never left open or reopened
-- by a deck changing state in either direction.
CREATE TRIGGER decks_unarchive_needs_no_open_session
BEFORE UPDATE OF archived_at ON decks
WHEN NEW.archived_at IS NULL
  AND OLD.archived_at IS NOT NULL
  AND EXISTS (SELECT 1 FROM sessions WHERE deck_id = NEW.id AND ended_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'deck has an unfinished session');
END;
