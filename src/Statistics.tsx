import { useEffect, useId, useState } from "react";
import {
  getDecks,
  getStatistics,
  getStudyCalendar,
  toCommandError,
  type DayCount,
  type Deck,
  type Statistics,
  type StudyCalendar,
} from "./api";
import { Message, useFocusOnMount } from "./ui";
import { useT } from "./i18n";

type StatsState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; stats: Statistics };

/** What to load: every deck (`deckId` null) or one, and what to announce. */
type Query = { deckId: number | null; attempt: number };

/**
 * Statistics: the last 30 days of reviewing, true retention, the current cards
 * by state, and what falls due over the next 14 days. Every number is counted
 * by Rust from what is stored; nothing here is a score or a goal. Opened from
 * the sidebar, so it takes focus.
 */
export function StatisticsScreen() {
  const { t } = useT();
  // The decks to filter by. If they can't be read, the screen still shows
  // every deck's numbers, just without the filter.
  const [decks, setDecks] = useState<Deck[]>([]);
  const [query, setQuery] = useState<Query>({ deckId: null, attempt: 0 });
  const [state, setState] = useState<StatsState>({ status: "loading" });
  const [notice, setNotice] = useState<"stats.archived" | null>(null);
  // The calendar covers every deck and is a side note: without it the rest shows.
  const [calendar, setCalendar] = useState<StudyCalendar | null>(null);
  const screenRef = useFocusOnMount<HTMLElement>(true);
  const filterId = useId();

  useEffect(() => {
    let current = true;
    getDecks()
      .then((list) => {
        if (current) setDecks(list);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [query.attempt]);

  useEffect(() => {
    let current = true;
    getStudyCalendar()
      .then((value) => {
        if (current) setCalendar(value);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [query.attempt]);

  useEffect(() => {
    let current = true;
    getStatistics(query.deckId).then(
      (stats) => {
        if (current) setState({ status: "ready", stats });
      },
      (err) => {
        if (!current) return;
        const error = toCommandError(err);
        if (error.kind === "stale" && query.deckId !== null) {
          // The chosen deck was archived since the list was read: go back to
          // every deck, with a fresh list, and say why.
          setNotice("stats.archived");
          setQuery((previous) => ({ deckId: null, attempt: previous.attempt + 1 }));
          return;
        }
        setState({ status: "error", message: error.message });
      },
    );
    return () => {
      current = false;
    };
  }, [query]);

  function choose(deckId: number | null) {
    setNotice(null);
    setState({ status: "loading" });
    setQuery((previous) => ({ deckId, attempt: previous.attempt }));
  }

  function retry() {
    setState({ status: "loading" });
    setQuery((previous) => ({ ...previous, attempt: previous.attempt + 1 }));
  }

  return (
    <section ref={screenRef} className="stats" tabIndex={-1} aria-labelledby="stats-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="stats-heading" className="page-title">
            {t("stats.title")}
          </h2>
          <p className="page-intro">
            {t("stats.intro")}
          </p>
        </div>
        {decks.length > 0 && (
          <div className="field stats-filter">
            <label className="field-label" htmlFor={filterId}>
              {t("stats.deck")}
            </label>
            <select
              id={filterId}
              className="field-input"
              value={query.deckId ?? ""}
              onChange={(e) => choose(e.target.value === "" ? null : Number(e.target.value))}
            >
              <option value="">{t("stats.allDecks")}</option>
              {decks.map((deck) => (
                <option key={deck.id} value={deck.id}>
                  {deck.name}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>

      {notice && (
        <p className="notice" role="status">
          {t(notice)}
        </p>
      )}

      {state.status === "loading" && (
        <p className="loading" role="status">
          {t("stats.loading")}
        </p>
      )}
      {state.status === "error" && (
        <Message focus={false} tone="alert" text={state.message}>
          <button type="button" className="button" onClick={retry}>
            {t("stats.retry")}
          </button>
        </Message>
      )}
      {state.status === "ready" && <StatsPanels stats={state.stats} />}
      {calendar && calendar.days.length > 0 && <CalendarPanel calendar={calendar} />}
    </section>
  );
}

function StatsPanels({ stats }: { stats: Statistics }) {
  const { t, plural } = useT();
  const totalReviews = sum(stats.reviewsByDay);
  const totalDue = sum(stats.forecast);
  const { reviews, recalled } = stats.retention;
  const { cardStates } = stats;
  const totalCards =
    cardStates.new + cardStates.learning + cardStates.review + cardStates.relearning;

  return (
    <div className="stats-grid">
      <section className="panel stats-wide" aria-labelledby="stats-reviews">
        <h3 id="stats-reviews" className="section-title">
          {t("stats.reviewsTitle")}
        </h3>
        <p className="field-hint">
          {totalReviews === 0
            ? t("stats.noReviews30")
            : plural(totalReviews, "stats.reviews30One", "stats.reviews30Other")}
        </p>
        <BarChart days={stats.reviewsByDay} unit="review" tone="primary" endsToday />
      </section>

      <section className="panel" aria-labelledby="stats-retention">
        <h3 id="stats-retention" className="section-title">
          {t("stats.retentionTitle")}
        </h3>
        {reviews === 0 ? (
          <p className="field-hint">
            {t("stats.retentionEmpty")}
          </p>
        ) : (
          <>
            <p className="stats-figure">{percent(recalled, reviews)}</p>
            <p className="field-hint">
              {plural(reviews, "stats.retentionOne", "stats.retentionOther", { recalled })}
            </p>
          </>
        )}
      </section>

      <section className="panel" aria-labelledby="stats-states">
        <h3 id="stats-states" className="section-title">
          {t("stats.statesTitle")}
        </h3>
        {totalCards === 0 ? (
          <p className="field-hint">{t("stats.noCards")}</p>
        ) : (
          <dl className="stats-states">
            <StateRow label={t("stats.new")} hint={t("stats.newHint")} count={cardStates.new} />
            <StateRow label={t("stats.learning")} hint={t("stats.learningHint")} count={cardStates.learning} />
            <StateRow label={t("stats.review")} hint={t("stats.reviewHint")} count={cardStates.review} />
            <StateRow
              label={t("stats.relearning")}
              hint={t("stats.relearningHint")}
              count={cardStates.relearning}
            />
          </dl>
        )}
      </section>

      <section className="panel stats-wide" aria-labelledby="stats-forecast">
        <h3 id="stats-forecast" className="section-title">
          {t("stats.forecastTitle")}
        </h3>
        <p className="field-hint">
          {totalDue === 0
            ? t("stats.noDue")
            : t("stats.due", { cards: plural(totalDue, "common.cardOne", "common.cardOther") })}
        </p>
        <BarChart days={stats.forecast} unit="card" tone="sage" startsToday />
      </section>
    </div>
  );
}

/** Heat level 0 (no reviews) to 4, relative to the busiest day. */
function level(count: number, max: number): number {
  return count === 0 ? 0 : Math.max(1, Math.ceil((count / max) * 4));
}

/**
 * The last 365 days as a calendar heatmap with the current and longest
 * streak. A streak is consecutive local days with at least one review; the
 * current one is still alive if today has no review yet but yesterday did.
 * Each square says its date and count in its title and accessible label.
 */
function CalendarPanel({ calendar }: { calendar: StudyCalendar }) {
  const { t, plural } = useT();
  const { days, currentStreak, longestStreak } = calendar;
  const max = Math.max(1, ...days.map((d) => d.reviews));
  const total = days.reduce((n, d) => n + d.reviews, 0);
  const [year, month, date] = days[0].date.split("-").map(Number);
  // Weeks are columns starting on Sunday, so the first day sits on its weekday.
  const lead = new Date(year, month - 1, date).getDay();

  return (
    <section className="panel stats-calendar" aria-labelledby="stats-calendar">
      <h3 id="stats-calendar" className="section-title">
        {t("stats.calendarTitle")}
      </h3>
      <dl className="stats-streaks">
        <div className="stats-state">
          <dt>{t("stats.currentStreak")}</dt>
          <dd>{plural(currentStreak, "stats.dayOne", "stats.dayOther")}</dd>
        </div>
        <div className="stats-state">
          <dt>{t("stats.longestStreak")}</dt>
          <dd>{plural(longestStreak, "stats.dayOne", "stats.dayOther")}</dd>
        </div>
      </dl>
      <p className="field-hint">
        {total === 0
          ? t("stats.noReviews365")
          : plural(total, "stats.reviews365One", "stats.reviews365Other")}
      </p>
      <ol className="heatmap" aria-label={t("stats.calendarLabel")}>
        {Array.from({ length: lead }, (_, i) => (
          <li key={`lead-${i}`} className="heat-cell heat-blank" aria-hidden="true" />
        ))}
        {days.map((d) => {
          const label = t("stats.reviewsOn", { reviews: plural(d.reviews, "stats.reviewOne", "stats.reviewOther"), date: d.date });
          return (
            <li key={d.date} className="heat-cell-item">
              <span
                role="img"
                aria-label={label}
                title={label}
                className={`heat-cell heat-${level(d.reviews, max)}`}
              />
            </li>
          );
        })}
      </ol>
      <p className="chart-axis heat-legend" aria-hidden="true">
        <span>{t("stats.less")}</span>
        {[0, 1, 2, 3, 4].map((n) => (
          <span key={n} className={`heat-cell heat-${n}`} />
        ))}
        <span>{t("stats.more")}</span>
      </p>
    </section>
  );
}

function StateRow({ label, hint, count }: { label: string; hint: string; count: number }) {
  return (
    <div className="stats-state">
      <dt>
        {label} <span className="stats-state-hint">({hint})</span>
      </dt>
      <dd>{count}</dd>
    </div>
  );
}

const BAR = 20;
const GAP = 6;
const HEIGHT = 120;

/**
 * A plain SVG bar chart, one bar per day. Each bar's tooltip and the chart's
 * description say the same numbers, so it reads without seeing it.
 */
function BarChart({
  days,
  unit,
  tone,
  endsToday = false,
  startsToday = false,
}: {
  days: DayCount[];
  unit: "review" | "card";
  tone: "primary" | "sage";
  endsToday?: boolean;
  startsToday?: boolean;
}) {
  const { t, plural } = useT();
  const countText = (count: number) => unit === "card"
    ? plural(count, "common.cardOne", "common.cardOther")
    : plural(count, "stats.reviewOne", "stats.reviewOther");
  const max = Math.max(1, ...days.map((d) => d.count));
  const width = days.length * (BAR + GAP) - GAP;
  const first = days[0];
  const last = days[days.length - 1];
  const busiest = days.reduce((top, d) => (d.count > top.count ? d : top), days[0]);
  const description =
    busiest.count === 0
      ? t(unit === "card" ? "stats.chartNoCards" : "stats.chartNoReviews", { first: formatDay(first.day), last: formatDay(last.day) })
      : t("stats.chartDescription", { first: formatDay(first.day), last: formatDay(last.day), count: countText(busiest.count), date: formatDay(busiest.day) });

  return (
    <figure className="chart">
      <svg
        className={`chart-svg chart-${tone}`}
        viewBox={`0 0 ${width} ${HEIGHT}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={description}
      >
        <line className="chart-base" x1={0} x2={width} y1={HEIGHT - 0.5} y2={HEIGHT - 0.5} />
        {days.map((d, i) => {
          const height = d.count === 0 ? 0 : Math.max(2, (d.count / max) * (HEIGHT - 4));
          return (
            <rect
              key={d.day}
              className="chart-bar"
              x={i * (BAR + GAP)}
              y={HEIGHT - height}
              width={BAR}
              height={height}
              rx={3}
            >
              <title>{`${formatDay(d.day)}: ${countText(d.count)}`}</title>
            </rect>
          );
        })}
      </svg>
      <figcaption className="chart-axis" aria-hidden="true">
        <span>{startsToday ? t("stats.today") : formatDay(first.day)}</span>
        <span>{t("stats.most", { count: busiest.count })}</span>
        <span>{endsToday ? t("stats.today") : formatDay(last.day)}</span>
      </figcaption>
    </figure>
  );
}

function sum(days: DayCount[]): number {
  return days.reduce((total, d) => total + d.count, 0);
}

/** A share as a whole percentage, e.g. "87%". */
function percent(part: number, whole: number): string {
  return `${Math.round((part / whole) * 100)}%`;
}

/** A local `YYYY-MM-DD` date as a short date, e.g. "Oct 19". */
function formatDay(day: string): string {
  const [year, month, date] = day.split("-").map(Number);
  return new Date(year, month - 1, date).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}
