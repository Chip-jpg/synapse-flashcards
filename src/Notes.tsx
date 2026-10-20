import { useEffect, useMemo, useRef, useState } from "react";
import {
  createFlashcard,
  createNote,
  createNoteFromSource,
  deleteNote,
  getDeletedNotes,
  getNote,
  getNoteSource,
  getNotes,
  getSource,
  importSource,
  restoreNote,
  searchNotes,
  toCommandError,
  updateNote,
  NOTE_PICTURES_MAX,
  type CommandError,
  type Deck,
  type DeckDetail,
  type DeletedNote,
  type Asset,
  type Note,
  type NoteSummary,
} from "./api";
import { loadDeckChoices, type DeckChoices } from "./deckChoices";
import { MarkdownView } from "./NoteMarkdown";
import { hasFormatting, parseMarkdown } from "./markdown";
import { FormActions, SelectField, TextField, useFormSave } from "./forms";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  NoteIcon,
  PencilIcon,
  PlusIcon,
  RestoreIcon,
  SparkleIcon,
  TrashIcon,
  UploadIcon,
} from "./icons";
import { Quiz } from "./Quiz";
import { SuggestCards } from "./SuggestCards";
import { SUGGESTION_NOTE_MAX_CHARS } from "./noteParts";
import { draftFromImport, type SourceNoteDraft } from "./Sources";
import { formatDateTime, Message, useFocusOnMount } from "./ui";
import { useUnsavedChanges } from "./unsaved";
import { PictureField, PictureGallery, samePictures, usePictureAdder } from "./pictures";

/** What takes focus when the library appears. */
type LibraryFocus =
  | { to: "screen" }
  | { to: "new" }
  | { to: "import" }
  | { to: "note"; noteId: number }
  | { to: "notice" };

/** Where in the notes area the user is. */
type Place =
  | { screen: "library"; focus: LibraryFocus; notice: string | null }
  | { screen: "new"; imported?: SourceNoteDraft }
  | { screen: "note"; noteId: number; justCreated: boolean };

/**
 * The notes area: the library of the user's typed notes, writing a new one or
 * importing one from a file, opening one to read or edit it, and writing a
 * card from it. Every screen change follows a button press, so each new
 * screen takes focus.
 */
export function NotesArea({
  onBack,
  onOpenDeck,
  onOpenSource,
  openNote,
  newFromSource,
}: {
  onBack: () => void;
  /** Leaves notes for the deck screen, e.g. to see a card just written. */
  onOpenDeck: (deckId: number) => void;
  /** Leaves notes for the source a note was written from. */
  onOpenSource: (sourceId: number) => void;
  /**
   * Opens straight at one note, e.g. from a search, and goes back there
   * instead of to the library.
   */
  openNote?: { noteId: number; backLabel: string; onBack: () => void };
  /**
   * Opens straight at a new note written from a source, and goes back there
   * if it's cancelled.
   */
  newFromSource?: { draft: SourceNoteDraft; onCancel: () => void };
}) {
  const [place, setPlace] = useState<Place>(
    newFromSource
      ? { screen: "new", imported: newFromSource.draft }
      : openNote
        ? { screen: "note", noteId: openNote.noteId, justCreated: false }
        : { screen: "library", focus: { to: "screen" }, notice: null },
  );

  if (place.screen === "new") {
    const imported = place.imported;
    return (
      <NoteForm
        imported={imported}
        onCancel={() =>
          newFromSource && imported === newFromSource.draft
            ? newFromSource.onCancel()
            : setPlace({
                screen: "library",
                focus: { to: imported ? "import" : "new" },
                notice: null,
              })
        }
        onSaved={(note) => setPlace({ screen: "note", noteId: note.id, justCreated: true })}
      />
    );
  }

  if (place.screen === "note") {
    const noteId = place.noteId;
    return (
      <NoteScreen
        key={noteId}
        noteId={noteId}
        justCreated={place.justCreated}
        backLabel={openNote?.noteId === noteId ? openNote.backLabel : "Back to notes"}
        onBack={() =>
          openNote?.noteId === noteId
            ? openNote.onBack()
            : setPlace({ screen: "library", focus: { to: "note", noteId }, notice: null })
        }
        // The note is gone from the library, so the screen it was opened from
        // takes over and says what happened.
        onDeleted={() =>
          setPlace({
            screen: "library",
            focus: { to: "notice" },
            notice: "Note deleted. It's kept in Deleted notes, where you can restore it.",
          })
        }
        onOpenDeck={onOpenDeck}
        onOpenSource={onOpenSource}
      />
    );
  }

  return (
    <NoteLibrary
      focus={place.focus}
      notice={place.notice}
      onNew={() => setPlace({ screen: "new" })}
      onImported={(imported) => setPlace({ screen: "new", imported })}
      onOpen={(noteId) => setPlace({ screen: "note", noteId, justCreated: false })}
      onBack={onBack}
    />
  );
}

type LibraryState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; notes: NoteSummary[]; deleted: DeletedNote[] };

/**
 * How the library appears next. `key` changes every time, so the list
 * remounts and its focus target takes effect (as after restoring a note).
 */
type LibraryView = { key: number; notice: string | null; focus: LibraryFocus };

/** Fetches the library and its deleted-notes history; never rejects. */
async function loadNotes(): Promise<LibraryState> {
  try {
    const [notes, deleted] = await Promise.all([getNotes(), getDeletedNotes()]);
    return { status: "ready", notes, deleted };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

function NoteLibrary({
  focus,
  notice,
  onNew,
  onImported,
  onOpen,
  onBack,
}: {
  focus: LibraryFocus;
  /** A confirmation to announce when the library appears (e.g. after deleting). */
  notice: string | null;
  onNew: () => void;
  /** A file was kept as a source and its text read, to become a new note. */
  onImported: (imported: SourceNoteDraft) => void;
  onOpen: (noteId: number) => void;
  onBack: () => void;
}) {
  const [state, setState] = useState<LibraryState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or after a restore).
  const [attempt, setAttempt] = useState(0);
  const [view, setView] = useState<LibraryView>({ key: 0, notice, focus });

  useEffect(() => {
    let current = true;
    loadNotes().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  /**
   * Reloads both lists, announcing `notice` and giving it focus if there is
   * one. Goes back to "loading" first: until the fresh lists arrive the old
   * ones are wrong, and showing a just-restored note under *Deleted notes*
   * with a live Restore button would invite a second, doomed restore.
   */
  function reload(notice: string | null) {
    setState({ status: "loading" });
    setView((previous) => ({
      key: previous.key + 1,
      notice,
      focus: notice === null ? { to: "screen" } : { to: "notice" },
    }));
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading your notes…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <div className="actions">
          <button type="button" className="button" onClick={() => reload(null)}>
            Retry
          </button>
          <button type="button" className="button" onClick={onBack}>
            Back to Study Desk
          </button>
        </div>
      </Message>
    );
  }

  return (
    <NoteList
      key={view.key}
      notes={state.notes}
      deleted={state.deleted}
      notice={view.notice}
      focus={view.focus}
      onNew={onNew}
      onImported={onImported}
      onOpen={onOpen}
      onBack={onBack}
      onRestored={(title) => reload(`${title} restored. It's back in your notes.`)}
      onStale={() =>
        // Restoring failed because the note isn't deleted after all. Say so,
        // rather than reloading silently and leaving the press unexplained.
        reload("That note wasn't deleted after all. Here's your library as it is now.")
      }
    />
  );
}

function NoteList({
  notes,
  deleted,
  notice,
  focus,
  onNew,
  onImported,
  onOpen,
  onBack,
  onRestored,
  onStale,
}: {
  notes: NoteSummary[];
  deleted: DeletedNote[];
  notice: string | null;
  focus: LibraryFocus;
  onNew: () => void;
  onImported: (imported: SourceNoteDraft) => void;
  onOpen: (noteId: number) => void;
  onBack: () => void;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  // A note that's gone from the list (it was deleted) falls back to the
  // screen, and so does a notice that isn't there.
  const focusNoteId =
    focus.to === "note" && notes.some((note) => note.id === focus.noteId) ? focus.noteId : null;
  const focusNotice = focus.to === "notice" && notice !== null;
  const screenRef = useFocusOnMount<HTMLElement>(
    focus.to === "screen" ||
      (focus.to === "note" && focusNoteId === null) ||
      (focus.to === "notice" && !focusNotice)
  );
  const newRef = useFocusOnMount<HTMLButtonElement>(focus.to === "new");
  const importRef = useFocusOnMount<HTMLButtonElement>(focus.to === "import");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focusNotice);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  // Blocks a second file window immediately, before the re-render lands.
  const importingRef = useRef(false);
  const search = useNoteSearch();
  const searching = search.query.trim() !== "";
  // While a new answer is on its way, the last one stays up, so the list
  // doesn't flash empty between keystrokes.
  const shown = searching ? (search.found?.notes ?? notes) : notes;

  /**
   * Chooses a file, keeps it as a source and reads it; its text opens as a
   * new note to check.
   */
  async function chooseFile() {
    if (importingRef.current) return;
    importingRef.current = true;
    setImporting(true);
    setImportError(null);

    try {
      const result = await importSource();
      if (result.status === "imported") {
        onImported(draftFromImport(result));
        return; // This screen is replaced.
      }
    } catch (err) {
      setImportError(toCommandError(err).message);
    }

    importingRef.current = false;
    setImporting(false);
  }

  return (
    <section ref={screenRef} className="notes" tabIndex={-1} aria-labelledby="notes-heading">
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        Back to Study Desk
      </button>

      <div className="page-header">
        <div className="page-heading">
          <h2 id="notes-heading" className="page-title">
            Notes
          </h2>
          <p className="page-intro">
            Your own typed study notes, kept on this computer. The most recently edited note comes
            first.
          </p>
        </div>
        <div className="deck-actions page-actions">
          <button
            ref={importRef}
            type="button"
            className="button"
            aria-disabled={importing}
            aria-describedby="import-hint"
            onClick={chooseFile}
          >
            <UploadIcon />
            Import file
          </button>
          <button ref={newRef} type="button" className="button button-primary" onClick={onNew}>
            <PlusIcon />
            New note
          </button>
        </div>
      </div>
      <p id="import-hint" className="field-hint">
        Import a PDF, text, Markdown, Word, or PowerPoint file to make a note from its text. You
        check the text before it's saved. The file itself is kept under Sources, so cards and
        quizzes from the note can show the page they came from.
      </p>
      <p className="form-status" role="status">
        {importing ? "Choose a file. Synapse then reads its text…" : ""}
      </p>
      {importError && (
        <p className="message-error" role="alert">
          {importError}
        </p>
      )}
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {notes.length > 0 && <NoteSearchField search={search} />}

      {notes.length === 0 ? (
        <p className="empty" role="status">
          {deleted.length === 0
            ? "No notes yet. Choose New note to write one."
            : "No notes in your library. Choose New note to write one, or restore a deleted note below."}
        </p>
      ) : searching && search.found?.query === search.query.trim() && shown.length === 0 ? (
        <p className="empty">
          No notes match “{search.found.query}”. Try other words, or clear the search to see every
          note.
        </p>
      ) : (
        <ul className="card-list">
          {shown.map((note) => (
            <li key={note.id}>
              <NoteItem
                note={note}
                focusOpen={note.id === focusNoteId}
                onOpen={() => onOpen(note.id)}
              />
            </li>
          ))}
        </ul>
      )}

      {deleted.length > 0 && (
        <DeletedNoteList notes={deleted} onRestored={onRestored} onStale={onStale} />
      )}
    </section>
  );
}

/** The last search Rust answered: the trimmed query and the notes it found. */
type FoundNotes = { query: string; notes: NoteSummary[] };

type NoteSearch = {
  /** What's in the field, as typed. */
  query: string;
  found: FoundNotes | null;
  error: string | null;
  setQuery: (query: string) => void;
};

/**
 * Searching the library. Rust does the matching, since the library list has
 * no bodies. Each change asks again; an answer to an older query is dropped,
 * so a slow reply can't overwrite a newer one. This is a filter, not a form:
 * nothing is saved, so it never counts as an unsaved change.
 */
function useNoteSearch(): NoteSearch {
  const [query, setQueryState] = useState("");
  const [found, setFound] = useState<FoundNotes | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Numbers each request, so only the latest one's answer is shown.
  const latest = useRef(0);

  useEffect(
    () => () => {
      latest.current += 1; // Unmounted: ignore any answer still on its way.
    },
    [],
  );

  function setQuery(next: string) {
    setQueryState(next);
    setError(null);
    const request = ++latest.current;
    const trimmed = next.trim();
    if (trimmed === "") {
      setFound(null);
      return;
    }
    searchNotes(trimmed).then(
      (notes) => {
        if (request === latest.current) setFound({ query: trimmed, notes });
      },
      (err) => {
        if (request === latest.current) setError(toCommandError(err).message);
      },
    );
  }

  return { query, found, error, setQuery };
}

function NoteSearchField({ search }: { search: NoteSearch }) {
  const { query, found, error, setQuery } = search;
  const trimmed = query.trim();
  const inputRef = useRef<HTMLInputElement>(null);

  let status = "";
  if (trimmed !== "" && error === null) {
    if (found?.query !== trimmed) status = "Searching…";
    else if (found.notes.length === 0) status = "No notes match.";
    else
      status = `${counted(found.notes.length, "note")} ${found.notes.length === 1 ? "matches" : "match"}.`;
  }

  return (
    <div className="field note-search">
      <label htmlFor="note-search" className="field-label">
        Search notes
      </label>
      <div className="note-search-row">
        <input
          ref={inputRef}
          id="note-search"
          type="search"
          className="field-input"
          value={query}
          autoComplete="off"
          spellCheck={false}
          aria-describedby="note-search-hint"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape" && query !== "") {
              event.preventDefault();
              setQuery("");
            }
          }}
        />
        {query !== "" && (
          <button
            type="button"
            className="button"
            onClick={() => {
              setQuery("");
              inputRef.current?.focus();
            }}
          >
            Clear
          </button>
        )}
      </div>
      <p id="note-search-hint" className="field-hint">
        Finds notes whose title or text contains what you type, ignoring capitals. Deleted notes
        aren't searched.
      </p>
      <p className="form-status" role="status">
        {status}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/**
 * Deleted notes: history, and the only place they appear. They can't be
 * opened or edited, so the body isn't shown; Restore puts one back in the
 * library exactly as it was.
 */
function DeletedNoteList({
  notes,
  onRestored,
  onStale,
}: {
  notes: DeletedNote[];
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  return (
    <section className="cards section-quiet" aria-labelledby="deleted-notes-heading">
      <div className="section-heading">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          <RestoreIcon />
        </span>
        <h3 id="deleted-notes-heading" className="section-title section-title-quiet">
          Deleted notes
        </h3>
      </div>
      <p className="field-hint">
        Kept for your history. A deleted note isn't in your library and can't be opened or edited,
        but restoring it brings it back with its text unchanged. Cards you wrote from a note are
        never affected.
      </p>
      <ul className="card-list">
        {notes.map((note) => (
          <li key={note.id}>
            <DeletedNoteItem note={note} onRestored={onRestored} onStale={onStale} />
          </li>
        ))}
      </ul>
    </section>
  );
}

/** One deleted note, with the one action it has: Restore. */
function DeletedNoteItem({
  note,
  onRestored,
  onStale,
}: {
  note: DeletedNote;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second restore immediately, before the re-render lands.
  const restoringRef = useRef(false);
  const titleId = `deleted-note-${note.id}-title`;

  async function restore() {
    if (restoringRef.current) return;
    restoringRef.current = true;
    setRestoring(true);
    setError(null);

    try {
      await restoreNote(note.id);
      onRestored(note.title);
      return; // The list is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // Not deleted after all: reload to show what's really there.
        onStale();
        return;
      }
      setError(failure.message);
    }

    restoringRef.current = false;
    setRestoring(false);
  }

  return (
    <div className="card-item card-item-recovery">
      <div>
        {/* A heading, like an active note's title, so screen-reader users can
            reach deleted notes by heading navigation. */}
        <h4 id={titleId} className="note-item-title">
          {note.title}
        </h4>
        <p className="item-meta">
          {`Deleted ${formatDateTime(note.deletedAt)} · last edited ${formatDateTime(
            note.updatedAt
          )}`}
        </p>
      </div>
      <div className="deck-actions item-actions">
        <button
          type="button"
          className="button button-small"
          aria-describedby={titleId}
          aria-disabled={restoring}
          onClick={restore}
        >
          <RestoreIcon />
          Restore note
        </button>
      </div>
      <p className="form-status" role="status">
        {restoring ? "Restoring…" : ""}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function NoteItem({
  note,
  focusOpen,
  onOpen,
}: {
  note: NoteSummary;
  focusOpen: boolean;
  onOpen: () => void;
}) {
  const openRef = useFocusOnMount<HTMLButtonElement>(focusOpen);
  const titleId = `note-${note.id}-title`;

  return (
    <article className="card-item note-item" aria-labelledby={titleId}>
      <div className="note-item-main">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          <NoteIcon />
        </span>
        <div>
          {/* A heading, so screen-reader users can move between notes by heading. */}
          <h3 id={titleId} className="note-item-title">
            {note.title}
          </h3>
          <p className="item-meta">
            {note.updatedAt === note.createdAt
              ? `Created ${formatDateTime(note.createdAt)}`
              : `Edited ${formatDateTime(note.updatedAt)}`}
          </p>
        </div>
      </div>
      <button
        ref={openRef}
        type="button"
        className="button button-small"
        aria-describedby={titleId}
        onClick={onOpen}
      >
        Open note
        <ArrowRightIcon />
      </button>
    </article>
  );
}

type NoteState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; note: Note };

/** What takes focus when the note view appears. */
type NoteFocus = "screen" | "notice" | "edit" | "card" | "suggest" | "quiz";

/**
 * How the note view appears next. `key` changes every time, so the view
 * remounts and its focus target takes effect. `cardDeck` is the deck cards
 * were just added to, offered as a way on.
 */
type NoteViewState = {
  key: number;
  notice: string | null;
  focus: NoteFocus;
  cardDeck: { id: number; name: string } | null;
};

/** Fetches one note and describes the outcome; never rejects. */
async function loadNote(noteId: number): Promise<NoteState> {
  try {
    return { status: "ready", note: await getNote(noteId) };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * One note: reading it, editing it in place, writing a card from it, and
 * having a local model suggest cards from it or quiz the user on it.
 */
function NoteScreen({
  noteId,
  justCreated,
  backLabel,
  onBack,
  onDeleted,
  onOpenDeck,
  onOpenSource,
}: {
  noteId: number;
  justCreated: boolean;
  backLabel: string;
  onBack: () => void;
  /** The note was deleted, so this screen can't show it any more. */
  onDeleted: () => void;
  onOpenDeck: (deckId: number) => void;
  onOpenSource: (sourceId: number) => void;
}) {
  const [state, setState] = useState<NoteState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  // The note itself, or a form in its place.
  const [mode, setMode] = useState<"view" | "edit" | "card" | "suggest" | "quiz">("view");
  const [view, setView] = useState<NoteViewState>({
    key: 0,
    notice: justCreated ? "Note saved." : null,
    focus: justCreated ? "notice" : "screen",
    cardDeck: null,
  });

  useEffect(() => {
    let current = true;
    loadNote(noteId).then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [noteId, attempt]);

  function showView(
    notice: string | null,
    focus: NoteFocus,
    cardDeck: NoteViewState["cardDeck"] = null
  ) {
    setMode("view");
    setView((previous) => ({ key: previous.key + 1, notice, focus, cardDeck }));
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading the note…
      </p>
    );
  }

  // Every note screen follows a button press, so errors take focus.
  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <div className="actions">
          <button
            type="button"
            className="button"
            onClick={() => {
              setState({ status: "loading" });
              setAttempt((n) => n + 1);
            }}
          >
            Retry
          </button>
          <button type="button" className="button" onClick={onBack}>
            {backLabel}
          </button>
        </div>
      </Message>
    );
  }

  if (mode === "edit") {
    return (
      <NoteForm
        note={state.note}
        onCancel={() => showView(null, "edit")}
        onSaved={(note) => {
          setState({ status: "ready", note });
          showView("Note updated.", "notice");
        }}
      />
    );
  }

  if (mode === "card") {
    return (
      <CardFromNote
        note={state.note}
        onCancel={() => showView(null, "card")}
        onCreated={(deck) =>
          showView(`Card added to ${deck.name}. It's due for review now.`, "notice", {
            id: deck.id,
            name: deck.name,
          })
        }
      />
    );
  }

  if (mode === "suggest") {
    return (
      <SuggestCards
        note={state.note}
        onCancel={() => showView(null, "suggest")}
        onSaved={(deck, count) =>
          showView(
            count === 1
              ? `1 card added to ${deck.name}. It's due for review now.`
              : `${count} cards added to ${deck.name}. They're due for review now.`,
            "notice",
            { id: deck.id, name: deck.name }
          )
        }
      />
    );
  }

  if (mode === "quiz") {
    return (
      <Quiz
        note={state.note}
        onCancel={() => showView(null, "quiz")}
        onSaved={(deck, count) =>
          showView(
            count === 1
              ? `1 card added to ${deck.name}. It's due for review now.`
              : `${count} cards added to ${deck.name}. They're due for review now.`,
            "notice",
            { id: deck.id, name: deck.name }
          )
        }
      />
    );
  }

  return (
    <NoteView
      key={view.key}
      note={state.note}
      notice={view.notice}
      focus={view.focus}
      cardDeck={view.cardDeck}
      onEdit={() => setMode("edit")}
      onCreateCard={() => setMode("card")}
      onSuggest={() => setMode("suggest")}
      onQuiz={() => setMode("quiz")}
      onDeleted={onDeleted}
      onOpenDeck={onOpenDeck}
      onOpenSource={onOpenSource}
      backLabel={backLabel}
      onBack={onBack}
    />
  );
}

function NoteView({
  note,
  notice,
  focus,
  cardDeck,
  onEdit,
  onCreateCard,
  onSuggest,
  onQuiz,
  onDeleted,
  onOpenDeck,
  onOpenSource,
  backLabel,
  onBack,
}: {
  note: Note;
  notice: string | null;
  focus: NoteFocus;
  cardDeck: NoteViewState["cardDeck"];
  onEdit: () => void;
  onCreateCard: () => void;
  onSuggest: () => void;
  onQuiz: () => void;
  onDeleted: () => void;
  onOpenDeck: (deckId: number) => void;
  onOpenSource: (sourceId: number) => void;
  backLabel: string;
  onBack: () => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(focus === "screen");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus === "notice");
  const editRef = useFocusOnMount<HTMLButtonElement>(focus === "edit");
  const cardRef = useFocusOnMount<HTMLButtonElement>(focus === "card");
  const suggestRef = useFocusOnMount<HTMLButtonElement>(focus === "suggest");
  const quizRef = useFocusOnMount<HTMLButtonElement>(focus === "quiz");
  const edited = note.updatedAt !== note.createdAt;
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  // Blocks a second delete immediately, before the re-render lands.
  const deletingRef = useRef(false);
  const deleteRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);

  // As on the deck screen: focus moves to the question when it opens, so it
  // is read out and a repeated Enter can't delete, and back to "Delete note"
  // if the note is kept.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      deleteRef.current?.focus();
    }
  }, [confirming]);

  async function confirmDelete() {
    if (deletingRef.current) return;
    deletingRef.current = true;
    setDeleting(true);
    setDeleteError(null);

    try {
      await deleteNote(note.id);
      onDeleted();
      return; // This screen is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // Already deleted: the note is gone from this screen either way.
        onDeleted();
        return;
      }
      setDeleteError(failure.message);
    }

    deletingRef.current = false;
    setDeleting(false);
  }

  return (
    <section
      ref={screenRef}
      className="deck-detail note-detail"
      tabIndex={-1}
      aria-labelledby="note-heading"
    >
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        {backLabel}
      </button>

      <article className="card">
        <div>
          <h2 id="note-heading" className="page-title">
            {note.title}
          </h2>
          <p className="item-meta">
            {`Created ${formatDateTime(note.createdAt)}`}
            {edited && ` · Edited ${formatDateTime(note.updatedAt)}`}
          </p>
          <NoteSourceLine noteId={note.id} onOpenSource={onOpenSource} />
        </div>

        {notice && (
          <p ref={noticeRef} id="note-notice" className="notice" role="status" tabIndex={-1}>
            {notice}
          </p>
        )}
        {cardDeck && (
          <div className="deck-actions">
            <button
              type="button"
              className="button button-small"
              aria-describedby="note-notice"
              onClick={() => onOpenDeck(cardDeck.id)}
            >
              {`Open ${cardDeck.name}`}
              <ArrowRightIcon />
            </button>
          </div>
        )}

        {/* The note's actions sit above its text, so they stay in reach
            however long the note is. */}
        {confirming ? (
          <div className="confirm">
            <p ref={questionRef} className="confirm-question" tabIndex={-1}>
              Delete this note? It will leave your notes and can't be opened or edited. Its text is
              kept under Deleted notes, where you can restore it. Cards you wrote from it aren't
              changed.
            </p>
            <div className="deck-actions">
              <button
                type="button"
                className="button button-danger-soft button-small"
                aria-describedby="note-heading"
                aria-disabled={deleting}
                onClick={confirmDelete}
              >
                Delete note
              </button>
              <button
                type="button"
                className="button button-small"
                aria-disabled={deleting}
                onClick={() => {
                  if (!deleting) setConfirming(false);
                }}
              >
                Keep note
              </button>
            </div>
            <p className="form-status" role="status">
              {deleting ? "Deleting…" : ""}
            </p>
          </div>
        ) : (
          <div className="deck-actions note-toolbar">
            <button
              ref={editRef}
              type="button"
              className="button"
              aria-describedby="note-heading"
              onClick={onEdit}
            >
              <PencilIcon />
              Edit note
            </button>
            <button
              ref={cardRef}
              type="button"
              className="button button-primary"
              aria-describedby="note-heading"
              onClick={onCreateCard}
            >
              <PlusIcon />
              Create card from note
            </button>
            <button
              ref={suggestRef}
              type="button"
              className="button button-tonal"
              aria-describedby="note-heading"
              onClick={onSuggest}
            >
              <SparkleIcon />
              Suggest cards
            </button>
            <button
              ref={quizRef}
              type="button"
              className="button button-tonal"
              aria-describedby="note-heading"
              onClick={onQuiz}
            >
              <SparkleIcon />
              Quiz me
            </button>
            <button
              ref={deleteRef}
              type="button"
              className="button button-quiet"
              aria-describedby="note-heading"
              onClick={() => setConfirming(true)}
            >
              <TrashIcon />
              Delete note
            </button>
          </div>
        )}

        {deleteError && (
          <p className="message-error" role="alert">
            {deleteError}
          </p>
        )}

        <NoteReading body={note.body} />
        <PictureGallery
          pictures={note.pictures}
          label="Picture in the note"
          listLabel="Pictures in the note"
        />
      </article>
    </section>
  );
}

/**
 * Which source a note was written from, with the way to it: nothing for a
 * typed note, or while Rust looks, and a word about a source since deleted.
 */
function NoteSourceLine({
  noteId,
  onOpenSource,
}: {
  noteId: number;
  onOpenSource: (sourceId: number) => void;
}) {
  const [found, setFound] = useState<
    { status: "active"; sourceId: number; title: string } | { status: "deleted" } | null
  >(null);

  useEffect(() => {
    let current = true;
    (async () => {
      const sourceId = await getNoteSource(noteId);
      if (sourceId === null) return null;
      try {
        const source = await getSource(sourceId);
        return { status: "active" as const, sourceId, title: source.title };
      } catch (err) {
        if (toCommandError(err).kind === "stale") return { status: "deleted" as const };
        throw err;
      }
    })().then(
      (next) => {
        if (current) setFound(next);
      },
      () => {}
    );
    return () => {
      current = false;
    };
  }, [noteId]);

  if (found === null) return null;
  if (found.status === "deleted") {
    return (
      <p className="item-meta note-source">
        Written from a source that's been deleted. Restore it under Sources to see its pages again.
      </p>
    );
  }
  return (
    <p className="item-meta note-source">
      {`Written from ${found.title}. `}
      <button
        type="button"
        className="button-link"
        onClick={() => onOpenSource(found.sourceId)}
      >
        Open source
      </button>
    </p>
  );
}

/** Where the way of showing notes last chosen is remembered, in this computer's web storage. */
const NOTE_FORMAT_KEY = "synapse.noteFormat";

// Remembering the choice is only a convenience: without storage, notes are
// shown formatted.
function rememberedFormatted(): boolean {
  try {
    return localStorage.getItem(NOTE_FORMAT_KEY) !== "plain";
  } catch {
    return true;
  }
}

function rememberFormatted(formatted: boolean) {
  try {
    localStorage.setItem(NOTE_FORMAT_KEY, formatted ? "formatted" : "plain");
  } catch {
    // As above.
  }
}

/**
 * A note's text, formatted as Markdown or as the plain text it's stored as.
 * The choice is only offered when formatting would change something; a note
 * with no Markdown in it is shown as plain text, as it was typed.
 */
function NoteReading({ body }: { body: string }) {
  // Parsed once, both to decide whether to offer formatting and to show it.
  const blocks = useMemo(() => parseMarkdown(body), [body]);
  const formattable = useMemo(() => hasFormatting(blocks), [blocks]);
  const [formatted, setFormatted] = useState(rememberedFormatted);

  if (!formattable) {
    return (
      <div className="note-reading">
        <p className="note-body">{body}</p>
      </div>
    );
  }

  function show(next: boolean) {
    setFormatted(next);
    rememberFormatted(next);
  }

  return (
    <div className="note-reading">
      <div className="note-view-switch" role="group" aria-label="Show the note">
        <button
          type="button"
          className="button button-small"
          aria-pressed={formatted}
          onClick={() => show(true)}
        >
          Formatted
        </button>
        <button
          type="button"
          className="button button-small"
          aria-pressed={!formatted}
          onClick={() => show(false)}
        >
          Plain text
        </button>
      </div>
      {formatted ? <MarkdownView blocks={blocks} /> : <p className="note-body">{body}</p>}
    </div>
  );
}

/**
 * Writing a new note, editing `note` when one is given, or checking the text
 * of an `imported` file before it's saved as a new note. All validation
 * happens in Rust.
 */
function NoteForm({
  note,
  imported,
  onCancel,
  onSaved,
}: {
  note?: Note;
  imported?: SourceNoteDraft;
  onCancel: () => void;
  onSaved: (note: Note) => void;
}) {
  const [title, setTitle] = useState(imported?.title ?? note?.title ?? "");
  const [body, setBody] = useState(imported?.body ?? note?.body ?? "");
  const savedPictures = note?.pictures ?? [];
  const [pictures, setPictures] = useState<Asset[]>(savedPictures);
  // Compared with what's saved: imported text isn't saved anywhere yet, so it
  // counts as unsaved from the start.
  useUnsavedChanges(
    title !== (note?.title ?? "") ||
      body !== (note?.body ?? "") ||
      !samePictures(pictures, savedPictures)
  );
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => {
      const ids = pictures.map((picture) => picture.id);
      return note
        ? updateNote(note.id, title, body, ids)
        : imported
          ? createNoteFromSource(imported.sourceId, title, body, ids)
          : createNote(title, body, ids);
    },
    onSaved,
    { title: "note-title", body: "note-body", pictures: "note-pictures" }
  );
  const changePictures = (next: Asset[]) => {
    setPictures(next);
    edited("pictures");
  };
  const adder = usePictureAdder({
    pictures,
    onChange: changePictures,
    max: NOTE_PICTURES_MAX,
  });

  return (
    <form
      className="card"
      aria-labelledby="note-form-heading"
      noValidate
      onSubmit={submit}
      onPaste={adder.onPaste}
    >
      <h2 id="note-form-heading" className="form-title">
        {note ? "Edit note" : imported ? `New note from ${imported.fileName}` : "New note"}
      </h2>
      {imported && <ImportSummary imported={imported} length={[...body].length} />}

      <TextField
        id="note-title"
        label="Title"
        hint="Required. Up to 200 characters."
        value={title}
        error={fieldError("title")}
        required
        autoFocus={!imported}
        onChange={(value) => {
          setTitle(value);
          edited("title");
        }}
      />
      <TextField
        id="note-body"
        label="Note"
        hint="Plain text; Markdown, such as # headings, **bold** and - lists, is shown formatted when you read the note. Required. Up to 100,000 characters. Line breaks and indentation inside the note are kept; spaces and blank lines at the very start and end are removed."
        value={body}
        error={fieldError("body")}
        required
        multiline
        rows={12}
        onChange={(value) => {
          setBody(value);
          edited("body");
        }}
      />
      <PictureField
        id="note-pictures"
        label="Pictures"
        hint="Optional. Up to 20 PNG, JPEG, GIF or WebP pictures of up to 20 MB each, shown under the note in this order."
        pictures={pictures}
        onChange={changePictures}
        adder={adder}
        error={fieldError("pictures")}
        disabled={saving}
      />

      <FormActions
        saveLabel={note ? "Save changes" : "Save note"}
        saving={saving}
        error={formError}
        onCancel={onCancel}
      />
    </form>
  );
}

/** The most characters a note's text can hold: `NOTE_BODY_MAX_CHARS` in `notes.rs`. */
const NOTE_BODY_MAX_CHARS = 100_000;

/** "1 page", "12 slides". */
function counted(count: number, thing: string): string {
  return count === 1 ? `1 ${thing}` : `${count} ${thing}s`;
}

/** "2", "2 and 7", "2, 5, and 7". */
function listed(numbers: number[]): string {
  if (numbers.length <= 2) return numbers.join(" and ");
  return `${numbers.slice(0, -1).join(", ")}, and ${numbers[numbers.length - 1]}`;
}

/**
 * What was read from an imported file, said before the user checks its text:
 * where it came from, any pages with no text, and whether the text (now
 * `length` characters, as edited) is too long for a note, or to suggest
 * cards from whole. Focus goes to it, so it's read out first.
 */
function ImportSummary({ imported, length }: { imported: SourceNoteDraft; length: number }) {
  const summaryRef = useFocusOnMount<HTMLParagraphElement>(true);
  const parts =
    imported.pages === null
      ? ""
      : ` (${counted(imported.pages, imported.kind === "powerPoint" ? "slide" : "page")})`;
  const missing = imported.pagesWithoutText;

  return (
    <>
      <p ref={summaryRef} className="notice" role="status" tabIndex={-1}>
        {imported.justKept
          ? `Synapse kept ${imported.fileName} under Sources and read its text${parts}. Check it and edit it as you like, then save it as a note. The source itself doesn't change.`
          : `This is the text of ${imported.fileName}${parts}, kept under Sources. Check it and edit it as you like, then save it as a note. The source itself doesn't change.`}
      </p>
      {missing.length > 0 && (
        <p className="import-warning">
          {`${
            missing.length > 12
              ? `${missing.length} pages`
              : `${missing.length === 1 ? "Page" : "Pages"} ${listed(missing)}`
          } had no text Synapse could read. They may be scanned images, so check whether anything is missing.`}
        </p>
      )}
      {length > NOTE_BODY_MAX_CHARS && (
        <p className="import-warning">
          {`This text is ${length.toLocaleString("en-US")} characters long, but a note can hold 100,000. Shorten it before you save it, for example by keeping one chapter.`}
        </p>
      )}
      {length > SUGGESTION_NOTE_MAX_CHARS && length <= NOTE_BODY_MAX_CHARS && (
        <p className="field-hint">
          {`At ${length.toLocaleString("en-US")} characters, this is more than a model reads at once, so Suggest cards will offer the note in parts of up to 12,000 characters.`}
        </p>
      )}
    </>
  );
}

/**
 * Writing an ordinary card while reading a note. Nothing is generated: the
 * user writes the front and back and chooses the deck, and the card is saved
 * by the same Rust command as on the deck screen, with the same rules. The
 * card isn't linked to the note, and the note isn't changed.
 */
function CardFromNote({
  note,
  onCancel,
  onCreated,
}: {
  note: Note;
  onCancel: () => void;
  onCreated: (deck: DeckDetail) => void;
}) {
  const [choices, setChoices] = useState<DeckChoices>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    loadDeckChoices().then((next) => {
      if (current) setChoices(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  if (choices.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading your decks…
      </p>
    );
  }

  if (choices.status === "error") {
    return (
      <Message focus tone="alert" text={choices.message}>
        <div className="actions">
          <button
            type="button"
            className="button"
            onClick={() => {
              setChoices({ status: "loading" });
              setAttempt((n) => n + 1);
            }}
          >
            Retry
          </button>
          <button type="button" className="button" onClick={onCancel}>
            Back to note
          </button>
        </div>
      </Message>
    );
  }

  if (choices.decks.length === 0) {
    return (
      <Message
        focus
        tone="status"
        text="You don't have a deck to add cards to yet. Create one on the Study Desk first; the Sample deck can't take new cards."
      >
        <button type="button" className="button" onClick={onCancel}>
          Back to note
        </button>
      </Message>
    );
  }

  return (
    <CardFromNoteForm
      note={note}
      decks={choices.decks}
      onCancel={onCancel}
      onCreated={onCreated}
      onNoDecks={() => setChoices({ status: "ready", decks: [] })}
    />
  );
}

function CardFromNoteForm({
  note,
  decks: initialDecks,
  onCancel,
  onCreated,
  onNoDecks,
}: {
  note: Note;
  decks: Deck[];
  onCancel: () => void;
  onCreated: (deck: DeckDetail) => void;
  /** Reloading the choices found none left, so there's nothing to save into. */
  onNoDecks: () => void;
}) {
  const [decks, setDecks] = useState(initialDecks);
  // An explicit choice: no deck is picked until the user picks one.
  const [deckId, setDeckId] = useState("");
  const [front, setFront] = useState("");
  const [back, setBack] = useState("");
  useUnsavedChanges(deckId !== "" || front !== "" || back !== "");

  /** Reloads the deck choices in place, keeping what the user wrote. */
  function refreshDecks() {
    loadDeckChoices().then((next) => {
      if (next.status !== "ready") return;
      if (next.decks.length === 0) {
        onNoDecks();
        return;
      }
      setDecks(next.decks);
      // Checked against the choice when the list arrives, not when it was
      // asked for, so a deck picked in the meantime isn't cleared.
      setDeckId((current) =>
        next.decks.some((deck) => String(deck.id) === current) ? current : ""
      );
    });
  }

  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      if (deckId === "") {
        // Nothing to send yet; shown under the deck list like any field error.
        const unchosen: CommandError = {
          kind: "invalid",
          message: "Choose a deck for this card.",
          field: "deck",
        };
        throw unchosen;
      }
      try {
        return await createFlashcard(Number(deckId), front, back);
      } catch (err) {
        const error = toCommandError(err);
        if (error.field || error.kind === "failed") throw err;
        // A refusal about neither side of the card is about the deck: it was
        // archived or is gone since the list loaded. Show it under the deck
        // list, and refresh the choices before the user picks again.
        refreshDecks();
        const refused: CommandError = { ...error, field: "deck" };
        throw refused;
      }
    },
    onCreated,
    { deck: "note-card-deck", front: "note-card-front", back: "note-card-back" }
  );

  return (
    <form className="card" aria-labelledby="note-card-heading" noValidate onSubmit={submit}>
      <h2 id="note-card-heading" className="form-title">
        {`Create a card from ${note.title}`}
      </h2>

      {/* Focusable so keyboard users can scroll a long note. */}
      <section className="note-context" tabIndex={0} aria-labelledby="note-context-label">
        <p id="note-context-label" className="side-label">
          Your note
        </p>
        <p className="note-body">{note.body}</p>
      </section>
      <p id="note-card-hint" className="field-hint">
        Write the question and answer yourself. The note stays exactly as it is, and the card
        isn't linked to it.
      </p>

      <SelectField
        id="note-card-deck"
        label="Deck"
        hint="The card is added to this deck and is due for review straight away."
        describedBy="note-card-hint"
        value={deckId}
        options={decks.map((deck) => ({ value: String(deck.id), label: deck.name }))}
        placeholder="Choose a deck"
        error={fieldError("deck")}
        required
        autoFocus
        onChange={(value) => {
          setDeckId(value);
          edited("deck");
        }}
      />
      <TextField
        id="note-card-front"
        label="Front"
        hint="The question. Required. Up to 2,000 characters."
        value={front}
        error={fieldError("front")}
        required
        multiline
        onChange={(value) => {
          setFront(value);
          edited("front");
        }}
      />
      <TextField
        id="note-card-back"
        label="Back"
        hint="The answer. Required. Up to 2,000 characters."
        value={back}
        error={fieldError("back")}
        required
        multiline
        onChange={(value) => {
          setBack(value);
          edited("back");
        }}
      />

      <FormActions saveLabel="Save card" saving={saving} error={formError} onCancel={onCancel} />
    </form>
  );
}
