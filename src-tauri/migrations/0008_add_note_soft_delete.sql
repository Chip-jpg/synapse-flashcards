-- V1.3: soft-deleting notes, so a note can leave the library without its
-- text being lost. Forward-only. Existing notes stay active and keep every
-- column unchanged.
--
-- Unlike a deleted card (0005) or an archived deck (0006), a deleted note is
-- not final: it is kept so it can be restored. What a deletion must never do
-- is rewrite the note, so the triggers below freeze everything except
-- `deleted_at` itself while a note is deleted.
--
-- Cards written from a note are untouched by any of this: a card has never
-- been linked to a note (0007), and nothing here adds such a link.

-- NULL means the note is active. Deleting sets this to the deletion time
-- (ISO-8601 UTC); restoring clears it back to NULL. The row, its text, and
-- its created/updated times are kept either way.
ALTER TABLE notes ADD COLUMN deleted_at TEXT;

-- Notes are only ever soft-deleted, so removing a row is refused outright.
CREATE TRIGGER notes_no_hard_delete
BEFORE DELETE ON notes
BEGIN
    SELECT RAISE(ABORT, 'notes are soft-deleted, never removed');
END;

-- A note is always written as an active one. Inserting a row that is already
-- deleted would create a note nobody ever wrote and that the trigger below
-- then freezes forever, so it is refused at the door.
CREATE TRIGGER notes_are_written_active
BEFORE INSERT ON notes
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'a new note cannot already be deleted');
END;

-- A deleted note is read-only: every column it has except `deleted_at` is
-- frozen while it is deleted, so restoring one gives back exactly what was
-- deleted. Clearing `deleted_at` (the restore) is the one allowed change, and
-- it is the only reason this lists the columns instead of refusing every
-- update the way 0005 and 0006 do for cards and decks. A column added to
-- `notes` later belongs in this list too.
-- `IS NOT` compares like `<>` but is also correct for NULLs.
CREATE TRIGGER notes_deleted_are_read_only
BEFORE UPDATE ON notes
WHEN OLD.deleted_at IS NOT NULL
  AND (NEW.id IS NOT OLD.id
    OR NEW.title IS NOT OLD.title
    OR NEW.body IS NOT OLD.body
    OR NEW.created_at IS NOT OLD.created_at
    OR NEW.updated_at IS NOT OLD.updated_at)
BEGIN
    SELECT RAISE(ABORT, 'note is deleted');
END;

-- A deletion time is written once and only ever cleared by a restore, so
-- deleting again can never move when a note was deleted.
CREATE TRIGGER notes_deleted_at_written_once
BEFORE UPDATE OF deleted_at ON notes
WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'note is already deleted');
END;
