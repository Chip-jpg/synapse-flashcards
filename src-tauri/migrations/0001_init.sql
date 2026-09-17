-- Slice 1: the minimal flashcards table.
-- FSRS columns arrive in a later migration (Slice 2); decks in Slice 3.
CREATE TABLE flashcards (
    id         INTEGER PRIMARY KEY,
    front      TEXT NOT NULL,
    back       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
