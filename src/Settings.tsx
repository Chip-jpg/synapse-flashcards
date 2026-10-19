import { useEffect, useRef, useState, type RefObject } from "react";
import {
  applyParameters,
  getScheduling,
  optimizeParameters,
  resetParameters,
  stopOptimizing,
  toCommandError,
  type IntervalCase,
  type OptimizeOutcome,
  type SchedulingSettings,
} from "./api";
import { RestoreIcon, SlidersIcon } from "./icons";
import { formatDateTime, Message, useFocusOnMount } from "./ui";

type SettingsState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; settings: SchedulingSettings };

/**
 * How the settings appear next. `key` changes every time, so the panel
 * remounts with a fresh state and its focus target takes effect.
 */
type SettingsView = { key: number; notice: string | null };

/** Fetches the settings and describes the outcome; never rejects. */
async function loadSettings(): Promise<SettingsState> {
  try {
    return { status: "ready", settings: await getScheduling() };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * Settings: for now, the FSRS parameters reviews are scheduled with, and
 * fitting them to the user's own history. Rust does the fitting, decides
 * whether the result may be applied, and keeps every change. Opened from the
 * sidebar, so it takes focus.
 */
export function Settings() {
  const [state, setState] = useState<SettingsState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  const [view, setView] = useState<SettingsView>({ key: 0, notice: null });

  useEffect(() => {
    let current = true;
    loadSettings().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  function reload(notice: string | null) {
    setState({ status: "loading" });
    setView((previous) => ({ key: previous.key + 1, notice }));
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading your settings…
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
    <Scheduling
      key={view.key}
      settings={state.settings}
      notice={view.notice}
      onChanged={(next, notice) => {
        setState({ status: "ready", settings: next });
        setView((previous) => ({ key: previous.key + 1, notice }));
      }}
      onStale={(message) => reload(message)}
    />
  );
}

/** Where optimizing is up to. Stopping returns to `idle` with a notice. */
type RunState =
  | { kind: "idle" }
  | { kind: "optimizing" }
  | { kind: "result"; outcome: Extract<OptimizeOutcome, { status: "optimized" }>; applying: boolean }
  | { kind: "confirmReset"; resetting: boolean };

function Scheduling({
  settings,
  notice,
  onChanged,
  onStale,
}: {
  settings: SchedulingSettings;
  notice: string | null;
  /** The parameters in use changed: show `next`, announcing `notice`. */
  onChanged: (next: SchedulingSettings, notice: string) => void;
  /** What's shown is out of date: reload, explaining with `message`. */
  onStale: (message: string) => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(notice === null);
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(notice !== null);
  const [run, setRun] = useState<RunState>({ kind: "idle" });
  const [status, setStatus] = useState("");
  const [error, setError] = useState<string | null>(null);
  // Blocks a second press before the re-render lands.
  const busyRef = useRef(false);
  const resultRef = useRef<HTMLHeadingElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  const optimizeRef = useRef<HTMLButtonElement>(null);
  const optimizingRef = useRef(false);

  const enough = settings.usableReviews >= settings.minimumReviews;
  const optimizing = run.kind === "optimizing";

  // Leaving the screen stops an optimization still running.
  useEffect(
    () => () => {
      if (optimizingRef.current) void stopOptimizing();
    },
    []
  );

  useEffect(() => {
    if (run.kind === "result") resultRef.current?.focus();
    if (run.kind === "confirmReset") questionRef.current?.focus();
  }, [run.kind]);

  async function optimize() {
    if (busyRef.current || !enough) return;
    busyRef.current = true;
    optimizingRef.current = true;
    setError(null);
    setStatus("");
    setRun({ kind: "optimizing" });
    try {
      const outcome = await optimizeParameters();
      if (outcome.status === "stopped") {
        setRun({ kind: "idle" });
        setStatus("Optimizing stopped. Nothing was changed.");
      } else {
        setRun({ kind: "result", outcome, applying: false });
      }
    } catch (err) {
      setRun({ kind: "idle" });
      setError(toCommandError(err).message);
    }
    optimizingRef.current = false;
    busyRef.current = false;
  }

  async function apply(outcome: Extract<OptimizeOutcome, { status: "optimized" }>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setError(null);
    setRun({ kind: "result", outcome, applying: true });
    try {
      const next = await applyParameters(outcome.token);
      onChanged(
        next,
        "Your own parameters are in use. Every rating from now on is scheduled with them; " +
          "cards keep the due dates they already have."
      );
      return;
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale(failure.message);
        return;
      }
      setRun({ kind: "result", outcome, applying: false });
      setError(failure.message);
    }
    busyRef.current = false;
  }

  function discard() {
    if (busyRef.current) return;
    setRun({ kind: "idle" });
    setStatus("The fitted parameters were discarded. Nothing was changed.");
    optimizeRef.current?.focus();
  }

  async function reset() {
    if (busyRef.current) return;
    busyRef.current = true;
    setError(null);
    setRun({ kind: "confirmReset", resetting: true });
    try {
      const next = await resetParameters(settings.version);
      onChanged(
        next,
        "FSRS's default parameters are in use again for every rating from now on. Cards keep " +
          "the due dates they already have."
      );
      return;
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale(failure.message);
        return;
      }
      setRun({ kind: "confirmReset", resetting: false });
      setError(failure.message);
    }
    busyRef.current = false;
  }

  const appliedOn = settings.appliedAt ? formatDateTime(settings.appliedAt) : null;

  return (
    <section ref={screenRef} className="settings" tabIndex={-1} aria-labelledby="settings-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="settings-heading" className="page-title">
            Settings
          </h2>
          <p className="page-intro">How Synapse schedules your reviews.</p>
        </div>
      </div>

      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      <section className="panel" aria-labelledby="scheduling-heading">
        <div className="section-heading">
          <span className="section-icon" aria-hidden="true">
            <SlidersIcon />
          </span>
          <h3 id="scheduling-heading" className="section-title">
            Personal scheduling
          </h3>
        </div>

        <p className="settings-text">
          {settings.personal
            ? `Reviews are scheduled with FSRS parameters fitted to your own history` +
              (settings.reviewsUsed !== null
                ? ` (${settings.reviewsUsed.toLocaleString()} reviews)`
                : "") +
              (appliedOn ? `, applied ${appliedOn}.` : ".")
            : "Reviews are scheduled with FSRS's default parameters, which fit the average " +
              "learner. Fitting them to your own reviews can predict better when you'll forget."}
        </p>

        <div className="data-action">
          <p id="optimize-hint" className="field-hint">
            {enough
              ? `${settings.usableReviews.toLocaleString()} of your reviews can be learned from. ` +
                "Optimizing compares fitted parameters with the ones in use; nothing changes " +
                "until you apply them."
              : `${settings.usableReviews.toLocaleString()} of the ` +
                `${settings.minimumReviews.toLocaleString()} reviews needed so far. Only ratings ` +
                "made at least a day after the card's previous one count. With fewer, a fit " +
                "mostly follows chance, and the defaults are the better guess."}
          </p>
          <div className="form-actions">
            <button
              ref={optimizeRef}
              type="button"
              className="button button-primary"
              aria-describedby="optimize-hint"
              aria-disabled={!enough || run.kind !== "idle"}
              onClick={optimize}
            >
              <SlidersIcon />
              Optimize
            </button>
            {optimizing && (
              <button type="button" className="button" onClick={() => void stopOptimizing()}>
                Stop
              </button>
            )}
          </div>
          <p className="form-status" role="status">
            {optimizing ? "Fitting FSRS to your reviews…" : status}
          </p>
        </div>

        {run.kind === "result" && (
          <Comparison
            outcome={run.outcome}
            headingRef={resultRef}
            applying={run.applying}
            onApply={() => apply(run.outcome)}
            onDiscard={discard}
          />
        )}

        {settings.personal && run.kind === "idle" && (
          <div className="form-actions">
            <button
              type="button"
              className="button"
              onClick={() => {
                setError(null);
                setRun({ kind: "confirmReset", resetting: false });
              }}
            >
              <RestoreIcon />
              Reset to defaults
            </button>
          </div>
        )}

        {run.kind === "confirmReset" && (
          <div className="confirm confirm-calm">
            <p ref={questionRef} className="confirm-question" tabIndex={-1}>
              Go back to FSRS&apos;s default parameters? Every rating from now on is scheduled
              with them. Cards keep the due dates they already have, and you can optimize again
              at any time.
            </p>
            <div className="form-actions">
              <button
                type="button"
                className="button button-primary button-small"
                aria-disabled={run.resetting}
                onClick={reset}
              >
                Reset to defaults
              </button>
              <button
                type="button"
                className="button button-small"
                aria-disabled={run.resetting}
                onClick={() => {
                  if (!run.resetting) setRun({ kind: "idle" });
                }}
              >
                Keep my parameters
              </button>
            </div>
            <p className="form-status" role="status">
              {run.resetting ? "Resetting…" : ""}
            </p>
          </div>
        )}

        {error && (
          <p className="message-error" role="alert">
            {error}
          </p>
        )}

        <details className="settings-details">
          <summary>The parameters in use</summary>
          <ParameterTable first={settings.parameters} firstLabel="In use" />
        </details>
      </section>
    </section>
  );
}

const CASES: { case: IntervalCase; label: string }[] = [
  { case: "firstAgain", label: "Again on a new card" },
  { case: "firstHard", label: "Hard on a new card" },
  { case: "firstGood", label: "Good on a new card" },
  { case: "firstEasy", label: "Easy on a new card" },
  { case: "secondGood", label: "Good, then Good again when due" },
];

/** Fitted parameters against the ones in use, and the choice to apply them. */
function Comparison({
  outcome,
  headingRef,
  applying,
  onApply,
  onDiscard,
}: {
  outcome: Extract<OptimizeOutcome, { status: "optimized" }>;
  headingRef: RefObject<HTMLHeadingElement | null>;
  applying: boolean;
  onApply: () => void;
  onDiscard: () => void;
}) {
  return (
    <div className="settings-result">
      <h4 ref={headingRef} className="item-title" tabIndex={-1}>
        Fitted to {outcome.reviewsUsed.toLocaleString()} reviews
      </h4>
      <p className="settings-text">
        On your own reviews, FSRS&apos;s predictions of whether you&apos;d remember were off by{" "}
        {formatPercent(outcome.currentError)} on average with the parameters in use, and{" "}
        {formatPercent(outcome.proposedError)} with the fitted ones.
      </p>

      <table className="compare-table">
        <caption>Next review after…</caption>
        <thead>
          <tr>
            <th scope="col">Rating</th>
            <th scope="col">In use</th>
            <th scope="col">Fitted</th>
          </tr>
        </thead>
        <tbody>
          {CASES.map(({ case: name, label }) => {
            const change = outcome.intervals.find((interval) => interval.case === name);
            if (!change) return null;
            return (
              <tr key={name}>
                <th scope="row">{label}</th>
                <td>{formatInterval(change.current)}</td>
                <td>{formatInterval(change.proposed)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {outcome.better ? (
        <div className="confirm confirm-calm">
          <p className="confirm-question">
            The fitted parameters predict your reviews better. Applying them changes how every
            rating from now on is scheduled. Cards keep the due dates they already have, and you
            can reset to the defaults at any time.
          </p>
          <div className="form-actions">
            <button
              type="button"
              className="button button-primary button-small"
              aria-disabled={applying}
              onClick={onApply}
            >
              Apply
            </button>
            <button
              type="button"
              className="button button-small"
              aria-disabled={applying}
              onClick={onDiscard}
            >
              Discard
            </button>
          </div>
          <p className="form-status" role="status">
            {applying ? "Applying…" : ""}
          </p>
        </div>
      ) : (
        <div className="confirm">
          <p className="confirm-question">
            The fitted parameters don&apos;t predict your reviews better than the ones in use, so
            Synapse keeps those.
          </p>
          <div className="form-actions">
            <button type="button" className="button button-small" onClick={onDiscard}>
              Close
            </button>
          </div>
        </div>
      )}

      <details className="settings-details">
        <summary>All 21 parameters</summary>
        <ParameterTable
          first={outcome.current}
          firstLabel="In use"
          second={outcome.proposed}
          secondLabel="Fitted"
        />
      </details>
    </div>
  );
}

function ParameterTable({
  first,
  firstLabel,
  second,
  secondLabel,
}: {
  first: number[];
  firstLabel: string;
  second?: number[];
  secondLabel?: string;
}) {
  return (
    <table className="compare-table compare-table-numbers">
      <thead>
        <tr>
          <th scope="col">Parameter</th>
          <th scope="col">{firstLabel}</th>
          {second && <th scope="col">{secondLabel}</th>}
        </tr>
      </thead>
      <tbody>
        {first.map((value, index) => (
          <tr key={index}>
            <th scope="row">{`w${index}`}</th>
            <td>{value.toFixed(4)}</td>
            {second && <td>{second[index]?.toFixed(4)}</td>}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** A share from 0 to 1 as a percentage, e.g. "9.5%". */
export function formatPercent(share: number): string {
  return `${(share * 100).toFixed(1)}%`;
}

/** An interval in days as words, e.g. "10 min", "5 hours", "2.3 days", "36 days". */
export function formatInterval(days: number): string {
  const minutes = days * 24 * 60;
  if (minutes < 60) return `${Math.max(1, Math.round(minutes))} min`;
  const hours = minutes / 60;
  if (hours < 23.5) {
    const rounded = Math.round(hours);
    return `${rounded} ${rounded === 1 ? "hour" : "hours"}`;
  }
  if (days < 10) {
    const rounded = Math.round(days * 10) / 10;
    return `${rounded} ${rounded === 1 ? "day" : "days"}`;
  }
  return `${Math.round(days).toLocaleString()} days`;
}
