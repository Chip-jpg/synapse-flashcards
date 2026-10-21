import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DeckDetail as Deck } from "./api";
import { DeckDetail } from "./DeckDetail";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const CARD = { id: 10, front: "What is a cell?", back: "The basic unit of life." };

/** The deck with its one card active. */
function activeDeck(): Deck {
  return {
    id: 1,
    name: "Biology",
    description: null,
    cardCount: 1,
    dueCount: 0,
    cards: [CARD],
    deletedCards: [],
  };
}

/** The deck with its one card deleted. */
function deckWithDeletedCard(): Deck {
  return {
    ...activeDeck(),
    cardCount: 0,
    cards: [],
    deletedCards: [{ ...CARD, deletedAt: CREATED }],
  };
}

function openDeck(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend(handlers);
  const onArchived = vi.fn();
  render(
    <DeckDetail
      deckId={1}
      justCreated={false}
      onBack={vi.fn()}
      onStart={vi.fn()}
      onArchived={onArchived}
    />,
  );
  return { ...backend, onArchived };
}

describe("deleting a card", () => {
  it("asks first, with focus on the question, and Keep card puts focus back", async () => {
    const deck = openDeck();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Delete card" }));

    expect(document.activeElement).toBe(screen.getByText(/^Delete this card\?/));
    expect(deck.argsOf("delete_flashcard")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Keep card" }));

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Delete card" }));
    expect(deck.argsOf("delete_flashcard")).toEqual([]);
  });

  it("moves the card under Deleted cards, and says so", async () => {
    const deck = openDeck({ delete_flashcard: () => deckWithDeletedCard() });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Delete card" }));
    await user.click(screen.getByRole("button", { name: "Delete card" }));

    const notice = await screen.findByText(
      "Card deleted. It's kept under Deleted cards, where you can restore it.",
    );
    expect(document.activeElement).toBe(notice);
    expect(deck.argsOf("delete_flashcard")).toEqual([{ cardId: 10 }]);
    expect(screen.getByRole("button", { name: "Restore card" })).toBeTruthy();
  });

  it("sends one delete, however often it's confirmed while deleting", async () => {
    const deleting = deferred<Deck>();
    const deck = openDeck({ delete_flashcard: () => deleting.promise });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Delete card" }));
    const confirm = screen.getByRole("button", { name: "Delete card" });
    await user.click(confirm);
    await user.click(confirm);

    expect(screen.getByText("Deleting…")).toBeTruthy();
    expect(deck.argsOf("delete_flashcard")).toHaveLength(1);

    await act(async () => deleting.resolve(deckWithDeletedCard()));
    expect(await screen.findByText(/^Card deleted\./)).toBeTruthy();
  });

  it("reloads the deck when the card was already deleted", async () => {
    const deck = openDeck({
      delete_flashcard: () => refuse("stale", "That card was already deleted."),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Delete card" }));
    await user.click(screen.getByRole("button", { name: "Delete card" }));

    await vi.waitFor(() => expect(deck.argsOf("get_deck_detail")).toHaveLength(2));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("restoring a card", () => {
  it("asks first, with focus on the question, then restores it and says so", async () => {
    const deck = openDeck({
      get_deck_detail: () => deckWithDeletedCard(),
      restore_flashcard: () => activeDeck(),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore card" }));
    expect(document.activeElement).toBe(screen.getByText(/^Restore this card\?/));
    expect(deck.argsOf("restore_flashcard")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Restore card" }));

    const notice = await screen.findByText(
      "Card restored. It's back in this deck, with its schedule unchanged.",
    );
    expect(document.activeElement).toBe(notice);
    expect(deck.argsOf("restore_flashcard")).toEqual([{ cardId: 10 }]);
    expect(screen.queryByRole("button", { name: "Restore card" })).toBeNull();
  });

  it("puts focus back on Restore card when the card is kept deleted", async () => {
    openDeck({ get_deck_detail: () => deckWithDeletedCard() });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore card" }));
    await user.click(screen.getByRole("button", { name: "Keep deleted" }));

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Restore card" }));
  });

  it("reloads the deck and says why when the restore is out of date", async () => {
    // Shown as deleted, but restored meanwhile: Rust refuses it as stale.
    let restored = false;
    const deck = openDeck({
      get_deck_detail: () => (restored ? activeDeck() : deckWithDeletedCard()),
      restore_flashcard: () => {
        restored = true;
        return refuse("stale", "That card isn't deleted.");
      },
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore card" }));
    await user.click(screen.getByRole("button", { name: "Restore card" }));

    const notice = await screen.findByText(
      "That card couldn't be restored. Here's this deck as it is now.",
    );
    // Focus moves once the reloaded deck has rendered.
    await waitFor(() => expect(document.activeElement).toBe(notice));
    expect(deck.argsOf("get_deck_detail")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Restore card" })).toBeNull();
  });
});

/** The deck with its one card active and three more deleted. */
function deckWithDeletedCards(count = 3): Deck {
  return {
    ...activeDeck(),
    deletedCards: Array.from({ length: count }, (_, n) => ({
      id: 30 + n,
      front: `Deleted ${n}`,
      back: `Answer ${n}`,
      deletedAt: CREATED,
    })),
  };
}

/** The deck after all its deleted cards came back. */
function deckWithAllRestored(): Deck {
  const deck = deckWithDeletedCards();
  const back = deck.deletedCards.map(({ id, front, back }) => ({ id, front, back }));
  return { ...deck, cardCount: 4, cards: [...deck.cards, ...back], deletedCards: [] };
}

describe("restoring all deleted cards", () => {
  it("isn't offered for a single deleted card", async () => {
    openDeck({ get_deck_detail: () => deckWithDeletedCard() });

    expect(await screen.findByRole("button", { name: "Restore card" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Restore all" })).toBeNull();
  });

  it("asks first, naming how many, and Keep deleted puts focus back", async () => {
    const deck = openDeck({ get_deck_detail: () => deckWithDeletedCards() });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore all" }));

    expect(document.activeElement).toBe(screen.getByText(/^Restore all 3 deleted cards\?/));
    expect(screen.getByRole("button", { name: "Restore 3 cards" })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Keep deleted" }));

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Restore all" }));
    expect(deck.argsOf("restore_deleted_cards")).toEqual([]);
  });

  it("restores them all with one request, and shows the deck with them back", async () => {
    const restoring = deferred<{ restored: number; deck: Deck }>();
    const deck = openDeck({
      get_deck_detail: () => deckWithDeletedCards(),
      restore_deleted_cards: () => restoring.promise,
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore all" }));
    const confirm = screen.getByRole("button", { name: "Restore 3 cards" });
    await user.click(confirm);
    await user.click(confirm);

    expect(screen.getByText("Restoring…")).toBeTruthy();
    expect(deck.argsOf("restore_deleted_cards")).toEqual([{ deckId: 1, expected: 3 }]);

    await act(async () => restoring.resolve({ restored: 3, deck: deckWithAllRestored() }));

    const notice = await screen.findByText(
      "3 cards restored. They're back in this deck, with their schedules unchanged.",
    );
    expect(document.activeElement).toBe(notice);
    expect(screen.getByText("4 cards in total")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Restore all" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Restore card" })).toBeNull();
    expect(deck.argsOf("get_deck_detail")).toHaveLength(1);
  });

  it("reloads the deck and says why when the list is out of date", async () => {
    let refused = false;
    const deck = openDeck({
      get_deck_detail: () => (refused ? deckWithDeletedCard() : deckWithDeletedCards()),
      restore_deleted_cards: () => {
        refused = true;
        return refuse("stale", "This deck's deleted cards have changed.");
      },
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore all" }));
    await user.click(screen.getByRole("button", { name: "Restore 3 cards" }));

    const notice = await screen.findByText(
      "The deleted cards couldn't be restored. Here's this deck as it is now.",
    );
    // Focus moves in an effect after the notice renders.
    await vi.waitFor(() => expect(document.activeElement).toBe(notice));
    expect(deck.argsOf("get_deck_detail")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Restore all" })).toBeNull();
  });

  it("shows a failure and lets the user try again", async () => {
    const deck = openDeck({
      get_deck_detail: () => deckWithDeletedCards(),
      restore_deleted_cards: () => refuse("failed", "Synapse couldn't restore the cards."),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore all" }));
    await user.click(screen.getByRole("button", { name: "Restore 3 cards" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Synapse couldn't restore the cards.",
    );
    await user.click(screen.getByRole("button", { name: "Restore 3 cards" }));
    await vi.waitFor(() => expect(deck.argsOf("restore_deleted_cards")).toHaveLength(2));
  });
});

describe("adding a card", () => {
  it("sends one save, however often Save card is pressed while saving", async () => {
    const saving = deferred<Deck>();
    const deck = openDeck({ create_flashcard: () => saving.promise });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.type(screen.getByLabelText("Front"), "What is DNA?");
    await user.type(screen.getByLabelText("Back"), "The molecule that carries genes.");
    const save = screen.getByRole("button", { name: "Save card" });
    await user.click(save);
    await user.click(save);

    expect(screen.getByText("Saving…")).toBeTruthy();
    expect(deck.argsOf("create_flashcard")).toEqual([
      {
        deckId: 1,
        front: "What is DNA?",
        back: "The molecule that carries genes.",
        pictures: { front: [], back: [] },
      },
    ]);

    await act(async () => saving.resolve(activeDeck()));
    const notice = await screen.findByText("Card added.");
    expect(document.activeElement).toBe(notice);
  });

  it("shows a refusal under the field it's about, and moves focus there", async () => {
    openDeck({
      create_flashcard: () =>
        refuse("invalid", "The front can't be longer than 2,000 characters.", "front"),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.type(screen.getByLabelText("Front"), "Too long");
    await user.type(screen.getByLabelText("Back"), "Answer");
    await user.click(screen.getByRole("button", { name: "Save card" }));

    const front = screen.getByLabelText("Front");
    expect(await screen.findByText("The front can't be longer than 2,000 characters.")).toBeTruthy();
    expect(document.activeElement).toBe(front);
    expect(front.getAttribute("aria-invalid")).toBe("true");
    // What was typed is kept.
    expect((front as HTMLTextAreaElement).value).toBe("Too long");

    await user.type(front, "!");

    expect(screen.queryByText("The front can't be longer than 2,000 characters.")).toBeNull();
    expect(front.getAttribute("aria-invalid")).toBeNull();
  });
});

describe("archiving the deck", () => {
  it("asks first, with focus on the question, and Keep deck puts focus back", async () => {
    const deck = openDeck();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Archive deck" }));
    expect(document.activeElement).toBe(screen.getByText(/^Archive this deck\?/));

    await user.click(screen.getByRole("button", { name: "Keep deck" }));

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Archive deck" }));
    expect(deck.argsOf("archive_deck")).toEqual([]);
  });

  it.each([
    ["archived", () => null],
    ["already archived", () => refuse("stale", "That deck is archived.")],
  ])("leaves the screen once the deck is %s", async (_case, archive) => {
    const deck = openDeck({ archive_deck: archive });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Archive deck" }));
    await user.click(screen.getByRole("button", { name: "Archive deck" }));

    await vi.waitFor(() => expect(deck.onArchived).toHaveBeenCalledOnce());
    expect(deck.argsOf("archive_deck")).toEqual([{ deckId: 1 }]);
  });
});
