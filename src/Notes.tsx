import { useEffect, useState } from "react";
import {
  createFlashcard,
  createNote,
  getDecks,
  getNote,
  getNotes,
  toCommandError,
  updateNote,
  type CommandError,
  type Deck,
  type DeckDetail,
  type Note,
  type NoteSummary,
} from "./api";
import { FormActions, SelectField, TextField, useFormSave } from "./forms";
import { formatDateTime, Message, useFocusOnMount } from "./ui";

/** What takes focus when the library appears. */
type LibraryFocus = { to: "screen" } | { to: "new" } | { to: "note"; noteId: number };

/** Where in the notes area the user is. */
type Place =
  | { screen: "library"; focus: LibraryFocus }
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
  const [place, setPlace] = useState<Place>({ screen: "library", focus: { to: "screen" } });

  if (place.screen === "new") {
    return (
      <NoteForm
        onCancel={() => setPlace({ screen: "library", focus: { to: "new" } })}
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
        onBack={() => setPlace({ screen: "library", focus: { to: "note", noteId } })}
        onOpenDeck={onOpenDeck}
      />
    );
  }

  return (
    <NoteLibrary
      focus={place.focus}
      onNew={() => setPlace({ screen: "new" })}
      onOpen={(noteId) => setPlace({ screen: "note", noteId, justCreated: false })}
      onBack={onBack}
    />
  );
}

type LibraryState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; notes: NoteSummary[] };

/** Fetches the library and describes the outcome; never rejects. */
async function loadNotes(): Promise<LibraryState> {
  try {
    return { status: "ready", notes: await getNotes() };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

function NoteLibrary({
  focus,
  onNew,
  onOpen,
  onBack,
}: {
  focus: LibraryFocus;
  onNew: () => void;
  onOpen: (noteId: number) => void;
  onBack: () => void;
}) {
  const [state, setState] = useState<LibraryState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry).
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    loadNotes().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  if (state.status === "loading") {
    return (
      <p className="message" role="status">
        Loading your notes…
      </p>
    );
  }

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
            Back to decks
          </button>
        </div>
      </Message>
    );
  }

  return (
    <NoteList notes={state.notes} focus={focus} onNew={onNew} onOpen={onOpen} onBack={onBack} />
  );
}

function NoteList({
  notes,
  focus,
  onNew,
  onOpen,
  onBack,
}: {
  notes: NoteSummary[];
  focus: LibraryFocus;
  onNew: () => void;
  onOpen: (noteId: number) => void;
  onBack: () => void;
}) {
  // A note that's gone from the list (it can't be, today) falls back to the screen.
  const focusNoteId =
    focus.to === "note" && notes.some((note) => note.id === focus.noteId) ? focus.noteId : null;
  const screenRef = useFocusOnMount<HTMLElement>(
    focus.to === "screen" || (focus.to === "note" && focusNoteId === null)
  );
  const newRef = useFocusOnMount<HTMLButtonElement>(focus.to === "new");

  return (
    <section ref={screenRef} className="notes" tabIndex={-1} aria-labelledby="notes-heading">
      <h2 id="notes-heading" className="section-title">
        Notes
      </h2>
      <p className="field-hint">
        Your own typed study notes, kept on this computer. The most recently edited note comes first.
      </p>
      <button ref={newRef} type="button" className="button" onClick={onNew}>
        New note
      </button>

      {notes.length === 0 ? (
        <p className="message" role="status">
          No notes yet. Choose New note to write one.
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

      <button type="button" className="button" onClick={onBack}>
        Back to decks
      </button>
    </section>
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
    <article className="card-item" aria-labelledby={titleId}>
      <div>
        {/* A heading, so screen-reader users can move between notes by heading. */}
        <h3 id={titleId} className="note-item-title">
          {note.title}
        </h3>
        <p className="field-hint">
          {note.updatedAt === note.createdAt
            ? `Created ${formatDateTime(note.createdAt)}`
            : `Edited ${formatDateTime(note.updatedAt)}`}
        </p>
      </div>
      <div className="deck-actions">
        <button
          ref={openRef}
          type="button"
          className="button"
          aria-describedby={titleId}
          onClick={onOpen}
        >
          Open note
        </button>
      </div>
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
  onOpenDeck,
}: {
  noteId: number;
  justCreated: boolean;
  onBack: () => void;
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
      <p className="message" role="status">
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
  onOpenDeck,
  onBack,
}: {
  note: Note;
  notice: string | null;
  focus: NoteFocus;
  cardDeck: NoteViewState["cardDeck"];
  onEdit: () => void;
  onCreateCard: () => void;
  onOpenDeck: (deckId: number) => void;
  onBack: () => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(focus === "screen");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus === "notice");
  const editRef = useFocusOnMount<HTMLButtonElement>(focus === "edit");
  const cardRef = useFocusOnMount<HTMLButtonElement>(focus === "card");
  const edited = note.updatedAt !== note.createdAt;

  return (
    <section ref={screenRef} className="deck-detail" tabIndex={-1} aria-labelledby="note-heading">
      <article className="card">
        <div>
          <h2 id="note-heading" className="deck-name">
            {note.title}
          </h2>
          <p className="field-hint">
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
              className="button"
              aria-describedby="note-notice"
              onClick={() => onOpenDeck(cardDeck.id)}
            >
              {`Open ${cardDeck.name}`}
            </button>
          </div>
        )}

        <p className="note-body">{note.body}</p>

        <div className="deck-actions">
          <button
            ref={editRef}
            type="button"
            className="button"
            aria-describedby="note-heading"
            onClick={onEdit}
          >
            Edit note
          </button>
          <button
            ref={cardRef}
            type="button"
            className="button"
            aria-describedby="note-heading"
            onClick={onCreateCard}
          >
            Create card from note
          </button>
        </div>
      </article>

      <button type="button" className="button" onClick={onBack}>
        Back to notes
      </button>
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
      <h2 id="note-form-heading" className="section-title">
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
      <p className="message" role="status">
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
      <h2 id="note-card-heading" className="section-title">
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
