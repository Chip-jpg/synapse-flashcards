import { Fragment, useEffect, useRef, useState } from "react";
import {
  searchSources,
  toCommandError,
  type SnippetPiece,
  type Source,
  type SourceHit,
  type SourceSearch,
} from "./api";
import { ArrowRightIcon, FileIcon } from "./icons";
import { useFocusOnMount } from "./ui";

/** The last search Rust answered: the trimmed query and what it found. */
type FoundSources = { query: string; found: SourceSearch };

export type SourceSearchState = {
  /** What's in the field, as typed. */
  query: string;
  found: FoundSources | null;
  error: string | null;
  setQuery: (query: string) => void;
  /** Asks again for the same query, e.g. after a source changed. */
  refresh: () => void;
};

/**
 * Searching the kept sources. Rust does the matching, since the list has no
 * pages. Each change asks again; an answer to an older query is dropped, so
 * a slow reply can't overwrite a newer one. This is a filter, not a form:
 * nothing is saved, so it never counts as an unsaved change.
 */
export function useSourceSearch(): SourceSearchState {
  const [query, setQueryState] = useState("");
  const [found, setFound] = useState<FoundSources | null>(null);
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
    searchSources(trimmed).then(
      (result) => {
        if (request === latest.current) setFound({ query: trimmed, found: result });
      },
      (err) => {
        if (request === latest.current) setError(toCommandError(err).message);
      },
    );
  }

  return { query, found, error, setQuery, refresh: () => setQuery(query) };
}

function counted(count: number, thing: string): string {
  return `${count} ${thing}${count === 1 ? "" : "s"}`;
}

/** The search field over the source list, and what it says it found. */
export function SourceSearchField({ search }: { search: SourceSearchState }) {
  const { query, found, error, setQuery } = search;
  const trimmed = query.trim();
  const inputRef = useRef<HTMLInputElement>(null);

  let status = "";
  if (trimmed !== "" && error === null) {
    if (found?.query !== trimmed) status = "Searching…";
    else if (found.found.sources.length === 0) status = "No sources match.";
    else {
      const count = found.found.sources.length;
      status = found.found.more
        ? `The first ${count} sources that match. Add words to narrow it down.`
        : `${counted(count, "source")} ${count === 1 ? "matches" : "match"}.`;
    }
  }

  return (
    <div className="field note-search">
      <label htmlFor="source-search" className="field-label">
        Search sources
      </label>
      <div className="note-search-row">
        <input
          ref={inputRef}
          id="source-search"
          type="search"
          className="field-input"
          value={query}
          autoComplete="off"
          spellCheck={false}
          aria-describedby="source-search-hint"
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
      <p id="source-search-hint" className="field-hint">
        Finds sources by title, file name, or the words on a page, including text recognised on
        scanned pages. Every word you type must start a word in the same place, ignoring capitals
        and accents. Deleted sources aren't searched.
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

/** Snippet text, with the words found marked. Text only: nothing is parsed as HTML. */
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

/** The sources a search found, each with the pages the words are on. */
export function SourceResults({
  found,
  describe,
  focusSourceId,
  onOpen,
}: {
  found: SourceSearch;
  describe: (source: Source) => string;
  /** The source whose *Open source* takes focus (coming back from it). */
  focusSourceId: number | null;
  /** Opens a source, at `page` with the words marked, or at its start. */
  onOpen: (sourceId: number, page: number | null) => void;
}) {
  if (found.sources.length === 0) return null;
  return (
    <ul className="card-list">
      {found.sources.map((hit) => (
        <li key={hit.source.id}>
          <SourceResult
            hit={hit}
            describe={describe}
            focusOpen={hit.source.id === focusSourceId}
            onOpen={(page) => onOpen(hit.source.id, page)}
          />
        </li>
      ))}
    </ul>
  );
}

function SourceResult({
  hit,
  describe,
  focusOpen,
  onOpen,
}: {
  hit: SourceHit;
  describe: (source: Source) => string;
  focusOpen: boolean;
  onOpen: (page: number | null) => void;
}) {
  const { source } = hit;
  const openRef = useFocusOnMount<HTMLButtonElement>(focusOpen);
  const titleId = `source-hit-${source.id}-title`;
  const thing = source.kind === "powerPoint" ? "slide" : "page";
  const Thing = thing === "slide" ? "Slide" : "Page";
  const left = hit.pageHits - hit.pages.length;

  return (
    <article className="card-item note-item source-hit" aria-labelledby={titleId}>
      <div className="note-item-main">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          <FileIcon />
        </span>
        <div className="search-hit-text">
          <h3 id={titleId} className="note-item-title">
            {source.title}
          </h3>
          <p className="item-meta">
            {[
              `${source.fileName} · ${describe(source)}`,
              hit.titleHit ? "Title or file name matches" : null,
              hit.pageHits > 0 ? `Found on ${counted(hit.pageHits, thing)}` : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>
      </div>
      <button
        ref={openRef}
        type="button"
        className="button button-small"
        aria-describedby={titleId}
        onClick={() => onOpen(null)}
      >
        Open source
        <ArrowRightIcon />
      </button>
      {hit.pages.length > 0 && (
        <ul className="source-hit-pages">
          {hit.pages.map((page) => (
            <li key={page.number} className="source-hit-page">
              <button
                type="button"
                className="button button-small"
                aria-label={`Open ${thing} ${page.number} of ${source.title}`}
                onClick={() => onOpen(page.number)}
              >
                {`${Thing} ${page.number}`}
              </button>
              <p className="search-snippet">
                {page.recognised && <span className="source-hit-label">Recognised text: </span>}
                <Snippet pieces={page.snippet} />
              </p>
            </li>
          ))}
          {left > 0 && (
            <li className="field-hint">{`And ${counted(left, `more ${thing}`)}: open the source to see them.`}</li>
          )}
        </ul>
      )}
    </article>
  );
}
