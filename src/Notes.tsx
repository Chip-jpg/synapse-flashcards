import { useEffect, useRef, useState } from "react";
import {
  createFlashcard,
  createNote,
  deleteNote,
  getDecks,
  getDeletedNotes,
  getNote,
  getNotes,
  restoreNote,
  toCommandError,
  updateNote,
  type CommandError,
  type Deck,
  type DeckDetail,
  type DeletedNote,
  type Note,
  type NoteSummary,
} from "./api";
import { FormActions, SelectField, TextField, useFormSave } from "./forms";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  NoteIcon,
  PencilIcon,
  PlusIcon,
  RestoreIcon,
  TrashIcon,
} from "./icons";
import { formatDateTime, Message, useFocusOnMount } from "./ui";

/** What takes focus when the library appears. */
type LibraryFocus =
  | { to: "screen" }
  | { to: "new" }
  | { to: "note"; noteId: number }
  | { to: "notice" };

/** Where in the notes area the user is. */
type Place =
  | { screen: "library"; focus: LibraryFocus; notice: string | null }
  | { screen: "new" }
  | { screen: "note"; noteId: number; justCreated: boolean };

/**
 * The notes area: the library of the user's typed notes, writing a new one,
 * opening one to read or edit it, and writing a card from it. Every screen
 * change follows a button press, so each new screen takes focus.
 */
export function NotesArea({
  onBack,
  onOpenDeck,
}: {
  onBack: () => void;
  /** Leaves notes for the deck screen, e.g. to see a card just written. */
  onOpenDeck: (deckId: number) => void;
}) {
  const [place, setPlace] = useState<Place>({
    screen: "library",
    focus: { to: "screen" },
    notice: null,
  });

  if (place.screen === "new") {
    return (
      <NoteForm
        onCancel={() => setPlace({ screen: "library", focus: { to: "new" }, notice: null })}
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
        onBack={() =>
          setPlace({ screen: "library", focus: { to: "note", noteId }, notice: null })
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
      />
    );
  }

  return (
    <NoteLibrary
      focus={place.focus}
      notice={place.notice}
      onNew={() => setPlace({ screen: "new" })}
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
  onOpen,
  onBack,
}: {
  focus: LibraryFocus;
  /** A confirmation to announce when the library appears (e.g. after deleting). */
  notice: string | null;
  onNew: () => void;
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
            Back to decks
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
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focusNotice);

  return (
    <section ref={screenRef} className="notes" tabIndex={-1} aria-labelledby="notes-heading">
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        Back to decks
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
        <button ref={newRef} type="button" className="button button-primary" onClick={onNew}>
          <PlusIcon />
          New note
        </button>
      </div>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {notes.length === 0 ? (
        <p className="empty" role="status">
          {deleted.length === 0
            ? "No notes yet. Choose New note to write one."
            : "No notes in your library. Choose New note to write one, or restore a deleted note below."}
        </p>
      ) : (
        <ul className="card-list">
          {notes.map((note) => (
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
type NoteFocus = "screen" | "notice" | "edit" | "card";

/**
 * How the note view appears next. `key` changes every time, so the view
 * remounts and its focus target takes effect. `cardDeck` is the deck a card
 * was just written into, offered as a way on.
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

/** One note: reading it, editing it in place, and writing a card from it. */
function NoteScreen({
  noteId,
  justCreated,
  onBack,
  onDeleted,
  onOpenDeck,
}: {
  noteId: number;
  justCreated: boolean;
  onBack: () => void;
  /** The note was deleted, so this screen can't show it any more. */
  onDeleted: () => void;
  onOpenDeck: (deckId: number) => void;
}) {
  const [state, setState] = useState<NoteState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  // The note itself, or a form in its place.
  const [mode, setMode] = useState<"view" | "edit" | "card">("view");
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
            Back to notes
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

  return (
    <NoteView
      key={view.key}
      note={state.note}
      notice={view.notice}
      focus={view.focus}
      cardDeck={view.cardDeck}
      onEdit={() => setMode("edit")}
      onCreateCard={() => setMode("card")}
      onDeleted={onDeleted}
      onOpenDeck={onOpenDeck}
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
  onDeleted,
  onOpenDeck,
  onBack,
}: {
  note: Note;
  notice: string | null;
  focus: NoteFocus;
  cardDeck: NoteViewState["cardDeck"];
  onEdit: () => void;
  onCreateCard: () => void;
  onDeleted: () => void;
  onOpenDeck: (deckId: number) => void;
  onBack: () => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(focus === "screen");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus === "notice");
  const editRef = useFocusOnMount<HTMLButtonElement>(focus === "edit");
  const cardRef = useFocusOnMount<HTMLButtonElement>(focus === "card");
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
        Back to notes
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
          <div className="deck-actions">
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

        <p className="note-body">{note.body}</p>
      </article>
    </section>
  );
}

/** Writing a new note, or editing `note` when one is given. All validation happens in Rust. */
function NoteForm({
  note,
  onCancel,
  onSaved,
}: {
  note?: Note;
  onCancel: () => void;
  onSaved: (note: Note) => void;
}) {
  const [title, setTitle] = useState(note?.title ?? "");
  const [body, setBody] = useState(note?.body ?? "");
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => (note ? updateNote(note.id, title, body) : createNote(title, body)),
    onSaved,
    { title: "note-title", body: "note-body" }
  );

  return (
    <form className="card" aria-labelledby="note-form-heading" noValidate onSubmit={submit}>
      <h2 id="note-form-heading" className="form-title">
        {note ? "Edit note" : "New note"}
      </h2>

      <TextField
        id="note-title"
        label="Title"
        hint="Required. Up to 200 characters."
        value={title}
        error={fieldError("title")}
        required
        autoFocus
        onChange={(value) => {
          setTitle(value);
          edited("title");
        }}
      />
      <TextField
        id="note-body"
        label="Note"
        hint="Plain text. Required. Up to 100,000 characters. Line breaks and indentation inside the note are kept; spaces and blank lines at the very start and end are removed."
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

      <FormActions
        saveLabel={note ? "Save changes" : "Save note"}
        saving={saving}
        error={formError}
        onCancel={onCancel}
      />
    </form>
  );
}

type DeckChoices =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; decks: Deck[] };

/**
 * The decks a card can be added to: active normal decks. The dashboard's list
 * is already active-only; the sample deck is left out because Rust refuses
 * cards for it (and still does, whatever is sent). Never rejects.
 */
async function loadDeckChoices(): Promise<DeckChoices> {
  try {
    const decks = await getDecks();
    return { status: "ready", decks: decks.filter((deck) => !deck.isSample) };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
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
        text="You don't have a deck to add cards to yet. Create one from the dashboard first; the Sample deck can't take new cards."
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
