import { Fragment, useEffect, useId, useRef, useState } from "react";
import {
  getDecks,
  searchEverything,
  toCommandError,
  type Deck,
  type SearchHit,
  type SearchResults,
  type SearchScope,
  type SnippetPiece,
} from "./api";
import { ArrowRightIcon, DeckIcon, NoteIcon } from "./icons";
import { useFocusOnMount } from "./ui";

/** What is being searched for, kept while a result is open so Back returns to it. */
export type SearchCriteria = {
  query: string;
  scope: SearchScope;
  deckId: number | null;
};

export const NEW_SEARCH: SearchCriteria = {
  query: "",
  scope: "all",
  deckId: null,
};

/** The longest search Rust accepts, in characters. */
const MAX_QUERY_CHARS = 500;

/** Names a result, so the screen can focus it again on the way back. */
export function hitKey(hit: SearchHit): string {
  return hit.kind === "card" ? `card-${hit.cardId}` : `note-${hit.noteId}`;
}

type Found =
  | { status: "idle" }
  | { status: "searching" }
  | { status: "error"; message: string }
  | { status: "ready"; criteria: SearchCriteria; found: SearchResults };

const SCOPES: { value: SearchScope; label: string }[] = [
  { value: "all", label: "Cards and notes" },
  { value: "cards", label: "Cards" },
  { value: "notes", label: "Notes" },
];

/**
 * Search: every card in every active deck, front and back, and every note,
 * title and body, at once. Rust does the matching, from its full-text index,
 * and sends snippets as plain pieces of text; matched words are marked here
 * with `<mark>`, never by inserting markup. This is a filter, not a form:
 * nothing is saved, so it never counts as an unsaved change.
 *
 * Opened from the sidebar, the search field takes focus. Coming back from a
 * result, the same search runs again and that result's button takes focus.
 */
export function SearchScreen({
  initial,
  returnTo,
  onOpenCard,
  onOpenNote,
}: {
  initial: SearchCriteria;
  /** The result just left, from `hitKey`, or null when opened afresh. */
  returnTo: string | null;
  onOpenCard: (criteria: SearchCriteria, deckId: number, cardId: number, key: string) => void;
  onOpenNote: (criteria: SearchCriteria, noteId: number, key: string) => void;
}) {
  const [criteria, setCriteria] = useState<SearchCriteria>(initial);
  const [state, setState] = useState<Found>({ status: "idle" });
  const [notice, setNotice] = useState<string | null>(null);
  // The decks to filter cards by. If they can't be read, every deck is searched.
  const [decks, setDecks] = useState<Deck[]>([]);
  const inputRef = useFocusOnMount<HTMLInputElement>(returnTo === null);
  // The result to focus once results arrive; used once.
  const focusKey = useRef(returnTo);
  const scopeId = useId();
  const deckId = useId();

  useEffect(() => {
    let current = true;
    getDecks()
      .then((list) => {
        if (current) setDecks(list);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, []);

  useEffect(() => {
    if (criteria.query.trim() === "") {
      setState({ status: "idle" });
      return;
    }
    let current = true;
    setState({ status: "searching" });
    searchEverything(criteria.query, criteria.scope, criteria.deckId).then(
      (found) => {
        if (current) setState({ status: "ready", criteria, found });
      },
      (err) => {
        if (!current) return;
        const error = toCommandError(err);
        if (error.kind === "stale" && criteria.deckId !== null) {
          // The chosen deck was archived since the list was read: search
          // every deck instead, and say why.
          setNotice("That deck was archived, so every deck is searched now.");
          setDecks((list) => list.filter((deck) => deck.id !== criteria.deckId));
          setCriteria((previous) => ({ ...previous, deckId: null }));
          return;
        }
        setState({ status: "error", message: error.message });
      },
    );
    return () => {
      current = false;
    };
  }, [criteria]);

  // Coming back: the result left takes focus as it mounts (children's effects
  // run first). After the first answer nothing else is focused that way, and
  // if that result is gone the search field takes focus instead.
  useEffect(() => {
    if (focusKey.current === null || state.status === "searching" || state.status === "idle")
      return;
    const shown =
      state.status === "ready" &&
      state.found.results.some((hit) => hitKey(hit) === focusKey.current);
    focusKey.current = null;
    if (!shown) inputRef.current?.focus();
  }, [state, inputRef]);

  function change(next: Partial<SearchCriteria>) {
    setNotice(null);
    setCriteria((previous) => {
      const merged = { ...previous, ...next };
      // Only cards belong to a deck.
      if (merged.scope !== "cards") merged.deckId = null;
      return merged;
    });
  }

  const trimmed = criteria.query.trim();
  let status = "";
  if (state.status === "searching") status = "Searching…";
  else if (state.status === "ready") {
    const count = state.found.results.length;
    if (count === 0) status = `Nothing matches “${trimmed}”.`;
    else if (state.found.more)
      status = `The best ${count} results. Add a word to narrow them down.`;
    else status = count === 1 ? "1 result." : `${count} results.`;
  }

  return (
    <section className="search-screen" aria-labelledby="search-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="search-heading" className="page-title">
            Search
          </h2>
          <p className="page-intro">
            Every card in your decks, front and back, and every note, title and text, in one place.
          </p>
        </div>
      </div>

      <div className="card search-panel">
        <div className="field">
          <label htmlFor="search-everything" className="field-label">
            Search for
          </label>
          <div className="note-search-row">
            <input
              ref={inputRef}
              id="search-everything"
              type="search"
              className="field-input"
              value={criteria.query}
              maxLength={MAX_QUERY_CHARS}
              autoComplete="off"
              spellCheck={false}
              aria-describedby="search-everything-hint"
              onChange={(event) => change({ query: event.target.value })}
              onKeyDown={(event) => {
                if (event.key === "Escape" && criteria.query !== "") {
                  event.preventDefault();
                  change({ query: "" });
                }
              }}
            />
            {criteria.query !== "" && (
              <button
                type="button"
                className="button"
                onClick={() => {
                  change({ query: "" });
                  inputRef.current?.focus();
                }}
              >
                Clear
              </button>
            )}
          </div>
          <p id="search-everything-hint" className="field-hint">
            Finds words that start with what you type, ignoring capitals and accents; every word
            must match. Deleted cards and notes, and cards in archived decks, aren't searched.
          </p>
        </div>

        <div className="search-filters">
          <div className="field">
            <label className="field-label" htmlFor={scopeId}>
              Show
            </label>
            <select
              id={scopeId}
              className="field-input"
              value={criteria.scope}
              onChange={(event) => change({ scope: event.target.value as SearchScope })}
            >
              {SCOPES.map((scope) => (
                <option key={scope.value} value={scope.value}>
                  {scope.label}
                </option>
              ))}
            </select>
          </div>
          {criteria.scope === "cards" && decks.length > 0 && (
            <div className="field">
              <label className="field-label" htmlFor={deckId}>
                Deck
              </label>
              <select
                id={deckId}
                className="field-input"
                value={criteria.deckId ?? ""}
                onChange={(event) =>
                  change({
                    deckId: event.target.value === "" ? null : Number(event.target.value),
                  })
                }
              >
                <option value="">All decks</option>
                {decks.map((deck) => (
                  <option key={deck.id} value={deck.id}>
                    {deck.name}
                  </option>
                ))}
              </select>
            </div>
          )}
        </div>
      </div>

      {notice && (
        <p className="notice" role="status">
          {notice}
        </p>
      )}
      <p className="form-status search-status" role="status">
        {status}
      </p>
      {state.status === "error" && (
        <p className="message-error" role="alert">
          {state.message}
        </p>
      )}

      {state.status === "ready" && state.found.results.length > 0 && (
        <ol className="card-list search-results" aria-label="Results">
          {state.found.results.map((hit) => {
            const key = hitKey(hit);
            return (
              <li key={key}>
                <SearchResult
                  hit={hit}
                  focus={focusKey.current === key}
                  onOpen={() =>
                    hit.kind === "card"
                      ? onOpenCard(state.criteria, hit.deckId, hit.cardId, key)
                      : onOpenNote(state.criteria, hit.noteId, key)
                  }
                />
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

/** One result: what it is, its matched text, and the way in. */
function SearchResult({
  hit,
  focus,
  onOpen,
}: {
  hit: SearchHit;
  focus: boolean;
  onOpen: () => void;
}) {
  const openRef = useFocusOnMount<HTMLButtonElement>(focus);
  const titleId = `search-${hitKey(hit)}-title`;
  const isCard = hit.kind === "card";

  return (
    <article className="card-item search-hit" aria-labelledby={titleId}>
      <div className="search-hit-main">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          {isCard ? <DeckIcon /> : <NoteIcon />}
        </span>
        <div className="search-hit-text">
          <p className="item-meta">{isCard ? `Card in ${hit.deckName}` : "Note"}</p>
          {/* A heading, so screen-reader users can move between results by heading. */}
          <h3 id={titleId} className="note-item-title">
            <Snippet pieces={isCard ? hit.front : hit.title} />
          </h3>
          <p className="search-snippet">
            <Snippet pieces={isCard ? hit.back : hit.body} />
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
        {isCard ? "Open in deck" : "Open note"}
        <ArrowRightIcon />
      </button>
    </article>
  );
}

/** Snippet text, with matched words marked. Text only: nothing is parsed as HTML. */
function Snippet({ pieces }: { pieces: SnippetPiece[] }) {
  return (
    <>
      {pieces.map((piece, index) =>
        piece.hit ? (
          <mark key={index} className="search-mark">
            {piece.text}
          </mark>
        ) : (
          <Fragment key={index}>{piece.text}</Fragment>
        ),
      )}
    </>
  );
}
