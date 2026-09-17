-- Slice 4A: users create their own decks and add cards to them.
-- Forward-only. Validation happens in Rust (`authoring.rs`); these rules repeat
-- the most important parts inside SQLite, so no code path can store a
-- duplicate deck name or blank text even if it skips that validation.

-- Deck names are unique, ignoring letter case ('Biology' and 'BIOLOGY' clash).
-- SQLite's NOCASE only folds ASCII letters; Rust also compares Unicode lowercase.
-- Before this migration no deck could be created, so an existing database holds
-- only the sample deck and this index can always be built.
CREATE UNIQUE INDEX decks_name_unique ON decks (name COLLATE NOCASE);

-- Deck names and card text can't be blank. `trim(x, chars)` strips spaces,
-- tabs, and line breaks from both ends; Rust rejects all other whitespace too.
CREATE TRIGGER decks_name_not_blank_insert
BEFORE INSERT ON decks
WHEN trim(NEW.name, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'deck name is blank');
END;

CREATE TRIGGER decks_name_not_blank_update
BEFORE UPDATE OF name ON decks
WHEN trim(NEW.name, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'deck name is blank');
END;

CREATE TRIGGER flashcards_text_not_blank_insert
BEFORE INSERT ON flashcards
WHEN trim(NEW.front, ' ' || char(9, 10, 13)) = ''
  OR trim(NEW.back, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'flashcard text is blank');
END;

CREATE TRIGGER flashcards_text_not_blank_update
BEFORE UPDATE OF front, back ON flashcards
WHEN trim(NEW.front, ' ' || char(9, 10, 13)) = ''
  OR trim(NEW.back, ' ' || char(9, 10, 13)) = ''
BEGIN
    SELECT RAISE(ABORT, 'flashcard text is blank');
END;
