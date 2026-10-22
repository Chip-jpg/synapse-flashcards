import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DeckDetail as Deck } from "./api";
import { DeckDetail } from "./DeckDetail";
import { taggedCards, tagCounts } from "./CardBulkActions";
import { cardTagHandlers, CREATED, fakeBackend, refuse, type Handler } from "./test/backend";

function taggedDeck(): Deck {
  return {
    id: 1,
    name: "Biology",
    description: null,
    cardCount: 3,
    dueCount: 0,
    cards: [
      { id: 10, front: "What is a cell?", back: "The basic unit of life.", tags: ["cells", "exam"] },
      { id: 11, front: "What does DNA stand for?", back: "Deoxyribonucleic acid.", tags: ["dna"] },
      { id: 12, front: "Where is DNA kept?", back: "In the nucleus.", tags: [] },
    ],
    deletedCards: [
      { id: 13, front: "Old card", back: "Old answer", deletedAt: CREATED, tags: ["archive"] },
    ],
  };
}

function openDeck(handlers: Record<string, Handler> = {}) {
  const deck = taggedDeck();
  const backend = fakeBackend({
    get_deck_detail: () => deck,
    create_flashcard: (args) => {
      const { front, back, tags } = args as { front: string; back: string; tags?: string[] };
      deck.cards = [...deck.cards, { id: 20, front, back, tags: tags ?? [] }];
      deck.cardCount = deck.cards.length;
      return deck;
    },
    ...cardTagHandlers(() => deck),
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

/** The list item of the card whose front is `front`. */
async function cardItem(front: string) {
  const text = await screen.findByText(front);
  return text.closest(".card-item") as HTMLElement;
}

describe("a deck's tags", () => {
  it("are listed once each, counting the cards a filter for each shows", () => {
    const cards = [
      { id: 1, front: "a", back: "a", tags: ["bio", "Exam"] },
      { id: 2, front: "b", back: "b", tags: ["bio::cells", "exam"] },
      { id: 3, front: "c", back: "c" },
    ];
    expect(tagCounts(cards)).toEqual([
      { name: "bio", cards: 2 },
      { name: "bio::cells", cards: 1 },
      { name: "Exam", cards: 2 },
    ]);
    expect(taggedCards(cards, "BIO").map((card) => card.id)).toEqual([1, 2]);
    expect(taggedCards(cards, "")).toHaveLength(3);
  });
});

describe("tags in the deck's card list", () => {
  it("shows each card's tags, deleted cards' included", async () => {
    openDeck();
    const cell = await cardItem("What is a cell?");
    const tags = within(cell).getByRole("list", { name: "Tags" });
    expect(within(tags).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "cells",
      "exam",
    ]);
    const untagged = await cardItem("Where is DNA kept?");
    expect(within(untagged).queryByRole("list", { name: "Tags" })).toBeNull();

    const deleted = screen.getByText("Old card").closest(".card-item") as HTMLElement;
    expect(within(deleted).getByRole("list", { name: "Tags" }).textContent).toBe("archive");
  });
});

describe("the tag field on the card form", () => {
  it("makes chips of typed words and saves them, the word still being typed included", async () => {
    const backend = openDeck();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.type(screen.getByLabelText("Front"), "What is mitosis?");
    await user.type(screen.getByLabelText("Back"), "Cell division.");
    const tags = screen.getByLabelText("Tags");
    await user.type(tags, "exam bio,Exam cells");

    const chosen = screen.getByRole("list", { name: "Tags added" });
    expect(within(chosen).getAllByRole("listitem").map((item) => item.textContent?.replace("×", ""))).toEqual([
      "exam",
      "bio",
    ]);
    expect((tags as HTMLInputElement).value).toBe("cells");

    // Enter makes a chip of the word rather than saving.
    await user.keyboard("{Enter}");
    expect(backend.argsOf("create_flashcard")).toEqual([]);
    expect(within(chosen).getAllByRole("listitem")).toHaveLength(3);
    // Backspace in the empty box takes the last chip off; its button takes any.
    await user.keyboard("{Backspace}");
    await user.click(screen.getByRole("button", { name: "Remove tag bio" }));
    await user.type(tags, "june");

    await user.click(screen.getByRole("button", { name: "Save card" }));
    await screen.findByText("Card added.");
    expect(backend.argsOf("create_flashcard")[0].tags).toEqual(["exam", "june"]);
  });

  it("suggests the tags already in use", async () => {
    openDeck();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    const tags = screen.getByLabelText("Tags");
    const offered = async () =>
      within(await screen.findByRole("list", { name: "Tags in use" }))
        .getAllByRole("button")
        .map((button) => button.textContent);
    // Tags in use containing what's typed are offered.
    await user.type(tags, "e");
    expect(await offered()).toEqual(["cells", "exam"]);
    // A tag the card has isn't suggested again.
    await user.clear(tags);
    await user.type(tags, "exam e");
    expect(await offered()).toEqual(["cells"]);
  });

  it("sends no tags for a card with none, and keeps them on an edit that leaves them", async () => {
    const backend = openDeck();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.type(screen.getByLabelText("Front"), "Q");
    await user.type(screen.getByLabelText("Back"), "A");
    await user.click(screen.getByRole("button", { name: "Save card" }));
    await screen.findByText("Card added.");
    expect(backend.argsOf("create_flashcard")[0].tags).toBeUndefined();

    const cell = await cardItem("What is a cell?");
    await user.click(within(cell).getByRole("button", { name: "Edit card" }));
    await user.type(screen.getByLabelText("Back"), " Really.");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText("Card updated.");
    expect(backend.argsOf("update_flashcard")[0].tags).toBeUndefined();

    const again = await cardItem("What is a cell?");
    await user.click(within(again).getByRole("button", { name: "Edit card" }));
    await user.click(screen.getByRole("button", { name: "Remove tag exam" }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText("Card updated.");
    expect(backend.argsOf("update_flashcard")[1].tags).toEqual(["cells"]);
    const saved = await cardItem("What is a cell?");
    expect(within(saved).getByRole("list", { name: "Tags" }).textContent).toBe("cells");
  });

  it("shows a refused tag under the field, which takes focus", async () => {
    openDeck({
      create_flashcard: () => refuse("invalid", "A tag can have at most 60 characters.", "tags"),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.type(screen.getByLabelText("Front"), "Q");
    await user.type(screen.getByLabelText("Back"), "A");
    await user.type(screen.getByLabelText("Tags"), "x".repeat(61));
    await user.click(screen.getByRole("button", { name: "Save card" }));

    expect(await screen.findByText("A tag can have at most 60 characters.")).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Tags")));
    expect(screen.getByLabelText("Tags").getAttribute("aria-invalid")).toBe("true");
  });

  it("gives a cloze text's tags to its cards", async () => {
    const backend = openDeck({
      preview_cloze: () => ({ cards: 1, kept: 0, restored: 0, added: 1, deleted: 0 }),
      create_cloze: () => ({ ...taggedDeck(), cardCount: 4 }),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.click(screen.getByRole("radio", { name: /Cloze/ }));
    await user.type(screen.getByLabelText("Text"), "{{{{c1::Mitochondria}} make ATP.");
    expect(screen.getByText(/Given to every card of this text/)).toBeTruthy();
    await user.type(screen.getByLabelText("Tags"), "energy");
    await user.click(screen.getByRole("button", { name: "Save cards" }));

    await screen.findByText("Cloze text added: it made 1 card.");
    expect(backend.argsOf("create_cloze")).toEqual([
      { deckId: 1, text: "{{c1::Mitochondria}} make ATP.", tags: ["energy"] },
    ]);
  });
});

describe("filtering a deck's cards by tag", () => {
  it("shows only the cards with the tag chosen, and Select all selects those", async () => {
    openDeck();
    const user = userEvent.setup();

    const filter = await screen.findByLabelText("Show cards tagged");
    expect(
      within(filter)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["Any tag or none", "cells (1 card)", "dna (1 card)", "exam (1 card)"]);

    await user.selectOptions(filter, "dna");
    expect(screen.queryByText("What is a cell?")).toBeNull();
    expect(screen.getByText("What does DNA stand for?")).toBeTruthy();
    await user.click(screen.getByRole("checkbox", { name: "Select all 1 card tagged dna" }));
    const bar = screen.getByRole("region", { name: "Selected cards" });
    expect(within(bar).getByText("1 card selected.")).toBeTruthy();

    // A search narrows within the tag.
    await user.type(screen.getByLabelText("Search cards"), "cell");
    expect(
      screen.getByText("No cards tagged dna match “cell”. Try other words, or another tag."),
    ).toBeTruthy();

    await user.clear(screen.getByLabelText("Search cards"));
    await user.selectOptions(filter, "");
    expect(screen.getByText("What is a cell?")).toBeTruthy();
  });

  it("isn't offered while no card has a tag", async () => {
    const deck = taggedDeck();
    deck.cards = deck.cards.map((card) => ({ ...card, tags: [] }));
    openDeck({ get_deck_detail: () => deck });
    await screen.findByText("What is a cell?");
    expect(screen.queryByLabelText("Show cards tagged")).toBeNull();
  });
});

describe("tagging the selected cards", () => {
  async function selectTwo(user: ReturnType<typeof userEvent.setup>) {
    for (const front of ["What is a cell?", "Where is DNA kept?"]) {
      await user.click(within(await cardItem(front)).getByRole("checkbox", { name: "Select" }));
    }
    await user.click(screen.getByRole("button", { name: "Tag selected" }));
  }

  it("adds tags to them, keeping the ones they have", async () => {
    const backend = openDeck();
    const user = userEvent.setup();
    await selectTwo(user);

    expect(document.activeElement?.textContent).toMatch(/^Change the tags of these 2 cards\?/);
    await user.type(screen.getByLabelText("Add tags"), "exam review");
    await user.click(screen.getByRole("button", { name: "Save tags on 2 cards" }));

    const notice = await screen.findByText(
      "Tags changed on 2 cards. Nothing else about them changed.",
    );
    await waitFor(() => expect(document.activeElement).toBe(notice));
    expect(backend.argsOf("tag_flashcards")).toEqual([
      { deckId: 1, cardIds: [10, 12], add: ["exam", "review"], remove: [] },
    ]);
    const cell = await cardItem("What is a cell?");
    expect(within(cell).getByRole("list", { name: "Tags" }).textContent).toBe("cellsexamreview");
  });

  it("takes a tag off them, offering only the tags they have", async () => {
    const backend = openDeck();
    const user = userEvent.setup();
    await selectTwo(user);

    const remove = screen.getByLabelText("Take off a tag");
    expect(
      within(remove)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["Don't take one off", "cells (1 card)", "exam (1 card)"]);
    await user.selectOptions(remove, "exam");
    await user.click(screen.getByRole("button", { name: "Save tags on 2 cards" }));

    await screen.findByText("Tags changed on 1 card. Nothing else about it changed.");
    expect(backend.argsOf("tag_flashcards")).toEqual([
      { deckId: 1, cardIds: [10, 12], add: [], remove: ["exam"] },
    ]);
  });

  it("asks for a tag before sending anything, and Cancel goes back", async () => {
    const backend = openDeck();
    const user = userEvent.setup();
    await selectTwo(user);

    await user.click(screen.getByRole("button", { name: "Save tags on 2 cards" }));
    expect(screen.getByText("Type a tag to add, or choose one to take off.")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Add tags"));
    expect(backend.argsOf("tag_flashcards")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Tag selected" }));
  });

  it("shows a refused name under the field", async () => {
    openDeck({
      tag_flashcards: () => refuse("invalid", "A tag can have at most 60 characters.", "tags"),
    });
    const user = userEvent.setup();
    await selectTwo(user);

    await user.type(screen.getByLabelText("Add tags"), "x".repeat(61));
    await user.click(screen.getByRole("button", { name: "Save tags on 2 cards" }));
    expect(await screen.findByText("A tag can have at most 60 characters.")).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Add tags")));
  });

  it("reloads and explains when the selection is out of date", async () => {
    const backend = openDeck({
      tag_flashcards: () => refuse("stale", "Some of the selected cards have changed."),
    });
    const user = userEvent.setup();
    await selectTwo(user);

    await user.type(screen.getByLabelText("Add tags"), "exam");
    await user.click(screen.getByRole("button", { name: "Save tags on 2 cards" }));
    const notice = await screen.findByText(
      "Some of the selected cards have changed. Nothing was changed. Here's this deck as it is now.",
    );
    expect(document.activeElement).toBe(notice);
    expect(backend.argsOf("get_deck_detail")).toHaveLength(2);
  });

  it("says when the cards hidden by the tag filter are selected too", async () => {
    openDeck();
    const user = userEvent.setup();
    await user.click(within(await cardItem("What is a cell?")).getByRole("checkbox", { name: "Select" }));
    await user.selectOptions(screen.getByLabelText("Show cards tagged"), "dna");
    const bar = screen.getByRole("region", { name: "Selected cards" });
    expect(within(bar).getByText(/It's hidden by the tag filter\./)).toBeTruthy();
  });
});

describe("tags on a picture's cards", () => {
  it("opens with the card's tags and sends them for every card only when changed", async () => {
    const pictureCards = [1, 2].map((number) => ({
      id: 30 + number,
      front: `The heart\n[picture: box ${number}]`,
      back: number === 1 ? "Aorta" : "Vena cava",
      occlusion: { occlusionId: 5, number },
      tags: ["heart"],
    }));
    const deck: Deck = { ...taggedDeck(), cards: pictureCards, cardCount: 2, deletedCards: [] };
    const backend = openDeck({
      get_deck_detail: () => deck,
      get_occlusion: () => ({
        id: 5,
        picture: { id: 3, sha256: "ab".repeat(32), mediaType: "image/png", size: 1, createdAt: CREATED },
        header: "The heart",
        mode: "hideAll",
        boxes: [
          { number: 1, left: 0.1, top: 0.1, width: 0.2, height: 0.1, label: "Aorta" },
          { number: 2, left: 0.5, top: 0.5, width: 0.2, height: 0.1, label: "Vena cava" },
        ],
        nextNumber: 3,
      }),
      update_occlusion: () => ({
        changes: { cards: 2, kept: 2, restored: 0, added: 0, deleted: 0 },
        deck,
      }),
    });
    const user = userEvent.setup();

    const row = await cardItem("Vena cava");
    await user.click(within(row).getByRole("button", { name: "Edit card" }));
    await screen.findByDisplayValue("Vena cava");
    expect(screen.getByText(/Given to every card of this picture/)).toBeTruthy();
    const chosen = screen.getByRole("list", { name: "Tags added" });
    expect(within(chosen).getByRole("button", { name: "Remove tag heart" })).toBeTruthy();

    await user.type(screen.getByLabelText("Tags"), "anatomy");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText(/^Picture saved/);
    expect(backend.argsOf("update_occlusion")[0].tags).toEqual(["heart", "anatomy"]);
  });
});
