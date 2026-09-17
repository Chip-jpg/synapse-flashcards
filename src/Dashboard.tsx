import { useEffect, useRef, useState } from "react";
import {
  cancelImport,
  confirmImport,
  exportData,
  getArchivedDecks,
  getDecks,
  prepareImport,
  toCommandError,
  type ArchivedDeck,
  type Deck,
  type DeckDetail,
} from "./api";
import { CreateDeckForm } from "./CreateDeckForm";
import { formatDateTime, Message, useFocusOnMount } from "./ui";
import { useStartReview } from "./useStartReview";

type DecksState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; decks: Deck[]; archived: ArchivedDeck[] };

/** Fetches the active and archived decks and describes the outcome; never rejects. */
async function loadDecks(): Promise<DecksState> {
  try {
    const [decks, archived] = await Promise.all([getDecks(), getArchivedDecks()]);
    return { status: "ready", decks, archived };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

export function Dashboard({
  focusOnLoad,
  notice: initialNotice,
  fromNotes,
  onStart,
  onOpen,
  onCreated,
  onOpenNotes,
  onRestored,
}: {
  focusOnLoad: boolean;
  /** A confirmation to announce when the dashboard appears (e.g. after archiving). */
  notice: string | null;
  /** The user just left the notes area, so focus returns to "Open notes". */
  fromNotes: boolean;
  onStart: (sessionId: number, deckName: string) => void;
  onOpen: (deckId: number) => void;
  onCreated: (deck: DeckDetail) => void;
  onOpenNotes: () => void;
  /** A backup replaced all study data, so every screen must reload. */
  onRestored: (fileName: string) => void;
}) {
  const [state, setState] = useState<DecksState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or refreshing counts).
  const [attempt, setAttempt] = useState(0);
  const [creating, setCreating] = useState(false);
  // After cancelling the create form, focus returns to "Create deck".
  const [cancelledCreate, setCancelledCreate] = useState(false);
  // Shown until the list reloads or the create form opens.
  const [notice, setNotice] = useState(initialNotice);
  // While a backup is being checked, waits for confirmation, or is being
  // restored, the deck list is inert: nothing in it may start work against a
  // database that is about to be replaced, or leave this screen mid-restore.
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    let current = true;
    loadDecks().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

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
      <p className="message" role="status">
        Loading your decks…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus={moveFocus} tone="alert" text={state.message}>
        <button type="button" className="button" onClick={reload}>
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
    <>
      <DeckList
        decks={state.decks}
        archived={state.archived}
        notice={notice}
        focus={focus === "notes" ? null : focus}
        inert={importing}
        onCreate={() => {
          setNotice(null);
          setCreating(true);
        }}
        onStart={onStart}
        onOpen={onOpen}
        onChanged={reload}
      />
      <NotesLink focus={focus === "notes"} inert={importing} onOpen={onOpenNotes} />
      {/* Its own section: exporting and restoring are about all your data, not about decks. */}
      <YourData onImporting={setImporting} onRestored={onRestored} />
    </>
  );
}

/** The way into the notes area. Inert, like the deck list, while an import runs. */
function NotesLink({
  focus,
  inert,
  onOpen,
}: {
  focus: boolean;
  inert: boolean;
  onOpen: () => void;
}) {
  const openRef = useFocusOnMount<HTMLButtonElement>(focus);

  return (
    <section className="cards" aria-labelledby="notes-link-heading" inert={inert}>
      <h2 id="notes-link-heading" className="section-title">
        Notes
      </h2>
      <p id="notes-link-hint" className="field-hint">
        Type up your own study notes and keep them here, then write cards from them.
      </p>
      <button
        ref={openRef}
        type="button"
        className="button"
        aria-describedby="notes-link-hint"
        onClick={onOpen}
      >
        Open notes
      </button>
    </section>
  );
}

function DeckList({
  decks,
  archived,
  notice,
  focus,
  inert,
  onCreate,
  onStart,
  onOpen,
  onChanged,
}: {
  decks: Deck[];
  archived: ArchivedDeck[];
  notice: string | null;
  /** What takes focus when the list appears, if anything. */
  focus: "list" | "create" | "notice" | null;
  /** An import is under way, so none of this can be used. */
  inert: boolean;
  onCreate: () => void;
  onStart: (sessionId: number, deckName: string) => void;
  onOpen: (deckId: number) => void;
  onChanged: () => void;
}) {
  const ref = useFocusOnMount<HTMLElement>(focus === "list");
  const createRef = useFocusOnMount<HTMLButtonElement>(focus === "create");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus === "notice");

  return (
    <section
      ref={ref}
      className="decks"
      tabIndex={-1}
      aria-labelledby="decks-heading"
      inert={inert}
    >
      <h2 id="decks-heading" className="section-title">
        Decks
      </h2>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}
      <button ref={createRef} type="button" className="button" onClick={onCreate}>
        Create deck
      </button>
      {decks.length === 0 ? (
        <p className="message" role="status">
          No decks yet.
        </p>
      ) : (
        <ul className="deck-list">
          {decks.map((deck) => (
            <li key={deck.id}>
              <DeckCard deck={deck} onStart={onStart} onOpen={onOpen} onChanged={onChanged} />
            </li>
          ))}
        </ul>
      )}
      {archived.length > 0 && <ArchivedDeckList decks={archived} />}
    </section>
  );
}

/**
 * Backing up and restoring everything Synapse stores. Rust opens the file
 * windows and does all the work. Only one of the two runs at a time.
 */
function YourData({
  onImporting,
  onRestored,
}: {
  /** Whether an import is under way (checking, waiting for confirmation, or restoring). */
  onImporting: (importing: boolean) => void;
  onRestored: (fileName: string) => void;
}) {
  const [running, setRunning] = useState<"export" | "import" | null>(null);

  return (
    <section className="cards" aria-labelledby="data-heading">
      <h2 id="data-heading" className="section-title">
        Your data
      </h2>
      <ExportData
        blocked={running === "import"}
        onRunning={(busy) => setRunning(busy ? "export" : null)}
      />
      <ImportData
        blocked={running === "export"}
        onRunning={(busy) => setRunning(busy ? "import" : null)}
        onImporting={onImporting}
        onRestored={onRestored}
      />
    </section>
  );
}

/** What the last export attempt did. Cancelling returns to `idle` silently. */
type ExportState =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "saved"; fileName: string }
  | { kind: "error"; message: string };

/** Saving a backup of everything Synapse stores. Rust asks where it goes. */
function ExportData({
  blocked,
  onRunning,
}: {
  /** An import is running, so this waits. */
  blocked: boolean;
  onRunning: (running: boolean) => void;
}) {
  const [state, setState] = useState<ExportState>({ kind: "idle" });
  // The button stays focusable while exporting (so focus is never lost), so
  // this guards against a second export starting on a repeated Enter.
  const busyRef = useRef(false);

  async function runExport() {
    if (busyRef.current || blocked) return;
    busyRef.current = true;
    onRunning(true);
    setState({ kind: "busy" });

    try {
      const outcome = await exportData();
      // Cancelling is a normal choice, not a failure, so say nothing.
      setState(
        outcome.status === "saved" ? { kind: "saved", fileName: outcome.fileName } : { kind: "idle" }
      );
    } catch (err) {
      setState({ kind: "error", message: toCommandError(err).message });
    }

    busyRef.current = false;
    onRunning(false);
  }

  // No `aria-busy` around this: it would hold back the status announcements.
  return (
    <div className="data-action">
      <p id="export-hint" className="field-hint">
        Save a copy of every deck, card, review, and note as a .zip file you keep.
      </p>
      <button
        type="button"
        className="button"
        aria-describedby="export-hint"
        aria-disabled={state.kind === "busy" || blocked}
        onClick={runExport}
      >
        Export data
      </button>
      {/* Announced as it changes; empty between attempts (see `.form-status:empty`). */}
      <p className="form-status" role="status">
        {state.kind === "busy" && "Preparing your export…"}
        {state.kind === "saved" && `Export saved as ${state.fileName}.`}
      </p>
      {state.kind === "error" && (
        <p className="message-error" role="alert">
          {state.message}
        </p>
      )}
    </div>
  );
}

/** A backup that passed Rust's checks and waits for the user's decision. */
type CheckedBackup = { token: number; fileName: string; exportedAt: string };

/** Where restoring a backup is up to. Cancelling returns to `idle` silently. */
type ImportState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "confirm"; backup: CheckedBackup; restoring: boolean }
  | { kind: "error"; message: string };

/**
 * Restoring a backup, which replaces all study data. Rust checks the chosen
 * file completely first; nothing changes until the user confirms.
 */
function ImportData({
  blocked,
  onRunning,
  onImporting,
  onRestored,
}: {
  /** An export is running, so this waits. */
  blocked: boolean;
  onRunning: (running: boolean) => void;
  onImporting: (importing: boolean) => void;
  onRestored: (fileName: string) => void;
}) {
  const [state, setState] = useState<ImportState>({ kind: "idle" });
  // Blocks a second check or restore immediately, before the re-render lands.
  const busyRef = useRef(false);
  const importRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);
  // The checked backup still waiting for an answer, if any.
  const waitingTokenRef = useRef<number | null>(null);
  const confirming = state.kind === "confirm";
  const checking = state.kind === "checking";
  const restoring = state.kind === "confirm" && state.restoring;
  const importing = checking || confirming;

  useEffect(() => {
    onImporting(importing);
  }, [importing, onImporting]);

  // Whether this is still on screen, for a check that finishes after it isn't.
  const mountedRef = useRef(false);

  // Leaving this screen while a backup still waits for an answer forgets it,
  // so its checked copy is removed now rather than at the next import.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      const token = waitingTokenRef.current;
      if (token !== null) cancelImport(token).catch(() => undefined);
    };
  }, []);

  // As with archiving a deck: focus moves to the question when it opens (so a
  // repeated Enter can't replace anything), and back to "Import data" when it
  // closes, whether the user kept their data or the restore failed.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      importRef.current?.focus();
    }
  }, [confirming]);

  function setBusy(busy: boolean) {
    busyRef.current = busy;
    onRunning(busy);
  }

  async function choose() {
    if (busyRef.current || blocked) return;
    setBusy(true);
    setState({ kind: "checking" });

    try {
      const check = await prepareImport();
      if (!mountedRef.current) {
        // The screen closed during the check (e.g. a review that was already
        // starting opened), so nobody can answer: remove the checked copy.
        if (check.status === "ready") cancelImport(check.token).catch(() => undefined);
        return;
      }
      waitingTokenRef.current = check.status === "ready" ? check.token : null;
      setState(
        check.status === "ready"
          ? {
              kind: "confirm",
              backup: {
                token: check.token,
                fileName: check.fileName,
                exportedAt: check.exportedAt,
              },
              restoring: false,
            }
          : { kind: "idle" }
      );
    } catch (err) {
      setState({ kind: "error", message: toCommandError(err).message });
    }

    setBusy(false);
  }

  async function restore(backup: CheckedBackup) {
    // Never while an export is still reading the data being replaced.
    if (busyRef.current || blocked) return;
    setBusy(true);
    // Confirming uses the token up, whether or not the restore succeeds.
    waitingTokenRef.current = null;
    setState({ kind: "confirm", backup, restoring: true });

    try {
      await confirmImport(backup.token);
      onRestored(backup.fileName);
      return; // Every screen reloads with the restored data.
    } catch (err) {
      setState({ kind: "error", message: toCommandError(err).message });
    }

    setBusy(false);
  }

  function keep(backup: CheckedBackup) {
    if (busyRef.current) return;
    waitingTokenRef.current = null;
    setState({ kind: "idle" });
    // Only removes Rust's checked copy; nothing was replaced, so there's
    // nothing to report if it fails (the next import clears it anyway).
    cancelImport(backup.token).catch(() => undefined);
  }

  // No `aria-busy` around this: it would hold back the status announcements.
  return (
    <div className="data-action">
      <p id="import-hint" className="field-hint">
        Replace everything in Synapse with a backup made by Export data. Synapse checks the file and
        asks you to confirm before anything changes.
      </p>

      {state.kind === "confirm" ? (
        <div className="card-item-confirm">
          <p ref={questionRef} id="import-question" className="notice" tabIndex={-1}>
            {`Replace all your study data with ${state.backup.fileName}, exported ${formatDateTime(
              state.backup.exportedAt
            )}? Every deck, card, review, session, and note in Synapse now will be replaced by the backup's, and this can't be undone. To keep a copy of your current data, choose Keep current data and export it first.`}
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button"
              aria-describedby="import-question"
              aria-disabled={restoring || blocked}
              onClick={() => restore(state.backup)}
            >
              Replace my data
            </button>
            <button
              type="button"
              className="button"
              aria-disabled={restoring}
              onClick={() => keep(state.backup)}
            >
              Keep current data
            </button>
          </div>
        </div>
      ) : (
        <button
          ref={importRef}
          type="button"
          className="button"
          aria-describedby="import-hint"
          aria-disabled={checking || blocked}
          onClick={choose}
        >
          Import data
        </button>
      )}

      {/* Announced as it changes; empty between attempts (see `.form-status:empty`). */}
      <p className="form-status" role="status">
        {checking && "Checking the backup…"}
        {restoring && "Restoring your backup…"}
      </p>
      {state.kind === "error" && (
        <p className="message-error" role="alert">
          {state.message}
        </p>
      )}
    </div>
  );
}

/** Archived decks, listed only as history: they have no actions. */
function ArchivedDeckList({ decks }: { decks: ArchivedDeck[] }) {
  return (
    <section className="cards" aria-labelledby="archived-heading">
      <h3 id="archived-heading" className="section-title">
        Archived decks
      </h3>
      <p className="field-hint">Kept for your history. Archived decks can't be reviewed or changed.</p>
      <ul className="card-list">
        {decks.map((deck) => {
          const cards = deck.cardCount;
          const archivedOn = new Date(deck.archivedAt).toLocaleDateString(undefined, {
            year: "numeric",
            month: "short",
            day: "numeric",
          });
          return (
            <li key={deck.id} className="card-item">
              <div>
                {/* A heading, like an active deck's name, so screen-reader
                    users can reach archived decks by heading navigation. */}
                <h4 className="archived-deck-name">{deck.name}</h4>
                {deck.description && <p className="deck-description">{deck.description}</p>}
              </div>
              <p className="field-hint">
                {`Archived ${archivedOn} · ${cards} ${cards === 1 ? "card" : "cards"} kept`}
              </p>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function DeckCard({
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
    <article className="card deck" aria-labelledby={nameId}>
      <div>
        <h3 id={nameId} className="deck-name">
          {deck.name}
        </h3>
        {deck.description && <p className="deck-description">{deck.description}</p>}
      </div>

      <p className="deck-due">
        {due === 0
          ? "No cards due right now."
          : `${due} ${due === 1 ? "card" : "cards"} due`}
      </p>

      {(due > 0 || canOpen) && (
        <div className="deck-actions">
          {due > 0 && (
            <button
              type="button"
              className="button button-primary"
              aria-describedby={nameId}
              aria-disabled={starting}
              onClick={start}
            >
              Start review
            </button>
          )}
          {canOpen && (
            <button
              type="button"
              className="button"
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
