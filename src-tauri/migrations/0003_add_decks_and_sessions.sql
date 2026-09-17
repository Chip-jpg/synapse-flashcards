-- Slice 3: decks and review sessions.
-- Forward-only. Creates the sample deck and moves existing cards into it, so an
-- upgraded database keeps every card's id, scheduling state, and review history.

CREATE TABLE decks (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    is_sample   BOOLEAN NOT NULL DEFAULT 0 CHECK (is_sample IN (0, 1)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- At most one deck can ever be the sample deck.
CREATE UNIQUE INDEX decks_single_sample ON decks (is_sample) WHERE is_sample = 1;

-- A migration runs exactly once per database, so this creates exactly one sample deck.
INSERT INTO decks (name, description, is_sample)
VALUES ('Sample deck', 'A small deck to try out Synapse.', 1);

ALTER TABLE flashcards ADD COLUMN deck_id INTEGER REFERENCES decks (id);

-- Before this migration there was no way to add cards, so the only card that can
-- exist is the seeded example. On a fresh database there are no cards yet; the
-- seed (in Rust) links its card to the sample deck instead.
UPDATE flashcards
SET deck_id = (SELECT id FROM decks WHERE is_sample = 1)
WHERE deck_id IS NULL;

-- Deck-scoped due-card lookups and due counts.
CREATE INDEX flashcards_deck_due ON flashcards (deck_id, due);

CREATE TABLE sessions (
    id             INTEGER PRIMARY KEY,
    deck_id        INTEGER NOT NULL REFERENCES decks (id),
    started_at     TEXT NOT NULL,
    ended_at       TEXT,
    cards_reviewed INTEGER NOT NULL DEFAULT 0 CHECK (cards_reviewed >= 0)
);

-- A deck has at most one unfinished session; also used to find it.
CREATE UNIQUE INDEX sessions_one_active_per_deck ON sessions (deck_id) WHERE ended_at IS NULL;
