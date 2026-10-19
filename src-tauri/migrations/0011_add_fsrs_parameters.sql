-- V1.6: FSRS parameters fitted to the user's own review history.
-- Forward-only. Adds one table; no existing table or row changes.
--
-- Every change to the parameters the scheduler uses is a new row: applying
-- optimized parameters adds one with them, and resetting to FSRS's defaults
-- adds one whose `parameters` is NULL. The newest row (highest id) is the one
-- in use; with no rows at all, FSRS's defaults are. Rows are never changed or
-- removed, so the history of what was applied, and when, stays with the
-- database and travels in backups like everything else.
--
-- New parameters only change how future ratings are scheduled. Nothing here
-- (or in Rust) rewrites a card's stored schedule or any `review_logs` row.
-- Validation happens in Rust (`optimizer.rs`); the checks below repeat the
-- most important rules inside SQLite, as every migration here does.

CREATE TABLE fsrs_parameters (
    id           INTEGER PRIMARY KEY,
    -- A JSON array of FSRS's 21 parameters, or NULL for its defaults.
    parameters   TEXT
        CHECK (parameters IS NULL
            OR (json_valid(parameters)
                AND json_type(parameters) = 'array'
                AND json_array_length(parameters) = 21)),
    -- How many reviews the parameters were fitted to. NULL with the defaults.
    reviews_used INTEGER CHECK (reviews_used IS NULL OR reviews_used > 0),
    applied_at   TEXT NOT NULL,
    CHECK ((parameters IS NULL) = (reviews_used IS NULL))
);

CREATE TRIGGER fsrs_parameters_no_update
BEFORE UPDATE ON fsrs_parameters
BEGIN
    SELECT RAISE(ABORT, 'applied FSRS parameters are never changed');
END;

CREATE TRIGGER fsrs_parameters_no_delete
BEFORE DELETE ON fsrs_parameters
BEGIN
    SELECT RAISE(ABORT, 'applied FSRS parameters are never removed');
END;
