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
import { useT } from "./i18n";
import { useUnsavedChanges } from "./unsaved";

type T = ReturnType<typeof useT>["t"];

const cards = (number: number, t: T) => t(number === 1 ? "common.cardOne" : "common.cardOther", { count: number.toLocaleString("en-US") });
const notes = (number: number, t: T) => t(number === 1 ? "anki.noteOne" : "anki.noteOther", { count: number.toLocaleString("en-US") });
const decks = (number: number, t: T) => t(number === 1 ? "anki.deckOne" : "anki.deckOther", { count: number.toLocaleString("en-US") });

/** The cards adding would create, plus the reversed ones. */
function cardTotal(file: ReadAnkiFile, bothDirections: boolean): number {
  return file.decks.reduce(
    (total, deck) => total + deck.cards + (bothDirections ? deck.reversible : 0),
    0,
  );
}

/** Why notes were left out, as one sentence, or null when none were. */
function leftOut(skipped: AnkiSkipped, t: T): string | null {
  const reasons = [
    skipped.imageOcclusion > 0 &&
      t("anki.skippedImageOcclusion", { notes: notes(skipped.imageOcclusion, t) }),
    skipped.badCloze > 0 &&
      t("anki.skippedBadCloze", { notes: notes(skipped.badCloze, t) }),
    skipped.otherNoteType > 0 &&
      t("anki.skippedOtherNoteType", { notes: notes(skipped.otherNoteType, t) }),
    skipped.empty > 0 && t("anki.skippedEmpty", { notes: notes(skipped.empty, t) }),
    skipped.tooLong > 0 && t("anki.skippedTooLong", { notes: notes(skipped.tooLong, t) }),
    skipped.noCard > 0 && t("anki.skippedNoCard", { notes: notes(skipped.noCard, t) }),
  ].filter(Boolean);
  return reasons.length === 0 ? null : t("anki.leftOut", { reasons: reasons.join(", ") });
}

/** Which pictures of front/back cards are left out, as one sentence, or null when none are. */
function picturesLeftOut(leftOut: AnkiPicturesLeftOut | undefined, t: T): string | null {
  if (!leftOut) return null;
  const pictures = (number: number) => t(number === 1 ? "anki.pictureOne" : "anki.pictureOther", { count: number.toLocaleString("en-US") });
  const reasons = [
    leftOut.missing > 0 &&
      t("anki.picturesMissing", { pictures: pictures(leftOut.missing) }),
    leftOut.unsupported > 0 && t("anki.picturesUnsupported", { pictures: pictures(leftOut.unsupported) }),
    leftOut.tooLarge > 0 && t("anki.picturesTooLarge", { pictures: pictures(leftOut.tooLarge) }),
    leftOut.tooMany > 0 &&
      t("anki.picturesTooMany", { pictures: pictures(leftOut.tooMany) }),
  ].filter(Boolean);
  return reasons.length === 0
    ? null
    : t("anki.picturesLeftOut", { reasons: reasons.join(", ") });
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
  const { t } = useT();
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
        t("anki.added", { cards: cards(saved.cards, t), decks: decks(saved.decks.length, t), names }),
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
          {t("anki.title")}
        </h3>
      </div>
      <p className="field-hint">
        {t("anki.intro")}
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
            {t("anki.import")}
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
              {t("anki.choose")}
            </button>
          </div>
          <p className="form-status" role="status">
            {state.kind === "reading" && t("anki.reading")}
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
  const { t } = useT();
  const { file, bothDirections, saving } = preview;
  const total = cardTotal(file, bothDirections);
  const reversible = file.decks.reduce((sum, deck) => sum + deck.reversible, 0);
  const skipped = leftOut(file.skipped, t);
  const picturesSkipped = picturesLeftOut(file.picturesLeftOut, t);

  const summary =
    file.decks.length === 0
      ? t("anki.empty", { notes: notes(file.notes, t), name: file.fileName })
      : t("anki.preview", { notes: notes(file.notes, t), name: file.fileName, cards: cards(total, t), decks: decks(file.decks.length, t) });

  return (
    <div className="data-action" role="group" aria-labelledby="anki-preview-summary">
      <p ref={previewRef} id="anki-preview-summary" className="notice" tabIndex={-1}>
        {summary}
      </p>
      {skipped && <p className="field-hint">{skipped}</p>}
      {(file.cardPictures ?? 0) > 0 && (
        <p className="field-hint">
          {t(file.cardPictures === 1 ? "anki.picturesIncludedOne" : "anki.picturesIncludedOther", { count: file.cardPictures.toLocaleString("en-US") })}
        </p>
      )}
      {picturesSkipped && <p className="field-hint">{picturesSkipped}</p>}
      {(file.tags ?? 0) > 0 && (
        <p className="field-hint">
          {t((file.tags ?? 0) === 1 ? "anki.tagsOne" : "anki.tagsOther", { count: (file.tags ?? 0).toLocaleString("en-US") })}
        </p>
      )}
      {(file.tagsLeftOut ?? 0) > 0 && (
        <p className="field-hint">
          {t((file.tagsLeftOut ?? 0) === 1 ? "anki.tagsLeftOutOne" : "anki.tagsLeftOutOther", { count: (file.tagsLeftOut ?? 0).toLocaleString("en-US") })}
        </p>
      )}
      {file.mediaRemoved > 0 && (
        <p className="field-hint">
          {t(file.mediaRemoved === 1 ? "anki.mediaRemovedOne" : "anki.mediaRemovedOther", { count: file.mediaRemoved.toLocaleString("en-US") })}
        </p>
      )}
      {file.extraRemoved > 0 && (
        <p className="field-hint">
          {t(file.extraRemoved === 1 ? "anki.extraRemovedOne" : "anki.extraRemovedOther", { count: file.extraRemoved.toLocaleString("en-US") })}
        </p>
      )}
      {(file.occlusions ?? 0) > 0 && (
        <p className="field-hint">
          {t(file.occlusions === 1 ? "anki.occlusionsOne" : "anki.occlusionsOther", { count: file.occlusions.toLocaleString("en-US") })}
          {(file.occlusionApproximated ?? 0) > 0 &&
            t("anki.approximated", { count: file.occlusionApproximated.toLocaleString("en-US") })}
        </p>
      )}
      {file.reviewed > 0 && (
        <p className="field-hint">
          {t(file.reviewed === 1 ? "anki.reviewedOne" : "anki.reviewedOther", { count: file.reviewed.toLocaleString("en-US") })}
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
            {t(reversible === 1 ? "anki.reversedOne" : "anki.reversedOther", { count: reversible.toLocaleString("en-US") })}
          </span>
        </label>
      )}

      {file.decks.length > 0 && (
        // Scrolls by itself, so a long list doesn't push the buttons away.
        <div className="csv-table-wrap" role="region" aria-label={t("anki.decksToAdd")} tabIndex={0}>
          <table className="csv-table">
            <thead>
              <tr>
                <th scope="col">{t("anki.ankiDeck")}</th>
                <th scope="col">{t("anki.newDeck")}</th>
                <th scope="col">{t("search.scopeCards")}</th>
                <th scope="col">{t("anki.firstCard")}</th>
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
            {t("anki.add", { cards: cards(total, t), decks: decks(file.decks.length, t) })}
          </button>
        )}
        <button type="button" className="button" aria-disabled={saving} onClick={onCancel}>
          {t(file.decks.length > 0 ? "common.cancel" : "settings.close")}
        </button>
      </div>
      <p className="form-status" role="status">
        {saving && t("anki.adding")}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
