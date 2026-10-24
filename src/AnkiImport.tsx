import { useEffect, useRef, useState } from "react";
import {
  cancelAnkiImport,
  importAnkiPackage,
  readAnkiPackage,
  toCommandError,
  type AnkiPicturesLeftOut,
  type AnkiSkipped,
  type ReadAnkiFile,
} from "./api";
import { UploadIcon } from "./icons";
import { useUnsavedChanges } from "./unsaved";

function count(number: number, one: string, many: string): string {
  return number === 1 ? `1 ${one}` : `${number.toLocaleString("en-US")} ${many}`;
}

const cards = (number: number) => count(number, "card", "cards");
const notes = (number: number) => count(number, "note", "notes");
const decks = (number: number) => count(number, "new deck", "new decks");

/** The cards adding would create, plus the reversed ones. */
function cardTotal(file: ReadAnkiFile, bothDirections: boolean): number {
  return file.decks.reduce(
    (total, deck) => total + deck.cards + (bothDirections ? deck.reversible : 0),
    0,
  );
}

/** Why notes were left out, as one sentence, or null when none were. */
function leftOut(skipped: AnkiSkipped): string | null {
  const reasons = [
    skipped.imageOcclusion > 0 &&
      `${notes(skipped.imageOcclusion)} of image occlusion whose picture isn't in the file (export with media included) or whose boxes can't be read`,
    skipped.badCloze > 0 &&
      `${notes(skipped.badCloze)} with cloze deletions Synapse can't read (nested, or with an empty answer)`,
    skipped.otherNoteType > 0 &&
      `${notes(skipped.otherNoteType)} of a type that isn't one front and one back, or cloze`,
    skipped.empty > 0 && `${notes(skipped.empty)} with an empty side (perhaps only a picture)`,
    skipped.tooLong > 0 && `${notes(skipped.tooLong)} with a side over 2,000 characters`,
    skipped.noCard > 0 && `${notes(skipped.noCard)} with no card in Anki`,
  ].filter(Boolean);
  return reasons.length === 0 ? null : `Left out: ${reasons.join(", ")}.`;
}

/** Which pictures of front/back cards are left out, as one sentence, or null when none are. */
function picturesLeftOut(leftOut: AnkiPicturesLeftOut | undefined): string | null {
  if (!leftOut) return null;
  const pictures = (number: number) => count(number, "picture", "pictures");
  const reasons = [
    leftOut.missing > 0 &&
      `${pictures(leftOut.missing)} not in the file (export with media included) or given by a web address`,
    leftOut.unsupported > 0 && `${pictures(leftOut.unsupported)} not a PNG, JPEG, GIF or WebP`,
    leftOut.tooLarge > 0 && `${pictures(leftOut.tooLarge)} over 20 MB`,
    leftOut.tooMany > 0 &&
      `${pictures(leftOut.tooMany)} past the 4 that one side of a card can show`,
  ].filter(Boolean);
  return reasons.length === 0
    ? null
    : `Pictures left out, so their cards come without them: ${reasons.join(", ")}.`;
}

/** Where importing an Anki package is up to. */
type ImportState =
  | { kind: "idle" }
  | { kind: "reading" }
  | { kind: "preview"; file: ReadAnkiFile; bothDirections: boolean; saving: boolean };

/**
 * Decks from Anki, on the Backup & restore screen: adding the front/back,
 * cloze and image occlusion notes of an Anki package as new decks of new
 * cards. Rust opens the file
 * window, reads the package and holds it; this shows what it found and asks
 * before anything is added. A preview on screen counts as unsaved changes.
 */
export function AnkiImport({
  blocked,
  onRunning,
  onImported,
}: {
  /** A backup or a spreadsheet is being worked on, so this waits. */
  blocked: boolean;
  onRunning: (running: boolean) => void;
  /** New decks were created. */
  onImported: () => void;
}) {
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
    setBusy(true);
    setError(null);
    setDone(null);
    setState({ kind: "reading" });

    try {
      const file = await readAnkiPackage();
      if (!mountedRef.current) return;
      setState(
        file.status === "read"
          ? { kind: "preview", file, bothDirections: false, saving: false }
          : { kind: "idle" },
      );
    } catch (err) {
      setState({ kind: "idle" });
      setError(toCommandError(err).message);
    }

    setBusy(false);
  }

  async function add(preview: ImportState & { kind: "preview" }) {
    if (busyRef.current || blocked || cardTotal(preview.file, preview.bothDirections) === 0) return;
    setBusy(true);
    setError(null);
    setState({ ...preview, saving: true });

    try {
      const saved = await importAnkiPackage(preview.file.token, preview.bothDirections);
      setState({ kind: "idle" });
      const names = saved.decks.map((deck) => deck.name).join(", ");
      setDone(
        `Added ${cards(saved.cards)} in ${decks(saved.decks.length)}: ${names}. They're on the Study Desk, ready to review.`,
      );
      onImported();
    } catch (err) {
      const refused = toCommandError(err);
      setError(refused.message);
      // Rust no longer holds this package, so the preview can't be added.
      setState(refused.kind === "stale" ? { kind: "idle" } : { ...preview, saving: false });
    }

    setBusy(false);
  }

  function cancel(preview: ImportState & { kind: "preview" }) {
    if (busyRef.current) return;
    setState({ kind: "idle" });
    setError(null);
    // Only frees memory in Rust, so a failure changes nothing for the user.
    cancelAnkiImport(preview.file.token).catch(() => {});
    // Back to the button that started it.
    setTimeout(() => chooseRef.current?.focus());
  }

  return (
    <section className="panel" aria-labelledby="anki-heading">
      <div className="section-heading">
        <span className="section-icon" aria-hidden="true">
          <UploadIcon />
        </span>
        <h3 id="anki-heading" className="section-title">
          Decks from Anki
        </h3>
      </div>
      <p className="field-hint">
        Add the cards of an Anki package (.apkg or .colpkg) as new decks. Notes with one front
        and one back come with their text and pictures, cloze notes as text, and image occlusion
        notes with their picture, when the package includes media. Anki's schedule, review
        history, sounds and pictures in cloze notes aren't imported, so every card starts as new.
      </p>

      {state.kind === "preview" ? (
        <AnkiPreview
          preview={state}
          previewRef={previewRef}
          blocked={blocked}
          error={error}
          onBothDirections={(both) => setState({ ...state, bothDirections: both })}
          onAdd={() => add(state)}
          onCancel={() => cancel(state)}
        />
      ) : (
        <div className="data-action" role="group" aria-labelledby="anki-import-title">
          <h4 id="anki-import-title" className="item-title">
            Import from Anki
          </h4>
          <div>
            <button
              ref={chooseRef}
              type="button"
              className="button"
              aria-disabled={state.kind === "reading" || blocked}
              onClick={choose}
            >
              <UploadIcon />
              Choose an Anki file
            </button>
          </div>
          <p className="form-status" role="status">
            {state.kind === "reading" && "Reading the Anki file…"}
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
      )}
    </section>
  );
}

/** What Rust read from a package, for the user to check before adding it. */
function AnkiPreview({
  preview,
  previewRef,
  blocked,
  error,
  onBothDirections,
  onAdd,
  onCancel,
}: {
  preview: ImportState & { kind: "preview" };
  previewRef: React.RefObject<HTMLParagraphElement | null>;
  blocked: boolean;
  error: string | null;
  onBothDirections: (both: boolean) => void;
  onAdd: () => void;
  onCancel: () => void;
}) {
  const { file, bothDirections, saving } = preview;
  const total = cardTotal(file, bothDirections);
  const reversible = file.decks.reduce((sum, deck) => sum + deck.reversible, 0);
  const skipped = leftOut(file.skipped);
  const picturesSkipped = picturesLeftOut(file.picturesLeftOut);

  const summary =
    file.decks.length === 0
      ? `Read ${notes(file.notes)} from ${file.fileName}. None of them can become cards here, so there's nothing to add.`
      : `Read ${notes(file.notes)} from ${file.fileName}. ${cards(total)} ready to add in ${decks(file.decks.length)}. Nothing has been added yet.`;

  return (
    <div className="data-action" role="group" aria-labelledby="anki-preview-summary">
      <p ref={previewRef} id="anki-preview-summary" className="notice" tabIndex={-1}>
        {summary}
      </p>
      {skipped && <p className="field-hint">{skipped}</p>}
      {(file.cardPictures ?? 0) > 0 && (
        <p className="field-hint">
          {`${count(file.cardPictures, "picture comes", "pictures come")} with the cards, on the same side as in Anki.`}
        </p>
      )}
      {picturesSkipped && <p className="field-hint">{picturesSkipped}</p>}
      {(file.tags ?? 0) > 0 && (
        <p className="field-hint">
          {`The cards keep their notes' tags: ${count(file.tags ?? 0, "tag", "tags")} in all.`}
        </p>
      )}
      {(file.tagsLeftOut ?? 0) > 0 && (
        <p className="field-hint">
          {file.tagsLeftOut === 1
            ? "1 tag is left out: it's over 60 characters, has a comma, or is past a card's 20 tags."
            : `${count(file.tagsLeftOut ?? 0, "tag", "tags")} are left out: they're over 60 characters, have a comma, or are past a card's 20 tags.`}
        </p>
      )}
      {file.mediaRemoved > 0 && (
        <p className="field-hint">
          {file.mediaRemoved === 1
            ? "1 note has a sound, a video, or a picture in a cloze, which is left out. Its text is kept."
            : `${count(file.mediaRemoved, "note", "notes")} have a sound, a video, or a picture in a cloze, which is left out. Their text is kept.`}
        </p>
      )}
      {file.extraRemoved > 0 && (
        <p className="field-hint">
          {`${count(file.extraRemoved, "cloze or image occlusion note has", "cloze or image occlusion notes have")} text in another field, such as Back Extra, which is left out.`}
        </p>
      )}
      {(file.occlusions ?? 0) > 0 && (
        <p className="field-hint">
          {`${count(file.occlusions, "image occlusion note comes", "image occlusion notes come")} with its picture, as picture cards with one card for each box.`}
          {(file.occlusionApproximated ?? 0) > 0 &&
            ` In ${count(file.occlusionApproximated, "of them", "of them")}, round or many-sided shapes become rectangles around them, and text written on the picture is left out.`}
        </p>
      )}
      {file.reviewed > 0 && (
        <p className="field-hint">
          {`${count(file.reviewed, "of these notes was", "of these notes were")} studied in Anki. The cards still start as new here.`}
        </p>
      )}
      {reversible > 0 && (
        <label className="check-field">
          <input
            type="checkbox"
            checked={bothDirections}
            aria-disabled={saving}
            onChange={(event) => {
              if (!saving) onBothDirections(event.target.checked);
            }}
          />
          <span>
            {`Also add the reversed card, with the back as the question, for the ${notes(reversible)} that ${reversible === 1 ? "has" : "have"} one in Anki`}
          </span>
        </label>
      )}

      {file.decks.length > 0 && (
        // Scrolls by itself, so a long list doesn't push the buttons away.
        <div className="csv-table-wrap" role="region" aria-label="Decks to add" tabIndex={0}>
          <table className="csv-table">
            <thead>
              <tr>
                <th scope="col">Anki deck</th>
                <th scope="col">New deck</th>
                <th scope="col">Cards</th>
                <th scope="col">First card</th>
              </tr>
            </thead>
            <tbody>
              {file.decks.map((deck) => (
                <tr key={deck.ankiName + deck.name}>
                  <td className="csv-cell">{deck.ankiName}</td>
                  <td className="csv-cell">{deck.name}</td>
                  <td>{(deck.cards + (bothDirections ? deck.reversible : 0)).toLocaleString("en-US")}</td>
                  <td className="csv-cell">{deck.examples[0]?.front ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="form-actions">
        {file.decks.length > 0 && (
          <button
            type="button"
            className="button button-primary"
            aria-disabled={saving || blocked}
            onClick={onAdd}
          >
            {`Add ${cards(total)} in ${decks(file.decks.length)}`}
          </button>
        )}
        <button type="button" className="button" aria-disabled={saving} onClick={onCancel}>
          {file.decks.length > 0 ? "Cancel" : "Close"}
        </button>
      </div>
      <p className="form-status" role="status">
        {saving && "Adding the decks…"}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
