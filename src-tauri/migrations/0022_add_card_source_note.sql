-- Whole-database backups retain this column; existing/imported cards default to NULL.
ALTER TABLE flashcards ADD COLUMN source_note_id INTEGER REFERENCES notes(id) ON DELETE SET NULL;
CREATE INDEX flashcards_by_source_note ON flashcards(source_note_id);

-- Extend 0018's deleted-card freeze to source-note links. Notes cannot be
-- hard-deleted (0008), so ON DELETE SET NULL cannot touch a frozen card.
DROP TRIGGER flashcards_deleted_are_read_only;

CREATE TRIGGER flashcards_deleted_are_read_only
BEFORE UPDATE ON flashcards
WHEN OLD.deleted_at IS NOT NULL
  AND (NEW.id IS NOT OLD.id
    OR NEW.deck_id IS NOT OLD.deck_id
    OR NEW.front IS NOT OLD.front
    OR NEW.back IS NOT OLD.back
    OR NEW.fsrs_state IS NOT OLD.fsrs_state
    OR NEW.fsrs_stability IS NOT OLD.fsrs_stability
    OR NEW.fsrs_difficulty IS NOT OLD.fsrs_difficulty
    OR NEW.due IS NOT OLD.due
    OR NEW.last_review IS NOT OLD.last_review
    OR NEW.reps IS NOT OLD.reps
    OR NEW.lapses IS NOT OLD.lapses
    OR NEW.created_at IS NOT OLD.created_at
    OR NEW.updated_at IS NOT OLD.updated_at
    OR NEW.kind IS NOT OLD.kind
    OR NEW.cloze_text_id IS NOT OLD.cloze_text_id
    OR NEW.cloze_number IS NOT OLD.cloze_number
    OR NEW.occlusion_id IS NOT OLD.occlusion_id
    OR NEW.occlusion_number IS NOT OLD.occlusion_number
    OR NEW.source_note_id IS NOT OLD.source_note_id)
BEGIN
    SELECT RAISE(ABORT, 'flashcard is deleted');
END;
