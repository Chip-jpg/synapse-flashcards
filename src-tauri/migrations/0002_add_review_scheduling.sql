-- Slice 2: FSRS scheduling state on each card, plus a log of every review.
-- Forward-only. Existing cards (from 0001) become New cards that are due now.

-- Memory state from FSRS. NULL until the card's first review.
ALTER TABLE flashcards ADD COLUMN fsrs_stability REAL;
ALTER TABLE flashcards ADD COLUMN fsrs_difficulty REAL;
ALTER TABLE flashcards ADD COLUMN fsrs_state TEXT NOT NULL DEFAULT 'New'
    CHECK (fsrs_state IN ('New', 'Learning', 'Review', 'Relearning'));
-- ISO-8601 UTC. A NULL `due` means a new card that is due immediately.
ALTER TABLE flashcards ADD COLUMN due TEXT;
ALTER TABLE flashcards ADD COLUMN last_review TEXT;
ALTER TABLE flashcards ADD COLUMN reps INTEGER NOT NULL DEFAULT 0;
ALTER TABLE flashcards ADD COLUMN lapses INTEGER NOT NULL DEFAULT 0;

-- One row appended per rating; never updated.
CREATE TABLE review_logs (
    id               INTEGER PRIMARY KEY,
    card_id          INTEGER NOT NULL REFERENCES flashcards (id),
    rating           INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 4),
    state_before     TEXT NOT NULL
        CHECK (state_before IN ('New', 'Learning', 'Review', 'Relearning')),
    scheduled_days   REAL NOT NULL,
    elapsed_days     REAL NOT NULL,
    stability_after  REAL NOT NULL,
    difficulty_after REAL NOT NULL,
    reviewed_at      TEXT NOT NULL
);
