import { createContext, useCallback, useContext, useEffect, useId, useRef, useState } from "react";

/** Tells the app whether the form with this id has changes that haven't been saved. */
type Report = (formId: string, unsaved: boolean) => void;

const UnsavedReport = createContext<Report>(() => {});

/** Lets the forms inside report unsaved changes to `useUnsavedTracker`'s owner. */
export const UnsavedChangesProvider = UnsavedReport.Provider;

/**
 * Which open forms hold unsaved changes. `report` goes to
 * `UnsavedChangesProvider`; `unsaved` is true while any form has some.
 */
export function useUnsavedTracker(): { unsaved: boolean; report: Report } {
  const [dirty, setDirty] = useState<ReadonlySet<string>>(() => new Set());
  const report = useCallback<Report>((formId, unsaved) => {
    setDirty((previous) => {
      if (previous.has(formId) === unsaved) return previous;
      const next = new Set(previous);
      if (unsaved) next.add(formId);
      else next.delete(formId);
      return next;
    });
  }, []);
  return { unsaved: dirty.size > 0, report };
}

/**
 * Marks this form as holding unsaved changes while `unsaved` is true: what the
 * user typed differs from what the form opened with. Saving or cancelling
 * closes the form, which clears it.
 */
export function useUnsavedChanges(unsaved: boolean) {
  const report = useContext(UnsavedReport);
  const formId = useId();
  useEffect(() => {
    report(formId, unsaved);
    return () => report(formId, false);
  }, [report, formId, unsaved]);
}

/**
 * Asked before unsaved changes are thrown away, when the user picks a place in
 * the sidebar (`leaving` is "navigate") or closes the window ("close"). Modal:
 * nothing else can be used until it's answered. Escape keeps editing.
 */
export function DiscardDialog({
  leaving,
  onDiscard,
  onKeep,
}: {
  /**
   * Why the question is open, or null while it's closed. The dialog closes
   * itself when answered, before this goes back to null.
   */
  leaving: "navigate" | "close" | null;
  onDiscard: () => void;
  onKeep: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || leaving === null || dialog.open) return;
    dialog.returnValue = "";
    dialog.showModal();
    // As with the app's other questions, focus goes to the question rather
    // than its first button: it's read out, and a held Enter can't discard.
    questionRef.current?.focus();
  }, [leaving]);

  return (
    <dialog
      ref={dialogRef}
      className="discard-dialog"
      role="alertdialog"
      aria-labelledby="discard-question"
      // Closing returns focus to whatever had it before the question opened.
      // A button, or Escape, closes it; the answer is read from `returnValue`.
      onClose={(event) => {
        if (event.currentTarget.returnValue === "discard") onDiscard();
        else onKeep();
      }}
    >
      <div className="confirm">
        <p ref={questionRef} id="discard-question" className="confirm-question" tabIndex={-1}>
          {leaving === "close"
            ? "Close Synapse without saving? What you've typed in this form hasn't been saved, " +
              "and closing now will discard it."
            : "Leave without saving? What you've typed in this form hasn't been saved, and " +
              "leaving now will discard it."}
        </p>
        <div className="deck-actions">
          <button
            type="button"
            className="button button-danger button-small"
            onClick={() => dialogRef.current?.close("discard")}
          >
            Discard changes
          </button>
          <button
            type="button"
            className="button button-small"
            onClick={() => dialogRef.current?.close("keep")}
          >
            Keep editing
          </button>
        </div>
      </div>
    </dialog>
  );
}
