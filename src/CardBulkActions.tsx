import { useEffect, useRef, useState } from "react";
import {
  deleteFlashcards,
  moveFlashcards,
  restoreFlashcards,
  tagFlashcards,
  toCommandError,
  type DeckCard,
  type DeckDetail as Deck,
} from "./api";
import { loadDeckChoices, type DeckChoices } from "./deckChoices";
import { SelectField } from "./forms";
import { ArrowRightIcon, RestoreIcon, TagIcon, TrashIcon } from "./icons";
import { hasTag, TagField, tagsIn } from "./tagInput";

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

/**
 * The tags `cards` have, each once (ignoring capitals), sorted by name, with
 * how many of the cards `taggedCards` shows for it (a tag below it, such as
 * `bio::cells` for `bio`, counts).
 */
export function tagCounts(cards: DeckCard[]): { name: string; cards: number }[] {
  return tagsIn(cards).map(({ name }) => ({ name, cards: taggedCards(cards, name).length }));
}

/** The cards with the tag `tag` or one below it, ignoring capitals; every card for "". */
export function taggedCards(cards: DeckCard[], tag: string): DeckCard[] {
  return tag === "" ? cards : cards.filter((card) => hasTag(card.tags, tag));
}

/**
 * Showing only a deck's cards with one tag, beside *Search cards*. Like the
 * search, it only narrows what is shown.
 */
export function CardTagFilter({
  tags,
  tag,
  onChange,
}: {
  /** The deck's tags, from `tagCounts`. */
  tags: { name: string; cards: number }[];
  /** The tag shown, or "" for every card. */
  tag: string;
  onChange: (tag: string) => void;
}) {
  return (
    <div className="card-tag-filter">
      <SelectField
        id="card-tag-filter"
        label="Show cards tagged"
        hint="Only this deck's cards with the tag you choose, or a tag below it (bio::cells for bio)."
        value={tag}
        placeholder="Any tag or none"
        options={tags.map((t) => ({ value: t.name, label: `${t.name} (${cardCount(t.cards)})` }))}
        onChange={onChange}
      />
    </div>
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
type Step = { kind: "choose" } | { kind: "delete" } | { kind: "move" } | { kind: "tag" };

/**
 * The actions for the cards selected in a deck: tag them (adding tags, or
 * taking one off), delete them, or move them to another deck, each after a
 * step that names how many. Rust checks that every selected card is still an
 * active card of this deck, and changes all of them or none.
 */
export function SelectionActions({
  deck,
  selected,
  hidden,
  hiddenBy = "the search",
  onClear,
  onDeleted,
  onMoved,
  onTagged,
  onStale,
}: {
  deck: Deck;
  /** The selected cards' ids, in the deck's order. */
  selected: number[];
  /** How many of them the search (or tag filter) is hiding. */
  hidden: number;
  /** What is hiding them, e.g. "the tag filter". */
  hiddenBy?: string;
  onClear: () => void;
  onDeleted: (deck: Deck, deleted: number) => void;
  onMoved: (deck: Deck, moved: number, toDeckName: string) => void;
  /** The selected cards' tags were changed: `changed` of them had a change. */
  onTagged: (deck: Deck, changed: number) => void;
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
  const tagRef = useRef<HTMLButtonElement>(null);
  const [addTags, setAddTags] = useState<string[]>([]);
  const [removeTag, setRemoveTag] = useState("");
  const [tagsError, setTagsError] = useState<string | undefined>(undefined);
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
    } else if (openedRef.current === "tag") {
      tagRef.current?.focus();
    }
  }, [step]);

  // A refused tag is said under the field, which then takes focus (once the
  // field is usable again).
  useEffect(() => {
    if (tagsError) document.getElementById("tag-selected-add")?.focus();
  }, [tagsError]);

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
    setAddTags([]);
    setRemoveTag("");
    setTagsError(undefined);
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
      if (failure.field === "tags") {
        // A refused tag name: said under the field, which takes focus.
        setTagsError(failure.message);
      } else {
        setError(failure.message);
      }
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

  function confirmTag() {
    if (busyRef.current) return;
    const add = addTags;
    const remove = removeTag === "" ? [] : [removeTag];
    if (add.length === 0 && remove.length === 0) {
      setTagsError("Type a tag to add, or choose one to take off.");
      return;
    }
    send(async () => {
      const { changed, deck: updated } = await tagFlashcards(deck.id, selected, add, remove);
      onTagged(updated, changed);
    });
  }

  // The tags the selected cards have, which can be taken off them.
  const selectedSet = new Set(selected);
  const selectedTags = tagsIn(deck.cards.filter((card) => selectedSet.has(card.id)));

  // Selected cards stay selected while the search changes, so say when some
  // of what would be changed isn't on screen.
  let hiddenNote = "";
  if (hidden === count) {
    hiddenNote =
      count === 1 ? ` It's hidden by ${hiddenBy}.` : ` They're all hidden by ${hiddenBy}.`;
  } else if (hidden > 0) {
    hiddenNote = ` ${hidden} of them ${hidden === 1 ? "is" : "are"} hidden by ${hiddenBy}.`;
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
            ref={tagRef}
            type="button"
            className="button button-small"
            onClick={() => setStep({ kind: "tag" })}
          >
            <TagIcon />
            Tag selected
          </button>
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

      {step.kind === "tag" && (
        <div className="confirm confirm-calm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            {`Change the tags of ${count === 1 ? "this card" : `these ${count} cards`}?${hiddenNote} Tags you add join the ones ${count === 1 ? "it has" : "each has"}; nothing else about ${count === 1 ? "it" : "them"} changes.`}
          </p>
          <TagField
            id="tag-selected-add"
            label="Add tags"
            hint="One word each, such as exam or bio::cells; a space or comma starts the next."
            tags={addTags}
            onChange={(next) => {
              setAddTags(next);
              setTagsError(undefined);
            }}
            error={tagsError}
            disabled={busy}
          />
          {selectedTags.length > 0 && (
            <SelectField
              id="tag-selected-remove"
              label="Take off a tag"
              hint="Optional. Removed from every selected card that has it."
              value={removeTag}
              placeholder="Don't take one off"
              options={selectedTags.map((t) => ({
                value: t.name,
                label: `${t.name} (${cardCount(t.count)})`,
              }))}
              onChange={(value) => {
                setRemoveTag(value);
                setTagsError(undefined);
              }}
            />
          )}
          <div className="deck-actions">
            <button
              type="button"
              className="button button-primary button-small"
              aria-disabled={busy}
              onClick={confirmTag}
            >
              {`Save tags on ${cardCount(count)}`}
            </button>
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
            {busy ? "Saving tags…" : ""}
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

/**
 * The action for the deleted cards selected in a deck: restore them, after a
 * confirmation that names how many. Rust checks that every one is still a
 * deleted card of this deck that can come back, and restores all of them or
 * none.
 */
export function RestoreSelected({
  deckId,
  selected,
  onClear,
  onRestored,
  onStale,
}: {
  deckId: number;
  /** The selected deleted cards' ids, in the list's order. */
  selected: number[];
  onClear: () => void;
  onRestored: (deck: Deck, restored: number) => void;
  /** Refused because this screen is out of date; `message` says what changed. */
  onStale: (message: string) => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second press immediately, before the re-render lands.
  const busyRef = useRef(false);
  const restoreRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);
  const count = selected.length;

  // As with restoring one card: focus moves to the question when it opens (so
  // a repeated Enter can't restore), and back to its button if declined.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      restoreRef.current?.focus();
    }
  }, [confirming]);

  async function confirmRestore() {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);

    try {
      const { changed, deck } = await restoreFlashcards(deckId, selected);
      onRestored(deck, changed);
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

  return (
    <div className="selection-bar" role="region" aria-label="Selected deleted cards">
      <p className="selection-summary">
        <strong>{`${cardCount(count)} selected.`}</strong>
      </p>

      {confirming ? (
        <div className="confirm confirm-calm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            {`Restore ${count === 1 ? "this card" : `these ${count} cards`}? ${count === 1 ? "It goes" : "They go"} back into this deck with the same ${count === 1 ? "schedule" : "schedules"} and review history ${count === 1 ? "it" : "they"} had, and ${count === 1 ? "becomes" : "each becomes"} due again only on the date it already had.`}
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-primary button-small"
              aria-disabled={busy}
              onClick={confirmRestore}
            >
              {`Restore ${cardCount(count)}`}
            </button>
            <button
              type="button"
              className="button button-small"
              aria-disabled={busy}
              onClick={() => {
                if (!busyRef.current) setConfirming(false);
              }}
            >
              Keep deleted
            </button>
          </div>
          <p className="form-status" role="status">
            {busy ? "Restoring…" : ""}
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
            Restore selected
          </button>
          <button type="button" className="button button-quiet button-small" onClick={onClear}>
            Clear selection
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
