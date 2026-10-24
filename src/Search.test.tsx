import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Deck, SearchHit, SearchResults } from "./api";
import App from "./App";
import { fakeBackend, refuse, type Handler } from "./test/backend";

/** The last element of `list`. */
function last<T>(list: T[]): T {
  return list[list.length - 1];
}

const cardHit: SearchHit = {
  kind: "card",
  cardId: 10,
  deckId: 1,
  deckName: "Biology",
  front: [
    { text: "What is a ", hit: false },
    { text: "cell", hit: true },
    { text: "?", hit: false },
  ],
  back: [{ text: "The basic unit of life.", hit: false }],
};

const noteHit: SearchHit = {
  kind: "note",
  noteId: 5,
  title: [{ text: "Mitosis", hit: false }],
  body: [
    { text: "… a ", hit: false },
    { text: "cell", hit: true },
    { text: " divides <b>in two</b> …", hit: false },
  ],
};

const decks: Deck[] = [
  { id: 1, name: "Biology", description: null, isSample: false, dueCount: 0 },
  { id: 2, name: "History", description: null, isSample: false, dueCount: 0 },
];

function found(results: SearchHit[], more = false): SearchResults {
  return { results, more };
}

/** Opens Search from the sidebar, with `search` answering `search_everything`. */
async function openSearch(search: Handler, overrides: Record<string, Handler> = {}) {
  const user = userEvent.setup();
  const backend = fakeBackend({
    search_everything: search,
    get_decks: () => decks,
    ...overrides,
  });
  render(<App />);
  const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));
  await user.click(sidebar.getByRole("button", { name: "Search" }));
  await screen.findByRole("heading", { name: "Search", level: 2 });
  return { user, sidebar, ...backend };
}

describe("the Search screen", () => {
  it("is in the sidebar, and its field takes focus", async () => {
    const { sidebar } = await openSearch(() => found([]));

    expect(sidebar.getByRole("button", { name: "Search" }).getAttribute("aria-current")).toBe(
      "page",
    );
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Search for")));
  });

  it("searches cards and notes as you type and marks the matched words as text", async () => {
    const { user, argsOf } = await openSearch(() => found([cardHit, noteHit]));

    await user.type(screen.getByLabelText("Search for"), "cel");

    expect(await screen.findByText("2 results.")).toBeTruthy();
    expect(last(argsOf("search_everything"))).toEqual({
      query: "cel",
      scope: "all",
      deckId: null,
      tag: null,
    });
    const results = within(screen.getByRole("list", { name: "Results" })).getAllByRole("listitem");
    expect(results).toHaveLength(2);

    const card = within(results[0]);
    expect(card.getByText("Card in Biology")).toBeTruthy();
    expect(card.getByRole("heading", { level: 3 }).textContent).toBe("What is a cell?");
    expect(results[0].querySelector("mark")?.textContent).toBe("cell");
    expect(card.getByRole("button", { name: "Open in deck" })).toBeTruthy();

    // Markup in stored text is shown as typed, never parsed.
    const note = within(results[1]);
    expect(note.getByText("Note")).toBeTruthy();
    expect(note.getByText(/divides <b>in two<\/b>/)).toBeTruthy();
    expect(results[1].querySelector("b")).toBeNull();
    expect(note.getByRole("button", { name: "Open note" })).toBeTruthy();
  });

  it("says when nothing matches, and when only the best results are shown", async () => {
    let more = false;
    const { user } = await openSearch((args) =>
      args.query === "zzz" ? found([]) : found([cardHit], more),
    );

    await user.type(screen.getByLabelText("Search for"), "zzz");
    expect(await screen.findByText("Nothing matches “zzz”.")).toBeTruthy();

    more = true;
    await user.clear(screen.getByLabelText("Search for"));
    await user.type(screen.getByLabelText("Search for"), "cell");
    expect(
      await screen.findByText("The best 1 results. Add a word to narrow them down."),
    ).toBeTruthy();
  });

  it("searches only cards, in one deck, or only notes", async () => {
    const { user, argsOf } = await openSearch(() => found([cardHit]));
    await user.type(screen.getByLabelText("Search for"), "cell");
    await screen.findByText("1 result.");
    expect(screen.queryByLabelText("Deck")).toBeNull();

    await user.selectOptions(screen.getByLabelText("Show"), "Cards");
    await user.selectOptions(await screen.findByLabelText("Deck"), "History");
    await waitFor(() =>
      expect(last(argsOf("search_everything"))).toEqual({
        query: "cell",
        scope: "cards",
        deckId: 2,
        tag: null,
      }),
    );

    // Notes have no deck, so the deck is dropped.
    await user.selectOptions(screen.getByLabelText("Show"), "Notes");
    expect(screen.queryByLabelText("Deck")).toBeNull();
    await waitFor(() =>
      expect(last(argsOf("search_everything"))).toEqual({
        query: "cell",
        scope: "notes",
        deckId: null,
        tag: null,
      }),
    );
  });

  it("searches every deck again when the chosen one was archived", async () => {
    const { user, argsOf } = await openSearch((args) =>
      args.deckId === 2 ? refuse("stale", "This deck has been archived.") : found([cardHit]),
    );
    await user.type(screen.getByLabelText("Search for"), "cell");
    await user.selectOptions(screen.getByLabelText("Show"), "Cards");
    await user.selectOptions(await screen.findByLabelText("Deck"), "History");

    expect(
      await screen.findByText("That deck was archived, so every deck is searched now."),
    ).toBeTruthy();
    expect(await screen.findByText("1 result.")).toBeTruthy();
    expect(last(argsOf("search_everything"))).toMatchObject({ deckId: null });
    expect(screen.queryByRole("option", { name: "History" })).toBeNull();
  });

  it("shows a refusal, and Escape clears the search", async () => {
    const { user } = await openSearch(() => refuse("failed", "Synapse couldn't search."));
    const field = screen.getByLabelText("Search for");
    await user.type(field, "cell");

    expect((await screen.findByRole("alert")).textContent).toBe("Synapse couldn't search.");
    await user.keyboard("{Escape}");
    expect((field as HTMLInputElement).value).toBe("");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("opens a card in its deck, and Back returns to the same search", async () => {
    const { user, sidebar, argsOf } = await openSearch(() => found([cardHit, noteHit]));
    await user.type(screen.getByLabelText("Search for"), "cell");
    const results = await screen.findByRole("list", { name: "Results" });

    await user.click(within(results).getByRole("button", { name: "Open in deck" }));

    expect(await screen.findByRole("heading", { name: "Biology", level: 2 })).toBeTruthy();
    expect(document.querySelector(".card-found")?.textContent).toContain("What is a cell?");
    // The card found has focus, and the sidebar still says Search.
    await waitFor(() =>
      expect(document.activeElement?.getAttribute("aria-describedby")).toBe("card-10-front"),
    );
    expect(sidebar.getByRole("button", { name: "Search" }).getAttribute("aria-current")).toBe(
      "true",
    );

    await user.click(screen.getByRole("button", { name: "Back to search" }));

    expect((await screen.findByLabelText("Search for")) as HTMLInputElement).toHaveProperty(
      "value",
      "cell",
    );
    await screen.findByText("2 results.");
    await waitFor(() => expect(document.activeElement?.textContent).toBe("Open in deck"));
    expect(last(argsOf("search_everything"))).toEqual({
      query: "cell",
      scope: "all",
      deckId: null,
      tag: null,
    });
  });

  it("opens a note, and Back returns to the search", async () => {
    const { user } = await openSearch(() => found([cardHit, noteHit]));
    await user.type(screen.getByLabelText("Search for"), "cell");
    const results = await screen.findByRole("list", { name: "Results" });

    await user.click(within(results).getByRole("button", { name: "Open note" }));

    expect(await screen.findByRole("heading", { name: "Mitosis", level: 2 })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Back to search" }));

    await screen.findByText("2 results.");
    await waitFor(() => expect(document.activeElement?.textContent).toBe("Open note"));
  });

  it("focuses the field on the way back if the result is gone", async () => {
    let hits = [cardHit, noteHit];
    const { user } = await openSearch(() => found(hits));
    await user.type(screen.getByLabelText("Search for"), "cell");
    const results = await screen.findByRole("list", { name: "Results" });
    await user.click(within(results).getByRole("button", { name: "Open note" }));
    await screen.findByRole("heading", { name: "Mitosis", level: 2 });

    hits = [cardHit];
    await user.click(screen.getByRole("button", { name: "Back to search" }));

    await screen.findByText("1 result.");
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Search for")));
  });
});
