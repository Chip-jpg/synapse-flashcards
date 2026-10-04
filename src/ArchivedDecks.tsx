import { useEffect, useRef, useState } from "react";
import { getArchivedDecks, toCommandError, unarchiveDeck, type ArchivedDeck } from "./api";
import { RestoreIcon } from "./icons";
import { Message, useFocusOnMount } from "./ui";

type ArchivedState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; decks: ArchivedDeck[] };

/**
 * How the list appears next. `key` changes every time, so the list remounts
 * and its focus target takes effect (as after unarchiving a deck).
 */
type ArchivedView = { key: number; notice: string | null };

/** Fetches the archived decks and describes the outcome; never rejects. */
async function loadArchived(): Promise<ArchivedState> {
  try {
    return { status: "ready", decks: await getArchivedDecks() };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * Archived decks: history, and the only place they appear. They can't be
 * reviewed or changed while archived; Unarchive puts one back on the Study
 * Desk exactly as it was. Opened from the sidebar, so it takes focus.
 */
export function ArchivedDecks() {
  const [state, setState] = useState<ArchivedState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or after unarchiving).
  const [attempt, setAttempt] = useState(0);
  const [view, setView] = useState<ArchivedView>({ key: 0, notice: null });

  useEffect(() => {
    let current = true;
    loadArchived().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  /**
   * Reloads the list, announcing `notice` and giving it focus if there is one.
   * Goes back to "loading" first: until the fresh list arrives the old one is
   * wrong, and leaving a just-unarchived deck here with a live Unarchive
   * button would invite a second, doomed press.
   */
  function reload(notice: string | null) {
    setState({ status: "loading" });
    setView((previous) => ({ key: previous.key + 1, notice }));
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading your archived decks…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <button type="button" className="button" onClick={() => reload(null)}>
          Retry
        </button>
      </Message>
    );
  }

  return (
    <ArchivedDeckList
      key={view.key}
      decks={state.decks}
      notice={view.notice}
      onUnarchived={(name) =>
        reload(`${name} unarchived. It's back on your Study Desk with its cards and review history.`)
      }
      onStale={() =>
        // Unarchiving failed because the deck isn't archived after all. Say
        // so, rather than reloading silently and leaving the press unexplained.
        reload("That deck wasn't archived after all. Here are your archived decks as they are now.")
      }
    />
  );
}

function ArchivedDeckList({
  decks,
  notice,
  onUnarchived,
  onStale,
}: {
  decks: ArchivedDeck[];
  notice: string | null;
  onUnarchived: (deckName: string) => void;
  onStale: () => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(notice === null);
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(notice !== null);

  return (
    <section
      ref={screenRef}
      className="archived"
      tabIndex={-1}
      aria-labelledby="archived-heading"
    >
      <div className="page-header">
        <div className="page-heading">
          <h2 id="archived-heading" className="page-title">
            Archived decks
          </h2>
          <p className="page-intro">
            Kept for your history. An archived deck isn't on your Study Desk and can't be reviewed
            or changed, but unarchiving it brings it back with its cards and review history
            unchanged. Cards you deleted stay deleted, and can be restored from the deck once it's
            back.
          </p>
        </div>
      </div>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}
      {decks.length === 0 ? (
        <p className="empty" role="status">
          No archived decks. Archiving a deck from its own screen keeps it here, with its cards and
          review history.
        </p>
      ) : (
        <ul className="card-list">
          {decks.map((deck) => (
            <li key={deck.id}>
              <ArchivedDeckItem deck={deck} onUnarchived={onUnarchived} onStale={onStale} />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** One archived deck, with the one action it has: a two-step Unarchive. */
function ArchivedDeckItem({
  deck,
  onUnarchived,
  onStale,
}: {
  deck: ArchivedDeck;
  onUnarchived: (deckName: string) => void;
  onStale: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [unarchiving, setUnarchiving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second unarchive immediately, before the re-render lands.
  const unarchivingRef = useRef(false);
  const unarchiveRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);
  const nameId = `archived-deck-${deck.id}-name`;
  const cards = deck.cardCount;
  const archivedOn = new Date(deck.archivedAt).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });

  // As with archiving a deck: the pressed button disappears either way, so
  // focus follows — to the question when it opens (reading it, and keeping a
  // repeated Enter from unarchiving), and back to Unarchive if it's kept.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      unarchiveRef.current?.focus();
    }
  }, [confirming]);

  async function confirmUnarchive() {
    if (unarchivingRef.current) return;
    unarchivingRef.current = true;
    setUnarchiving(true);
    setError(null);

    try {
      await unarchiveDeck(deck.id);
      onUnarchived(deck.name);
      return; // The list is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // Not archived after all: reload to show what's really there.
        onStale();
        return;
      }
      setError(failure.message);
    }

    unarchivingRef.current = false;
    setUnarchiving(false);
  }

  return (
    <div className="card-item card-item-recovery">
      <div>
        {/* A heading, like an active deck's name, so screen-reader
            users can reach archived decks by heading navigation. */}
        <h3 id={nameId} className="item-title">
          {deck.name}
        </h3>
        {deck.description && <p className="deck-description">{deck.description}</p>}
        <p className="item-meta">
          {`Archived ${archivedOn} · ${cards} ${cards === 1 ? "card" : "cards"} kept`}
        </p>
      </div>

      {confirming ? (
        <div className="confirm confirm-calm">
          <p
            ref={questionRef}
            id={`${nameId}-question`}
            className="confirm-question"
            tabIndex={-1}
          >
            {`Unarchive ${deck.name}? It goes back on your Study Desk with the same cards and ` +
              "review history, and its cards become due again on the dates they already had. " +
              "No review starts, and cards you deleted stay deleted until you restore them."}
          </p>
          <div className="deck-actions">
            {/* Both buttons name the deck as well as the question: several
                archived decks can have their question open at once, and
                otherwise every one of these reads identically. */}
            <button
              type="button"
              className="button button-primary button-small"
              aria-describedby={`${nameId} ${nameId}-question`}
              aria-disabled={unarchiving}
              onClick={confirmUnarchive}
            >
              Unarchive deck
            </button>
            <button
              type="button"
              className="button button-small"
              aria-describedby={nameId}
              aria-disabled={unarchiving}
              onClick={() => {
                if (!unarchiving) setConfirming(false);
              }}
            >
              Keep archived
            </button>
          </div>
          <p className="form-status" role="status">
            {unarchiving ? "Unarchiving…" : ""}
          </p>
        </div>
      ) : (
        <div className="deck-actions item-actions">
          <button
            ref={unarchiveRef}
            type="button"
            className="button button-small"
            aria-describedby={nameId}
            onClick={() => setConfirming(true)}
          >
            <RestoreIcon />
            Unarchive deck
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
