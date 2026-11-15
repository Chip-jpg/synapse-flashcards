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
import { Retention } from "./Retention";
import { translate, useLang, useT, type Lang, type MessageKey } from "./i18n";
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
 * Settings: the FSRS parameters reviews are scheduled with, fitting them to
 * the user's own history, and the desired retention FSRS aims for (see
 * `Retention.tsx`). Rust does the fitting, decides whether the result may be
 * applied, and keeps every change. Opened from the sidebar, so it takes focus.
 */
export function Settings() {
  const { lang, setLang } = useLang();
  const { t } = useT();
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

  const language = (
    <section className="panel" aria-labelledby="language-heading">
      <div className="section-heading">
        <h3 id="language-heading" className="section-title">{t("settings.language")}</h3>
      </div>
      <label htmlFor="language">{t("settings.language")}</label>{" "}
      <select id="language" value={lang} onChange={(event) => setLang(event.target.value as Lang)}>
        <option value="en">{t("settings.english")}</option>
        <option value="it">{t("settings.italian")}</option>
      </select>
    </section>
  );

  if (state.status === "loading") {
    return (
      <>
        {language}
        <p className="loading" role="status">{t("settings.loading")}</p>
      </>
    );
  }

  if (state.status === "error") {
    return (
      <>
      {language}
      <Message focus tone="alert" text={state.message}>
        <button type="button" className="button" onClick={() => reload(null)}>
          {t("settings.retry")}
        </button>
      </Message>
      </>
    );
  }

  return (
    <>
    {language}
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
    </>
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
  const { t } = useT();
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
        setStatus(t("settings.stopped"));
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
        t("settings.applied")
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
    setStatus(t("settings.discarded"));
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
        t("settings.resetNotice")
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
            {t("settings.title")}
          </h2>
          <p className="page-intro">{t("settings.intro")}</p>
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
            {t("settings.personal")}
          </h3>
        </div>

        <p className="settings-text">
          {settings.personal
            ? t("settings.personalHistory", {
                reviews: settings.reviewsUsed !== null
                  ? t(settings.reviewsUsed === 1 ? "settings.reviewCountOne" : "settings.reviewCount", {
                      count: settings.reviewsUsed.toLocaleString(),
                    })
                  : "",
                applied: appliedOn ? t("settings.appliedDate", { date: appliedOn }) : "",
              })
            : t("settings.defaultHistory")}
        </p>

        <div className="data-action">
          <p id="optimize-hint" className="field-hint">
            {enough
              ? t("settings.learnable", {
                  count: settings.usableReviews.toLocaleString(),
                })
              : t("settings.needMore", {
                  count: settings.usableReviews.toLocaleString(),
                  minimum: settings.minimumReviews.toLocaleString(),
                })}
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
              {t("settings.optimize")}
            </button>
            {optimizing && (
              <button type="button" className="button" onClick={() => void stopOptimizing()}>
                {t("settings.stop")}
              </button>
            )}
          </div>
          <p className="form-status" role="status">
            {optimizing ? t("settings.fitting") : status}
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
              {t("settings.reset")}
            </button>
          </div>
        )}

        {run.kind === "confirmReset" && (
          <div className="confirm confirm-calm">
            <p ref={questionRef} className="confirm-question" tabIndex={-1}>
              {t("settings.resetQuestion")}
            </p>
            <div className="form-actions">
              <button
                type="button"
                className="button button-primary button-small"
                aria-disabled={run.resetting}
                onClick={reset}
              >
                {t("settings.reset")}
              </button>
              <button
                type="button"
                className="button button-small"
                aria-disabled={run.resetting}
                onClick={() => {
                  if (!run.resetting) setRun({ kind: "idle" });
                }}
              >
                {t("settings.keep")}
              </button>
            </div>
            <p className="form-status" role="status">
              {run.resetting ? t("settings.resetting") : ""}
            </p>
          </div>
        )}

        {error && (
          <p className="message-error" role="alert">
            {error}
          </p>
        )}

        <details className="settings-details">
          <summary>{t("settings.parametersInUse")}</summary>
          <ParameterTable first={settings.parameters} firstLabel={t("settings.inUse")} />
        </details>
      </section>

      <Retention />
    </section>
  );
}

const CASES: { case: IntervalCase; label: MessageKey }[] = [
  { case: "firstAgain", label: "settings.caseAgain" },
  { case: "firstHard", label: "settings.caseHard" },
  { case: "firstGood", label: "settings.caseGood" },
  { case: "firstEasy", label: "settings.caseEasy" },
  { case: "secondGood", label: "settings.caseSecondGood" },
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
  const { t } = useT();
  return (
    <div className="settings-result">
      <h4 ref={headingRef} className="item-title" tabIndex={-1}>
        {t(outcome.reviewsUsed === 1 ? "settings.fittedCountOne" : "settings.fittedCount", {
          count: outcome.reviewsUsed.toLocaleString(),
        })}
      </h4>
      <p className="settings-text">
        {t("settings.prediction", {
          current: formatPercent(outcome.currentError).slice(0, -1),
          proposed: formatPercent(outcome.proposedError).slice(0, -1),
        })}
      </p>

      <table className="compare-table">
        <caption>{t("settings.nextReview")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("settings.rating")}</th>
            <th scope="col">{t("settings.inUse")}</th>
            <th scope="col">{t("settings.fitted")}</th>
          </tr>
        </thead>
        <tbody>
          {CASES.map(({ case: name, label }) => {
            const change = outcome.intervals.find((interval) => interval.case === name);
            if (!change) return null;
            return (
              <tr key={name}>
                <th scope="row">{t(label)}</th>
                <td>{formatInterval(change.current, t)}</td>
                <td>{formatInterval(change.proposed, t)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {outcome.better ? (
        <div className="confirm confirm-calm">
          <p className="confirm-question">
            {t("settings.better")}
          </p>
          <div className="form-actions">
            <button
              type="button"
              className="button button-primary button-small"
              aria-disabled={applying}
              onClick={onApply}
            >
              {t("settings.apply")}
            </button>
            <button
              type="button"
              className="button button-small"
              aria-disabled={applying}
              onClick={onDiscard}
            >
              {t("settings.discard")}
            </button>
          </div>
          <p className="form-status" role="status">
            {applying ? t("settings.applying") : ""}
          </p>
        </div>
      ) : (
        <div className="confirm">
          <p className="confirm-question">
            {t("settings.notBetter")}
          </p>
          <div className="form-actions">
            <button type="button" className="button button-small" onClick={onDiscard}>
              {t("settings.close")}
            </button>
          </div>
        </div>
      )}

      <details className="settings-details">
        <summary>{t("settings.allParameters")}</summary>
        <ParameterTable
          first={outcome.current}
          firstLabel={t("settings.inUse")}
          second={outcome.proposed}
          secondLabel={t("settings.fitted")}
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
  const { t } = useT();
  return (
    <table className="compare-table compare-table-numbers">
      <thead>
        <tr>
          <th scope="col">{t("settings.parameter")}</th>
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
export function formatInterval(
  days: number,
  t: ReturnType<typeof useT>["t"] = (key, params) => translate("en", key, params)
): string {
  const minutes = days * 24 * 60;
  if (minutes < 60) return t("settings.minute", { count: Math.max(1, Math.round(minutes)) });
  const hours = minutes / 60;
  if (hours < 23.5) {
    const rounded = Math.round(hours);
    return t(rounded === 1 ? "settings.hourOne" : "settings.hourOther", { count: rounded });
  }
  if (days < 10) {
    const rounded = Math.round(days * 10) / 10;
    return t(rounded === 1 ? "settings.dayOne" : "settings.dayOther", { count: rounded });
  }
  return t("settings.dayOther", { count: Math.round(days).toLocaleString() });
}
