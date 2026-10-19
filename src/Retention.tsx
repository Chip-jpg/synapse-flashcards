import { useEffect, useRef, useState, type RefObject } from "react";
import {
  getRetention,
  previewRetention,
  setRetention,
  toCommandError,
  type RetentionPreview,
  type RetentionSettings,
} from "./api";
import { ChartIcon } from "./icons";
import { useFocusOnMount } from "./ui";

type LoadState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; settings: RetentionSettings };

/** Fetches the settings and describes the outcome; never rejects. */
async function loadRetention(): Promise<LoadState> {
  try {
    return { status: "ready", settings: await getRetention() };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * The desired retention panel of Settings: the value FSRS aims for, for all
 * decks and for each deck. Rust checks every change, estimates its reviews
 * with FSRS's simulator, and keeps every change; this only presents them.
 */
export function Retention() {
  const [state, setState] = useState<LoadState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  // `key` changes every time, so the panel remounts fresh and its notice takes focus.
  const [view, setView] = useState<{ key: number; notice: string | null }>({
    key: 0,
    notice: null,
  });

  useEffect(() => {
    let current = true;
    loadRetention().then((next) => {
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

  return (
    <section className="panel" aria-labelledby="retention-heading">
      <div className="section-heading">
        <span className="section-icon" aria-hidden="true">
          <ChartIcon />
        </span>
        <h3 id="retention-heading" className="section-title">
          Desired retention
        </h3>
      </div>

      {state.status === "loading" && (
        <p className="loading" role="status">
          Loading the desired retention…
        </p>
      )}

      {state.status === "error" && (
        <>
          <p className="message-error" role="alert">
            {state.message}
          </p>
          <div className="form-actions">
            <button type="button" className="button" onClick={() => reload(null)}>
              Retry
            </button>
          </div>
        </>
      )}

      {state.status === "ready" && (
        <RetentionPanel
          key={view.key}
          settings={state.settings}
          notice={view.notice}
          onChanged={(next, notice) => {
            setState({ status: "ready", settings: next });
            setView((previous) => ({ key: previous.key + 1, notice }));
          }}
          onStale={reload}
        />
      )}
    </section>
  );
}

/** What's being changed: all decks (`deckId` null) or one deck. */
type Target = { deckId: number | null; name: string };

type Editing = {
  target: Target;
  /** The value chosen; null for a deck to follow the value for all decks. */
  choice: number | null;
  preview: RetentionPreview | null;
  busy: "estimating" | "applying" | null;
};

function RetentionPanel({
  settings,
  notice,
  onChanged,
  onStale,
}: {
  settings: RetentionSettings;
  notice: string | null;
  onChanged: (next: RetentionSettings, notice: string) => void;
  onStale: (message: string) => void;
}) {
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(notice !== null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second press before the re-render lands.
  const busyRef = useRef(false);
  const selectRef = useRef<HTMLSelectElement>(null);
  const resultRef = useRef<HTMLHeadingElement>(null);

  const editingKey = editing ? (editing.target.deckId ?? "all") : null;
  const hasPreview = editing?.preview != null;

  useEffect(() => {
    if (editingKey !== null) selectRef.current?.focus();
  }, [editingKey]);

  useEffect(() => {
    if (hasPreview) resultRef.current?.focus();
  }, [hasPreview]);

  /** The value a target has now: a percentage, or null for a deck following all decks. */
  function inUse(target: Target): number | null {
    if (target.deckId === null) return settings.global;
    return settings.decks.find((deck) => deck.id === target.deckId)?.percent ?? null;
  }

  function label(target: Target, value: number | null): string {
    if (value !== null) return `${value}%`;
    return target.deckId === null ? `${settings.global}%` : `all decks (${settings.global}%)`;
  }

  function open(target: Target) {
    if (busyRef.current) return;
    setError(null);
    setEditing({ target, choice: inUse(target), preview: null, busy: null });
  }

  function close() {
    if (busyRef.current || !editing) return;
    const key = editing.target.deckId ?? "all";
    setEditing(null);
    setError(null);
    document.getElementById(`retention-change-${key}`)?.focus();
  }

  async function estimate(current: Editing) {
    if (busyRef.current || current.choice === inUse(current.target)) return;
    busyRef.current = true;
    setError(null);
    setEditing({ ...current, preview: null, busy: "estimating" });
    try {
      const preview = await previewRetention(settings.version, {
        deckId: current.target.deckId,
        percent: current.choice,
      });
      setEditing({ ...current, preview, busy: null });
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale(failure.message);
        return;
      }
      setEditing({ ...current, preview: null, busy: null });
      setError(failure.message);
    }
    busyRef.current = false;
  }

  async function apply(current: Editing) {
    if (busyRef.current || !current.preview) return;
    busyRef.current = true;
    setError(null);
    setEditing({ ...current, busy: "applying" });
    try {
      const next = await setRetention(settings.version, {
        deckId: current.target.deckId,
        percent: current.choice,
      });
      const what =
        current.target.deckId === null
          ? `Decks without a value of their own now aim for ${current.choice}%.`
          : current.choice === null
            ? `${current.target.name} now follows the value for all decks.`
            : `${current.target.name} now aims for ${current.choice}%.`;
      onChanged(
        next,
        `${what} Each card is scheduled with it from its next rating; due dates already set stay.`
      );
      return;
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale(failure.message);
        return;
      }
      setEditing({ ...current, busy: null });
      setError(failure.message);
    }
    busyRef.current = false;
  }

  const targets: (Target & { value: string })[] = [
    { deckId: null, name: "All decks", value: `${settings.global}%` },
    ...settings.decks.map((deck) => ({
      deckId: deck.id,
      name: deck.name,
      value: deck.percent === null ? `${settings.global}% (all decks)` : `${deck.percent}%`,
    })),
  ];

  const options: number[] = [];
  for (let percent = settings.minimum; percent <= settings.maximum; percent++) {
    options.push(percent);
  }

  return (
    <>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      <p className="settings-text">
        FSRS schedules each card for when you&apos;d still have this chance of remembering it.
        Aiming higher means shorter gaps and more reviews; aiming lower means fewer reviews and
        more forgetting. A change applies to each card from its next rating: due dates already set
        stay.
      </p>

      <table className="compare-table">
        <caption>Aims for</caption>
        <thead>
          <tr>
            <th scope="col">Decks</th>
            <th scope="col">Desired retention</th>
            <th scope="col" aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {targets.map((target) => {
            const key = target.deckId ?? "all";
            return (
              <tr key={key}>
                <th scope="row">{target.name}</th>
                <td>{target.value}</td>
                <td>
                  <button
                    id={`retention-change-${key}`}
                    type="button"
                    className="button button-small"
                    aria-label={`Change ${target.deckId === null ? "for all decks" : target.name}`}
                    aria-disabled={editing?.busy != null}
                    onClick={() => open(target)}
                  >
                    Change
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      <p className="field-hint">
        The sample deck and archived decks follow the value for all decks.
        {settings.archivedOwn > 0 &&
          (settings.archivedOwn === 1
            ? " One archived deck keeps a value of its own for when it's unarchived."
            : ` ${settings.archivedOwn} archived decks keep a value of their own for when ` +
              "they're unarchived.")}
      </p>

      {editing && (
        <div className="settings-result">
          <div className="field">
            <label className="field-label" htmlFor="retention-choice">
              {editing.target.deckId === null
                ? "Desired retention for all decks"
                : `Desired retention for ${editing.target.name}`}
            </label>
            <select
              ref={selectRef}
              id="retention-choice"
              className="field-input"
              value={editing.choice ?? ""}
              disabled={editing.busy !== null}
              onChange={(e) => {
                setError(null);
                setEditing({
                  ...editing,
                  choice: e.target.value === "" ? null : Number(e.target.value),
                  preview: null,
                });
              }}
            >
              {editing.target.deckId !== null && (
                <option value="">Follow all decks ({settings.global}%)</option>
              )}
              {options.map((percent) => (
                <option key={percent} value={percent}>
                  {percent === settings.default
                    ? `${percent}% (FSRS's recommendation)`
                    : `${percent}%`}
                </option>
              ))}
            </select>
          </div>

          {!editing.preview && (
            <>
              <div className="form-actions">
                <button
                  type="button"
                  className="button button-primary button-small"
                  aria-disabled={
                    editing.busy !== null || editing.choice === inUse(editing.target)
                  }
                  onClick={() => estimate(editing)}
                >
                  Estimate reviews
                </button>
                <button
                  type="button"
                  className="button button-small"
                  aria-disabled={editing.busy !== null}
                  onClick={close}
                >
                  Cancel
                </button>
              </div>
              <p className="form-status" role="status">
                {editing.busy === "estimating"
                  ? "Estimating your reviews…"
                  : editing.choice === inUse(editing.target)
                    ? "That's the value in use. Choose another to see what it would change."
                    : ""}
              </p>
            </>
          )}

          {editing.preview && (
            <Estimate
              preview={editing.preview}
              headingRef={resultRef}
              currentLabel={label(editing.target, inUse(editing.target))}
              chosenLabel={label(editing.target, editing.choice)}
              question={
                editing.target.deckId === null
                  ? `Aim for ${editing.choice}% in every deck without a value of its own?`
                  : editing.choice === null
                    ? `Have ${editing.target.name} follow the value for all decks (${settings.global}%)?`
                    : `Aim for ${editing.choice}% in ${editing.target.name}?`
              }
              applying={editing.busy === "applying"}
              onApply={() => apply(editing)}
              onCancel={close}
            />
          )}

          {error && (
            <p className="message-error" role="alert">
              {error}
            </p>
          )}
        </div>
      )}
    </>
  );
}

/** The estimated reviews, in use against chosen, and the choice to apply. */
function Estimate({
  preview,
  headingRef,
  currentLabel,
  chosenLabel,
  question,
  applying,
  onApply,
  onCancel,
}: {
  preview: RetentionPreview;
  headingRef: RefObject<HTMLHeadingElement | null>;
  currentLabel: string;
  chosenLabel: string;
  question: string;
  applying: boolean;
  onApply: () => void;
  onCancel: () => void;
}) {
  return (
    <>
      <h4 ref={headingRef} className="item-title" tabIndex={-1}>
        Estimated reviews
      </h4>
      {preview.cards === 0 ? (
        <p className="settings-text">
          You have no cards to study yet, so there&apos;s nothing to estimate.
        </p>
      ) : (
        <>
          <table className="compare-table">
            <caption>Over the next {preview.days} days</caption>
            <thead>
              <tr>
                <th scope="col">Estimate</th>
                <th scope="col">In use ({currentLabel})</th>
                <th scope="col">Chosen ({chosenLabel})</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <th scope="row">Reviews a day, on average</th>
                <td>{formatReviews(preview.current.reviewsPerDay)}</td>
                <td>{formatReviews(preview.proposed.reviewsPerDay)}</td>
              </tr>
              <tr>
                <th scope="row">Cards you&apos;d remember on day {preview.days}</th>
                <td>{formatRemembered(preview.current.remembered, preview.cards)}</td>
                <td>{formatRemembered(preview.proposed.remembered, preview.cards)}</td>
              </tr>
            </tbody>
          </table>
          <p className="field-hint">
            An estimate, not a promise. Synapse simulated your{" "}
            {preview.cards.toLocaleString()} {preview.cards === 1 ? "card" : "cards"} with the
            scheduling parameters in use and{" "}
            {preview.ownRatings
              ? "how often you choose each rating"
              : "FSRS's average mix of ratings, since you have few reviews yet"}
            , assuming you review everything that&apos;s due each day and add no new cards. It
            counts first reviews of new cards but not repeats on the same day after Again, and
            real days vary.
          </p>
        </>
      )}
      <div className="confirm confirm-calm">
        <p className="confirm-question">{question}</p>
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
            onClick={onCancel}
          >
            Cancel
          </button>
        </div>
        <p className="form-status" role="status">
          {applying ? "Applying…" : ""}
        </p>
      </div>
    </>
  );
}

/** Reviews a day as words, e.g. "0.4", "3.5", "12". */
export function formatReviews(perDay: number): string {
  return perDay < 10 ? (Math.round(perDay * 10) / 10).toString() : Math.round(perDay).toString();
}

/** Cards remembered, rounded, out of all of them, e.g. "131 of 141". */
export function formatRemembered(remembered: number, cards: number): string {
  return `${Math.round(remembered).toLocaleString()} of ${cards.toLocaleString()}`;
}
