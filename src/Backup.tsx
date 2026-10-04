import { useEffect, useRef, useState } from "react";
import { cancelImport, confirmImport, exportData, prepareImport, toCommandError } from "./api";
import { DownloadIcon, UploadIcon } from "./icons";
import { formatDateTime, useFocusOnMount } from "./ui";

/**
 * Backing up and restoring everything Synapse stores. Rust opens the file
 * windows and does all the work. Only one of the two runs at a time. Opened
 * from the sidebar, so it takes focus.
 */
export function Backup({
  onLock,
  onRestored,
}: {
  /**
   * Whether the sidebar must be inert: while an import is under way
   * (checking, waiting for confirmation, or restoring), nothing may start
   * work against a database that is about to be replaced or leave
   * mid-restore; while an export is being saved, this screen can't be opened
   * afresh, which would forget the export and let an import start beside it.
   */
  onLock: (locked: boolean) => void;
  onRestored: (fileName: string) => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(true);
  const [running, setRunning] = useState<"export" | "import" | null>(null);
  const [importing, setImporting] = useState(false);
  const locked = importing || running === "export";

  useEffect(() => {
    onLock(locked);
  }, [locked, onLock]);

  // However this screen goes away, the sidebar is usable afterwards.
  useEffect(() => () => onLock(false), [onLock]);

  return (
    <section ref={screenRef} className="backup" tabIndex={-1} aria-labelledby="backup-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="backup-heading" className="page-title">
            Backup &amp; restore
          </h2>
          <p className="page-intro">
            Everything Synapse stores is kept in one database on this computer. Save a copy of it,
            or replace it with a copy you saved before.
          </p>
        </div>
      </div>

      <section className="panel" aria-labelledby="export-heading">
        <div className="section-heading">
          <span className="section-icon" aria-hidden="true">
            <DownloadIcon />
          </span>
          <h3 id="export-heading" className="section-title">
            Export a backup
          </h3>
        </div>
        <ExportData
          blocked={running === "import"}
          onRunning={(busy) => setRunning(busy ? "export" : null)}
        />
      </section>

      <section className="panel" aria-labelledby="import-heading">
        <div className="section-heading">
          <span className="section-icon section-icon-caution" aria-hidden="true">
            <UploadIcon />
          </span>
          <h3 id="import-heading" className="section-title">
            Restore a backup
          </h3>
        </div>
        <ImportData
          blocked={running === "export"}
          onRunning={(busy) => setRunning(busy ? "import" : null)}
          onImporting={setImporting}
          onRestored={onRestored}
        />
      </section>
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
        <DownloadIcon />
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
        // The screen closed during the check, so nobody can answer: remove
        // the checked copy.
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
        <div className="confirm confirm-danger">
          <p ref={questionRef} id="import-question" className="confirm-question" tabIndex={-1}>
            {`Replace all your study data with ${state.backup.fileName}, exported ${formatDateTime(
              state.backup.exportedAt
            )}? Every deck, card, review, session, and note in Synapse now will be replaced by the backup's, and this can't be undone. To keep a copy of your current data, choose Keep current data and export it first.`}
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-danger"
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
          <UploadIcon />
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
