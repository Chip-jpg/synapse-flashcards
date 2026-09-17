import { invoke } from "@tauri-apps/api/core";

/** Mirrors `DeckSummary` in `src-tauri/src/study.rs`. */
export type Deck = {
  id: number;
  name: string;
  description: string | null;
  /** The built-in sample deck, which can't be opened for editing. */
  isSample: boolean;
  /** Cards in this deck that are due now. */
  dueCount: number;
};

/** Mirrors `ArchivedDeck` in `src-tauri/src/authoring.rs`: history only, never reviewed or changed. */
export type ArchivedDeck = {
  id: number;
  name: string;
  description: string | null;
  /** Active (not deleted) cards kept in the deck. */
  cardCount: number;
  /** When it was archived (ISO-8601 UTC). */
  archivedAt: string;
};

/** Mirrors `DeckDetail` in `src-tauri/src/authoring.rs`. Always an active normal (non-sample) deck. */
export type DeckDetail = {
  id: number;
  name: string;
  description: string | null;
  /** Every active (not deleted) card in the deck. */
  cardCount: number;
  /** Active cards in this deck that are due now. */
  dueCount: number;
  /** The active cards, oldest first. */
  cards: DeckCard[];
};

/** Mirrors `DeckCard` in `src-tauri/src/authoring.rs`. */
export type DeckCard = {
  id: number;
  front: string;
  back: string;
};

/** Mirrors `Flashcard` in `src-tauri/src/study.rs`. */
export type Flashcard = {
  id: number;
  front: string;
  back: string;
  /** Times reviewed; sent back with a rating so stale submissions are rejected. */
  reps: number;
};

/** Mirrors `StartedSession` in `src-tauri/src/study.rs`. */
export type StartedSession = { status: "started"; sessionId: number } | { status: "noneDue" };

/** Mirrors `SessionCard` in `src-tauri/src/study.rs`. */
export type SessionCard =
  | { status: "due"; card: Flashcard }
  | { status: "completed"; cardsReviewed: number };

/** 1 = Again, 2 = Hard, 3 = Good, 4 = Easy (matches `review_logs.rating`). */
export type Rating = 1 | 2 | 3 | 4;

/**
 * Mirrors `ExportOutcome` in `src-tauri/src/commands.rs`. `fileName` is the
 * saved file's name only; Rust never sends the folder it went to.
 */
export type ExportOutcome = { status: "saved"; fileName: string } | { status: "cancelled" };

/**
 * Mirrors `ImportCheck` in `src-tauri/src/commands.rs`. A "ready" backup has
 * passed every check but has replaced nothing yet; `token` identifies it to
 * `confirmImport` and `cancelImport`. `fileName` is the name only, never the
 * folder, and `exportedAt` is ISO-8601 UTC.
 */
export type ImportCheck =
  | { status: "ready"; token: number; fileName: string; exportedAt: string }
  | { status: "cancelled" };

/** Mirrors `NoteSummary` in `src-tauri/src/notes.rs`: one note in the library. */
export type NoteSummary = {
  id: number;
  title: string;
  /** When it was created (ISO-8601 UTC). */
  createdAt: string;
  /** When it was last saved (ISO-8601 UTC). */
  updatedAt: string;
};

/** Mirrors `Note` in `src-tauri/src/notes.rs`. `body` is plain text with its line breaks. */
export type Note = {
  id: number;
  title: string;
  body: string;
  createdAt: string;
  updatedAt: string;
};

/**
 * A form field that a validation error can point at: `Field` in `commands.rs`,
 * plus "deck", used only by the frontend's own check that a deck was chosen
 * before asking Rust to add a card to it.
 */
export type Field = "name" | "description" | "front" | "back" | "title" | "body" | "deck";

/** Mirrors `CommandError` in `src-tauri/src/commands.rs`. */
export type CommandError = {
  kind: "failed" | "stale" | "invalid";
  message: string;
  /** Only for input that failed validation: the form field to fix. */
  field?: Field;
};

/** Every active (not archived) deck with its due-card count. */
export function getDecks(): Promise<Deck[]> {
  return invoke<Deck[]>("get_decks");
}

/** Every archived deck, most recently archived first. */
export function getArchivedDecks(): Promise<ArchivedDeck[]> {
  return invoke<ArchivedDeck[]>("get_archived_decks");
}

/** Starts (or resumes) a review session, if the deck has due cards. */
export function startSession(deckId: number): Promise<StartedSession> {
  return invoke<StartedSession>("start_session", { deckId });
}

/** The session's next due card, or its completed state. */
export function getSessionCard(sessionId: number): Promise<SessionCard> {
  return invoke<SessionCard>("get_session_card", { sessionId });
}

/** Saves a rating for `card` within a session. Rejects with a `CommandError`. */
export function reviewCard(sessionId: number, card: Flashcard, rating: Rating): Promise<void> {
  return invoke<void>("review_card", {
    sessionId,
    cardId: card.id,
    expectedReps: card.reps,
    rating,
  });
}

/** Creates a normal deck. Rust trims and validates both fields; a blank description means none. */
export function createDeck(name: string, description: string): Promise<DeckDetail> {
  return invoke<DeckDetail>("create_deck", { name, description });
}

/** One normal deck with its card and due counts. */
export function getDeckDetail(deckId: number): Promise<DeckDetail> {
  return invoke<DeckDetail>("get_deck_detail", { deckId });
}

/** Adds a new, immediately due card to a normal deck; resolves with the updated deck. */
export function createFlashcard(deckId: number, front: string, back: string): Promise<DeckDetail> {
  return invoke<DeckDetail>("create_flashcard", { deckId, front, back });
}

/** Replaces a card's text, keeping its schedule and history; resolves with its deck. */
export function updateFlashcard(cardId: number, front: string, back: string): Promise<DeckDetail> {
  return invoke<DeckDetail>("update_flashcard", { cardId, front, back });
}

/**
 * Soft-deletes a card (its history is kept); resolves with its deck without it.
 * Rejects with kind "stale" if the card was already deleted.
 */
export function deleteFlashcard(cardId: number): Promise<DeckDetail> {
  return invoke<DeckDetail>("delete_flashcard", { cardId });
}

/** Renames an active normal deck (Rust trims and validates the name); resolves with the deck. */
export function renameDeck(deckId: number, name: string): Promise<DeckDetail> {
  return invoke<DeckDetail>("rename_deck", { deckId, name });
}

/**
 * Archives an active normal deck: it leaves the dashboard and reviews, but its
 * cards and history are kept. Rejects with kind "stale" if already archived.
 */
export function archiveDeck(deckId: number): Promise<void> {
  return invoke<void>("archive_deck", { deckId });
}

/**
 * Saves a backup of all Synapse study data as a `.zip`. Rust opens the OS
 * "save as" dialog, so the user picks the file and confirms any replacement;
 * closing it resolves with `{ status: "cancelled" }` and writes nothing.
 */
export function exportData(): Promise<ExportOutcome> {
  return invoke<ExportOutcome>("export_data");
}

/**
 * Chooses a backup to restore and checks it, without replacing anything. Rust
 * opens the OS "open file" dialog; closing it resolves with
 * `{ status: "cancelled" }`. A file that isn't a usable Synapse export rejects
 * with kind "invalid".
 */
export function prepareImport(): Promise<ImportCheck> {
  return invoke<ImportCheck>("prepare_import");
}

/**
 * Replaces ALL current study data with the checked backup. Irreversible: only
 * call after the user confirms. Rejects with kind "stale" if the backup is no
 * longer waiting, or "failed" if the restore couldn't complete.
 */
export function confirmImport(token: number): Promise<void> {
  return invoke<void>("confirm_import", { token });
}

/** Forgets a checked backup without restoring it. Nothing else changes. */
export function cancelImport(token: number): Promise<void> {
  return invoke<void>("cancel_import", { token });
}

/** Every note, most recently saved first. */
export function getNotes(): Promise<NoteSummary[]> {
  return invoke<NoteSummary[]>("get_notes");
}

/** One whole note. Rejects with kind "invalid" if it doesn't exist. */
export function getNote(noteId: number): Promise<Note> {
  return invoke<Note>("get_note", { noteId });
}

/** Saves a new note. Rust trims and validates both fields; errors name the field to fix. */
export function createNote(title: string, body: string): Promise<Note> {
  return invoke<Note>("create_note", { title, body });
}

/** Replaces a note's title and body (same rules as creating); resolves with the saved note. */
export function updateNote(noteId: number, title: string, body: string): Promise<Note> {
  return invoke<Note>("update_note", { noteId, title, body });
}

/** Normalizes anything a command rejected with into a `CommandError`. */
export function toCommandError(err: unknown): CommandError {
  if (typeof err === "object" && err !== null && "kind" in err && "message" in err) {
    return err as CommandError;
  }
  return { kind: "failed", message: "Something went wrong." };
}
