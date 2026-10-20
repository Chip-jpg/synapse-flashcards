import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import {
  getQuotedPage,
  locateQuotes,
  toCommandError,
  type QuoteSpot,
  type QuotedSource,
  type SnippetPiece,
} from "./api";
import { ArrowLeftIcon, ArrowRightIcon, NoteIcon } from "./icons";

/** "PDF", "Word document": what kind of file a source is, said plainly. */
export const KIND_NAMES = {
  pdf: "PDF",
  text: "Text file",
  markdown: "Markdown file",
  word: "Word document",
  powerPoint: "PowerPoint presentation",
} as const;

/** "page 7", "slides 3–4": where a quote is, in the words for its kind. */
export function pagesText(spot: QuoteSpot, slides: boolean): string {
  const thing = slides ? "slide" : "page";
  return spot.page === spot.lastPage
    ? `${thing} ${spot.page}`
    : `${thing}s ${spot.page}–${spot.lastPage}`;
}

/** A page's text as pieces, the quoted part marked. Never read as HTML. */
export function PageText({ pieces, id }: { pieces: SnippetPiece[]; id?: string }) {
  return (
    <p id={id} className="note-body source-page-text">
      {pieces.map((piece, index) =>
        piece.hit ? (
          <mark key={index} className="quote-mark">
            {piece.text}
          </mark>
        ) : (
          piece.text
        ),
      )}
    </p>
  );
}

/** Where a list of quotes are in the source of the note they came from. */
type Located = { source: QuotedSource; spots: Map<string, QuoteSpot | null> };

const LocatedQuotes = createContext<Located | null>(null);

/**
 * Finds the quotes of what's inside (suggested cards, quiz questions) in the
 * source their note was written from, so each can say which page it came
 * from (`QuotePageLink`). Nothing shows for a note without a source, and
 * nothing shows while Rust looks or if it can't: the quotes are still there.
 */
export function QuotesFromSource({
  noteId,
  quotes,
  children,
}: {
  noteId: number;
  quotes: string[];
  children: ReactNode;
}) {
  const [located, setLocated] = useState<Located | null>(null);
  // The quotes asked about, as one string, so a new array of the same quotes
  // doesn't ask again.
  const asked = JSON.stringify([...new Set(quotes)]);

  useEffect(() => {
    let current = true;
    const unique = JSON.parse(asked) as string[];
    if (unique.length === 0) return;
    locateQuotes(noteId, unique).then(
      (source) => {
        if (!current || source === null) return;
        setLocated({
          source,
          spots: new Map(unique.map((quote, index) => [quote, source.spots[index] ?? null])),
        });
      },
      () => {},
    );
    return () => {
      current = false;
    };
  }, [noteId, asked]);

  return <LocatedQuotes.Provider value={located}>{children}</LocatedQuotes.Provider>;
}

/**
 * "From page 7 of Lecture 3", for a quote whose note was written from a
 * source, opening that page with the quote marked. Nothing when the quote
 * can't be placed on a page.
 */
export function QuotePageLink({ quote }: { quote: string }) {
  const located = useContext(LocatedQuotes);
  const [open, setOpen] = useState(false);
  const spot = located?.spots.get(quote) ?? null;
  if (!located || !spot) return null;
  const slides = located.source.kind === "powerPoint";

  return (
    <>
      <button
        type="button"
        className="button button-quiet button-small quote-page-link"
        onClick={() => setOpen(true)}
      >
        <NoteIcon />
        {`From ${pagesText(spot, slides)} of ${located.source.title}`}
      </button>
      {open && (
        <SourcePageDialog
          sourceId={located.source.sourceId}
          title={located.source.title}
          pageCount={located.source.pageCount}
          slides={slides}
          page={spot.page}
          quote={quote}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

type PageState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; pieces: SnippetPiece[] };

/**
 * A page of a source over the screen, with `quote` marked where it is, and
 * the pages around it a press away. Modal, so drafts and answers behind it
 * stay as they are; Escape or *Close* returns to them.
 */
export function SourcePageDialog({
  sourceId,
  title,
  pageCount,
  slides,
  page: firstPage,
  quote,
  onClose,
}: {
  sourceId: number;
  title: string;
  pageCount: number;
  slides: boolean;
  page: number;
  quote: string;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [page, setPage] = useState(firstPage);
  const [state, setState] = useState<PageState>({ status: "loading" });
  const thing = slides ? "Slide" : "Page";

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || dialog.open) return;
    dialog.showModal();
    dialog.focus();
  }, []);

  useEffect(() => {
    let current = true;
    setState({ status: "loading" });
    getQuotedPage(sourceId, page, quote).then(
      (pieces) => {
        if (current) setState({ status: "ready", pieces });
      },
      (err) => {
        if (current) setState({ status: "error", message: toCommandError(err).message });
      },
    );
    return () => {
      current = false;
    };
  }, [sourceId, page, quote]);

  const empty = state.status === "ready" && state.pieces.every((piece) => piece.text.trim() === "");

  return (
    <dialog
      ref={dialogRef}
      className="dialog source-page-dialog"
      tabIndex={-1}
      aria-labelledby="source-page-title"
      onClose={onClose}
    >
      <div>
        <h2 id="source-page-title" className="form-title">
          {`${thing} ${page} of ${pageCount}`}
        </h2>
        <p className="item-meta">{title}</p>
      </div>
      <div className="source-page-scroll" tabIndex={0} aria-label={`${thing} ${page} text`}>
        {state.status === "loading" && (
          <p className="loading" role="status">
            {`Loading ${thing.toLowerCase()} ${page}…`}
          </p>
        )}
        {state.status === "error" && (
          <p className="message-error" role="alert">
            {state.message}
          </p>
        )}
        {state.status === "ready" &&
          (empty ? (
            <p className="empty">
              {`Synapse found no text on this ${thing.toLowerCase()}. It may be a scanned image.`}
            </p>
          ) : (
            <PageText pieces={state.pieces} />
          ))}
      </div>
      <div className="form-actions source-page-actions">
        <button
          type="button"
          className="button button-small"
          aria-disabled={page <= 1}
          onClick={() => {
            if (page > 1) setPage(page - 1);
          }}
        >
          <ArrowLeftIcon />
          {`Previous ${thing.toLowerCase()}`}
        </button>
        <button
          type="button"
          className="button button-small"
          aria-disabled={page >= pageCount}
          onClick={() => {
            if (page < pageCount) setPage(page + 1);
          }}
        >
          {`Next ${thing.toLowerCase()}`}
          <ArrowRightIcon />
        </button>
        <button
          type="button"
          className="button button-small"
          onClick={() => dialogRef.current?.close()}
        >
          Close
        </button>
      </div>
    </dialog>
  );
}
