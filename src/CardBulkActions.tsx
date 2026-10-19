import { useEffect, useRef, useState } from "react";
import {
  deleteFlashcards,
  moveFlashcards,
  toCommandError,
  type DeckCard,
  type DeckDetail as Deck,
} from "./api";
import { loadDeckChoices, type DeckChoices } from "./deckChoices";
import { SelectField } from "./forms";
import { ArrowRightIcon, TrashIcon } from "./icons";

/** "1 card", "3 cards". */
export function cardCount(count: number): string {
  return `${count} ${count === 1 ? "card" : "cards"}`;
}

/**
 * The cards whose front or back contains `query`, ignoring capitals and outer
 * spaces, in the deck's order. An empty query matches every card.
 *
 * This only narrows what the deck screen shows: the deck's cards are already
 * on screen, and nothing is changed by searching, so it isn't asked of Rust.
 */
export function matchingCards(cards: DeckCard[], query: string): DeckCard[] {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === "") return cards;
  return cards.filter(
    (card) =>
      card.front.toLocaleLowerCase().includes(needle) ||
      card.back.toLocaleLowerCase().includes(needle),
  );
}

/** Searching a deck's cards, with Clear beside the field. */
export function CardSearchField({
  query,
  shown,
  onChange,
}: {
  query: string;
  /** How many cards match `query`. */
  shown: number;
  onChange: (query: string) => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const searching = query.trim() !== "";

  return (
    <div className="field card-search">
      <label htmlFor="card-search" className="field-label">
        Search cards
      </label>
      <div className="note-search-row">
        <input
          ref={inputRef}
          id="card-search"
          type="search"
          className="field-input"
          value={query}
          autoComplete="off"
          spellCheck={false}
          aria-describedby="card-search-hint"
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape" && query !== "") {
              event.preventDefault();
              onChange("");
            }
          }}
        />
        {query !== "" && (
          <button
            type="button"
            className="button"
            onClick={() => {
              onChange("");
              inputRef.current?.focus();
            }}
          >
            Clear
          </button>
        )}
      </div>
      <p id="card-search-hint" className="field-hint">
        Finds this deck's cards whose front or back contains what you type, ignoring capitals.
      </p>
      <p className="form-status" role="status">
        {!searching
          ? ""
          : shown === 0
            ? "No cards match."
            : `${cardCount(shown)} ${shown === 1 ? "matches" : "match"}.`}
      </p>
    </div>
  );
}

/** What the selection bar is doing: offering its actions, or asking about one. */
type Step = { kind: "choose" } | { kind: "delete" } | { kind: "move" };

/**
 * The actions for the cards selected in a deck: delete them, or move them to
 * another deck, each after a confirmation that names how many. Rust checks
 * that every selected card is still an active card of this deck, and changes
 * all of them or none.
 */
export function SelectionActions({
  deck,
  selected,
  hidden,
  onClear,
  onDeleted,
  onMoved,
  onStale,
}: {
  deck: Deck;
  /** The selected cards' ids, in the deck's order. */
  selected: number[];
  /** How many of them the search is hiding. */
  hidden: number;
  onClear: () => void;
  onDeleted: (deck: Deck, deleted: number) => void;
  onMoved: (deck: Deck, moved: number, toDeckName: string) => void;
  /** Refused because this screen is out of date; `message` says what changed. */
  onStale: (message: string) => void;
}) {
  const [step, setStep] = useState<Step>({ kind: "choose" });
  const [choices, setChoices] = useState<DeckChoices>({ status: "loading" });
  const [target, setTarget] = useState("");
  const [targetError, setTargetError] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second press immediately, before the re-render lands.
  const busyRef = useRef(false);
  const deleteRef = useRef<HTMLButtonElement>(null);
  const moveRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // The step a question was last opened for, so closing it returns focus to
  // the button that opened it.
  const openedRef = useRef<Step["kind"] | null>(null);
  const count = selected.length;

  // As with deleting one card: focus moves to the question when it opens (so
  // a repeated Enter can't act on it), and back to its button if declined.
  useEffect(() => {
    if (step.kind !== "choose") {
      openedRef.current = step.kind;
      questionRef.current?.focus();
    } else if (openedRef.current === "delete") {
      deleteRef.current?.focus();
    } else if (openedRef.current === "move") {
      moveRef.current?.focus();
    }
  }, [step]);

  // The decks to move to are loaded when Move is chosen, so the list is as
  // fresh as it can be. This deck is left out: the cards are already here.
  useEffect(() => {
    if (step.kind !== "move") return;
    let current = true;
    setChoices({ status: "loading" });
    loadDeckChoices().then((loaded) => {
      if (!current) return;
      setChoices(
        loaded.status === "ready"
          ? {
              status: "ready",
              decks: loaded.decks.filter((other) => other.id !== deck.id),
            }
          : loaded,
      );
    });
    return () => {
      current = false;
    };
  }, [step, deck.id]);

  function back() {
    if (busyRef.current) return;
    setStep({ kind: "choose" });
    setTarget("");
    setTargetError(undefined);
    setError(null);
  }

  /** Sends one change, at most one at a time, and reports how it went. */
  async function send(change: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);

    try {
      await change();
      return; // The deck screen is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale(failure.message);
        return;
      }
      setError(failure.message);
    }

    busyRef.current = false;
    setBusy(false);
  }

  function confirmDelete() {
    send(async () => {
      const { changed, deck: updated } = await deleteFlashcards(deck.id, selected);
      onDeleted(updated, changed);
    });
  }

  function confirmMove() {
    if (busyRef.current) return;
    const to =
      choices.status === "ready" ? choices.decks.find((d) => String(d.id) === target) : null;
    if (!to) {
      setTargetError("Choose a deck to move the cards to.");
      document.getElementById("move-target")?.focus();
      return;
    }
    send(async () => {
      const { changed, deck: updated } = await moveFlashcards(deck.id, selected, to.id);
      onMoved(updated, changed, to.name);
    });
  }

  // Selected cards stay selected while the search changes, so say when some
  // of what would be changed isn't on screen.
  let hiddenNote = "";
  if (hidden === count) {
    hiddenNote = count === 1 ? " It's hidden by the search." : " They're all hidden by the search.";
  } else if (hidden > 0) {
    hiddenNote = ` ${hidden} of them ${hidden === 1 ? "is" : "are"} hidden by the search.`;
  }

  return (
    <div className="selection-bar" role="region" aria-label="Selected cards">
      <p className="selection-summary">
        <strong>{`${cardCount(count)} selected.`}</strong>
        {hiddenNote}
      </p>

      {step.kind === "choose" && (
        <div className="deck-actions">
          <button
            ref={moveRef}
            type="button"
            className="button button-small"
            onClick={() => setStep({ kind: "move" })}
          >
            <ArrowRightIcon />
            Move selected
          </button>
          <button
            ref={deleteRef}
            type="button"
            className="button button-small"
            onClick={() => setStep({ kind: "delete" })}
          >
            <TrashIcon />
            Delete selected
          </button>
          <button type="button" className="button button-quiet button-small" onClick={onClear}>
            Clear selection
          </button>
        </div>
      )}

      {step.kind === "delete" && (
        <div className="confirm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            {`Delete ${count === 1 ? "this card" : `these ${count} cards`}?${hiddenNote} ${count === 1 ? "It" : "They"} will leave this deck and won't appear in reviews. Schedules and review history are kept, and you can restore ${count === 1 ? "it" : "them"} from Deleted cards below.`}
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-danger-soft button-small"
              aria-disabled={busy}
              onClick={confirmDelete}
            >
              {`Delete ${cardCount(count)}`}
            </button>
            <button
              type="button"
              className="button button-small"
              aria-disabled={busy}
              onClick={back}
            >
              Keep cards
            </button>
          </div>
          <p className="form-status" role="status">
            {busy ? "Deleting…" : ""}
          </p>
        </div>
      )}

      {step.kind === "move" && (
        <div className="confirm confirm-calm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            {`Move ${count === 1 ? "this card" : `these ${count} cards`} to another deck?${hiddenNote} ${count === 1 ? "It keeps its" : "They keep their"} schedule and review history, and ${count === 1 ? "is" : "are"} due there exactly when ${count === 1 ? "it" : "they"} would have been here.`}
          </p>
          {choices.status === "loading" && (
            <p className="loading" role="status">
              Loading your decks…
            </p>
          )}
          {choices.status === "error" && (
            <p className="message-error" role="alert">
              {choices.message}
            </p>
          )}
          {choices.status === "ready" && choices.decks.length === 0 && (
            <p className="field-hint">
              There's no other deck to move them to. Create one from the Study Desk first.
            </p>
          )}
          {choices.status === "ready" && choices.decks.length > 0 && (
            <SelectField
              id="move-target"
              label="Move to"
              hint="Your other decks. The sample deck can't take cards."
              value={target}
              placeholder="Choose a deck"
              options={choices.decks.map((d) => ({
                value: String(d.id),
                label: d.name,
              }))}
              error={targetError}
              required
              onChange={(value) => {
                setTarget(value);
                setTargetError(undefined);
              }}
            />
          )}
          <div className="deck-actions">
            {choices.status === "ready" && choices.decks.length > 0 && (
              <button
                type="button"
                className="button button-primary button-small"
                aria-disabled={busy}
                onClick={confirmMove}
              >
                {`Move ${cardCount(count)}`}
              </button>
            )}
            <button
              type="button"
              className="button button-small"
              aria-disabled={busy}
              onClick={back}
            >
              Cancel
            </button>
          </div>
          <p className="form-status" role="status">
            {busy ? "Moving…" : ""}
          </p>
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
