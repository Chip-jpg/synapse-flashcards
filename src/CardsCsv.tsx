import { useEffect, useRef, useState } from "react";
import {
  exportCardsCsv,
  getDecks,
  importCardsCsv,
  readCardsCsv,
  toCommandError,
  type CardsFileRow,
  type Deck,
  type ReadCardsFile,
} from "./api";
import { SelectField } from "./forms";
import { DeckIcon, DownloadIcon, UploadIcon } from "./icons";
import { useUnsavedChanges } from "./unsaved";

function cardCount(count: number): string {
  return count === 1 ? "1 card" : `${count.toLocaleString("en-US")} cards`;
}

function rowCount(count: number): string {
  return count === 1 ? "1 row" : `${count.toLocaleString("en-US")} rows`;
}

const DELIMITER_NAMES = { comma: "commas", semicolon: "semicolons", tab: "tabs" };

/**
 * Cards in spreadsheet files, on the Backup & restore screen: importing
 * front/back cards from a CSV or TSV file into a deck, and exporting a deck's
 * cards as CSV. Rust opens the file windows, reads and checks every row, and
 * saves; this shows what it found and asks before anything is added.
 */
export function CardsCsv({
  blocked,
  onRunning,
  decksChanged = 0,
}: {
  /** A backup is being exported or restored, so this waits. */
  blocked: boolean;
  onRunning: (running: boolean) => void;
  /** Changes when decks were added elsewhere on the screen, to reload them. */
  decksChanged?: number;
}) {
  const [decks, setDecks] = useState<Deck[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  async function loadDecks() {
    try {
      setDecks(await getDecks());
      setLoadError(null);
    } catch (err) {
      setLoadError(toCommandError(err).message);
    }
  }

  useEffect(() => {
    loadDecks();
  }, [decksChanged]);

  return (
    <section className="panel" aria-labelledby="cards-csv-heading">
      <div className="section-heading">
        <span className="section-icon" aria-hidden="true">
          <DeckIcon />
        </span>
        <h3 id="cards-csv-heading" className="section-title">
          Cards in a spreadsheet
        </h3>
      </div>
      <p className="field-hint">
        Add cards from a CSV or TSV file, with the front in the first column and the back in the
        second, or save a deck's cards as a CSV file. A third column headed Tags gives each card its
        tags, separated by spaces. Only the text and tags of each card are copied, not its schedule
        or reviews, so this isn't a backup.
      </p>
      {loadError ? (
        <p className="message-error" role="alert">
          {loadError}
        </p>
      ) : decks === null ? (
        <p className="form-status" role="status">
          Loading your decks…
        </p>
      ) : (
        <>
          <ImportCards
            decks={decks.filter((deck) => !deck.isSample)}
            blocked={blocked}
            onRunning={onRunning}
            onStale={loadDecks}
          />
          <ExportCards decks={decks} blocked={blocked} onRunning={onRunning} />
        </>
      )}
    </section>
  );
}

/** Where importing a file of cards is up to. */
type ImportState =
  | { kind: "idle" }
  | { kind: "reading" }
  | {
      kind: "preview";
      deck: Deck;
      file: ReadCardsFile;
      skipDuplicates: boolean;
      saving: boolean;
    };

/** The cards of a previewed file that would be sent to save, with their rows. */
function rowsToAdd(rows: CardsFileRow[], skipDuplicates: boolean): CardsFileRow[] {
  return rows.filter((row) => row.problem === null && !(skipDuplicates && row.duplicate));
}

/**
 * Importing cards: choose a deck, then a file. Rust reads and checks every
 * row; the preview shows which can be added, and nothing is saved until
 * the user presses Add. A preview on screen counts as unsaved changes.
 */
function ImportCards({
  decks,
  blocked,
  onRunning,
  onStale,
}: {
  /** The decks cards can be added to: active, not the sample deck. */
  decks: Deck[];
  blocked: boolean;
  onRunning: (running: boolean) => void;
  /** The decks on screen are out of date. */
  onStale: () => void;
}) {
  const [deckId, setDeckId] = useState("");
  const [deckError, setDeckError] = useState<string | undefined>();
  const [state, setState] = useState<ImportState>({ kind: "idle" });
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  // Blocks a second read or save immediately, before the re-render lands.
  const busyRef = useRef(false);
  const mountedRef = useRef(false);
  const chooseRef = useRef<HTMLButtonElement>(null);
  const previewRef = useRef<HTMLParagraphElement>(null);
  const doneRef = useRef<HTMLParagraphElement>(null);
  const previewing = state.kind === "preview";

  useUnsavedChanges(previewing);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Focus follows the step: to the preview's summary when it opens, and to
  // what happened when it closes.
  useEffect(() => {
    if (previewing) previewRef.current?.focus();
  }, [previewing]);
  useEffect(() => {
    if (done) doneRef.current?.focus();
  }, [done]);

  function setBusy(busy: boolean) {
    busyRef.current = busy;
    onRunning(busy);
  }

  async function choose() {
    if (busyRef.current || blocked) return;
    const deck = decks.find((each) => String(each.id) === deckId);
    if (!deck) {
      setDeckError("Choose the deck to add the cards to.");
      document.getElementById("cards-import-deck")?.focus();
      return;
    }
    setBusy(true);
    setError(null);
    setDone(null);
    setState({ kind: "reading" });

    try {
      const file = await readCardsCsv(deck.id);
      if (!mountedRef.current) return;
      setState(
        file.status === "read"
          ? { kind: "preview", deck, file, skipDuplicates: true, saving: false }
          : { kind: "idle" }
      );
    } catch (err) {
      const refused = toCommandError(err);
      setState({ kind: "idle" });
      setError(refused.message);
      if (refused.kind === "stale") onStale();
    }

    setBusy(false);
  }

  async function add(preview: ImportState & { kind: "preview" }) {
    if (busyRef.current || blocked) return;
    const rows = rowsToAdd(preview.file.rows, preview.skipDuplicates);
    if (rows.length === 0) return;
    setBusy(true);
    setError(null);
    setState({ ...preview, saving: true });

    try {
      const saved = await importCardsCsv(
        preview.deck.id,
        rows.map((row) =>
          preview.file.tagsColumn
            ? { front: row.front, back: row.back, tags: row.tags ?? [] }
            : { front: row.front, back: row.back },
        ),
        preview.skipDuplicates
      );
      setState({ kind: "idle" });
      const skipped =
        saved.skipped === 0
          ? ""
          : ` ${cardCount(saved.skipped)} already in the deck ${saved.skipped === 1 ? "was" : "were"} left out.`;
      setDone(
        `Added ${cardCount(saved.added)} to ${preview.deck.name}. It now has ${cardCount(saved.cardCount)}.${skipped}`
      );
    } catch (err) {
      const refused = toCommandError(err);
      const row = refused.index === undefined ? undefined : rows[refused.index];
      setError(row ? `Line ${row.line}: ${refused.message}` : refused.message);
      setState({ ...preview, saving: false });
      if (refused.kind === "stale") onStale();
    }

    setBusy(false);
  }

  function cancel() {
    if (busyRef.current) return;
    setState({ kind: "idle" });
    setError(null);
    // Back to the button that started it.
    setTimeout(() => chooseRef.current?.focus());
  }

  if (state.kind === "preview") {
    return (
      <CardsPreview
        preview={state}
        previewRef={previewRef}
        blocked={blocked}
        error={error}
        onSkipDuplicates={(skip) => setState({ ...state, skipDuplicates: skip })}
        onAdd={() => add(state)}
        onCancel={cancel}
      />
    );
  }

  const reading = state.kind === "reading";
  return (
    <div className="data-action" role="group" aria-labelledby="cards-import-title">
      <h4 id="cards-import-title" className="item-title">
        Import cards
      </h4>
      {decks.length === 0 ? (
        <p className="field-hint">Create a deck on the Study Desk first, then add cards to it here.</p>
      ) : (
        <>
          <SelectField
            id="cards-import-deck"
            label="Deck"
            hint="The cards are added to this deck as new cards, due for review straight away."
            value={deckId}
            options={decks.map((deck) => ({ value: String(deck.id), label: deck.name }))}
            placeholder="Choose a deck"
            error={deckError}
            onChange={(value) => {
              setDeckId(value);
              setDeckError(undefined);
            }}
          />
          <div>
            <button
              ref={chooseRef}
              type="button"
              className="button"
              aria-disabled={reading || blocked}
              onClick={choose}
            >
              <UploadIcon />
              Choose a CSV file
            </button>
          </div>
        </>
      )}
      <p className="form-status" role="status">
        {reading && "Reading the file…"}
      </p>
      {done && (
        <p ref={doneRef} className="notice" role="status" tabIndex={-1}>
          {done}
        </p>
      )}
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** What will happen to a row, as its last column says. */
function rowStatus(row: CardsFileRow, skipDuplicates: boolean): string {
  if (row.problem) return row.problem.message;
  if (!row.duplicate) {
    return row.clozeCards ? `Ready: a cloze text making ${cardCount(row.clozeCards)}` : "Ready";
  }
  const why =
    row.duplicate.kind === "inDeck"
      ? "Already in this deck"
      : `Same front as line ${row.duplicate.line}`;
  return skipDuplicates ? `${why}: left out` : `${why}: added anyway`;
}

/** The rows Rust read from a file, for the user to check before adding them. */
function CardsPreview({
  preview,
  previewRef,
  blocked,
  error,
  onSkipDuplicates,
  onAdd,
  onCancel,
}: {
  preview: ImportState & { kind: "preview" };
  previewRef: React.RefObject<HTMLParagraphElement | null>;
  blocked: boolean;
  error: string | null;
  onSkipDuplicates: (skip: boolean) => void;
  onAdd: () => void;
  onCancel: () => void;
}) {
  const { deck, file, skipDuplicates, saving } = preview;
  const problems = file.rows.filter((row) => row.problem).length;
  const duplicates = file.rows.filter((row) => !row.problem && row.duplicate).length;
  // A cloze row makes one card per number.
  const toAdd = rowsToAdd(file.rows, skipDuplicates).reduce(
    (sum, row) => sum + (row.clozeCards ?? 1),
    0,
  );

  const read = `Read ${rowCount(file.rows.length)} from ${file.fileName}, separated by ${DELIMITER_NAMES[file.delimiter]}${file.header ? ", after a header row" : ""}.`;
  const counts = [
    `${cardCount(file.rows.length - problems - duplicates)} ready to add`,
    duplicates > 0 && `${duplicates.toLocaleString("en-US")} already in the deck or repeated`,
    problems > 0 && `${rowCount(problems)} with problems, which won't be added`,
  ]
    .filter(Boolean)
    .join(", ");

  return (
    <div className="data-action" role="group" aria-labelledby="cards-preview-summary">
      <p ref={previewRef} id="cards-preview-summary" className="notice" tabIndex={-1}>
        {`${read} ${counts}. Nothing has been added to ${deck.name} yet.`}
      </p>
      {file.rowsWithExtraColumns > 0 && (
        <p className="field-hint">
          {file.tagsColumn
            ? `${rowCount(file.rowsWithExtraColumns)} ${file.rowsWithExtraColumns === 1 ? "has" : "have"} more than three columns. Only the front, back and tags are read.`
            : `${rowCount(file.rowsWithExtraColumns)} ${file.rowsWithExtraColumns === 1 ? "has" : "have"} more than two columns. Only the first two are read. To add tags, head the third column Tags.`}
        </p>
      )}
      {problems > 0 && (
        <p className="field-hint">
          To add the rows with problems, fix them in the file and choose it again.
        </p>
      )}
      {duplicates > 0 && (
        <label className="check-field">
          <input
            type="checkbox"
            checked={skipDuplicates}
            aria-disabled={saving}
            onChange={(event) => {
              if (!saving) onSkipDuplicates(event.target.checked);
            }}
          />
          <span>
            Leave out cards whose front is already in the deck, or on an earlier row of the file
          </span>
        </label>
      )}

      {/* Scrolls by itself, so a long file doesn't push the buttons away. */}
      <div className="csv-table-wrap" role="region" aria-label="Rows of the file" tabIndex={0}>
        <table className="csv-table">
          <thead>
            <tr>
              <th scope="col">Line</th>
              <th scope="col">Front</th>
              <th scope="col">Back</th>
              {file.tagsColumn && <th scope="col">Tags</th>}
              <th scope="col">What happens</th>
            </tr>
          </thead>
          <tbody>
            {file.rows.map((row) => {
              const left = row.problem !== null || (skipDuplicates && row.duplicate !== null);
              return (
                <tr key={row.line} className={left ? "csv-row-left-out" : undefined}>
                  <td>{row.line}</td>
                  <td className="csv-cell">{row.front}</td>
                  <td className="csv-cell">{row.clozeCards ? "(cloze text)" : row.back}</td>
                  {file.tagsColumn && <td className="csv-cell">{(row.tags ?? []).join(" ")}</td>}
                  <td className={row.problem ? "csv-status csv-status-problem" : "csv-status"}>
                    {rowStatus(row, skipDuplicates)}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <div className="form-actions">
        <button
          type="button"
          className="button button-primary"
          aria-disabled={saving || blocked || toAdd === 0}
          onClick={onAdd}
        >
          {`Add ${cardCount(toAdd)} to ${deck.name}`}
        </button>
        <button type="button" className="button" aria-disabled={saving} onClick={onCancel}>
          Cancel
        </button>
      </div>
      <p className="form-status" role="status">
        {saving ? "Adding the cards…" : toAdd === 0 ? "There's nothing new to add from this file." : ""}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/** What the last cards export did. Cancelling returns to `idle` silently. */
type ExportState =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "saved"; fileName: string }
  | { kind: "error"; message: string };

/** Exporting a deck's cards as CSV. Rust asks where the file goes. */
function ExportCards({
  decks,
  blocked,
  onRunning,
}: {
  /** Every active deck, the sample deck included. */
  decks: Deck[];
  blocked: boolean;
  onRunning: (running: boolean) => void;
}) {
  const [deckId, setDeckId] = useState("");
  const [deckError, setDeckError] = useState<string | undefined>();
  const [state, setState] = useState<ExportState>({ kind: "idle" });
  const busyRef = useRef(false);

  async function runExport() {
    if (busyRef.current || blocked) return;
    const deck = decks.find((each) => String(each.id) === deckId);
    if (!deck) {
      setDeckError("Choose the deck to export.");
      document.getElementById("cards-export-deck")?.focus();
      return;
    }
    busyRef.current = true;
    onRunning(true);
    setState({ kind: "busy" });

    try {
      const outcome = await exportCardsCsv(deck.id);
      setState(
        outcome.status === "saved" ? { kind: "saved", fileName: outcome.fileName } : { kind: "idle" }
      );
    } catch (err) {
      setState({ kind: "error", message: toCommandError(err).message });
    }

    busyRef.current = false;
    onRunning(false);
  }

  return (
    <div className="data-action" role="group" aria-labelledby="cards-export-title">
      <h4 id="cards-export-title" className="item-title">
        Export cards
      </h4>
      <SelectField
        id="cards-export-deck"
        label="Deck"
        hint="Its cards are saved as a CSV file, with a front and a back column, that spreadsheets can open."
        value={deckId}
        options={decks.map((deck) => ({ value: String(deck.id), label: deck.name }))}
        placeholder="Choose a deck"
        error={deckError}
        onChange={(value) => {
          setDeckId(value);
          setDeckError(undefined);
        }}
      />
      <div>
        <button
          type="button"
          className="button"
          aria-disabled={state.kind === "busy" || blocked}
          onClick={runExport}
        >
          <DownloadIcon />
          Export as CSV
        </button>
      </div>
      <p className="form-status" role="status">
        {state.kind === "busy" && "Preparing the file…"}
        {state.kind === "saved" && `Cards saved as ${state.fileName}.`}
      </p>
      {state.kind === "error" && (
        <p className="message-error" role="alert">
          {state.message}
        </p>
      )}
    </div>
  );
}
