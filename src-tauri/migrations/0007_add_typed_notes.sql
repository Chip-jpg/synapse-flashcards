-- V1.2: typed study notes, written and kept locally.
-- Forward-only. Adds one table; no existing table or row changes.
--
-- Notes are global, not tied to a deck: a note is study material a card in
-- any deck can be written from, and cards stay self-contained (no link back
-- to a note). The ingestion columns sketched in the spec for PDFs and slides
-- (source type, raw file) aren't added until that slice exists; a later
-- migration can add them with a default of 'typed' for these rows.
-- Validation happens in Rust (`notes.rs`); the triggers below repeat the
-- most important rule inside SQLite, as migration 0004 does for decks and cards.

CREATE TABLE notes (
    id         INTEGER PRIMARY KEY,
    title      TEXT NOT NULL,
    -- Plain text as typed: outer whitespace trimmed, inner line breaks kept.
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- A note's title and body can't be blank. `trim(x, chars)` strips spaces,
-- tabs, and line breaks from both ends; Rust rejects all other whitespace too.
CREATE TRIGGER notes_text_not_blank_insert
BEFORE INSERT ON notes
WHEN trim(NEW.title, ' ' || char(9, 10, 13)) = ''
  OR trim(NEW.body, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'note text is blank');
END;

CREATE TRIGGER notes_text_not_blank_update
BEFORE UPDATE OF title, body ON notes
WHEN trim(NEW.title, ' ' || char(9, 10, 13)) = ''
  OR trim(NEW.body, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'note text is blank');
END;
