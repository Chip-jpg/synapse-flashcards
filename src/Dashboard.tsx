import { useEffect, useState } from "react";
import {
  getDecks,
  getTagsDue,
  getToday,
  toCommandError,
  type Deck,
  type DeckDetail,
  type TagDue,
  type TodaySummary,
} from "./api";
import { CreateDeckForm } from "./CreateDeckForm";
import { ArrowRightIcon, BookIcon, CheckIcon, PlayIcon, PlusIcon } from "./icons";
import type { ReviewedTag } from "./ReviewSession";
import { Message, useFocusOnMount } from "./ui";
import { useStartReview } from "./useStartReview";

type DecksState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; decks: Deck[]; today: TodaySummary | null; tags: TagDue[] };

/**
 * Fetches the active decks, today's review count and the tags with cards due,
 * and describes the outcome; never rejects. Today's count and the tags are
 * side notes: if they can't be read, the desk shows without them rather than
 * failing.
 */
async function loadDecks(): Promise<DecksState> {
  try {
    const [decks, today, tags] = await Promise.all([
      getDecks(),
      getToday().catch(() => null),
      getTagsDue().catch(() => []),
    ]);
    return { status: "ready", decks, today, tags };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * The Study Desk: the deck due for review first, every active deck with its
 * due count, creating a deck, and the way into notes.
 */
export function Dashboard({
  focusOnLoad,
  notice: initialNotice,
  fromNotes,
  onStart,
  onStartTag = () => {},
  onOpen,
  onCreated,
  onOpenNotes,
}: {
  focusOnLoad: boolean;
  /** A confirmation to announce when the dashboard appears (e.g. after archiving). */
  notice: string | null;
  /** The user just left the notes area, so focus returns to "Open notes". */
  fromNotes: boolean;
  onStart: (sessionId: number, deckName: string) => void;
  /** Review the due cards with a tag, from every deck. */
  onStartTag?: (tag: ReviewedTag) => void;
  onOpen: (deckId: number) => void;
  onCreated: (deck: DeckDetail) => void;
  onOpenNotes: () => void;
}) {
  const [state, setState] = useState<DecksState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or refreshing counts).
  const [attempt, setAttempt] = useState(0);
  const [creating, setCreating] = useState(false);
  // After cancelling the create form, focus returns to "Create deck".
  const [cancelledCreate, setCancelledCreate] = useState(false);
  // Shown until the list reloads or the create form opens.
  const [notice, setNotice] = useState(initialNotice);

  useEffect(() => {
    let current = true;
    loadDecks().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  /**
   * Reloads the deck list. Goes back to "loading" first: until the fresh list
   * arrives the old due counts are wrong.
   */
  function reload() {
    setState({ status: "loading" });
    setCancelledCreate(false);
    setNotice(null);
    setAttempt((n) => n + 1);
  }

  const moveFocus = focusOnLoad || attempt > 0;
  const focus = cancelledCreate
    ? "create"
    : notice
      ? "notice"
      : fromNotes && attempt === 0
        ? "notes"
        : moveFocus
          ? "list"
          : null;

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading your decks…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus={moveFocus} tone="alert" text={state.message}>
        <button type="button" className="button" onClick={() => reload()}>
          Retry
        </button>
      </Message>
    );
  }

  if (creating) {
    return (
      <CreateDeckForm
        onCancel={() => {
          setCancelledCreate(true);
          setCreating(false);
        }}
        onCreated={onCreated}
      />
    );
  }

  return (
    <StudyDesk
      decks={state.decks}
      today={state.today}
      tags={state.tags}
      notice={notice}
      focus={focus}
      onCreate={() => {
        setNotice(null);
        setCreating(true);
      }}
      onStart={onStart}
      onStartTag={onStartTag}
      onOpen={onOpen}
      onOpenNotes={onOpenNotes}
      onChanged={() => reload()}
    />
  );
}

function StudyDesk({
  decks,
  today,
  tags,
  notice,
  focus,
  onCreate,
  onStart,
  onStartTag,
  onOpen,
  onOpenNotes,
  onChanged,
}: {
  decks: Deck[];
  today: TodaySummary | null;
  tags: TagDue[];
  notice: string | null;
  /** What takes focus when the desk appears, if anything. */
  focus: "list" | "create" | "notice" | "notes" | null;
  onCreate: () => void;
  onStart: (sessionId: number, deckName: string) => void;
  onStartTag: (tag: ReviewedTag) => void;
  onOpen: (deckId: number) => void;
  onOpenNotes: () => void;
  onChanged: () => void;
}) {
  const ref = useFocusOnMount<HTMLElement>(focus === "list");
  const createRef = useFocusOnMount<HTMLButtonElement>(focus === "create");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus === "notice");
  // The first deck with cards due is the one place to begin, so it leads the
  // desk with the screen's only filled Start review button.
  const next = decks.find((deck) => deck.dueCount > 0);

  return (
    <section ref={ref} className="desk" tabIndex={-1} aria-labelledby="desk-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="desk-heading" className="page-title">
            Study Desk
          </h2>
          <p className="page-intro">
            Review the cards that are due, or open a deck you made to add and edit its cards.
          </p>
          {today && <TodayLine today={today} />}
        </div>
        <button ref={createRef} type="button" className="button" onClick={onCreate}>
          <PlusIcon />
          Create deck
        </button>
      </div>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {decks.length === 0 ? (
        <p className="empty" role="status">
          No decks yet. Choose Create deck to make one.
        </p>
      ) : (
        <>
          {next ? (
            <NextReview deck={next} onStart={onStart} onOpen={onOpen} onChanged={onChanged} />
          ) : (
            <NothingDue />
          )}

          <section className="deck-section" aria-labelledby="deck-list-heading">
            <h3 id="deck-list-heading" className="section-title">
              Your decks <span className="count">{decks.length}</span>
            </h3>
            <ul className="deck-list">
              {decks.map((deck) => (
                <li key={deck.id}>
                  <DeckRow deck={deck} onStart={onStart} onOpen={onOpen} onChanged={onChanged} />
                </li>
              ))}
            </ul>
          </section>

          <TagReviews tags={tags} onStart={onStartTag} />
        </>
      )}

      <NotesLink focus={focus === "notes"} onOpen={onOpenNotes} />
    </section>
  );
}

/**
 * How much reviewing happened today: ratings recorded, and the different cards
 * they were for, which differ once a card is rated more than once.
 */
function TodayLine({ today }: { today: TodaySummary }) {
  const { reviews, cards } = today;
  return (
    <p className="today-line">
      {reviews === 0
        ? "No reviews yet today"
        : `Today: ${reviews} ${reviews === 1 ? "review" : "reviews"} · ${cards} ${cards === 1 ? "card" : "cards"}`}
    </p>
  );
}

/** The first deck with cards due, offered as the place to begin. */
function NextReview({
  deck,
  onStart,
  onOpen,
  onChanged,
}: {
  deck: Deck;
  onStart: (sessionId: number, deckName: string) => void;
  onOpen: (deckId: number) => void;
  onChanged: () => void;
}) {
  const { starting, error, start } = useStartReview(deck.id, deck.name, onStart, onChanged);
  const due = deck.dueCount;

  return (
    <section className="focus-card" aria-labelledby="next-review-name">
      {/* The name comes first, so moving by heading reaches it before its
          details; the tag and count are drawn above it (see `.focus-meta`). */}
      <div>
        <h3 id="next-review-name" className="focus-title">
          {deck.name}
        </h3>
        {deck.description && <p className="focus-description">{deck.description}</p>}
      </div>
      <div className="focus-meta">
        <p className="tag tag-primary">Ready to review</p>
        <p className="due-line">
          <span className="dot" aria-hidden="true" />
          {`${due} ${due === 1 ? "card" : "cards"} due`}
        </p>
      </div>
      <div className="focus-actions">
        <button
          type="button"
          className="button button-primary button-large focus-start"
          aria-describedby="next-review-name"
          aria-disabled={starting}
          onClick={start}
        >
          <PlayIcon />
          Start review
        </button>
        {/* The sample deck can be reviewed but not opened for editing. */}
        {!deck.isSample && (
          <button
            type="button"
            className="button button-large"
            aria-describedby="next-review-name"
            onClick={() => onOpen(deck.id)}
          >
            Open deck
          </button>
        )}
      </div>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** In place of the next review when no deck has cards due. */
function NothingDue() {
  return (
    <section className="focus-card focus-card-calm" aria-labelledby="nothing-due-heading">
      <div>
        <h3 id="nothing-due-heading" className="focus-title">
          No cards are due right now
        </h3>
        <p className="focus-description">
          Cards become due again on the dates FSRS scheduled for them. In the meantime, you can
          open a deck you made to add or edit cards.
        </p>
      </div>
      <div className="focus-meta">
        <p className="tag tag-sage">
          <CheckIcon />
          All caught up
        </p>
      </div>
    </section>
  );
}

/**
 * Reviewing by tag: each tag with cards due, from however many decks, and a
 * way to review just those. Shown only when some tag has cards due.
 */
function TagReviews({
  tags,
  onStart,
}: {
  tags: TagDue[];
  onStart: (tag: ReviewedTag) => void;
}) {
  const due = tags.filter((tag) => tag.dueCount > 0);
  if (due.length === 0) return null;

  return (
    <section className="deck-section" aria-labelledby="tag-review-heading">
      <h3 id="tag-review-heading" className="section-title">
        Review by tag <span className="count">{due.length}</span>
      </h3>
      <p className="field-hint">
        Review the due cards with one tag, from every deck that has them.
      </p>
      <ul className="deck-list">
        {due.map((tag) => {
          const nameId = `tag-review-${tag.id}`;
          return (
            <li key={tag.id}>
              <article className="deck-row" aria-labelledby={nameId}>
                <div className="deck-row-main">
                  <h4 id={nameId} className="deck-name">
                    {tag.name}
                  </h4>
                  <p className="due-line">
                    <span className="dot" aria-hidden="true" />
                    {`${tag.dueCount} ${tag.dueCount === 1 ? "card" : "cards"} due in ${tag.decks} ${tag.decks === 1 ? "deck" : "decks"}`}
                  </p>
                </div>
                <div className="deck-actions">
                  <button
                    type="button"
                    className="button button-tonal button-small"
                    aria-describedby={nameId}
                    onClick={() => onStart({ id: tag.id, name: tag.name })}
                  >
                    <PlayIcon />
                    Review tag
                  </button>
                </div>
              </article>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** The way into the notes area. */
function NotesLink({ focus, onOpen }: { focus: boolean; onOpen: () => void }) {
  const openRef = useFocusOnMount<HTMLButtonElement>(focus);

  return (
    <section className="panel panel-row" aria-labelledby="notes-link-heading">
      <span className="section-icon section-icon-amber" aria-hidden="true">
        <BookIcon />
      </span>
      <div className="panel-row-text">
        <h3 id="notes-link-heading" className="section-title">
          Notes
        </h3>
        <p id="notes-link-hint" className="field-hint">
          Type up your own study notes and keep them here, then write cards from them.
        </p>
      </div>
      <button
        ref={openRef}
        type="button"
        className="button button-small"
        aria-describedby="notes-link-hint"
        onClick={onOpen}
      >
        Open notes
        <ArrowRightIcon />
      </button>
    </section>
  );
}

function DeckRow({
  deck,
  onStart,
  onOpen,
  onChanged,
}: {
  deck: Deck;
  onStart: (sessionId: number, deckName: string) => void;
  onOpen: (deckId: number) => void;
  onChanged: () => void;
}) {
  const { starting, error, start } = useStartReview(deck.id, deck.name, onStart, onChanged);
  const nameId = `deck-${deck.id}-name`;
  const due = deck.dueCount;
  // The sample deck can be reviewed but not opened for editing.
  const canOpen = !deck.isSample;

  return (
    <article className="deck-row" aria-labelledby={nameId}>
      {/* The name comes first, so moving by heading reaches it before its due
          count; the count is drawn above it (see `.deck-row-main`). */}
      <div className="deck-row-main">
        <h4 id={nameId} className="deck-name">
          {deck.name}
        </h4>
        <p className={due === 0 ? "due-line due-line-idle" : "due-line"}>
          {due > 0 && <span className="dot" aria-hidden="true" />}
          {due === 0
            ? "No cards due right now"
            : `${due} ${due === 1 ? "card" : "cards"} due`}
        </p>
        {deck.description && <p className="deck-description">{deck.description}</p>}
      </div>

      {(due > 0 || canOpen) && (
        <div className="deck-actions">
          {due > 0 && (
            <button
              type="button"
              className="button button-tonal button-small"
              aria-describedby={nameId}
              aria-disabled={starting}
              onClick={start}
            >
              <PlayIcon />
              Start review
            </button>
          )}
          {canOpen && (
            <button
              type="button"
              className="button button-small"
              aria-describedby={nameId}
              onClick={() => onOpen(deck.id)}
            >
              Open deck
            </button>
          )}
        </div>
      )}

      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </article>
  );
}
