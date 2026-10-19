import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ChangedCards, Deck as DeckSummary, DeckDetail as Deck } from "./api";
import { matchingCards } from "./CardBulkActions";
import { DeckDetail } from "./DeckDetail";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const CARDS = [
  { id: 10, front: "What is a cell?", back: "The basic unit of life." },
  { id: 11, front: "What does DNA stand for?", back: "Deoxyribonucleic acid." },
  { id: 12, front: "Where is DNA kept?", back: "In the nucleus of the CELL." },
];

function threeCards(): Deck {
  return {
    id: 1,
    name: "Biology",
    description: null,
    cardCount: 3,
    dueCount: 0,
    cards: CARDS,
    deletedCards: [],
  };
}

/** Biology after `ids` left it: deleted (kept under Deleted cards) or moved away. */
function without(ids: number[], deleted: boolean): Deck {
  const deck = threeCards();
  const gone = deck.cards.filter((card) => ids.includes(card.id));
  const cards = deck.cards.filter((card) => !ids.includes(card.id));
  return {
    ...deck,
    cards,
    cardCount: cards.length,
    deletedCards: deleted ? gone.map((card) => ({ ...card, deletedAt: CREATED })) : [],
  };
}

const DECKS: DeckSummary[] = [
  { id: 1, name: "Biology", description: null, isSample: false, dueCount: 0 },
  { id: 2, name: "Chemistry", description: null, isSample: false, dueCount: 0 },
  {
    id: 3,
    name: "Sample deck",
    description: null,
    isSample: true,
    dueCount: 1,
  },
];

function openDeck(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({
    get_deck_detail: () => threeCards(),
    get_decks: () => DECKS,
    ...handlers,
  });
  render(
    <DeckDetail
      deckId={1}
      justCreated={false}
      onBack={vi.fn()}
      onStart={vi.fn()}
      onArchived={vi.fn()}
    />,
  );
  return backend;
}

/** The Select box of the card whose front is `front`. */
async function selectBox(front: string) {
  const text = await screen.findByText(front);
  const item = text.closest(".card-item") as HTMLElement;
  return within(item).getByRole("checkbox", { name: "Select" });
}

describe("matching cards", () => {
  it("finds the words in either side, ignoring capitals and outer spaces", () => {
    expect(matchingCards(CARDS, "  cell ").map((card) => card.id)).toEqual([10, 12]);
    expect(matchingCards(CARDS, "dna").map((card) => card.id)).toEqual([11, 12]);
    expect(matchingCards(CARDS, "   ")).toEqual(CARDS);
    expect(matchingCards(CARDS, "mitochondria")).toEqual([]);
  });
});

describe("searching a deck's cards", () => {
  it("shows only matching cards, says how many, and Escape shows them all again", async () => {
    openDeck();
    const user = userEvent.setup();

    const search = await screen.findByRole("searchbox", {
      name: "Search cards",
    });
    await user.type(search, "dna");

    expect(screen.getByText("2 cards match.")).toBeTruthy();
    expect(screen.queryByText("What is a cell?")).toBeNull();
    expect(screen.getByText("Where is DNA kept?")).toBeTruthy();

    await user.clear(search);
    await user.type(search, "mitochondria");
    expect(screen.getByText(/^No cards in this deck match/)).toBeTruthy();
    expect(screen.queryByRole("checkbox", { name: /Select all/ })).toBeNull();

    await user.keyboard("{Escape}");
    expect(screen.getByText("What is a cell?")).toBeTruthy();
    expect(screen.getAllByRole("checkbox", { name: "Select" })).toHaveLength(3);
  });
});

describe("selecting cards", () => {
  it("selects all the matching cards only, and says when the search hides some", async () => {
    openDeck();
    const user = userEvent.setup();

    await user.type(await screen.findByRole("searchbox", { name: "Search cards" }), "cell");
    await user.click(screen.getByRole("checkbox", { name: "Select all 2 matching cards" }));

    expect(screen.getByText("2 cards selected.")).toBeTruthy();

    await user.clear(screen.getByRole("searchbox", { name: "Search cards" }));
    await user.type(screen.getByRole("searchbox", { name: "Search cards" }), "acid");
    expect(screen.getByText(/They're all hidden by the search\./)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Clear selection" }));
    expect(screen.queryByText(/selected\./)).toBeNull();
  });

  it("shows Select all as partly checked when only some cards are selected", async () => {
    openDeck();
    const user = userEvent.setup();

    await user.click(await selectBox("What is a cell?"));
    const all = screen.getByRole("checkbox", {
      name: "Select all cards",
    }) as HTMLInputElement;
    expect(all.indeterminate).toBe(true);
    expect(all.checked).toBe(false);

    await user.click(all);
    expect(all.checked).toBe(true);
    expect(screen.getByText("3 cards selected.")).toBeTruthy();

    await user.click(all);
    expect(screen.queryByText(/selected\./)).toBeNull();
  });
});

describe("deleting selected cards", () => {
  it("asks first, then deletes them in the deck's order and says so", async () => {
    const backend = openDeck({
      delete_flashcards: (args) => {
        const { cardIds } = args as { cardIds: number[] };
        return {
          changed: cardIds.length,
          deck: without(cardIds, true),
        } satisfies ChangedCards;
      },
    });
    const user = userEvent.setup();

    await user.click(await selectBox("Where is DNA kept?"));
    await user.click(await selectBox("What is a cell?"));
    await user.click(screen.getByRole("button", { name: "Delete selected" }));

    expect(document.activeElement).toBe(screen.getByText(/^Delete these 2 cards\?/));
    expect(backend.argsOf("delete_flashcards")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Keep cards" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Delete selected" }));

    await user.click(screen.getByRole("button", { name: "Delete selected" }));
    await user.click(screen.getByRole("button", { name: "Delete 2 cards" }));

    const notice = await screen.findByText(
      "2 cards deleted. They're kept under Deleted cards, where you can restore them.",
    );
    expect(document.activeElement).toBe(notice);
    expect(backend.argsOf("delete_flashcards")).toEqual([{ deckId: 1, cardIds: [10, 12] }]);
    expect(screen.getAllByRole("button", { name: "Restore card" })).toHaveLength(2);
    // The selection went with them.
    expect(screen.queryByText(/selected\./)).toBeNull();
  });

  it("sends one delete, however often it's confirmed while deleting", async () => {
    const deleting = deferred<ChangedCards>();
    const backend = openDeck({ delete_flashcards: () => deleting.promise });
    const user = userEvent.setup();

    await user.click(await selectBox("What is a cell?"));
    await user.click(screen.getByRole("button", { name: "Delete selected" }));
    await user.click(screen.getByRole("button", { name: "Delete 1 card" }));
    await user.click(screen.getByRole("button", { name: "Delete 1 card" }));

    expect(screen.getByText("Deleting…")).toBeTruthy();
    deleting.resolve({ changed: 1, deck: without([10], true) });
    await screen.findByText(/^1 card deleted\. It's kept/);
    expect(backend.argsOf("delete_flashcards")).toHaveLength(1);
  });

  it("reloads and explains when the selection is out of date", async () => {
    let loads = 0;
    const backend = openDeck({
      get_deck_detail: () => (++loads === 1 ? threeCards() : without([11], true)),
      delete_flashcards: () => refuse("stale", "Some of the selected cards have changed."),
    });
    const user = userEvent.setup();

    await user.click(await selectBox("What does DNA stand for?"));
    await user.click(screen.getByRole("button", { name: "Delete selected" }));
    await user.click(screen.getByRole("button", { name: "Delete 1 card" }));

    const notice = await screen.findByText(
      "Some of the selected cards have changed. Nothing was changed. Here's this deck as it is now.",
    );
    expect(document.activeElement).toBe(notice);
    expect(backend.argsOf("get_deck_detail")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Restore card" })).toBeTruthy();
  });

  it("keeps the question open with the error when deleting fails", async () => {
    openDeck({
      delete_flashcards: () => refuse("failed", "Synapse couldn't delete the cards."),
    });
    const user = userEvent.setup();

    await user.click(await selectBox("What is a cell?"));
    await user.click(screen.getByRole("button", { name: "Delete selected" }));
    await user.click(screen.getByRole("button", { name: "Delete 1 card" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Synapse couldn't delete the cards.",
    );
    expect(screen.getByRole("button", { name: "Delete 1 card" })).toBeTruthy();
  });
});

describe("moving selected cards", () => {
  it("offers only the other normal decks, then moves them and names the deck", async () => {
    const backend = openDeck({
      move_flashcards: (args) => {
        const { cardIds } = args as { cardIds: number[] };
        return {
          changed: cardIds.length,
          deck: without(cardIds, false),
        } satisfies ChangedCards;
      },
    });
    const user = userEvent.setup();

    await user.click(await selectBox("What does DNA stand for?"));
    await user.click(await selectBox("Where is DNA kept?"));
    await user.click(screen.getByRole("button", { name: "Move selected" }));

    expect(document.activeElement).toBe(screen.getByText(/^Move these 2 cards to another deck\?/));
    const target = await screen.findByRole("combobox", { name: "Move to" });
    const options = within(target)
      .getAllByRole("option")
      .map((option) => option.textContent);
    expect(options).toEqual(["Choose a deck", "Chemistry"]);

    await user.selectOptions(target, "2");
    await user.click(screen.getByRole("button", { name: "Move 2 cards" }));

    const notice = await screen.findByText(
      "2 cards moved to Chemistry, with their schedules unchanged.",
    );
    expect(document.activeElement).toBe(notice);
    expect(backend.argsOf("move_flashcards")).toEqual([
      { deckId: 1, cardIds: [11, 12], toDeckId: 2 },
    ]);
    expect(screen.queryByText("Where is DNA kept?")).toBeNull();
  });

  it("asks for a deck before moving, and sends nothing until one is chosen", async () => {
    const backend = openDeck();
    const user = userEvent.setup();

    await user.click(await selectBox("What is a cell?"));
    await user.click(screen.getByRole("button", { name: "Move selected" }));
    await screen.findByRole("combobox", { name: "Move to" });
    await user.click(screen.getByRole("button", { name: "Move 1 card" }));

    expect(screen.getByText("Choose a deck to move the cards to.")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByRole("combobox", { name: "Move to" }));
    expect(backend.argsOf("move_flashcards")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Move selected" }));
  });

  it("says so when there is no other deck to move to", async () => {
    openDeck({ get_decks: () => [DECKS[0], DECKS[2]] });
    const user = userEvent.setup();

    await user.click(await selectBox("What is a cell?"));
    await user.click(screen.getByRole("button", { name: "Move selected" }));

    expect(await screen.findByText(/^There's no other deck to move them to\./)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Move 1 card" })).toBeNull();
  });

  it("reloads and explains when the chosen deck can't take cards any more", async () => {
    const backend = openDeck({
      move_flashcards: () => refuse("stale", "That deck can't take cards any more."),
    });
    const user = userEvent.setup();

    await user.click(await selectBox("What is a cell?"));
    await user.click(screen.getByRole("button", { name: "Move selected" }));
    await user.selectOptions(await screen.findByRole("combobox", { name: "Move to" }), "2");
    await user.click(screen.getByRole("button", { name: "Move 1 card" }));

    expect(
      await screen.findByText(
        "That deck can't take cards any more. Nothing was changed. Here's this deck as it is now.",
      ),
    ).toBeTruthy();
    expect(backend.argsOf("get_deck_detail")).toHaveLength(2);
  });
});
