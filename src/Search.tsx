import { Fragment, useEffect, useId, useRef, useState } from "react";
import {
  getDecks,
  getTags,
  searchEverything,
  toCommandError,
  type Deck,
  type SearchHit,
  type SearchResults,
  type SearchScope,
  type SnippetPiece,
  type TagSummary,
} from "./api";
import { ArrowRightIcon, DeckIcon, NoteIcon } from "./icons";
import { TagChips } from "./tagInput";
import { useFocusOnMount } from "./ui";
import { useT, type MessageKey } from "./i18n";

/** What is being searched for, kept while a result is open so Back returns to it. */
export type SearchCriteria = {
  query: string;
  scope: SearchScope;
  deckId: number | null;
  /** A tag's name: only what has it (or a tag below it) is found. */
  tag: string | null;
};

export const NEW_SEARCH: SearchCriteria = {
  query: "",
  scope: "all",
  deckId: null,
  tag: null,
};

/** Whether `criteria` asks for anything: some words, a typed `#tag`, or a chosen tag. */
function asksForSomething(criteria: SearchCriteria): boolean {
  return criteria.query.trim() !== "" || criteria.tag !== null;
}

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

const SCOPES: { value: SearchScope; label: MessageKey }[] = [
  { value: "all", label: "search.scopeAll" },
  { value: "cards", label: "search.scopeCards" },
  { value: "notes", label: "search.scopeNotes" },
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
  const { t, plural } = useT();
  const [criteria, setCriteria] = useState<SearchCriteria>(initial);
  const [state, setState] = useState<Found>({ status: "idle" });
  const [notice, setNotice] = useState<"search.archived" | null>(null);
  // The decks to filter cards by. If they can't be read, every deck is searched.
  const [decks, setDecks] = useState<Deck[]>([]);
  // The tags to filter by. If they can't be read, there's no list (a tag can
  // still be typed as #name).
  const [tags, setTags] = useState<TagSummary[]>([]);
  const inputRef = useFocusOnMount<HTMLInputElement>(returnTo === null);
  // The result to focus once results arrive; used once.
  const focusKey = useRef(returnTo);
  const scopeId = useId();
  const deckId = useId();
  const tagId = useId();

  useEffect(() => {
    let current = true;
    getDecks()
      .then((list) => {
        if (current) setDecks(list);
      })
      .catch(() => {});
    getTags()
      .then((list) => {
        if (current) setTags(list);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, []);

  useEffect(() => {
    if (!asksForSomething(criteria)) {
      setState({ status: "idle" });
      return;
    }
    let current = true;
    setState({ status: "searching" });
    searchEverything(criteria.query, criteria.scope, criteria.deckId, criteria.tag).then(
      (found) => {
        if (current) setState({ status: "ready", criteria, found });
      },
      (err) => {
        if (!current) return;
        const error = toCommandError(err);
        if (error.kind === "stale" && criteria.deckId !== null) {
          // The chosen deck was archived since the list was read: search
          // every deck instead, and say why.
          setNotice("search.archived");
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
  if (state.status === "searching") status = t("search.searching");
  else if (state.status === "ready") {
    const count = state.found.results.length;
    if (count === 0)
      status = trimmed === ""
        ? t("search.noTagged", { tag: criteria.tag ?? "" })
        : criteria.tag === null
          ? t("search.noMatches", { query: trimmed })
          : t("search.noTaggedMatches", { tag: criteria.tag, query: trimmed });
    else if (state.found.more)
      status = plural(count, "search.bestOne", "search.bestOther");
    else status = plural(count, "search.resultOne", "search.resultOther");
  }

  return (
    <section className="search-screen" aria-labelledby="search-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="search-heading" className="page-title">
            {t("search.title")}
          </h2>
          <p className="page-intro">
            {t("search.intro")}
          </p>
        </div>
      </div>

      <div className="card search-panel">
        <div className="field">
          <label htmlFor="search-everything" className="field-label">
            {t("search.query")}
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
                {t("search.clear")}
              </button>
            )}
          </div>
          <p id="search-everything-hint" className="field-hint">
            {t("search.hint")}
          </p>
        </div>

        <div className="search-filters">
          <div className="field">
            <label className="field-label" htmlFor={scopeId}>
              {t("search.show")}
            </label>
            <select
              id={scopeId}
              className="field-input"
              value={criteria.scope}
              onChange={(event) => change({ scope: event.target.value as SearchScope })}
            >
              {SCOPES.map((scope) => (
                <option key={scope.value} value={scope.value}>
                  {t(scope.label)}
                </option>
              ))}
            </select>
          </div>
          {criteria.scope === "cards" && decks.length > 0 && (
            <div className="field">
              <label className="field-label" htmlFor={deckId}>
                {t("search.deck")}
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
                <option value="">{t("search.allDecks")}</option>
                {decks.map((deck) => (
                  <option key={deck.id} value={deck.id}>
                    {deck.name}
                  </option>
                ))}
              </select>
            </div>
          )}
          {(tags.length > 0 || criteria.tag !== null) && (
            <div className="field">
              <label className="field-label" htmlFor={tagId}>
                {t("search.tag")}
              </label>
              <select
                id={tagId}
                className="field-input"
                value={criteria.tag ?? ""}
                onChange={(event) =>
                  change({ tag: event.target.value === "" ? null : event.target.value })
                }
              >
                <option value="">{t("search.anyTag")}</option>
                {criteria.tag !== null &&
                  !tags.some((tag) => tag.name === criteria.tag) && (
                    <option value={criteria.tag}>{criteria.tag}</option>
                  )}
                {tags.map((tag) => (
                  <option key={tag.id} value={tag.name}>
                    {tag.name}
                  </option>
                ))}
              </select>
            </div>
          )}
        </div>
      </div>

      {notice && (
        <p className="notice" role="status">
          {t(notice)}
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
        <ol className="card-list search-results" aria-label={t("search.results")}>
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
  const { t } = useT();
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
          <p className="item-meta">{isCard ? t("search.cardIn", { name: hit.deckName }) : t("search.note")}</p>
          {/* A heading, so screen-reader users can move between results by heading. */}
          <h3 id={titleId} className="note-item-title">
            <Snippet pieces={isCard ? hit.front : hit.title} />
          </h3>
          <p className="search-snippet">
            <Snippet pieces={isCard ? hit.back : hit.body} />
          </p>
          <TagChips tags={hit.tags} />
        </div>
      </div>
      <button
        ref={openRef}
        type="button"
        className="button button-small"
        aria-describedby={titleId}
        onClick={onOpen}
      >
        {isCard ? t("search.openDeck") : t("search.openNote")}
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
