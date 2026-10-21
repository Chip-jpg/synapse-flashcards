-- Text recognition: what was read from a picture, or from a scanned page of
-- a PDF, by an OCR engine (`ocr.rs`, `recognition.rs`). Forward-only. Adds
-- one table; no existing table or row changes.
--
-- A recognition belongs to a kept file (`assets`, 0016), not to a note, a
-- card or a source: the same bytes always hold the same text, and a file
-- is stored once however often it's used. So a picture on two notes, or a
-- PDF imported twice, is read once. Page 0 is a picture; pages 1 and up are
-- pages of a PDF.
--
-- What extraction read from a document stays in `source_pages`, unchanged;
-- what an engine recognised is kept here, beside it, and marked as such
-- wherever it is shown, since recognition makes mistakes. The user's
-- corrections belong in their own note, never here.
--
-- Rows are never changed or removed. Reading a page again, with another
-- engine or model, adds a row; the newest row for a file and page is the one
-- in use. Every row travels in a backup with the database (its file already
-- does, through `assets`), so backups need no change.
CREATE TABLE recognitions (
    id         INTEGER PRIMARY KEY,
    asset_id   INTEGER NOT NULL REFERENCES assets (id),
    -- 0 for a picture; the page number (from 1) for a PDF.
    page       INTEGER NOT NULL CHECK (typeof(page) = 'integer' AND page >= 0),
    -- 'ocrs' is the engine built into Synapse; 'ollama' a vision model in
    -- Ollama on this computer, chosen by the user.
    engine     TEXT NOT NULL CHECK (engine IN ('ocrs', 'ollama')),
    -- Which models read it (the built-in engine's model files, or the
    -- Ollama model's name), so a result can be told from a later one.
    model      TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 200),
    -- The text read, line by line, with a blank line between paragraphs.
    -- Empty when nothing was found. At most a note body's length.
    text       TEXT NOT NULL CHECK (length(text) <= 100000),
    -- Where each line and word is on the picture, as a JSON array of
    -- `{ text, box, words: [{ text, box }] }`, each box `{ left, top, width,
    -- height }` in fractions (0 to 1) of the picture's size. Empty for an
    -- engine that doesn't say where text is.
    lines      TEXT NOT NULL CHECK (json_valid(lines) AND json_type(lines) = 'array'),
    -- The picture's size in pixels as it was read, when the engine says.
    width      INTEGER CHECK (width IS NULL OR (typeof(width) = 'integer' AND width > 0)),
    height     INTEGER CHECK (height IS NULL OR (typeof(height) = 'integer' AND height > 0)),
    created_at TEXT NOT NULL
);

CREATE INDEX recognitions_by_page ON recognitions (asset_id, page, id);

-- A picture is read as page 0, a PDF page by page; nothing else is read.
CREATE TRIGGER recognitions_fit_the_file
BEFORE INSERT ON recognitions
WHEN NOT CASE
         WHEN (SELECT media_type FROM assets WHERE id = NEW.asset_id) LIKE 'image/%'
             THEN NEW.page = 0
         WHEN (SELECT media_type FROM assets WHERE id = NEW.asset_id) = 'application/pdf'
             THEN NEW.page >= 1
         ELSE 0
     END
BEGIN
    SELECT RAISE(ABORT, 'only pictures and pages of a PDF are recognised');
END;

CREATE TRIGGER recognitions_never_change
BEFORE UPDATE ON recognitions
BEGIN
    SELECT RAISE(ABORT, 'a recognition never changes');
END;

CREATE TRIGGER recognitions_no_hard_delete
BEFORE DELETE ON recognitions
BEGIN
    SELECT RAISE(ABORT, 'recognitions are never removed');
END;
