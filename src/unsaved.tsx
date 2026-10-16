import { createContext, useCallback, useContext, useEffect, useId, useRef, useState } from "react";
import { AlertIcon } from "./icons";

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

/** What the question says, by why it was asked. */
const WORDING = {
  navigate: {
    title: "Leave without saving?",
    text: "What you've typed in this form hasn't been saved. If you leave now, it will be lost.",
  },
  close: {
    title: "Close Synapse without saving?",
    text: "What you've typed in this form hasn't been saved. If you close Synapse now, it will be lost.",
  },
};

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
  const wording = WORDING[leaving ?? "navigate"];

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog || leaving === null || dialog.open) return;
    dialog.returnValue = "";
    dialog.showModal();
    // Focus goes to the dialog itself rather than its first button, so its
    // title and text are read out and a held Enter can't discard.
    dialog.focus();
  }, [leaving]);

  return (
    <dialog
      ref={dialogRef}
      className="dialog"
      role="alertdialog"
      tabIndex={-1}
      aria-labelledby="discard-title"
      aria-describedby="discard-text"
      // Closing returns focus to whatever had it before the question opened.
      // A button, or Escape, closes it; the answer is read from `returnValue`.
      onClose={(event) => {
        if (event.currentTarget.returnValue === "discard") onDiscard();
        else onKeep();
      }}
    >
      <div className="section-heading">
        <span className="section-icon section-icon-warning" aria-hidden="true">
          <AlertIcon />
        </span>
        <h2 id="discard-title" className="form-title">
          {wording.title}
        </h2>
      </div>
      <p id="discard-text" className="dialog-text">
        {wording.text}
      </p>
      <div className="form-actions">
        <button
          type="button"
          className="button button-danger-soft"
          onClick={() => dialogRef.current?.close("discard")}
        >
          Discard changes
        </button>
        <button
          type="button"
          className="button"
          onClick={() => dialogRef.current?.close("keep")}
        >
          Keep editing
        </button>
      </div>
    </dialog>
  );
}
