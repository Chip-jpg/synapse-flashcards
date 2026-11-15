import { useEffect, useRef, useState, type RefObject } from "react";
import {
  getRetention,
  previewRetention,
  setRetention,
  toCommandError,
  type RetentionPreview,
  type RetentionSettings,
} from "./api";
import { translate, useT } from "./i18n";
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
  const { t } = useT();
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
          {t("retention.title")}
        </h3>
      </div>

      {state.status === "loading" && (
        <p className="loading" role="status">
          {t("retention.loading")}
        </p>
      )}

      {state.status === "error" && (
        <>
          <p className="message-error" role="alert">
            {state.message}
          </p>
          <div className="form-actions">
            <button type="button" className="button" onClick={() => reload(null)}>
              {t("retention.retry")}
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
  const { t, plural } = useT();
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
    return target.deckId === null ? `${settings.global}%` : t("retention.allDecksValue", { value: settings.global });
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
          ? t("retention.changedAll", { value: current.choice ?? "" })
          : current.choice === null
            ? t("retention.changedFollow", { name: current.target.name })
            : t("retention.changedValue", { name: current.target.name, value: current.choice });
      onChanged(
        next,
        what + t("retention.changedSuffix")
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
    { deckId: null, name: t("retention.allDecks"), value: `${settings.global}%` },
    ...settings.decks.map((deck) => ({
      deckId: deck.id,
      name: deck.name,
      value: deck.percent === null ? t("retention.inheritedValue", { value: settings.global }) : `${deck.percent}%`,
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
        {t("retention.description")}
      </p>

      <table className="compare-table">
        <caption>{t("retention.aimsFor")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("retention.decks")}</th>
            <th scope="col">{t("retention.title")}</th>
            <th scope="col" aria-label={t("retention.actions")} />
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
                    aria-label={t("retention.changeLabel", {
                      name: target.deckId === null ? t("retention.changeForAll") : target.name,
                    })}
                    aria-disabled={editing?.busy != null}
                    onClick={() => open(target)}
                  >
                    {t("retention.change")}
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      <p className="field-hint">
        {t("retention.followHint")}
        {settings.archivedOwn > 0 &&
          " " + plural(settings.archivedOwn, "retention.archivedOne", "retention.archivedOther")}
      </p>

      {editing && (
        <div className="settings-result">
          <div className="field retention-choice">
            <label className="field-label" htmlFor="retention-choice">
              {editing.target.deckId === null
                ? t("retention.forAll")
                : t("retention.forDeck", { name: editing.target.name })}
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
                <option value="">{t("retention.followAll", { value: settings.global })}</option>
              )}
              {options.map((percent) => (
                <option key={percent} value={percent}>
                  {percent === settings.default
                    ? t("retention.recommendation", { value: percent })
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
                  {t("retention.estimateReviews")}
                </button>
                <button
                  type="button"
                  className="button button-small"
                  aria-disabled={editing.busy !== null}
                  onClick={close}
                >
                  {t("retention.cancel")}
                </button>
              </div>
              <p className="form-status" role="status">
                {editing.busy === "estimating"
                  ? t("retention.estimating")
                  : editing.choice === inUse(editing.target)
                    ? t("retention.sameValue")
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
                  ? t("retention.aimAllQuestion", { value: editing.choice ?? "" })
                  : editing.choice === null
                    ? t("retention.followQuestion", { name: editing.target.name, value: settings.global })
                    : t("retention.aimQuestion", { name: editing.target.name, value: editing.choice })
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
  const { t, plural } = useT();
  return (
    <>
      <h4 ref={headingRef} className="item-title" tabIndex={-1}>
        {t("retention.estimated")}
      </h4>
      {preview.cards === 0 ? (
        <p className="settings-text">
          {t("retention.noCards")}
        </p>
      ) : (
        <>
          <table className="compare-table">
            <caption>{plural(preview.days, "retention.nextDayOne", "retention.nextDays")}</caption>
            <thead>
              <tr>
                <th scope="col">{t("retention.estimate")}</th>
                <th scope="col">{t("retention.inUse", { value: currentLabel })}</th>
                <th scope="col">{t("retention.chosen", { value: chosenLabel })}</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <th scope="row">{t("retention.reviewsPerDay")}</th>
                <td>{formatReviews(preview.current.reviewsPerDay)}</td>
                <td>{formatReviews(preview.proposed.reviewsPerDay)}</td>
              </tr>
              <tr>
                <th scope="row">{t("retention.rememberedDay", { count: preview.days })}</th>
                <td>{formatRemembered(preview.current.remembered, preview.cards, t)}</td>
                <td>{formatRemembered(preview.proposed.remembered, preview.cards, t)}</td>
              </tr>
            </tbody>
          </table>
          <p className="field-hint">
            {t("retention.estimateCaveat", {
              count: preview.cards.toLocaleString(),
              cardWord: plural(preview.cards, "retention.cardOne", "retention.cardOther"),
              ratings: t(preview.ownRatings ? "retention.ownRatings" : "retention.averageRatings"),
            })}
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
            {t("retention.apply")}
          </button>
          <button
            type="button"
            className="button button-small"
            aria-disabled={applying}
            onClick={onCancel}
          >
            {t("retention.cancel")}
          </button>
        </div>
        <p className="form-status" role="status">
          {applying ? t("retention.applying") : ""}
        </p>
      </div>
    </>
  );
}

/** Reviews a day as words, e.g. "0.4", "4.0", "12". */
export function formatReviews(perDay: number): string {
  return perDay < 9.95 ? perDay.toFixed(1) : Math.round(perDay).toLocaleString();
}

/**
 * Cards remembered out of all of them, with the share, which still shows a
 * difference when the counts round the same: e.g. "131 of 141 (93.0%)".
 */
export function formatRemembered(
  remembered: number,
  cards: number,
  t: ReturnType<typeof useT>["t"] = (key, params) => translate("en", key, params)
): string {
  const share = cards > 0 ? (remembered / cards) * 100 : 0;
  return t("retention.remembered", {
    remembered: Math.round(remembered).toLocaleString(),
    cards: cards.toLocaleString(),
    percent: share.toFixed(1),
  });
}
