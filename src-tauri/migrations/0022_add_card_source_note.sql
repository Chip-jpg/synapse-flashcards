-- Whole-database backups retain this column; existing/imported cards default to NULL.
ALTER TABLE flashcards ADD COLUMN source_note_id INTEGER REFERENCES notes(id) ON DELETE SET NULL;
CREATE INDEX flashcards_by_source_note ON flashcards(source_note_id);
