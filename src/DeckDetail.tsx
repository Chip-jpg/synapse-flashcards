import { useEffect, useMemo, useRef, useState } from "react";
import {
  archiveDeck,
  createCloze,
  createFlashcard,
  deleteFlashcard,
  getDeckDetail,
  previewCloze,
  renameDeck,
  restoreDeletedCards,
  restoreFlashcard,
  toCommandError,
  updateCloze,
  updateFlashcard,
  CARD_SIDE_PICTURES_MAX,
  type Asset,
  type CardPictures,
  type ClozeChanges,
  type DeckCard,
  type DeckDetail as Deck,
  type DeletedCard,
} from "./api";
import {
  CardSearchField,
  cardCount,
  matchingCards,
  RestoreSelected,
  SelectionActions,
} from "./CardBulkActions";
import { FormActions, TextField, useFormSave } from "./forms";
import {
  ArchiveIcon,
  ArrowLeftIcon,
  PencilIcon,
  PlayIcon,
  PlusIcon,
  RestoreIcon,
  TrashIcon,
} from "./icons";
import { OcclusionForm } from "./Occlusion";
import { formatDateTime, Message, useFocusOnMount } from "./ui";
import { useUnsavedChanges } from "./unsaved";
import { PictureField, PictureGallery, samePictures, usePictureAdder } from "./pictures";

/** A card with no pictures, as cards from before pictures (and tests) arrive. */
const NO_PICTURES: CardPictures = { front: [], back: [] };
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
  focusCardId = null,
  backLabel = "Back to Study Desk",
  onBack,
  onStart,
  onArchived,
}: {
  deckId: number;
  justCreated: boolean;
  /** A card to focus when the deck opens, e.g. one opened from a search. */
  focusCardId?: number | null;
  /** Where Back goes, e.g. "Back to search". */
  backLabel?: string;
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
    focus: justCreated
      ? { to: "notice" }
      : focusCardId !== null
        ? { to: "editCard", cardId: focusCardId }
        : { to: "screen" },
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

  /**
   * Reloads the deck, announcing `notice` and giving it focus if there is one.
   * Goes back to "loading" first: until the fresh deck arrives the lists on
   * screen are wrong, and leaving a just-restored card under *Deleted cards*
   * with a live Restore button would invite a second, doomed press.
   */
  function reload(notice: string | null = null) {
    setState({ status: "loading" });
    showView(notice, notice === null ? { to: "screen" } : { to: "notice" });
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading the deck…
      </p>
    );
  }

  // Every deck screen follows a button press, so errors take focus.
  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <div className="actions">
          <button type="button" className="button" onClick={() => reload()}>
            Retry
          </button>
          <button type="button" className="button" onClick={onBack}>
            {backLabel}
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
        onSaved={showSaved}
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
        onSaved={showSaved}
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
      foundCardId={focusCardId}
      notice={view.notice}
      focus={view.focus}
      onAdd={() => setMode({ kind: "add" })}
      onEdit={(card) => setMode({ kind: "edit", card })}
      onDeleted={(deck) =>
        showSaved(deck, "Card deleted. It's kept under Deleted cards, where you can restore it.")
      }
      onRestored={(deck) =>
        showSaved(deck, "Card restored. It's back in this deck, with its schedule unchanged.")
      }
      onRestoreStale={() =>
        // The card wasn't deleted after all, or the deck was archived since
        // this screen loaded. Say so, rather than reloading silently and
        // leaving the press unexplained.
        reload("That card couldn't be restored. Here's this deck as it is now.")
      }
      onRestoredAll={(deck, restored) =>
        showSaved(
          deck,
          `${restored} cards restored. They're back in this deck, with their schedules unchanged.`,
        )
      }
      onRestoreAllStale={() =>
        // The deleted cards on screen aren't the ones the deck has now, or
        // the deck was archived meanwhile. Nothing was restored.
        reload("The deleted cards couldn't be restored. Here's this deck as it is now.")
      }
      onRestoredSelected={(deck, restored) =>
        showSaved(
          deck,
          `${cardCount(restored)} restored. ${restored === 1 ? "It's" : "They're"} back in this deck, with ${restored === 1 ? "its schedule" : "their schedules"} unchanged.`,
        )
      }
      onBulkDeleted={(deck, deleted) =>
        showSaved(
          deck,
          `${cardCount(deleted)} deleted. ${deleted === 1 ? "It's" : "They're"} kept under Deleted cards, where you can restore ${deleted === 1 ? "it" : "them"}.`,
        )
      }
      onBulkMoved={(deck, moved, toDeckName) =>
        showSaved(
          deck,
          `${cardCount(moved)} moved to ${toDeckName}, with ${moved === 1 ? "its schedule" : "their schedules"} unchanged.`,
        )
      }
      onBulkStale={(message) =>
        // The selected cards aren't all in this deck any more, or a deck was
        // archived meanwhile. Nothing was changed; say why, and reload.
        reload(`${message} Nothing was changed. Here's this deck as it is now.`)
      }
      onRename={() => setMode({ kind: "rename" })}
      onArchived={onArchived}
      backLabel={backLabel}
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
  onRestored,
  onRestoreStale,
  onRestoredAll,
  onRestoreAllStale,
  onRestoredSelected,
  onBulkDeleted,
  onBulkMoved,
  onBulkStale,
  onRename,
  onArchived,
  foundCardId,
  backLabel,
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
  onRestored: (deck: Deck) => void;
  /** A restore was refused because this screen is out of date. */
  onRestoreStale: () => void;
  /** *Restore all* brought back `restored` cards. */
  onRestoredAll: (deck: Deck, restored: number) => void;
  /** *Restore all* was refused because this screen is out of date. */
  onRestoreAllStale: () => void;
  /** The selected deleted cards were restored. */
  onRestoredSelected: (deck: Deck, restored: number) => void;
  /** The selected cards were deleted. */
  onBulkDeleted: (deck: Deck, deleted: number) => void;
  /** The selected cards were moved to the deck named `toDeckName`. */
  onBulkMoved: (deck: Deck, moved: number, toDeckName: string) => void;
  /** A change to the selected cards was refused because this screen is out of date. */
  onBulkStale: (message: string) => void;
  onRename: () => void;
  onArchived: () => void;
  /** The card a search opened this deck for, marked in the list. */
  foundCardId: number | null;
  backLabel: string;
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
  const deleted = deck.deletedCards.length;
  // The search and selection last as long as this view: any change to the
  // deck remounts it (see `ViewState`), which clears both.
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  // Kept between renders, so ticking a card doesn't search the deck again.
  const shown = useMemo(() => matchingCards(deck.cards, query), [deck.cards, query]);
  // Each cloze text's and each picture's cards, in the deck's order, worked
  // out once per deck.
  const cardsOfText = useMemo(() => {
    const byText = new Map<string, number[]>();
    for (const card of deck.cards) {
      const group = siblingGroup(card);
      if (group === null) continue;
      const ids = byText.get(group);
      if (ids) ids.push(card.id);
      else byText.set(group, [card.id]);
    }
    return byText;
  }, [deck.cards]);
  // In the deck's order, which is the order they are sent in.
  const selectedIds = deck.cards.map((card) => card.id).filter((id) => selected.has(id));
  const shownSelected = shown.filter((card) => selected.has(card.id)).length;
  const allShownSelected = shown.length > 0 && shownSelected === shown.length;

  /**
   * The cards that are selected together with `card`: the cards made from
   * the same cloze text or picture, which move only together, or the card
   * alone.
   */
  function withSiblings(card: DeckCard): number[] {
    const group = siblingGroup(card);
    if (group === null) return [card.id];
    return cardsOfText.get(group) ?? [card.id];
  }

  function toggle(card: DeckCard) {
    setSelected((previous) => {
      const next = new Set(previous);
      const ids = withSiblings(card);
      if (previous.has(card.id)) for (const id of ids) next.delete(id);
      else for (const id of ids) next.add(id);
      return next;
    });
  }

  /** Selects every card the search shows, or unselects them if all already are. */
  function toggleShown() {
    setSelected((previous) => {
      const next = new Set(previous);
      for (const card of shown) {
        for (const id of withSiblings(card)) {
          if (allShownSelected) next.delete(id);
          else next.add(id);
        }
      }
      return next;
    });
  }

  return (
    <section ref={screenRef} className="deck-detail" tabIndex={-1} aria-labelledby="deck-detail-name">
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        {backLabel}
      </button>

      <div className="detail-header">
        <h2 id="deck-detail-name" className="page-title">
          {deck.name}
        </h2>
        {deck.description && <p className="page-intro">{deck.description}</p>}
        <div className="stat-row">
          <p className="pill">
            {cards === 0 ? "No cards yet" : `${cards} ${cards === 1 ? "card" : "cards"} in total`}
          </p>
          <p className={due === 0 ? "pill" : "pill pill-due"}>
            {due > 0 && <span className="dot" aria-hidden="true" />}
            {due === 0
              ? "No cards due right now"
              : `${due} ${due === 1 ? "card" : "cards"} due`}
          </p>
          {deleted > 0 && (
            <p className="pill">{`${deleted} deleted ${deleted === 1 ? "card" : "cards"}`}</p>
          )}
        </div>
      </div>

      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {/* The main action comes first: reviewing when cards are due,
          otherwise adding one. */}
      <div className="action-row">
        {due > 0 && (
          <button
            type="button"
            className="button button-primary button-large"
            aria-describedby="deck-detail-name"
            aria-disabled={starting}
            onClick={start}
          >
            <PlayIcon />
            Start review
          </button>
        )}
        <button
          ref={addRef}
          type="button"
          className={due > 0 ? "button button-large" : "button button-primary button-large"}
          onClick={onAdd}
        >
          <PlusIcon />
          Add card
        </button>
      </div>

      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}

      <section className="cards" aria-labelledby="cards-heading">
        <h3 id="cards-heading" className="section-title">
          Cards <span className="count">{cards}</span>
        </h3>
        {deck.cards.length >= 2 && (
          <CardSearchField query={query} shown={shown.length} onChange={setQuery} />
        )}
        {deck.cards.length > 0 && shown.length > 0 && (
          <label className="select-all">
            <SelectAllBox
              checked={allShownSelected}
              mixed={shownSelected > 0 && !allShownSelected}
              onChange={toggleShown}
            />
            {query.trim() === ""
              ? "Select all cards"
              : `Select all ${shown.length === 1 ? "matching card" : `${shown.length} matching cards`}`}
          </label>
        )}
        {deck.cards.length === 0 ? (
          <p className="empty">Choose Add card to write this deck's first card.</p>
        ) : shown.length === 0 ? (
          <p className="empty">
            No cards in this deck match “{query.trim()}”. Try other words, or clear the search.
          </p>
        ) : (
          <ul className="card-list">
            {shown.map((card) => (
              <li key={card.id}>
                <CardItem
                  card={card}
                  selected={selected.has(card.id)}
                  onToggle={() => toggle(card)}
                  found={card.id === foundCardId}
                  focusEdit={card.id === editFocusId}
                  onEdit={() => onEdit(card)}
                  onDeleted={onDeleted}
                  onStale={onChanged}
                />
              </li>
            ))}
          </ul>
        )}
        {/* After the list, and held at the bottom of the window while the list
            scrolls past, so selecting a card never moves the cards. */}
        {selectedIds.length > 0 && (
          <SelectionActions
            deck={deck}
            selected={selectedIds}
            hidden={selectedIds.length - shownSelected}
            onClear={() => setSelected(new Set())}
            onDeleted={onBulkDeleted}
            onMoved={onBulkMoved}
            onStale={onBulkStale}
          />
        )}
      </section>

      {deck.deletedCards.length > 0 && (
        <DeletedCardList
          deckId={deck.id}
          cards={deck.deletedCards}
          onRestored={onRestored}
          onStale={onRestoreStale}
          onRestoredAll={onRestoredAll}
          onRestoreAllStale={onRestoreAllStale}
          onRestoredSelected={onRestoredSelected}
          onSelectionStale={onBulkStale}
        />
      )}

      <ManageDeck
        deckId={deck.id}
        focusRename={focus.to === "renameDeck"}
        onRename={onRename}
        onArchived={onArchived}
      />
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
    <section className="cards section-quiet" aria-labelledby="manage-deck-heading">
      <h3 id="manage-deck-heading" className="section-title section-title-quiet">
        Manage deck
      </h3>

      {confirming ? (
        <div className="confirm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            Archive this deck? It will leave your deck list and can't be reviewed or changed. Its
            cards and review history are kept, and you can unarchive it from Archived decks later.
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-primary button-small"
              aria-describedby="deck-detail-name"
              aria-disabled={archiving}
              onClick={confirmArchive}
            >
              Archive deck
            </button>
            <button
              type="button"
              className="button button-small"
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
            className="button button-small"
            aria-describedby="deck-detail-name"
            onClick={onRename}
          >
            <PencilIcon />
            Rename deck
          </button>
          <button
            ref={archiveRef}
            type="button"
            className="button button-small"
            aria-describedby="deck-detail-name"
            onClick={() => setConfirming(true)}
          >
            <ArchiveIcon />
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
  useUnsavedChanges(name !== deck.name);
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => renameDeck(deck.id, name),
    onSaved,
    { name: "rename-deck-name" },
  );

  return (
    <form className="card" aria-labelledby="rename-deck-heading" noValidate onSubmit={submit}>
      <h2 id="rename-deck-heading" className="form-title">
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

/** Which cards move together with `card`: its cloze text's, or its picture's. */
function siblingGroup(card: DeckCard): string | null {
  if (card.cloze) return `cloze:${card.cloze.textId}`;
  if (card.occlusion) return `picture:${card.occlusion.occlusionId}`;
  return null;
}

/**
 * A card's full front and back, each beside its label. `frontId` lets the
 * card's buttons name it.
 */
function CardText({
  frontId,
  front,
  back,
  clozeNumber,
  pictures,
  pictureNumber,
}: {
  frontId: string;
  front: string;
  back: string;
  /** A cloze card's number, shown as a tag: the card hides that `{{cN::…}}`. */
  clozeNumber?: number;
  /** Each side's pictures, shown small under its text. */
  pictures?: CardPictures;
  /** A picture card's number, shown as a tag: the card hides that box. */
  pictureNumber?: number;
}) {
  return (
    <div className="card-sides">
      {clozeNumber !== undefined && <p className="card-kind-tag">{`Cloze ${clozeNumber}`}</p>}
      {pictureNumber !== undefined && (
        <p className="card-kind-tag">{`Picture, box ${pictureNumber}`}</p>
      )}
      <p className="side-label">Front</p>
      <p id={frontId} className="card-item-text card-item-front">
        {front}
      </p>
      <PictureGallery
        pictures={pictures?.front}
        label="Picture on the front"
        listLabel="Pictures on the front"
        compact
      />
      <p className="side-label">Back</p>
      <p className="card-item-text card-item-back">{back}</p>
      <PictureGallery
        pictures={pictures?.back}
        label="Picture on the back"
        listLabel="Pictures on the back"
        compact
      />
    </div>
  );
}

/**
 * The "select all" checkbox, shown as partly checked (`mixed`) when only some
 * of the cards it covers are selected.
 */
function SelectAllBox({
  checked,
  mixed,
  onChange,
}: {
  checked: boolean;
  mixed: boolean;
  onChange: () => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = mixed;
  }, [mixed]);
  return <input ref={ref} type="checkbox" checked={checked} onChange={onChange} />;
}

/** One card in the list: its full text, a Select box, Edit, and a two-step Delete. */
function CardItem({
  card,
  selected,
  onToggle,
  found,
  focusEdit,
  onEdit,
  onDeleted,
  onStale,
}: {
  card: DeckCard;
  /** Whether the card is selected for a change to several cards at once. */
  selected: boolean;
  onToggle: () => void;
  /** Marked as the card a search opened the deck for. */
  found: boolean;
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
    <div className={found ? "card-item card-row card-found" : "card-item card-row"}>
      <CardText
        frontId={frontId}
        front={card.front}
        back={card.back}
        clozeNumber={card.cloze?.number}
        pictures={card.pictures}
        pictureNumber={card.occlusion?.number}
      />

      {confirming ? (
        <div className="confirm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            Delete this card? It will leave this deck and won't appear in reviews. Its schedule and
            review history are kept, and you can restore it from Deleted cards below.
            {card.cloze &&
              ` The other cards of its cloze text stay, and this one stays deleted while {{c${card.cloze.number}::…}} is in the text.`}
            {card.occlusion &&
              " The other cards of its picture stay, and this one stays deleted while its box is on the picture."}
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-danger-soft button-small"
              aria-describedby={frontId}
              aria-disabled={deleting}
              onClick={confirmDelete}
            >
              Delete card
            </button>
            <button
              type="button"
              className="button button-small"
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
        <div className="deck-actions item-actions">
          <label className="card-select">
            <input
              type="checkbox"
              checked={selected}
              aria-describedby={frontId}
              onChange={onToggle}
            />
            Select
          </label>
          <button
            ref={editRef}
            type="button"
            className="button button-quiet button-small"
            aria-describedby={frontId}
            onClick={onEdit}
          >
            <PencilIcon />
            Edit card
          </button>
          <button
            ref={deleteRef}
            type="button"
            className="button button-quiet button-small"
            aria-describedby={frontId}
            onClick={() => setConfirming(true)}
          >
            <TrashIcon />
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

/**
 * Deleted cards: this deck's history, and the only place they appear. They
 * can't be edited or reviewed while they're deleted; *Restore card* puts one
 * back in the deck above exactly as it was, *Restore selected* puts back the
 * ones ticked, and *Restore all* (offered when there are two or more) puts
 * every one back the same way.
 */
function DeletedCardList({
  deckId,
  cards,
  onRestored,
  onStale,
  onRestoredAll,
  onRestoreAllStale,
  onRestoredSelected,
  onSelectionStale,
}: {
  deckId: number;
  cards: DeletedCard[];
  onRestored: (deck: Deck) => void;
  onStale: () => void;
  onRestoredAll: (deck: Deck, restored: number) => void;
  onRestoreAllStale: () => void;
  onRestoredSelected: (deck: Deck, restored: number) => void;
  /** *Restore selected* was refused because this screen is out of date. */
  onSelectionStale: (message: string) => void;
}) {
  // A cloze card whose number has left its text (or a picture card whose box
  // was removed) can't come back yet, so it can't be selected either.
  const restorable = cards.filter(canRestore).length;
  // Lasts as long as the deck view: any change to the deck remounts it.
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  // In the list's order, which is the order they are sent in.
  const selectedIds = cards.map((card) => card.id).filter((id) => selected.has(id));

  function toggle(cardId: number) {
    setSelected((previous) => {
      const next = new Set(previous);
      if (!next.delete(cardId)) next.add(cardId);
      return next;
    });
  }
  return (
    <section className="cards section-quiet" aria-labelledby="deleted-cards-heading">
      <div className="section-heading">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          <RestoreIcon />
        </span>
        <h3 id="deleted-cards-heading" className="section-title section-title-quiet">
          Deleted cards <span className="count">{cards.length}</span>
        </h3>
      </div>
      <p className="field-hint">
        Kept for your history. A deleted card isn't in this deck's counts or reviews, but restoring
        it brings it back with its text, schedule, and review history unchanged — it becomes due
        again only on the date it already had.
      </p>
      {restorable >= 2 && (
        <RestoreAll
          deckId={deckId}
          count={restorable}
          all={restorable === cards.length}
          onRestored={onRestoredAll}
          onStale={onRestoreAllStale}
        />
      )}
      <ul className="card-list">
        {cards.map((card) => (
          <li key={card.id}>
            <DeletedCardItem
              card={card}
              selected={selected.has(card.id)}
              onToggle={canRestore(card) ? () => toggle(card.id) : undefined}
              onRestored={onRestored}
              onStale={onStale}
            />
          </li>
        ))}
      </ul>
      {selectedIds.length > 0 && (
        <RestoreSelected
          deckId={deckId}
          selected={selectedIds}
          onClear={() => setSelected(new Set())}
          onRestored={onRestoredSelected}
          onStale={onSelectionStale}
        />
      )}
    </section>
  );
}

/**
 * *Restore all*: a two-step restore of every deleted card in the deck that
 * can be restored (`count` of them; `all` when that is every one).
 */
function RestoreAll({
  deckId,
  count,
  all,
  onRestored,
  onStale,
}: {
  deckId: number;
  count: number;
  all: boolean;
  onRestored: (deck: Deck, restored: number) => void;
  onStale: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second restore immediately, before the re-render lands.
  const restoringRef = useRef(false);
  const restoreRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);

  // As with restoring one card: focus moves to the question when it opens (so
  // a repeated Enter can't restore), and back to Restore all if it's declined.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      restoreRef.current?.focus();
    }
  }, [confirming]);

  async function confirmRestore() {
    if (restoringRef.current) return;
    restoringRef.current = true;
    setRestoring(true);
    setError(null);

    try {
      // Rust restores only if the deck still has exactly `count` deleted
      // cards, so the user never brings back more than they agreed to.
      const { restored, deck } = await restoreDeletedCards(deckId, count);
      onRestored(deck, restored);
      return; // The list is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale();
        return;
      }
      setError(failure.message);
    }

    restoringRef.current = false;
    setRestoring(false);
  }

  return (
    <div className="restore-all">
      {confirming ? (
        <div className="confirm confirm-calm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            {`${all ? `Restore all ${count} deleted cards?` : `Restore the ${count} deleted cards that can be restored?`} They go back into this deck with the same schedules and review history they had, and each becomes due again only on the date it already had.`}
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-primary button-small"
              aria-disabled={restoring}
              onClick={confirmRestore}
            >
              {`Restore ${count} cards`}
            </button>
            <button
              type="button"
              className="button button-small"
              aria-disabled={restoring}
              onClick={() => {
                if (!restoring) setConfirming(false);
              }}
            >
              Keep deleted
            </button>
          </div>
          <p className="form-status" role="status">
            {restoring ? "Restoring…" : ""}
          </p>
        </div>
      ) : (
        <div className="deck-actions">
          <button
            ref={restoreRef}
            type="button"
            className="button button-small"
            onClick={() => setConfirming(true)}
          >
            <RestoreIcon />
            {all ? "Restore all" : `Restore ${count} cards`}
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

/** Whether a deleted card can be restored now, as far as the screen knows. */
function canRestore(card: DeletedCard): boolean {
  return !card.cloze?.blocked && !card.occlusion?.blocked;
}

/**
 * One deleted card, with a two-step Restore and, when it can be restored, a
 * Select box for restoring several at once.
 */
function DeletedCardItem({
  card,
  selected,
  onToggle,
  onRestored,
  onStale,
}: {
  card: DeletedCard;
  /** Whether the card is selected for restoring several at once. */
  selected: boolean;
  /** Ticks or unticks the card; left out when it can't be restored. */
  onToggle?: () => void;
  onRestored: (deck: Deck) => void;
  onStale: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second restore immediately, before the re-render lands.
  const restoringRef = useRef(false);
  const restoreRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);
  const frontId = `deleted-card-${card.id}-front`;

  // As with deleting a card: focus moves to the question when it opens (so a
  // repeated Enter can't restore), and back to Restore if the card is left
  // deleted.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      restoreRef.current?.focus();
    }
  }, [confirming]);

  async function confirmRestore() {
    if (restoringRef.current) return;
    restoringRef.current = true;
    setRestoring(true);
    setError(null);

    try {
      onRestored(await restoreFlashcard(card.id));
      return; // The list is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // Not deleted after all, or the deck was archived meanwhile: reload
        // to show what's really there.
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
      <CardText
        frontId={frontId}
        front={card.front}
        back={card.back}
        clozeNumber={card.cloze?.number}
        pictureNumber={card.occlusion?.number}
      />
      <p className="item-meta">{`Deleted ${formatDateTime(card.deletedAt)}`}</p>

      {card.cloze?.blocked ? (
        <p className="item-meta">
          {card.cloze.blocked === "numberGone"
            ? `It can't be restored while {{c${card.cloze.number}::…}} isn't in its cloze text. Put it back in the text, and this card returns with its schedule.`
            : "It can't be restored here: the other cards of its cloze text are in another deck now."}
        </p>
      ) : card.occlusion?.blocked ? (
        <p className="item-meta">
          {card.occlusion.blocked === "numberGone"
            ? "It can't be restored while its box isn't on the picture. Draw a box for it again (edit the picture's cards and choose its card), and this card returns with its schedule."
            : "It can't be restored here: the other cards of its picture are in another deck now."}
        </p>
      ) : confirming ? (
        <div className="confirm confirm-calm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            Restore this card? It goes back into this deck with the same schedule and review
            history it had, and becomes due again only on the date it already had.
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-primary button-small"
              aria-describedby={frontId}
              aria-disabled={restoring}
              onClick={confirmRestore}
            >
              Restore card
            </button>
            <button
              type="button"
              className="button button-small"
              aria-disabled={restoring}
              onClick={() => {
                if (!restoring) setConfirming(false);
              }}
            >
              Keep deleted
            </button>
          </div>
          <p className="form-status" role="status">
            {restoring ? "Restoring…" : ""}
          </p>
        </div>
      ) : (
        <div className="deck-actions item-actions">
          {onToggle && (
            <label className="card-select">
              <input
                type="checkbox"
                checked={selected}
                aria-describedby={frontId}
                onChange={onToggle}
              />
              Select
            </label>
          )}
          <button
            ref={restoreRef}
            type="button"
            className="button button-small"
            aria-describedby={frontId}
            onClick={() => setConfirming(true)}
          >
            <RestoreIcon />
            Restore card
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

type CardKind = "basic" | "cloze" | "picture";

/** "a, b and c". */
function listed(items: string[]): string {
  return items.length <= 1
    ? (items[0] ?? "")
    : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * What saving a cloze text does (`saved` false: will do, for the live
 * count) or did, in words.
 */
function describeChanges(changes: ClozeChanges, editing: boolean, saved: boolean): string {
  if (!editing) {
    return saved
      ? `Cloze text added: it made ${cardCount(changes.added)}.`
      : `This text makes ${cardCount(changes.cards)}.`;
  }
  const parts = [
    changes.kept > 0 && `${saved ? "kept" : "keeps"} ${cardCount(changes.kept)}`,
    changes.restored > 0 &&
      `${saved ? "brought back" : "brings back"} ${cardCount(changes.restored)} from Deleted cards`,
    changes.added > 0 && `${saved ? "added" : "adds"} ${changes.added} new`,
    changes.deleted > 0 &&
      `${saved ? "deleted" : "deletes"} ${cardCount(changes.deleted)} whose number left the text`,
  ].filter((part): part is string => typeof part === "string");
  return saved ? `Cloze text saved: it ${listed(parts)}.` : `Saving ${listed(parts)}.`;
}

/** The live count under a cloze text, or why the text can't be saved yet. */
type Preview =
  | { status: "none" }
  | { status: "ready"; changes: ClozeChanges }
  | { status: "problem"; message: string };

/**
 * Adding a card to `deck`, or editing `card` when one is given. A new card is
 * Basic (a front and a back), Cloze (a text with hidden parts, one card per
 * number) or Picture (boxes over a picture, one card per box, in
 * `Occlusion.tsx`); a card is edited as the kind it is.
 */
function CardForm({
  deck,
  card,
  onCancel,
  onSaved,
}: {
  deck: Deck;
  card?: DeckCard;
  onCancel: () => void;
  /** Saved: the deck Rust returned, and what to tell the user. */
  onSaved: (deck: Deck, notice: string) => void;
}) {
  const cloze = card?.cloze ?? null;
  const occlusion = card?.occlusion ?? null;
  const [kind, setKind] = useState<CardKind>(cloze ? "cloze" : occlusion ? "picture" : "basic");
  const initialFront = card && !cloze && !occlusion ? card.front : "";
  const initialBack = card && !cloze && !occlusion ? card.back : "";
  const initialText = cloze?.text ?? "";
  const [front, setFront] = useState(initialFront);
  const [back, setBack] = useState(initialBack);
  const [text, setText] = useState(initialText);
  const initialPictures = card?.pictures ?? NO_PICTURES;
  const [frontPictures, setFrontPictures] = useState<Asset[]>(initialPictures.front);
  const [backPictures, setBackPictures] = useState<Asset[]>(initialPictures.back);
  const [preview, setPreview] = useState<Preview>({ status: "none" });
  // Once a kind has been chosen, focus stays on the choice rather than
  // jumping to the field that appears.
  const [chosen, setChosen] = useState(false);
  useUnsavedChanges(
    front !== initialFront ||
      back !== initialBack ||
      text !== initialText ||
      !samePictures(frontPictures, initialPictures.front) ||
      !samePictures(backPictures, initialPictures.back),
  );
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async (): Promise<{ deck: Deck; notice: string }> => {
      if (kind === "cloze") {
        if (card) {
          const saved = await updateCloze(card.id, text);
          return { deck: saved.deck, notice: describeChanges(saved.changes, true, true) };
        }
        const saved = await createCloze(deck.id, text);
        const added = saved.cardCount - deck.cardCount;
        return {
          deck: saved,
          notice: describeChanges(
            { cards: added, kept: 0, restored: 0, added, deleted: 0 },
            false,
            true,
          ),
        };
      }
      const pictures = {
        front: frontPictures.map((picture) => picture.id),
        back: backPictures.map((picture) => picture.id),
      };
      const saved = card
        ? await updateFlashcard(card.id, front, back, pictures)
        : await createFlashcard(deck.id, front, back, pictures);
      return { deck: saved, notice: card ? "Card updated." : "Card added." };
    },
    (saved) => onSaved(saved.deck, saved.notice),
    {
      front: "card-front",
      back: "card-back",
      text: "card-text",
      frontPictures: "card-front-pictures",
      backPictures: "card-back-pictures",
    },
  );
  const changeFrontPictures = (next: Asset[]) => {
    setFrontPictures(next);
    edited("frontPictures");
  };
  const changeBackPictures = (next: Asset[]) => {
    setBackPictures(next);
    edited("backPictures");
  };
  const frontAdder = usePictureAdder({
    pictures: frontPictures,
    onChange: changeFrontPictures,
    max: CARD_SIDE_PICTURES_MAX,
  });
  const backAdder = usePictureAdder({
    pictures: backPictures,
    onChange: changeBackPictures,
    max: CARD_SIDE_PICTURES_MAX,
  });

  // The live count: Rust reads the text a moment after typing stops, so the
  // count always follows the same rules as saving.
  const cardId = card?.id ?? null;
  useEffect(() => {
    if (kind !== "cloze" || text.trim() === "") {
      setPreview({ status: "none" });
      return;
    }
    let current = true;
    const timer = setTimeout(() => {
      previewCloze(text, cardId).then(
        (changes) => {
          if (current) setPreview({ status: "ready", changes });
        },
        (err) => {
          if (current) setPreview({ status: "problem", message: toCommandError(err).message });
        },
      );
    }, 200);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [kind, text, cardId]);

  const kindChoice = (
      <fieldset className="field kind-choice">
        <legend className="field-label">Kind of card</legend>
        <label className="kind-choice-option">
          <input
            type="radio"
            name="card-kind"
            value="basic"
            checked={kind === "basic"}
            onChange={() => {
              setKind("basic");
              setChosen(true);
            }}
          />
          <span>
            <strong>Basic</strong>: a front and a back.
          </span>
        </label>
        <label className="kind-choice-option">
          <input
            type="radio"
            name="card-kind"
            value="cloze"
            checked={kind === "cloze"}
            onChange={() => {
              setKind("cloze");
              setChosen(true);
            }}
          />
          <span>
            <strong>Cloze</strong>: a text with parts hidden, one card for each part.
          </span>
        </label>
        <label className="kind-choice-option">
          <input
            type="radio"
            name="card-kind"
            value="picture"
            checked={kind === "picture"}
            onChange={() => {
              setKind("picture");
              setChosen(true);
            }}
          />
          <span>
            <strong>Picture</strong>: boxes drawn over a picture, one card for each box.
          </span>
        </label>
      </fieldset>
  );

  if (kind === "picture") {
    return (
      <OcclusionForm
        deck={deck}
        card={card}
        kindChoice={card ? undefined : kindChoice}
        onCancel={onCancel}
        onSaved={onSaved}
      />
    );
  }

  return (
    <form className="card" aria-labelledby="card-form-heading" noValidate onSubmit={submit}>
      <h2 id="card-form-heading" className="form-title">
        {card ? (cloze ? "Edit a cloze text in " : "Edit a card in ") : "Add a card to "}
        {deck.name}
      </h2>

      {!card && kindChoice}

      {kind === "cloze" ? (
        <>
          <TextField
            id="card-text"
            label="Text"
            hint={
              "Hide a part as {{c1::answer}}, or {{c1::answer::hint}} to show a hint in its place. " +
              "Each number is its own card; give parts the same number to hide them together. " +
              (cloze
                ? "Numbers you keep keep their schedules; a number you take out deletes its card. "
                : "") +
              "Up to 2,000 characters."
            }
            value={text}
            error={fieldError("text")}
            required
            multiline
            autoFocus={!chosen}
            onChange={(value) => {
              setText(value);
              edited("text");
            }}
          />
          <p
            className={
              preview.status === "problem" ? "cloze-preview cloze-preview-problem" : "cloze-preview"
            }
            role="status"
          >
            {preview.status === "ready"
              ? describeChanges(preview.changes, cloze !== null, false)
              : preview.status === "problem"
                ? preview.message
                : ""}
          </p>
        </>
      ) : (
        <>
          <div className="card-form-side" onPaste={frontAdder.onPaste}>
            <TextField
              id="card-front"
              label="Front"
              hint="The question. Required. Up to 2,000 characters."
              value={front}
              error={fieldError("front")}
              required
              multiline
              autoFocus={!chosen}
              onChange={(value) => {
                setFront(value);
                edited("front");
              }}
            />
            <PictureField
              id="card-front-pictures"
              label="Pictures on the front"
              hint="Optional. Up to 4, shown under the question."
              pictures={frontPictures}
              onChange={changeFrontPictures}
              adder={frontAdder}
              error={fieldError("frontPictures")}
              disabled={saving}
            />
          </div>
          <div className="card-form-side" onPaste={backAdder.onPaste}>
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
            <PictureField
              id="card-back-pictures"
              label="Pictures on the back"
              hint="Optional. Up to 4, shown with the answer."
              pictures={backPictures}
              onChange={changeBackPictures}
              adder={backAdder}
              error={fieldError("backPictures")}
              disabled={saving}
            />
          </div>
        </>
      )}

      <FormActions
        saveLabel={card ? "Save changes" : kind === "cloze" ? "Save cards" : "Save card"}
        saving={saving}
        error={formError}
        onCancel={onCancel}
      />
    </form>
  );
}
