-- Sources and pictures, the foundation: files Synapse keeps, and imported
-- documents kept as sources with their pages. Forward-only. Adds four
-- tables; no existing table or row changes.
--
-- Before this migration an import kept only its text, as an ordinary note,
-- and the database was everything a backup needed. From here Synapse keeps
-- files too (an imported document, a picture), in a folder beside the
-- database (`assets/`, see `assets.rs`). Each file is named by the SHA-256
-- of its bytes, and this database lists every one of them in `assets`. A
-- backup (`export.rs`, format 2) carries the database and exactly the files
-- `assets` lists; restoring one checks every file against its row.
--
-- Validation happens in Rust (`assets.rs`, `sources.rs`); the rules below
-- repeat the most important parts inside SQLite, as every migration does.

-- One file Synapse keeps. The bytes live in `assets/<sha256>`; the row says
-- what they are. A file is stored once however often it's used, so two
-- imports of the same document, or one picture on two cards, share a row.
--
-- Rows are never changed or removed, so whatever points at a file can rely
-- on it for good: deleting a source or a picture's note is a soft delete,
-- and the file stays. (Reclaiming the space of files nothing uses any more
-- would be a separate, explicit feature.)
CREATE TABLE assets (
    id         INTEGER PRIMARY KEY,
    -- Lowercase hex, which is also the file's name.
    sha256     TEXT NOT NULL UNIQUE
               CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    -- What the bytes are, worked out from the bytes themselves when the file
    -- was added, never from a file name. Text files are 'text/plain' whether
    -- they were .txt or .md (a source's `kind` says which).
    media_type TEXT NOT NULL CHECK (media_type IN (
                   'image/png', 'image/jpeg', 'image/gif', 'image/webp',
                   'application/pdf', 'text/plain',
                   'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
                   'application/vnd.openxmlformats-officedocument.presentationml.presentation')),
    -- Bytes, from 1 to 64 MiB (`assets::MAX_ASSET_BYTES`).
    size       INTEGER NOT NULL
               CHECK (typeof(size) = 'integer' AND size BETWEEN 1 AND 67108864),
    created_at TEXT NOT NULL
);

CREATE TRIGGER assets_never_change
BEFORE UPDATE ON assets
BEGIN
    SELECT RAISE(ABORT, 'a kept file never changes');
END;

CREATE TRIGGER assets_no_hard_delete
BEFORE DELETE ON assets
BEGIN
    SELECT RAISE(ABORT, 'kept files are never removed');
END;

-- An imported document, kept: the original file (`asset_id`) and the text of
-- each of its pages (`source_pages`), as extraction read them. The note a
-- user saves from it is their own, editable copy; the source is what the
-- document said. Sources are soft-deleted and restored like notes (0008).
CREATE TABLE sources (
    id         INTEGER PRIMARY KEY,
    asset_id   INTEGER NOT NULL REFERENCES assets (id),
    -- The kind of document, as `extract::FileKind` names it.
    kind       TEXT NOT NULL
               CHECK (kind IN ('pdf', 'text', 'markdown', 'word', 'powerPoint')),
    -- Up to 200 characters, trimmed, never blank (a note title's rules).
    title      TEXT NOT NULL
               CHECK (length(title) BETWEEN 1 AND 200
                      AND trim(title, ' ' || char(9, 10, 13)) = title),
    -- The name the file had, without its folder: where it was is never kept.
    file_name  TEXT NOT NULL CHECK (length(file_name) BETWEEN 1 AND 255),
    -- Pages of a PDF, slides of a presentation; 1 for any other document.
    page_count INTEGER NOT NULL CHECK (typeof(page_count) = 'integer' AND page_count >= 1),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    -- NULL while the source is active; the deletion time once deleted.
    deleted_at TEXT
);

CREATE INDEX sources_by_asset ON sources (asset_id);

-- The file must be the kind of document the source says it is.
CREATE TRIGGER sources_file_matches_kind
BEFORE INSERT ON sources
WHEN (SELECT media_type FROM assets WHERE id = NEW.asset_id) IS NOT
     CASE NEW.kind
         WHEN 'pdf' THEN 'application/pdf'
         WHEN 'text' THEN 'text/plain'
         WHEN 'markdown' THEN 'text/plain'
         WHEN 'word' THEN 'application/vnd.openxmlformats-officedocument.wordprocessingml.document'
         WHEN 'powerPoint' THEN 'application/vnd.openxmlformats-officedocument.presentationml.presentation'
     END
BEGIN
    SELECT RAISE(ABORT, 'a source''s file must be the kind of document it says');
END;

CREATE TRIGGER sources_are_written_active
BEFORE INSERT ON sources
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'a new source cannot already be deleted');
END;

CREATE TRIGGER sources_no_hard_delete
BEFORE DELETE ON sources
BEGIN
    SELECT RAISE(ABORT, 'sources are soft-deleted, never removed');
END;

-- What was imported never changes: the file, its kind and name, how many
-- pages it has, and when it came in. Only the title (a rename), `updated_at`
-- and `deleted_at` can.
CREATE TRIGGER sources_original_is_fixed
BEFORE UPDATE ON sources
WHEN NEW.id IS NOT OLD.id
  OR NEW.asset_id IS NOT OLD.asset_id
  OR NEW.kind IS NOT OLD.kind
  OR NEW.file_name IS NOT OLD.file_name
  OR NEW.page_count IS NOT OLD.page_count
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'an imported source never changes');
END;

-- As 0008 does for notes: a deleted source is read-only, every column but
-- `deleted_at` frozen, so restoring it gives back exactly what was deleted.
-- A column added to `sources` later belongs in this list too.
CREATE TRIGGER sources_deleted_are_read_only
BEFORE UPDATE ON sources
WHEN OLD.deleted_at IS NOT NULL
  AND (NEW.id IS NOT OLD.id
    OR NEW.asset_id IS NOT OLD.asset_id
    OR NEW.kind IS NOT OLD.kind
    OR NEW.title IS NOT OLD.title
    OR NEW.file_name IS NOT OLD.file_name
    OR NEW.page_count IS NOT OLD.page_count
    OR NEW.created_at IS NOT OLD.created_at
    OR NEW.updated_at IS NOT OLD.updated_at)
BEGIN
    SELECT RAISE(ABORT, 'source is deleted');
END;

CREATE TRIGGER sources_deleted_at_written_once
BEFORE UPDATE OF deleted_at ON sources
WHEN OLD.deleted_at IS NOT NULL AND NEW.deleted_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'source is already deleted');
END;

-- The text of each page (or slide) of a source, as extraction read it,
-- numbered from 1. A page without text (a scanned page) has empty text. All
-- of a source's pages are written with it, in the same transaction, and
-- never change or go.
CREATE TABLE source_pages (
    source_id INTEGER NOT NULL REFERENCES sources (id),
    number    INTEGER NOT NULL CHECK (typeof(number) = 'integer' AND number >= 1),
    text      TEXT NOT NULL,
    PRIMARY KEY (source_id, number)
) WITHOUT ROWID;

CREATE TRIGGER source_pages_within_source
BEFORE INSERT ON source_pages
WHEN NOT EXISTS (SELECT 1 FROM sources
                 WHERE id = NEW.source_id AND deleted_at IS NULL
                   AND NEW.number <= page_count)
BEGIN
    SELECT RAISE(ABORT, 'a page belongs to an active source, within its page count');
END;

CREATE TRIGGER source_pages_never_change
BEFORE UPDATE ON source_pages
BEGIN
    SELECT RAISE(ABORT, 'a source''s pages never change');
END;

CREATE TRIGGER source_pages_no_delete
BEFORE DELETE ON source_pages
BEGIN
    SELECT RAISE(ABORT, 'a source''s pages are never removed');
END;

-- Which source a note was written from: at most one per note, recorded when
-- the note is saved from the source and never changed. The note stays the
-- user's own text (cards still don't link to notes, 0007); this is what
-- lets a quote in that note be found on a page of the original.
CREATE TABLE source_notes (
    note_id    INTEGER PRIMARY KEY REFERENCES notes (id),
    source_id  INTEGER NOT NULL REFERENCES sources (id),
    created_at TEXT NOT NULL
);

CREATE INDEX source_notes_by_source ON source_notes (source_id);

CREATE TRIGGER source_notes_link_active_rows
BEFORE INSERT ON source_notes
WHEN NOT EXISTS (SELECT 1 FROM notes WHERE id = NEW.note_id AND deleted_at IS NULL)
  OR NOT EXISTS (SELECT 1 FROM sources WHERE id = NEW.source_id AND deleted_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'only an active note and an active source can be linked');
END;

CREATE TRIGGER source_notes_never_change
BEFORE UPDATE ON source_notes
BEGIN
    SELECT RAISE(ABORT, 'a note''s source never changes');
END;

CREATE TRIGGER source_notes_no_delete
BEFORE DELETE ON source_notes
BEGIN
    SELECT RAISE(ABORT, 'a note''s source is never removed');
END;
