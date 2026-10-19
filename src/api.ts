import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

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

/**
 * Mirrors `ArchivedDeck` in `src-tauri/src/authoring.rs`: an archived deck,
 * which can't be reviewed or changed while it is archived, but can be
 * unarchived back into the deck list.
 */
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
  /**
   * The deck's soft-deleted cards, most recently deleted first. They are in
   * neither count above and in no review; each can be restored.
   */
  deletedCards: DeletedCard[];
};

/** Mirrors `DeckCard` in `src-tauri/src/authoring.rs`. */
export type DeckCard = {
  id: number;
  front: string;
  back: string;
};

/**
 * Mirrors `DeletedCard` in `src-tauri/src/authoring.rs`: a soft-deleted card,
 * listed under its deck's *Deleted cards*. Its text is shown so the user can
 * tell which card they would bring back; its schedule isn't sent, because a
 * restore keeps whatever schedule the card already had.
 */
export type DeletedCard = {
  id: number;
  front: string;
  back: string;
  /** When it was deleted (ISO-8601 UTC). */
  deletedAt: string;
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

/**
 * Mirrors `SessionProgress` in `src-tauri/src/study.rs`: how far the session
 * has got, as read when the card was handed out. `remaining` counts the card
 * being shown, so it is card `reviewed + 1` of `reviewed + remaining`. The
 * total can rise while a session is open (a card falls due, or one is added),
 * which is why the bar fills only once the session completes.
 */
export type SessionProgress = {
  /** Cards rated in this session so far. */
  reviewed: number;
  /** Cards still due in the deck, counting the one being shown. */
  remaining: number;
};

/** Mirrors `SessionCard` in `src-tauri/src/study.rs`. */
export type SessionCard =
  | {
      status: "due";
      card: Flashcard;
      progress: SessionProgress;
      /** How many times in a row the card was last rated Again. */
      againStreak: number;
      /** Rust's call: offer help with the card once its answer is shown. */
      offerHelp: boolean;
    }
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

/**
 * Mirrors `DeletedNote` in `src-tauri/src/notes.rs`: history only. The body
 * isn't sent, because a deleted note is listed, not opened. `updatedAt` is
 * when its text was last saved, before it was deleted.
 */
export type DeletedNote = {
  id: number;
  title: string;
  createdAt: string;
  updatedAt: string;
  /** When it was deleted (ISO-8601 UTC). */
  deletedAt: string;
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
  /**
   * Only when several cards were sent at once (`createFlashcards`): which one
   * `field` belongs to, counting from 0.
   */
  index?: number;
};

/**
 * Mirrors `CardText` in `src-tauri/src/authoring.rs`: a card's front and back
 * before it is saved, such as a suggested card.
 */
export type CardText = { front: string; back: string };

/**
 * Mirrors `SuggestedCard` in `src-tauri/src/suggest.rs`: a suggested card, with
 * the passage of the note it comes from (checked to be in the note) and the
 * words of its answer that the note never uses, for the user to check.
 */
export type SuggestedCard = CardText & { source: string; unfamiliar: string[] };

/**
 * Mirrors `SuggestOutcome` in `src-tauri/src/commands.rs`. Suggested cards are
 * drafts: nothing has been saved. `leftOut` counts suggestions dropped as
 * unusable (blank, too long, or repeated), and `unsupported` those dropped as
 * not coming from the note.
 */
export type SuggestOutcome =
  | { status: "suggested"; cards: SuggestedCard[]; leftOut: number; unsupported: number }
  | { status: "stopped" };

/** Every active (not archived) deck with its due-card count. */
export function getDecks(): Promise<Deck[]> {
  return invoke<Deck[]>("get_decks");
}

/**
 * Today's reviewing, counted since local midnight: `reviews` is every rating
 * recorded, `cards` how many different cards they were for. Ratings of cards
 * deleted or archived since still count.
 */
export type TodaySummary = {
  reviews: number;
  cards: number;
};

/** How many reviews were recorded today, and for how many cards. */
export function getToday(): Promise<TodaySummary> {
  return invoke<TodaySummary>("get_today");
}

/** Mirrors `DayCount` in `src-tauri/src/stats.rs`: a local date (`YYYY-MM-DD`) and a count. */
export type DayCount = {
  day: string;
  count: number;
};

/**
 * Mirrors `Statistics` in `src-tauri/src/stats.rs`. Days are calendar days on
 * this computer's clock.
 */
export type Statistics = {
  /** Ratings recorded on each of the last 30 days, oldest first, ending today. */
  reviewsByDay: DayCount[];
  /**
   * Reviews in the last 30 days of cards that were in the Review state, and
   * how many of them weren't rated Again.
   */
  retention: { reviews: number; recalled: number };
  /** Current cards (not deleted, in active decks) in each FSRS state. */
  cardStates: { new: number; learning: number; review: number; relearning: number };
  /**
   * Cards due on each of the next 14 days, starting today. Today also holds
   * cards already overdue and new cards not yet studied.
   */
  forecast: DayCount[];
};

/**
 * The Statistics screen's numbers, for every deck or only `deckId` (an active
 * deck). Reads only. Refused as stale if that deck was archived since.
 */
export function getStatistics(deckId: number | null): Promise<Statistics> {
  return invoke<Statistics>("get_statistics", { deckId });
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

/**
 * Adds several new, immediately due cards to a normal deck at once; resolves
 * with the updated deck. All or nothing: a refused card rejects with its
 * `field` and `index`, and then no card is added.
 */
export function createFlashcards(deckId: number, cards: CardText[]): Promise<DeckDetail> {
  return invoke<DeckDetail>("create_flashcards", { deckId, cards });
}

/** Replaces a card's text, keeping its schedule and history; resolves with its deck. */
export function updateFlashcard(cardId: number, front: string, back: string): Promise<DeckDetail> {
  return invoke<DeckDetail>("update_flashcard", { cardId, front, back });
}

/**
 * Soft-deletes a card (its history is kept, and it can be restored); resolves
 * with its deck without it, now listing it under `deletedCards`. Rejects with
 * kind "stale" if the card was already deleted.
 */
export function deleteFlashcard(cardId: number): Promise<DeckDetail> {
  return invoke<DeckDetail>("delete_flashcard", { cardId });
}

/**
 * Restores a soft-deleted card: it returns to its deck with its FSRS state,
 * due date, counts, and review history exactly as they were, and is due again
 * only if the due date it already had has passed. No review session starts.
 * Resolves with the deck, the card back among its `cards`. Rejects with kind
 * "stale" if the card isn't deleted or its deck has been archived, having
 * written nothing.
 */
export function restoreFlashcard(cardId: number): Promise<DeckDetail> {
  return invoke<DeckDetail>("restore_flashcard", { cardId });
}

/** Mirrors `RestoredCards` in `src-tauri/src/authoring.rs`. */
export type RestoredCards = {
  /** How many cards came back. */
  restored: number;
  /** The deck, with them back in it. */
  deck: DeckDetail;
};

/**
 * Restores every deleted card of a deck at once, each exactly as
 * `restoreFlashcard` restores one. `expected` is how many the user confirmed.
 * Rejects with kind "stale", having written nothing, if the deck no longer has
 * that many deleted cards or was archived meanwhile.
 */
export function restoreDeletedCards(deckId: number, expected: number): Promise<RestoredCards> {
  return invoke<RestoredCards>("restore_deleted_cards", { deckId, expected });
}

/** Mirrors `ChangedCards` in `src-tauri/src/authoring.rs`. */
export type ChangedCards = {
  /** How many cards were deleted or moved. */
  changed: number;
  /** The deck they were selected in, now without them. */
  deck: DeckDetail;
};

/**
 * Soft-deletes several cards of a deck at once, each exactly as
 * `deleteFlashcard` deletes one. `cardIds` are the cards the user confirmed.
 * Rejects with kind "stale", having written nothing, if any of them is no
 * longer an active card of the deck, or the deck was archived meanwhile.
 */
export function deleteFlashcards(deckId: number, cardIds: number[]): Promise<ChangedCards> {
  return invoke<ChangedCards>("delete_flashcards", { deckId, cardIds });
}

/**
 * Moves several cards of a deck into another active normal deck. Each keeps
 * its id, text, FSRS state, due date, counts, and review history. Rejects with
 * kind "stale", having written nothing, if any card is no longer an active
 * card of `deckId`, or either deck can no longer take the change.
 */
export function moveFlashcards(
  deckId: number,
  cardIds: number[],
  toDeckId: number,
): Promise<ChangedCards> {
  return invoke<ChangedCards>("move_flashcards", { deckId, cardIds, toDeckId });
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
 * Unarchives an archived normal deck: it returns to the deck list exactly as it
 * was, with its cards, their schedules, and its review history. No review
 * session is started, and deleted cards stay deleted. Rejects with kind "stale"
 * if the deck isn't archived, having written nothing.
 */
export function unarchiveDeck(deckId: number): Promise<void> {
  return invoke<void>("unarchive_deck", { deckId });
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

/** Mirrors `FileKind` in `src-tauri/src/extract.rs`: the kinds of file that can be imported. */
export type FileKind = "pdf" | "text" | "markdown" | "word" | "powerPoint";

/**
 * Mirrors `FileImport` in `src-tauri/src/commands.rs`: the text read from a
 * file, ready to become a note, or the file window closed. `fileName` is the
 * name only, never the folder. `pages` counts a PDF's pages or a
 * presentation's slides; `pagesWithoutText` lists a PDF's pages (from 1) that
 * had no text.
 */
export type FileImport =
  | {
      status: "read";
      fileName: string;
      kind: FileKind;
      title: string;
      body: string;
      pages: number | null;
      pagesWithoutText: number[];
    }
  | { status: "cancelled" };

/** A file whose text has been read. */
export type ImportedFile = FileImport & { status: "read" };

/**
 * Chooses a file (PDF, text, Markdown, Word, or PowerPoint) and reads its
 * text. Rust opens the OS "open file" dialog; closing it resolves with
 * `{ status: "cancelled" }`. Nothing is saved, and the file isn't kept: the
 * text is for a new note. A file that can't be read rejects with kind
 * "invalid" or "failed".
 */
export function importFile(): Promise<FileImport> {
  return invoke<FileImport>("import_file");
}

/**
 * Mirrors `CardsFileRow` in `src-tauri/src/commands.rs`: one row of a cards
 * file, checked by Rust as a typed card. `line` is where the row starts in
 * the file (from 1); `front` and `back` are trimmed. `problem` says why the
 * row can't become a card; `duplicate` that its front is already in the deck,
 * or on an earlier `line` of the file.
 */
export type CardsFileRow = {
  line: number;
  front: string;
  back: string;
  problem: { field: Field; message: string } | null;
  duplicate: { kind: "inDeck" } | { kind: "inFile"; line: number } | null;
};

/**
 * Mirrors `CardsFile` in `src-tauri/src/commands.rs`: a CSV or TSV file read
 * and checked against a deck, with nothing saved, or the file window closed.
 * `header` says the first row held column names and isn't among `rows`.
 */
export type CardsFile =
  | {
      status: "read";
      fileName: string;
      delimiter: "comma" | "semicolon" | "tab";
      header: boolean;
      rows: CardsFileRow[];
      /** Rows with something in a third or later column, which is ignored. */
      rowsWithExtraColumns: number;
    }
  | { status: "cancelled" };

/** A cards file that has been read. */
export type ReadCardsFile = CardsFile & { status: "read" };

/** Mirrors `CardsImported` in `src-tauri/src/cards_csv.rs`. */
export type CardsImported = {
  added: number;
  /** Cards left out because their front was already in the deck. */
  skipped: number;
  /** The deck's active cards afterwards. */
  cardCount: number;
};

/**
 * Chooses a CSV or TSV file of cards for an active normal deck, and checks
 * every row against it. Rust opens the OS "open file" dialog (after checking
 * the deck); closing it resolves with `{ status: "cancelled" }`. Nothing is
 * saved.
 */
export function readCardsCsv(deckId: number): Promise<CardsFile> {
  return invoke<CardsFile>("read_cards_csv", { deckId });
}

/**
 * Adds cards read from a file to a deck, all or nothing. With
 * `skipDuplicates`, Rust leaves out cards whose front the deck already has.
 * A refused card rejects with its `field` and `index` in `cards`; nothing new
 * to add rejects with kind "stale".
 */
export function importCardsCsv(
  deckId: number,
  cards: CardText[],
  skipDuplicates: boolean,
): Promise<CardsImported> {
  return invoke<CardsImported>("import_cards_csv", { deckId, cards, skipDuplicates });
}

/**
 * Saves an active deck's cards (front and back only) as a CSV file. Rust
 * opens the OS "save as" dialog; closing it resolves with
 * `{ status: "cancelled" }` and writes nothing.
 */
export function exportCardsCsv(deckId: number): Promise<ExportOutcome> {
  return invoke<ExportOutcome>("export_cards_csv", { deckId });
}

/**
 * Mirrors `AnkiDeckPreview` in `src-tauri/src/commands.rs`: one Anki deck of
 * a package. `name` is the new deck's name in Synapse (made unique), `cards`
 * the cards it gets one way, `reversible` how many of those also have a
 * reversed card in Anki, and `examples` its first few cards.
 */
export type AnkiDeckPreview = {
  ankiName: string;
  name: string;
  cards: number;
  reversible: number;
  examples: CardText[];
};

/** Mirrors `Skipped` in `src-tauri/src/anki.rs`: notes left out, by why. */
export type AnkiSkipped = {
  cloze: number;
  otherNoteType: number;
  empty: number;
  tooLong: number;
  noCard: number;
};

/**
 * Mirrors `AnkiFile` in `src-tauri/src/commands.rs`: an Anki package read,
 * with nothing saved and the package held by Rust under `token`, or the file
 * window closed.
 */
export type AnkiFile =
  | {
      status: "read";
      token: number;
      fileName: string;
      decks: AnkiDeckPreview[];
      /** Every note in the collection. */
      notes: number;
      skipped: AnkiSkipped;
      /** Notes to add that had a picture or sound, which is left out. */
      mediaRemoved: number;
      /** Notes to add that had been reviewed in Anki; they start as new. */
      reviewed: number;
    }
  | { status: "cancelled" };

/** An Anki package that has been read. */
export type ReadAnkiFile = AnkiFile & { status: "read" };

/** Mirrors `AnkiImported` in `src-tauri/src/anki.rs`. */
export type AnkiImported = {
  decks: { id: number; name: string; cards: number }[];
  cards: number;
};

/**
 * Chooses an Anki package (.apkg or .colpkg) and reads its front/back notes.
 * Rust opens the OS "open file" dialog; closing it resolves with
 * `{ status: "cancelled" }`. Nothing is saved.
 */
export function readAnkiPackage(): Promise<AnkiFile> {
  return invoke<AnkiFile>("read_anki_package");
}

/**
 * Adds the package read under `token` as new decks of new cards, all or
 * nothing; with `bothDirections`, also the reversed cards. A package Rust no
 * longer holds rejects with kind "stale".
 */
export function importAnkiPackage(token: number, bothDirections: boolean): Promise<AnkiImported> {
  return invoke<AnkiImported>("import_anki_package", { token, bothDirections });
}

/** Lets Rust forget the package read under `token`. */
export function cancelAnkiImport(token: number): Promise<void> {
  return invoke<void>("cancel_anki_import", { token });
}

/** Every active (not deleted) note, most recently saved first. */
export function getNotes(): Promise<NoteSummary[]> {
  return invoke<NoteSummary[]>("get_notes");
}

/**
 * The active notes whose title or body contains `query`, ignoring case, most
 * recently saved first. A blank query lists every active note. Reads only.
 */
export function searchNotes(query: string): Promise<NoteSummary[]> {
  return invoke<NoteSummary[]>("search_notes", { query });
}

/** Every deleted note, most recently deleted first. Read-only history. */
export function getDeletedNotes(): Promise<DeletedNote[]> {
  return invoke<DeletedNote[]>("get_deleted_notes");
}

/**
 * One whole active note. Rejects with kind "invalid" if it doesn't exist, or
 * kind "stale" if it has been deleted.
 */
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

/**
 * Soft-deletes a note: it leaves the library but its text is kept, so it can
 * be restored. No card changes, because no card is linked to a note. Rejects
 * with kind "stale" if it was already deleted, having written nothing.
 */
export function deleteNote(noteId: number): Promise<void> {
  return invoke<void>("delete_note", { noteId });
}

/**
 * Restores a deleted note: it returns to the library exactly as it was.
 * Rejects with kind "stale" if it isn't deleted, having written nothing.
 */
export function restoreNote(noteId: number): Promise<void> {
  return invoke<void>("restore_note", { noteId });
}

/**
 * The models cards can be suggested with: Ollama's local models, by name.
 * Empty if Ollama has none; rejects with kind "failed" if Ollama can't be
 * reached. Ollama's cloud models are never listed.
 */
export function getModels(): Promise<string[]> {
  return invoke<string[]>("get_models");
}

/**
 * Asks `model`, in Ollama on this computer, to suggest cards from a note, or
 * from `part` of one too long to use whole (see `noteParts.ts`). Nothing is
 * saved. Resolves with `{ status: "stopped" }` if `stopSuggesting` is called
 * first; rejects with kind "stale" if the model has left Ollama, or the note
 * was deleted or no longer has that part.
 */
export function suggestCards(
  noteId: number,
  model: string,
  part?: { start: number; end: number }
): Promise<SuggestOutcome> {
  const where = part && { part: { start: part.start, end: part.end } };
  return invoke<SuggestOutcome>("suggest_cards", { noteId, model, ...where });
}

/** Stops the suggestion being worked on, if any, which frees up the model. */
export function stopSuggesting(): Promise<void> {
  return invoke<void>("stop_suggesting");
}

/** Mirrors `HelpKind` in `src-tauri/src/aid.rs`. */
export type HelpKind = "mnemonic" | "explanation" | "analogy" | "question";

/** Mirrors `CardHelp` in `src-tauri/src/aid.rs`. */
export type CardHelp = {
  kind: HelpKind;
  text: string;
  /** Words (lowercase) the card never uses; only for an explanation or a question. */
  unfamiliar: string[];
};

/** Mirrors `HelpOutcome` in `src-tauri/src/commands.rs`. */
export type HelpOutcome = { status: "written"; help: CardHelp } | { status: "stopped" };

/**
 * Asks `model`, in Ollama on this computer, for `kind` of help with a card,
 * written from its front and back only. Nothing is saved. Resolves with
 * `{ status: "stopped" }` if `stopCardHelp` is called first; rejects with kind
 * "stale" if the card was deleted or the model has left Ollama.
 */
export function getCardHelp(cardId: number, model: string, kind: HelpKind): Promise<HelpOutcome> {
  return invoke<HelpOutcome>("get_card_help", { cardId, model, kind });
}

/** Stops the card help being written, if any, which frees up the model. */
export function stopCardHelp(): Promise<void> {
  return invoke<void>("stop_card_help");
}

/**
 * Mirrors `QuizQuestion` in `src-tauri/src/quiz.rs`: one multiple-choice
 * question, with its correct answer, three wrong ones, the passage of the
 * note the answer comes from, and answer words the note never uses.
 */
export type QuizQuestion = {
  question: string;
  answer: string;
  distractors: string[];
  source: string;
  unfamiliar: string[];
};

/** Mirrors `QuizOutcome` in `src-tauri/src/commands.rs`. Nothing has been saved. */
export type QuizOutcome =
  | { status: "written"; questions: QuizQuestion[]; leftOut: number; unsupported: number }
  | { status: "stopped" };

/**
 * Asks `model`, in Ollama on this computer, for a multiple-choice quiz on a
 * note, or on `part` of one too long to use whole (see `noteParts.ts`).
 * Nothing is saved. Resolves with `{ status: "stopped" }` if `stopQuiz` is
 * called first; rejects with kind "stale" if the model has left Ollama, or
 * the note was deleted or no longer has that part.
 */
export function quizNote(
  noteId: number,
  model: string,
  part?: { start: number; end: number }
): Promise<QuizOutcome> {
  const where = part && { part: { start: part.start, end: part.end } };
  return invoke<QuizOutcome>("quiz_note", { noteId, model, ...where });
}

/** Stops the quiz being written, if any, which frees up the model. */
export function stopQuiz(): Promise<void> {
  return invoke<void>("stop_quiz");
}

/**
 * Mirrors `Settings` in `src-tauri/src/optimizer.rs`: the FSRS parameters
 * reviews are scheduled with, and how many reviews an optimization could learn
 * from. `version` changes with every apply and reset.
 */
export type SchedulingSettings = {
  version: number;
  /** Parameters fitted to the user's history are in use (else FSRS's defaults). */
  personal: boolean;
  /** The 21 parameters in use. */
  parameters: number[];
  defaults: number[];
  /** How many reviews the parameters in use were fitted to. */
  reviewsUsed: number | null;
  /** When they were applied, or the defaults restored (ISO-8601 UTC). */
  appliedAt: string | null;
  /** Ratings made at least a day after the card's previous one. */
  usableReviews: number;
  /** Optimizing is refused below this many usable reviews. */
  minimumReviews: number;
};

/** Mirrors `IntervalCase` in `src-tauri/src/optimizer.rs`. */
export type IntervalCase = "firstAgain" | "firstHard" | "firstGood" | "firstEasy" | "secondGood";

/** Mirrors `IntervalChange` in `src-tauri/src/optimizer.rs`: intervals in days. */
export type IntervalChange = { case: IntervalCase; current: number; proposed: number };

/**
 * Mirrors `OptimizeOutcome` in `src-tauri/src/commands.rs`. Nothing is saved
 * until `applyParameters`. The errors are how far predicted recall is from
 * actual recall, on average (0 to 1, lower is better). Only a result that is
 * `better` can be applied.
 */
export type OptimizeOutcome =
  | {
      status: "optimized";
      token: number;
      reviewsUsed: number;
      current: number[];
      proposed: number[];
      currentError: number;
      proposedError: number;
      better: boolean;
      intervals: IntervalChange[];
    }
  | { status: "stopped" };

/** The scheduling settings. */
export function getScheduling(): Promise<SchedulingSettings> {
  return invoke<SchedulingSettings>("get_scheduling");
}

/**
 * Fits FSRS to the whole review history and compares the result with the
 * parameters in use. Nothing is saved. Resolves with `{ status: "stopped" }` if
 * `stopOptimizing` is called first; rejects with kind "invalid" below the
 * minimum number of reviews, or "failed" if it took too long.
 */
export function optimizeParameters(): Promise<OptimizeOutcome> {
  return invoke<OptimizeOutcome>("optimize_parameters");
}

/** Stops the optimization running now, if any. */
export function stopOptimizing(): Promise<void> {
  return invoke<void>("stop_optimizing");
}

/**
 * Schedules every rating from now on with the parameters of the optimization
 * held under `token`. Rejects with kind "stale" if it's no longer waiting or
 * the parameters in use changed since.
 */
export function applyParameters(token: number): Promise<SchedulingSettings> {
  return invoke<SchedulingSettings>("apply_parameters", { token });
}

/**
 * Goes back to FSRS's defaults for every rating from now on. `version` is the
 * settings' version as shown; rejects with kind "stale" if they changed since.
 */
export function resetParameters(version: number): Promise<SchedulingSettings> {
  return invoke<SchedulingSettings>("reset_parameters", { version });
}

/**
 * Until the returned function is called, closing the window doesn't close it:
 * `onRequest` runs instead, and the window stays open until `closeWindow`.
 * With no hold in place, the window closes as usual.
 */
export function holdWindowClose(onRequest: () => void): () => void {
  let unlisten: (() => void) | null = null;
  let released = false;
  getCurrentWindow()
    .onCloseRequested((event) => {
      event.preventDefault();
      onRequest();
    })
    .then((stop) => {
      // Released before the listener was in place: remove it straight away.
      if (released) stop();
      else unlisten = stop;
    });
  return () => {
    released = true;
    unlisten?.();
  };
}

/** Closes the window for good, even while `holdWindowClose` is holding it. */
export function closeWindow(): Promise<void> {
  return getCurrentWindow().destroy();
}

/** Normalizes anything a command rejected with into a `CommandError`. */
export function toCommandError(err: unknown): CommandError {
  if (typeof err === "object" && err !== null && "kind" in err && "message" in err) {
    return err as CommandError;
  }
  return { kind: "failed", message: "Something went wrong." };
}
