-- Phase 2: renaming and archiving normal decks.
-- Forward-only. Existing decks stay active and keep every column unchanged.
-- Renaming needs no schema change: migration 0004's rules (unique name
-- ignoring case, never blank) already apply to every update.

-- NULL means the deck is active. Archiving sets this, once, to the archive
-- time (ISO-8601 UTC). The deck row, its cards with their FSRS state and
-- review logs, and its sessions are all kept.
ALTER TABLE decks ADD COLUMN archived_at TEXT;

-- Decks are only ever archived, so removing a row is refused outright.
CREATE TRIGGER decks_no_hard_delete
BEFORE DELETE ON decks
BEGIN
    SELECT RAISE(ABORT, 'decks are archived, never removed');
END;

-- An archived deck is final: its name, description, and archive time can't
-- change afterwards (there is no restore).
CREATE TRIGGER decks_archived_are_final
BEFORE UPDATE ON decks
WHEN OLD.archived_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'deck is archived');
END;

-- The sample deck is never archived.
CREATE TRIGGER decks_sample_never_archived
BEFORE UPDATE ON decks
WHEN NEW.is_sample = 1 AND NEW.archived_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'the sample deck cannot be archived');
END;

-- A deck's unfinished review session must end before (in the same
-- transaction as) the deck is archived, so no session is left open.
CREATE TRIGGER decks_archive_needs_no_open_session
BEFORE UPDATE OF archived_at ON decks
WHEN NEW.archived_at IS NOT NULL
  AND EXISTS (SELECT 1 FROM sessions WHERE deck_id = NEW.id AND ended_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'deck has an unfinished session');
END;

-- Nothing new happens inside an archived deck: no cards are added to it, its
-- cards can't change (no edits, deletions, or reviews), and no review session
-- is opened or reopened in it.
CREATE TRIGGER flashcards_archived_deck_insert
BEFORE INSERT ON flashcards
WHEN EXISTS (SELECT 1 FROM decks WHERE id = NEW.deck_id AND archived_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'deck is archived');
END;

CREATE TRIGGER flashcards_archived_deck_update
BEFORE UPDATE ON flashcards
WHEN EXISTS (SELECT 1 FROM decks
             WHERE id IN (OLD.deck_id, NEW.deck_id) AND archived_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'deck is archived');
END;

CREATE TRIGGER sessions_archived_deck_insert
BEFORE INSERT ON sessions
WHEN NEW.ended_at IS NULL
  AND EXISTS (SELECT 1 FROM decks WHERE id = NEW.deck_id AND archived_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'deck is archived');
END;

CREATE TRIGGER sessions_archived_deck_update
BEFORE UPDATE ON sessions
WHEN NEW.ended_at IS NULL
  AND EXISTS (SELECT 1 FROM decks WHERE id = NEW.deck_id AND archived_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'deck is archived');
END;
