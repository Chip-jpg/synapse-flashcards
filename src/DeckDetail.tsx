import { useEffect, useRef, useState } from "react";
import {
  archiveDeck,
  createFlashcard,
  deleteFlashcard,
  getDeckDetail,
  renameDeck,
  toCommandError,
  updateFlashcard,
  type DeckCard,
  type DeckDetail as Deck,
} from "./api";
import { FormActions, TextField, useFormSave } from "./forms";
import { Message, useFocusOnMount } from "./ui";
import { useStartReview } from "./useStartReview";

type DetailState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; deck: Deck };

/** What the deck screen shows: the deck itself, or a form in its place. */
type Mode =
  | { kind: "view" }
  | { kind: "add" }
  | { kind: "edit"; card: DeckCard }
  | { kind: "rename" };

/** What takes focus when the deck view appears. */
type Focus =
  | { to: "screen" }
  | { to: "notice" }
  | { to: "addCard" }
  | { to: "editCard"; cardId: number }
  | { to: "renameDeck" };

/**
 * How the deck view appears next. `key` changes every time, so the view
 * remounts and its focus target takes effect (even when no form was open,
 * e.g. after deleting a card).
 */
type ViewState = { key: number; notice: string | null; focus: Focus };

/** Fetches the deck and describes the outcome; never rejects. */
async function loadDeck(deckId: number): Promise<DetailState> {
  try {
    return { status: "ready", deck: await getDeckDetail(deckId) };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * One normal deck: its counts and cards, adding, editing, and deleting cards,
 * starting a review, and renaming or archiving the deck.
 */
export function DeckDetail({
  deckId,
  justCreated,
  onBack,
  onStart,
  onArchived,
}: {
  deckId: number;
  justCreated: boolean;
  onBack: () => void;
  onStart: (sessionId: number, deckName: string) => void;
  /** The deck was archived, so this screen can't show it any more. */
  onArchived: () => void;
}) {
  const [state, setState] = useState<DetailState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or refreshing counts).
  const [attempt, setAttempt] = useState(0);
  const [mode, setMode] = useState<Mode>({ kind: "view" });
  // A confirmation after a save takes focus, so it's announced.
  const [view, setView] = useState<ViewState>({
    key: 0,
    notice: justCreated ? "Deck created. Add your first card." : null,
    focus: justCreated ? { to: "notice" } : { to: "screen" },
  });

  useEffect(() => {
    let current = true;
    loadDeck(deckId).then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [deckId, attempt]);

  /** Shows the deck view again, with `focus` and an optional confirmation. */
  function showView(notice: string | null, focus: Focus) {
    setMode({ kind: "view" });
    setView((previous) => ({ key: previous.key + 1, notice, focus }));
  }

  /** A card was saved or deleted: show the deck Rust returned, and confirm. */
  function showSaved(deck: Deck, notice: string) {
    setState({ status: "ready", deck });
    showView(notice, { to: "notice" });
  }

  function reload() {
    setState({ status: "loading" });
    showView(null, { to: "screen" });
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="message" role="status">
        Loading the deck…
      </p>
    );
  }

  // Every deck screen follows a button press, so errors take focus.
  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <div className="actions">
          <button type="button" className="button" onClick={reload}>
            Retry
          </button>
          <button type="button" className="button" onClick={onBack}>
            Back to decks
          </button>
        </div>
      </Message>
    );
  }

  if (mode.kind === "add") {
    return (
      <CardForm
        deck={state.deck}
        onCancel={() => showView(null, { to: "addCard" })}
        onSaved={(deck) => showSaved(deck, "Card added.")}
      />
    );
  }

  if (mode.kind === "edit") {
    const card = mode.card;
    return (
      <CardForm
        key={card.id}
        deck={state.deck}
        card={card}
        onCancel={() => showView(null, { to: "editCard", cardId: card.id })}
        onSaved={(deck) => showSaved(deck, "Card updated.")}
      />
    );
  }

  if (mode.kind === "rename") {
    return (
      <RenameDeckForm
        deck={state.deck}
        onCancel={() => showView(null, { to: "renameDeck" })}
        onSaved={(deck) => showSaved(deck, "Deck renamed.")}
      />
    );
  }

  return (
    <DeckView
      key={view.key}
      deck={state.deck}
      notice={view.notice}
      focus={view.focus}
      onAdd={() => setMode({ kind: "add" })}
      onEdit={(card) => setMode({ kind: "edit", card })}
      onDeleted={(deck) => showSaved(deck, "Card deleted.")}
      onRename={() => setMode({ kind: "rename" })}
      onArchived={onArchived}
      onBack={onBack}
      onStart={onStart}
      onChanged={reload}
    />
  );
}

function DeckView({
  deck,
  notice,
  focus,
  onAdd,
  onEdit,
  onDeleted,
  onRename,
  onArchived,
  onBack,
  onStart,
  onChanged,
}: {
  deck: Deck;
  notice: string | null;
  focus: Focus;
  onAdd: () => void;
  onEdit: (card: DeckCard) => void;
  onDeleted: (deck: Deck) => void;
  onRename: () => void;
  onArchived: () => void;
  onBack: () => void;
  onStart: (sessionId: number, deckName: string) => void;
  onChanged: () => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(focus.to === "screen");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus.to === "notice");
  const addRef = useFocusOnMount<HTMLButtonElement>(focus.to === "addCard");
  const { starting, error, start } = useStartReview(deck.id, deck.name, onStart, onChanged);
  const editFocusId = focus.to === "editCard" ? focus.cardId : null;
  const cards = deck.cardCount;
  const due = deck.dueCount;

  return (
    <section ref={screenRef} className="deck-detail" tabIndex={-1} aria-labelledby="deck-detail-name">
      <article className="card deck">
        <div>
          <h2 id="deck-detail-name" className="deck-name">
            {deck.name}
          </h2>
          {deck.description && <p className="deck-description">{deck.description}</p>}
        </div>

        <div>
          <p className="deck-due">
            {cards === 0 ? "No cards yet" : `${cards} ${cards === 1 ? "card" : "cards"} in total`}
          </p>
          <p className="deck-due">
            {due === 0
              ? "No cards due right now"
              : `${due} ${due === 1 ? "card" : "cards"} due`}
          </p>
        </div>

        {notice && (
          <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
            {notice}
          </p>
        )}

        <div className="deck-actions">
          <button
            ref={addRef}
            type="button"
            className={due > 0 ? "button" : "button button-primary"}
            onClick={onAdd}
          >
            Add card
          </button>
          {due > 0 && (
            <button
              type="button"
              className="button button-primary"
              aria-describedby="deck-detail-name"
              aria-disabled={starting}
              onClick={start}
            >
              Start review
            </button>
          )}
        </div>

        {error && (
          <p className="message-error" role="alert">
            {error}
          </p>
        )}
      </article>

      {deck.cards.length > 0 && (
        <section className="cards" aria-labelledby="cards-heading">
          <h3 id="cards-heading" className="section-title">
            Cards
          </h3>
          <ul className="card-list">
            {deck.cards.map((card) => (
              <li key={card.id}>
                <CardItem
                  card={card}
                  focusEdit={card.id === editFocusId}
                  onEdit={() => onEdit(card)}
                  onDeleted={onDeleted}
                  onStale={onChanged}
                />
              </li>
            ))}
          </ul>
        </section>
      )}

      <ManageDeck
        deckId={deck.id}
        focusRename={focus.to === "renameDeck"}
        onRename={onRename}
        onArchived={onArchived}
      />

      <button type="button" className="button" onClick={onBack}>
        Back to decks
      </button>
    </section>
  );
}

/** Renaming the deck, and archiving it after a two-step confirmation. */
function ManageDeck({
  deckId,
  focusRename,
  onRename,
  onArchived,
}: {
  deckId: number;
  focusRename: boolean;
  onRename: () => void;
  onArchived: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second archive immediately, before the re-render lands.
  const archivingRef = useRef(false);
  const renameRef = useFocusOnMount<HTMLButtonElement>(focusRename);
  const archiveRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);

  // As with deleting a card: focus moves to the question when it opens (so a
  // repeated Enter can't archive), and back to "Archive deck" if it's kept.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      archiveRef.current?.focus();
    }
  }, [confirming]);

  async function confirmArchive() {
    if (archivingRef.current) return;
    archivingRef.current = true;
    setArchiving(true);
    setError(null);

    try {
      await archiveDeck(deckId);
      onArchived();
      return; // This screen is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // Already archived: the deck is gone from this screen either way.
        onArchived();
        return;
      }
      setError(failure.message);
    }

    archivingRef.current = false;
    setArchiving(false);
  }

  return (
    <section className="cards" aria-labelledby="manage-deck-heading">
      <h3 id="manage-deck-heading" className="section-title">
        Manage deck
      </h3>

      {confirming ? (
        <div className="card-item-confirm">
          <p ref={questionRef} className="notice" tabIndex={-1}>
            Archive this deck? It will leave your deck list and can't be reviewed or changed. Its
            cards and review history are kept, and you can unarchive it from your deck list later.
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button"
              aria-describedby="deck-detail-name"
              aria-disabled={archiving}
              onClick={confirmArchive}
            >
              Archive deck
            </button>
            <button
              type="button"
              className="button"
              aria-disabled={archiving}
              onClick={() => {
                if (!archiving) setConfirming(false);
              }}
            >
              Keep deck
            </button>
          </div>
          <p className="form-status" role="status">
            {archiving ? "Archiving…" : ""}
          </p>
        </div>
      ) : (
        <div className="deck-actions">
          <button
            ref={renameRef}
            type="button"
            className="button"
            aria-describedby="deck-detail-name"
            onClick={onRename}
          >
            Rename deck
          </button>
          <button
            ref={archiveRef}
            type="button"
            className="button"
            aria-describedby="deck-detail-name"
            onClick={() => setConfirming(true)}
          >
            Archive deck
          </button>
        </div>
      )}

      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** Renaming `deck`. All validation happens in Rust; errors come back for the name field. */
function RenameDeckForm({
  deck,
  onCancel,
  onSaved,
}: {
  deck: Deck;
  onCancel: () => void;
  onSaved: (deck: Deck) => void;
}) {
  const [name, setName] = useState(deck.name);
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => renameDeck(deck.id, name),
    onSaved,
    { name: "rename-deck-name" },
  );

  return (
    <form className="card" aria-labelledby="rename-deck-heading" noValidate onSubmit={submit}>
      <h2 id="rename-deck-heading" className="section-title">
        Rename {deck.name}
      </h2>

      <TextField
        id="rename-deck-name"
        label="Deck name"
        hint="Required. Up to 100 characters."
        value={name}
        error={fieldError("name")}
        required
        autoFocus
        onChange={(value) => {
          setName(value);
          edited("name");
        }}
      />

      <FormActions saveLabel="Save name" saving={saving} error={formError} onCancel={onCancel} />
    </form>
  );
}

/** One card in the list: its full text, Edit, and a two-step Delete. */
function CardItem({
  card,
  focusEdit,
  onEdit,
  onDeleted,
  onStale,
}: {
  card: DeckCard;
  focusEdit: boolean;
  onEdit: () => void;
  onDeleted: (deck: Deck) => void;
  onStale: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second delete immediately, before the re-render lands.
  const deletingRef = useRef(false);
  const editRef = useFocusOnMount<HTMLButtonElement>(focusEdit);
  const deleteRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);
  const frontId = `card-${card.id}-front`;

  // The pressed button disappears either way, so focus follows: to the
  // question when it opens (reading it, and keeping a repeated Enter from
  // deleting), and back to Delete when the card is kept.
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
    setError(null);

    try {
      onDeleted(await deleteFlashcard(card.id));
      return; // The list is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // Already deleted: reload to show what's really there.
        onStale();
        return;
      }
      setError(failure.message);
    }

    deletingRef.current = false;
    setDeleting(false);
  }

  return (
    <div className="card-item">
      <div>
        <p className="side-label">Front</p>
        <p id={frontId} className="card-item-text">
          {card.front}
        </p>
      </div>
      <div>
        <p className="side-label">Back</p>
        <p className="card-item-text">{card.back}</p>
      </div>

      {confirming ? (
        <div className="card-item-confirm">
          <p ref={questionRef} className="notice" tabIndex={-1}>
            Delete this card? It will leave this deck and won't appear in reviews again. Its review
            history is kept, but it can't be restored.
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button"
              aria-describedby={frontId}
              aria-disabled={deleting}
              onClick={confirmDelete}
            >
              Delete card
            </button>
            <button
              type="button"
              className="button"
              aria-disabled={deleting}
              onClick={() => {
                if (!deleting) setConfirming(false);
              }}
            >
              Keep card
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
            aria-describedby={frontId}
            onClick={onEdit}
          >
            Edit card
          </button>
          <button
            ref={deleteRef}
            type="button"
            className="button"
            aria-describedby={frontId}
            onClick={() => setConfirming(true)}
          >
            Delete card
          </button>
        </div>
      )}

      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** Adding a card to `deck`, or editing `card` when one is given. */
function CardForm({
  deck,
  card,
  onCancel,
  onSaved,
}: {
  deck: Deck;
  card?: DeckCard;
  onCancel: () => void;
  onSaved: (deck: Deck) => void;
}) {
  const [front, setFront] = useState(card?.front ?? "");
  const [back, setBack] = useState(card?.back ?? "");
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => (card ? updateFlashcard(card.id, front, back) : createFlashcard(deck.id, front, back)),
    onSaved,
    { front: "card-front", back: "card-back" },
  );

  return (
    <form className="card" aria-labelledby="card-form-heading" noValidate onSubmit={submit}>
      <h2 id="card-form-heading" className="section-title">
        {card ? "Edit a card in " : "Add a card to "}
        {deck.name}
      </h2>

      <TextField
        id="card-front"
        label="Front"
        hint="The question. Required. Up to 2,000 characters."
        value={front}
        error={fieldError("front")}
        required
        multiline
        autoFocus
        onChange={(value) => {
          setFront(value);
          edited("front");
        }}
      />
      <TextField
        id="card-back"
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

      <FormActions
        saveLabel={card ? "Save changes" : "Save card"}
        saving={saving}
        error={formError}
        onCancel={onCancel}
      />
    </form>
  );
}
