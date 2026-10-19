import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DayCount, Deck, Statistics } from "./api";
import App from "./App";
import { StatisticsScreen } from "./Statistics";
import { deferred, fakeBackend, refuse, type Handler } from "./test/backend";

/** `count` consecutive days from `first` (a `YYYY-MM-DD` date), all with `value`. */
function days(first: string, count: number, value = 0): DayCount[] {
  const [year, month, date] = first.split("-").map(Number);
  return Array.from({ length: count }, (_, i) => {
    const day = new Date(Date.UTC(year, month - 1, date + i));
    return { day: day.toISOString().slice(0, 10), count: value };
  });
}

function stats(overrides: Partial<Statistics> = {}): Statistics {
  return {
    reviewsByDay: days("2026-09-20", 30),
    retention: { reviews: 0, recalled: 0 },
    cardStates: { new: 0, learning: 0, review: 0, relearning: 0 },
    forecast: days("2026-10-19", 14),
    ...overrides,
  };
}

const decks: Deck[] = [
  { id: 1, name: "Biology", description: null, isSample: false, dueCount: 0 },
  { id: 2, name: "History", description: null, isSample: false, dueCount: 3 },
];

function openStats(getStatistics: Handler, getDecks: Handler = () => decks) {
  const backend = fakeBackend({ get_statistics: getStatistics, get_decks: getDecks });
  render(<StatisticsScreen />);
  return backend;
}

describe("the Statistics screen", () => {
  it("is in the sidebar and opens from it", async () => {
    const user = userEvent.setup();
    fakeBackend({ get_statistics: () => stats() });
    render(<App />);
    const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));

    await user.click(sidebar.getByRole("button", { name: "Statistics" }));

    expect(await screen.findByRole("heading", { name: "Statistics", level: 2 })).toBeTruthy();
    expect(sidebar.getByRole("button", { name: "Statistics" }).getAttribute("aria-current")).toBe(
      "page",
    );
  });

  it("totals the reviews of the last 30 days and draws a bar for each day", async () => {
    const reviewsByDay = days("2026-09-20", 30);
    reviewsByDay[28].count = 5;
    reviewsByDay[29].count = 7;
    const { argsOf } = openStats(() => stats({ reviewsByDay }));

    expect(
      await screen.findByText("12 reviews in the last 30 days, every rating counted."),
    ).toBeTruthy();
    const chart = screen.getAllByRole("img")[0];
    expect(chart.querySelectorAll("rect")).toHaveLength(30);
    expect(chart.getAttribute("aria-label")).toMatch(/The most was 7 reviews/);
    expect(argsOf("get_statistics")).toEqual([{ deckId: null }]);
  });

  it("shows retention as a share with the count it is based on", async () => {
    openStats(() => stats({ retention: { reviews: 40, recalled: 35 } }));

    expect(await screen.findByText("88%")).toBeTruthy();
    expect(
      screen.getByText(/35 of 40 reviews of cards in the Review state in the last 30 days/),
    ).toBeTruthy();
  });

  it("shows no share when there is nothing to measure", async () => {
    openStats(() => stats());

    expect(await screen.findByText(/Nothing to measure yet/)).toBeTruthy();
    expect(screen.queryByText(/%$/)).toBeNull();
    expect(screen.getByText("No reviews in the last 30 days.")).toBeTruthy();
    expect(screen.getByText("No cards to count.")).toBeTruthy();
  });

  it("counts cards by state and what falls due", async () => {
    const forecast = days("2026-10-19", 14);
    forecast[0].count = 4;
    forecast[3].count = 2;
    openStats(() =>
      stats({ cardStates: { new: 3, learning: 1, review: 8, relearning: 2 }, forecast }),
    );

    const states = await screen.findByRole("region", { name: "Cards by state" });
    const term = (label: string) =>
      within(states).getByText(label).closest(".stats-state")?.querySelector("dd")?.textContent;
    expect(term("New")).toBe("3");
    expect(term("Learning")).toBe("1");
    expect(term("Review")).toBe("8");
    expect(term("Relearning")).toBe("2");

    const due = screen.getByRole("region", { name: "Due in the next 14 days" });
    expect(within(due).getByText(/^6 cards due\./)).toBeTruthy();
    expect(within(due).getAllByText("Today")).toHaveLength(1);
  });

  it("limits the numbers to the chosen deck, and back to all", async () => {
    const user = userEvent.setup();
    const { argsOf } = openStats(({ deckId }) =>
      deckId === 2 ? stats({ retention: { reviews: 4, recalled: 1 } }) : stats(),
    );
    const filter = await screen.findByRole("combobox", { name: "Deck" });
    await screen.findByText(/Nothing to measure yet/);

    await user.selectOptions(filter, "History");
    expect(await screen.findByText("25%")).toBeTruthy();

    await user.selectOptions(filter, "All decks");
    expect(await screen.findByText(/Nothing to measure yet/)).toBeTruthy();
    expect(argsOf("get_statistics")).toEqual([{ deckId: null }, { deckId: 2 }, { deckId: null }]);
  });

  it("goes back to all decks, and says why, when the chosen deck was archived", async () => {
    const user = userEvent.setup();
    let archived = false;
    const { argsOf } = openStats(
      ({ deckId }) => (deckId === 2 ? refuse("stale", "This deck is archived.") : stats()),
      () => (archived ? [decks[0]] : decks),
    );
    const filter = await screen.findByRole("combobox", { name: "Deck" });

    archived = true;
    await user.selectOptions(filter, "History");

    expect(await screen.findByText(/That deck was archived/)).toBeTruthy();
    expect(await screen.findByText(/Nothing to measure yet/)).toBeTruthy();
    expect((filter as HTMLSelectElement).value).toBe("");
    expect(within(filter).queryByRole("option", { name: "History" })).toBeNull();
    expect(argsOf("get_statistics")).toEqual([{ deckId: null }, { deckId: 2 }, { deckId: null }]);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says when the numbers can't be read, and tries again", async () => {
    const user = userEvent.setup();
    let fail = true;
    openStats(() => (fail ? refuse("failed", "Synapse couldn't load your statistics.") : stats()));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Synapse couldn't load your statistics.",
    );
    fail = false;
    await user.click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByText(/Nothing to measure yet/)).toBeTruthy();
  });

  it("still shows every deck's numbers when the deck list can't be read", async () => {
    openStats(
      () => stats(),
      () => refuse("failed", "Synapse couldn't load your decks."),
    );

    expect(await screen.findByText(/Nothing to measure yet/)).toBeTruthy();
    expect(screen.queryByRole("combobox", { name: "Deck" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows loading until the numbers arrive", async () => {
    const pending = deferred<Statistics>();
    openStats(() => pending.promise);

    expect(screen.getByRole("status").textContent).toBe("Loading your statistics…");
    pending.resolve(stats());
    expect(await screen.findByText(/Nothing to measure yet/)).toBeTruthy();
  });
});
