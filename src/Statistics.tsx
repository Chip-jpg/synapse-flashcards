import { useEffect, useId, useState } from "react";
import {
  getDecks,
  getStatistics,
  toCommandError,
  type DayCount,
  type Deck,
  type Statistics,
} from "./api";
import { Message, useFocusOnMount } from "./ui";

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
  // The decks to filter by. If they can't be read, the screen still shows
  // every deck's numbers, just without the filter.
  const [decks, setDecks] = useState<Deck[]>([]);
  const [query, setQuery] = useState<Query>({ deckId: null, attempt: 0 });
  const [state, setState] = useState<StatsState>({ status: "loading" });
  const [notice, setNotice] = useState<string | null>(null);
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
          setNotice("That deck was archived, so these are the numbers for all your decks.");
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
            Statistics
          </h2>
          <p className="page-intro">
            Counted from your review history and your cards' schedules, by the days on this
            computer's calendar. Reviews of cards you've since deleted or archived still count as
            reviews; cards by state and what's due count only the cards you can study now.
          </p>
        </div>
        {decks.length > 0 && (
          <div className="field stats-filter">
            <label className="field-label" htmlFor={filterId}>
              Deck
            </label>
            <select
              id={filterId}
              className="field-input"
              value={query.deckId ?? ""}
              onChange={(e) => choose(e.target.value === "" ? null : Number(e.target.value))}
            >
              <option value="">All decks</option>
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
          {notice}
        </p>
      )}

      {state.status === "loading" && (
        <p className="loading" role="status">
          Loading your statistics…
        </p>
      )}
      {state.status === "error" && (
        <Message focus={false} tone="alert" text={state.message}>
          <button type="button" className="button" onClick={retry}>
            Retry
          </button>
        </Message>
      )}
      {state.status === "ready" && <StatsPanels stats={state.stats} />}
    </section>
  );
}

function StatsPanels({ stats }: { stats: Statistics }) {
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
          Reviews per day
        </h3>
        <p className="field-hint">
          {totalReviews === 0
            ? "No reviews in the last 30 days."
            : `${plural(totalReviews, "review")} in the last 30 days, every rating counted.`}
        </p>
        <BarChart days={stats.reviewsByDay} unit="review" tone="primary" endsToday />
      </section>

      <section className="panel" aria-labelledby="stats-retention">
        <h3 id="stats-retention" className="section-title">
          True retention
        </h3>
        {reviews === 0 ? (
          <p className="field-hint">
            Nothing to measure yet: no cards in the Review state were reviewed in the last 30 days.
          </p>
        ) : (
          <>
            <p className="stats-figure">{percent(recalled, reviews)}</p>
            <p className="field-hint">
              {recalled} of {plural(reviews, "review")} of cards in the Review state in the last 30
              days weren't rated Again. New and (re)learning cards aren't counted.
            </p>
          </>
        )}
      </section>

      <section className="panel" aria-labelledby="stats-states">
        <h3 id="stats-states" className="section-title">
          Cards by state
        </h3>
        {totalCards === 0 ? (
          <p className="field-hint">No cards to count.</p>
        ) : (
          <dl className="stats-states">
            <StateRow label="New" hint="not studied yet" count={cardStates.new} />
            <StateRow label="Learning" hint="first steps" count={cardStates.learning} />
            <StateRow label="Review" hint="learned, spaced out" count={cardStates.review} />
            <StateRow
              label="Relearning"
              hint="forgotten, relearning"
              count={cardStates.relearning}
            />
          </dl>
        )}
      </section>

      <section className="panel stats-wide" aria-labelledby="stats-forecast">
        <h3 id="stats-forecast" className="section-title">
          Due in the next 14 days
        </h3>
        <p className="field-hint">
          {totalDue === 0
            ? "Nothing falls due in the next 14 days."
            : `${plural(totalDue, "card")} due. Today's bar also holds cards already overdue and new cards not yet studied.`}
        </p>
        <BarChart days={stats.forecast} unit="card" tone="sage" startsToday />
      </section>
    </div>
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
  unit: string;
  tone: "primary" | "sage";
  endsToday?: boolean;
  startsToday?: boolean;
}) {
  const max = Math.max(1, ...days.map((d) => d.count));
  const width = days.length * (BAR + GAP) - GAP;
  const first = days[0];
  const last = days[days.length - 1];
  const busiest = days.reduce((top, d) => (d.count > top.count ? d : top), days[0]);
  const description =
    busiest.count === 0
      ? `No ${unit}s on any day from ${formatDay(first.day)} to ${formatDay(last.day)}.`
      : `From ${formatDay(first.day)} to ${formatDay(last.day)}. The most was ${plural(busiest.count, unit)}, on ${formatDay(busiest.day)}.`;

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
              <title>{`${formatDay(d.day)}: ${plural(d.count, unit)}`}</title>
            </rect>
          );
        })}
      </svg>
      <figcaption className="chart-axis" aria-hidden="true">
        <span>{startsToday ? "Today" : formatDay(first.day)}</span>
        <span>Most: {busiest.count}</span>
        <span>{endsToday ? "Today" : formatDay(last.day)}</span>
      </figcaption>
    </figure>
  );
}

function sum(days: DayCount[]): number {
  return days.reduce((total, d) => total + d.count, 0);
}

function plural(count: number, unit: string): string {
  return `${count} ${count === 1 ? unit : `${unit}s`}`;
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
