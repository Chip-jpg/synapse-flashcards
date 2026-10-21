import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ClozeChanges, DeckCard, DeckDetail as Deck } from "./api";
import { DeckDetail } from "./DeckDetail";
import { ReviewSession } from "./ReviewSession";
import { CREATED, fakeBackend, refuse, type Handler } from "./test/backend";

const TEXT = "{{c1::Mitochondria}} make {{c2::ATP::a molecule}}.";

/** The two cards Rust makes from `TEXT`. */
const CLOZE_CARDS: DeckCard[] = [
  {
    id: 21,
    front: "[...] make ATP.",
    back: "Mitochondria",
    cloze: { textId: 4, number: 1, text: TEXT },
  },
  {
    id: 22,
    front: "Mitochondria make [a molecule].",
    back: "ATP",
    cloze: { textId: 4, number: 2, text: TEXT },
  },
];

const BASIC: DeckCard = { id: 10, front: "What is a cell?", back: "The basic unit of life." };

function deck(cards: DeckCard[], deletedCards: Deck["deletedCards"] = []): Deck {
  return {
    id: 1,
    name: "Biology",
    description: null,
    cardCount: cards.length,
    dueCount: 0,
    cards,
    deletedCards,
  };
}

function last<T>(items: T[]): T {
  return items[items.length - 1];
}

function changes(partial: Partial<ClozeChanges>): ClozeChanges {
  return { cards: 0, kept: 0, restored: 0, added: 0, deleted: 0, ...partial };
}

/** Counts distinct `{{cN::` numbers, as a stand-in for Rust's check. */
function numbers(text: string): number {
  return new Set([...text.matchAll(/\{\{c(\d+)::/g)].map((match) => match[1])).size;
}

function openDeck(start: Deck, handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({
    get_deck_detail: () => start,
    preview_cloze: (args) => {
      const count = numbers(args.text as string);
      if (count === 0) {
        return refuse(
          "invalid",
          "Hide at least one part of the text, like {{c1::answer}}.",
          "text",
        );
      }
      return changes({ cards: count, added: count });
    },
    ...handlers,
  });
  render(
    <DeckDetail deckId={1} justCreated={false} onBack={vi.fn()} onStart={vi.fn()} onArchived={vi.fn()} />,
  );
  return backend;
}

describe("adding cloze cards", () => {
  it("offers Basic or Cloze, counts the cards as the text is typed, and saves the text", async () => {
    const backend = openDeck(deck([BASIC]), {
      create_cloze: () => deck([BASIC, ...CLOZE_CARDS]),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    expect(screen.getByRole("radio", { name: /Basic/ })).toHaveProperty("checked", true);
    expect(screen.getByLabelText("Front")).toBeTruthy();

    await user.click(screen.getByRole("radio", { name: /Cloze/ }));
    // Choosing keeps focus on the choice.
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: /Cloze/ }));
    expect(screen.queryByLabelText("Front")).toBeNull();

    const text = screen.getByLabelText("Text");
    await user.click(text);
    await user.type(text, "No gaps yet");
    expect(
      await screen.findByText("Hide at least one part of the text, like {{c1::answer}}."),
    ).toBeTruthy();

    await user.clear(text);
    // `{` starts a key description in user-event; `{{` types one brace.
    await user.type(text, TEXT.replace(/[{[]/g, "$&$&"));
    expect(await screen.findByText("This text makes 2 cards.")).toBeTruthy();
    expect(last(backend.argsOf("preview_cloze"))).toEqual({ text: TEXT, cardId: null });

    await user.click(screen.getByRole("button", { name: "Save cards" }));

    const notice = await screen.findByText("Cloze text added: it made 2 cards.");
    await waitFor(() => expect(document.activeElement).toBe(notice));
    expect(backend.argsOf("create_cloze")).toEqual([{ deckId: 1, text: TEXT }]);
    expect(backend.argsOf("create_flashcard")).toEqual([]);
    expect(screen.getAllByText(/^Cloze \d$/).map((tag) => tag.textContent)).toEqual([
      "Cloze 1",
      "Cloze 2",
    ]);
  });

  it("puts a refused text's error under the text, which takes focus", async () => {
    openDeck(deck([BASIC]), {
      create_cloze: () =>
        refuse("invalid", "Close each {{c1::…}} with }} before the next one starts.", "text"),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.click(screen.getByRole("radio", { name: /Cloze/ }));
    await user.type(screen.getByLabelText("Text"), "{{{{c1::open");
    await user.click(screen.getByRole("button", { name: "Save cards" }));

    const text = screen.getByLabelText("Text");
    await waitFor(() => expect(document.activeElement).toBe(text));
    expect(text.getAttribute("aria-invalid")).toBe("true");
  });
});

describe("editing a cloze text", () => {
  it("opens the card's text, says what saving will do, and reports what it did", async () => {
    const edited = "{{c1::Mitochondria}} make energy.";
    const backend = openDeck(deck(CLOZE_CARDS), {
      preview_cloze: () => changes({ cards: 1, kept: 1, deleted: 1 }),
      update_cloze: () => ({
        changes: changes({ cards: 1, kept: 1, deleted: 1 }),
        deck: deck([CLOZE_CARDS[0]], [{ ...CLOZE_CARDS[1], deletedAt: CREATED, cloze: null, occlusion: null }]),
      }),
    });
    const user = userEvent.setup();

    await user.click((await screen.findAllByRole("button", { name: "Edit card" }))[1]);
    expect(screen.getByRole("heading", { name: "Edit a cloze text in Biology" })).toBeTruthy();
    expect(screen.queryByRole("radio")).toBeNull();
    const text = screen.getByLabelText("Text");
    expect((text as HTMLTextAreaElement).value).toBe(TEXT);

    await user.clear(text);
    await user.type(text, edited.replace(/[{[]/g, "$&$&"));
    expect(
      await screen.findByText("Saving keeps 1 card and deletes 1 card whose number left the text."),
    ).toBeTruthy();
    expect(last(backend.argsOf("preview_cloze"))).toEqual({ text: edited, cardId: 22 });

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(
      await screen.findByText(
        "Cloze text saved: it kept 1 card and deleted 1 card whose number left the text.",
      ),
    ).toBeTruthy();
    expect(backend.argsOf("update_cloze")).toEqual([{ cardId: 22, text: edited }]);
    expect(backend.argsOf("update_flashcard")).toEqual([]);
  });
});

describe("cloze cards in the deck's lists", () => {
  it("selects the cards of one cloze text together", async () => {
    openDeck(deck([BASIC, ...CLOZE_CARDS]));
    const user = userEvent.setup();

    const boxes = await screen.findAllByRole("checkbox", { name: "Select" });
    await user.click(boxes[1]);

    expect(boxes.map((box) => (box as HTMLInputElement).checked)).toEqual([false, true, true]);
    expect(screen.getByText(/2 cards selected/)).toBeTruthy();

    await user.click(boxes[2]);
    expect(boxes.map((box) => (box as HTMLInputElement).checked)).toEqual([false, false, false]);
  });

  it("explains why a deleted cloze card can't be restored, and Restore all counts only the rest", async () => {
    const backend = openDeck(
      deck(
        [],
        [
          {
            ...CLOZE_CARDS[1],
            deletedAt: CREATED,
            cloze: { number: 2, blocked: "numberGone" },
            occlusion: null,
          },
          { ...BASIC, deletedAt: CREATED, cloze: null, occlusion: null },
          { id: 11, front: "B", back: "b", deletedAt: CREATED },
        ],
      ),
      {
        restore_deleted_cards: () => ({ restored: 2, deck: deck([BASIC]) }),
      },
    );
    const user = userEvent.setup();

    expect(
      await screen.findByText(/It can't be restored while \{\{c2::…\}\} isn't in its cloze text/),
    ).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Restore card" })).toHaveLength(2);
    // Nor can it be selected for Restore selected.
    expect(screen.getAllByRole("checkbox", { name: "Select" })).toHaveLength(2);

    await user.click(screen.getByRole("button", { name: "Restore 2 cards" }));
    expect(
      screen.getByText(/^Restore the 2 deleted cards that can be restored\?/),
    ).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Restore 2 cards" }));

    await screen.findByText(/2 cards restored/);
    expect(backend.argsOf("restore_deleted_cards")).toEqual([{ deckId: 1, expected: 2 }]);
  });
});

describe("reviewing a cloze card", () => {
  it("shows the gap in the question and the answer highlighted in place", async () => {
    fakeBackend({
      get_session_card: () => ({
        status: "due",
        card: { id: 22, front: CLOZE_CARDS[1].front, back: "ATP", reps: 0 },
        progress: { reviewed: 0, remaining: 1 },
        againStreak: 0,
        offerHelp: false,
        cloze: [
          { type: "text", text: "Mitochondria make " },
          { type: "gap", answer: "ATP", hint: "a molecule" },
          { type: "text", text: "." },
        ],
      }),
    });
    const user = userEvent.setup();
    render(<ReviewSession sessionId={7} deckName="Biology" onExit={vi.fn()} />);

    const gap = await screen.findByText("[a molecule]");
    expect(gap.className).toBe("cloze-gap");
    expect(gap.parentElement?.textContent).toBe("Mitochondria make [a molecule].");
    expect(screen.queryByText("ATP")).toBeNull();

    await user.keyboard("[Space]");

    const answer = await screen.findByText("ATP");
    expect(answer.tagName).toBe("MARK");
    expect(answer.parentElement?.textContent).toBe("Mitochondria make ATP.");
  });
});
