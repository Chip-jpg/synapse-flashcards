import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Asset, DeckCard, DeckDetail as Deck, OcclusionBox } from "./api";
import { DeckDetail } from "./DeckDetail";
import { ReviewSession } from "./ReviewSession";
import { CREATED, fakeBackend, type Handler } from "./test/backend";

const PICTURE: Asset = {
  id: 3,
  sha256: "ab".repeat(32),
  mediaType: "image/png",
  size: 1234,
  createdAt: CREATED,
};

const BOXES: OcclusionBox[] = [
  { number: 1, left: 0.1, top: 0.1, width: 0.2, height: 0.1, label: "Aorta" },
  { number: 2, left: 0.5, top: 0.5, width: 0.2, height: 0.1, label: "Vena cava" },
];

/** The two cards Rust makes from `BOXES` under the header "The heart". */
const PICTURE_CARDS: DeckCard[] = [
  {
    id: 31,
    front: "The heart\n[picture: box 1]",
    back: "Aorta",
    occlusion: { occlusionId: 5, number: 1 },
  },
  {
    id: 32,
    front: "The heart\n[picture: box 2]",
    back: "Vena cava",
    occlusion: { occlusionId: 5, number: 2 },
  },
];

const BASIC: DeckCard = { id: 10, front: "What is a cell?", back: "The basic unit of life." };

function deck(cards: DeckCard[]): Deck {
  return {
    id: 1,
    name: "Biology",
    description: null,
    cardCount: cards.length,
    dueCount: 0,
    cards,
    deletedCards: [],
  };
}

function openDeck(start: Deck, handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({ get_deck_detail: () => start, ...handlers });
  render(
    <DeckDetail deckId={1} justCreated={false} onBack={vi.fn()} onStart={vi.fn()} onArchived={vi.fn()} />,
  );
  return backend;
}

describe("adding picture cards", () => {
  it("asks for a picture first, then saves its boxes as one card each", async () => {
    const backend = openDeck(deck([BASIC]), {
      choose_picture: () => ({ status: "added", asset: PICTURE }),
      create_occlusion: () => deck([BASIC, PICTURE_CARDS[0]]),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.click(screen.getByRole("radio", { name: /Picture/ }));
    expect(screen.queryByLabelText("Front")).toBeNull();
    expect(screen.getByText("No picture yet. Choose one, or paste one here.")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Save cards" }));
    expect(await screen.findByText("Choose a picture first.")).toBeTruthy();
    expect(backend.argsOf("create_occlusion")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Choose a picture" }));
    expect(await screen.findByRole("button", { name: "Choose another picture" })).toBeTruthy();
    expect(screen.getByText("No boxes yet.")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Add a box" }));
    expect(screen.getByText("1 box makes 1 card.")).toBeTruthy();
    await user.type(screen.getByLabelText("Label for box 1"), "Aorta");
    await user.type(screen.getByLabelText("Header"), "The heart");
    await user.click(screen.getByRole("button", { name: "Save cards" }));

    const notice = await screen.findByText("Picture added: it made 1 card.");
    await waitFor(() => expect(document.activeElement).toBe(notice));
    const [args] = backend.argsOf("create_occlusion");
    expect(args).toMatchObject({ deckId: 1, assetId: 3, header: "The heart", mode: "hideAll" });
    expect(args.boxes).toEqual([
      { number: 1, left: 0.4, top: 0.45, width: expect.any(Number), height: expect.any(Number), label: "Aorta" },
    ]);
    expect(screen.getByText("Picture, box 1")).toBeTruthy();
  });

  it("gives two boxes the same card when chosen, and moves a box with the arrow keys", async () => {
    const backend = openDeck(deck([]), {
      choose_picture: () => ({ status: "added", asset: PICTURE }),
      create_occlusion: () => deck(PICTURE_CARDS.slice(0, 1)),
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.click(screen.getByRole("radio", { name: /Picture/ }));
    await user.click(screen.getByRole("button", { name: "Choose a picture" }));
    await user.click(await screen.findByRole("button", { name: "Add a box" }));
    await user.click(screen.getByRole("button", { name: "Add a box" }));
    expect(screen.getByText("2 boxes make 2 cards.")).toBeTruthy();

    await user.selectOptions(screen.getByLabelText("Card for box 2"), "Card 1");
    expect(screen.getByText("2 boxes make 1 card.")).toBeTruthy();

    const box = screen.getAllByRole("button", { name: "Box for card 1" })[0];
    box.focus();
    await user.keyboard("{ArrowRight}");
    await user.click(screen.getByRole("button", { name: "Save cards" }));

    await screen.findByText("Picture added: it made 1 card.");
    const [args] = backend.argsOf("create_occlusion");
    const boxes = args.boxes as OcclusionBox[];
    expect(boxes.map((shape) => shape.number)).toEqual([1, 1]);
    expect(boxes[0].left).toBeGreaterThan(boxes[1].left);
  });
});

describe("editing picture cards", () => {
  it("opens the picture's boxes and says what saving did", async () => {
    const backend = openDeck(deck(PICTURE_CARDS), {
      get_occlusion: () => ({
        id: 5,
        picture: PICTURE,
        header: "The heart",
        mode: "hideAll",
        boxes: BOXES,
        nextNumber: 3,
      }),
      update_occlusion: () => ({
        changes: { cards: 1, kept: 1, restored: 0, added: 0, deleted: 1 },
        deck: deck(PICTURE_CARDS.slice(0, 1)),
      }),
    });
    const user = userEvent.setup();

    const rows = await screen.findAllByRole("listitem");
    const row = rows.find((item) => item.textContent?.includes("Picture, box 2"));
    await user.click(within(row!).getByRole("button", { name: /Edit/ }));

    expect(await screen.findByDisplayValue("Vena cava")).toBeTruthy();
    expect(screen.getByLabelText("Header")).toHaveProperty("value", "The heart");
    expect(screen.queryByRole("radio", { name: /Basic/ })).toBeNull();
    expect(backend.argsOf("get_occlusion")).toEqual([{ cardId: 32 }]);

    await user.click(screen.getByRole("button", { name: "Remove box 2" }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(
      await screen.findByText(
        "Picture saved: it kept 1 card and deleted 1 card whose boxes were removed.",
      ),
    ).toBeTruthy();
    expect(backend.argsOf("update_occlusion")).toEqual([
      { cardId: 32, header: "The heart", mode: "hideAll", boxes: [BOXES[0]] },
    ]);
  });
});

describe("reviewing a picture card", () => {
  it("covers the asked box, uncovers it with its label, and offers no help", async () => {
    fakeBackend({
      get_session_card: () => ({
        status: "due",
        card: { id: 31, front: PICTURE_CARDS[0].front, back: "Aorta", reps: 0 },
        progress: { reviewed: 0, remaining: 1 },
        againStreak: 5,
        offerHelp: false,
        occlusion: { picture: PICTURE, header: "The heart", mode: "hideAll", number: 1, boxes: BOXES },
      }),
    });
    const user = userEvent.setup();
    render(<ReviewSession sessionId={7} deckName="Biology" onExit={vi.fn()} />);

    const picture = await screen.findByRole("img", {
      name: "Picture with one box to recall, marked with a question mark.",
    });
    expect(screen.getByText("The heart")).toBeTruthy();
    expect(picture.querySelectorAll(".occlusion-box")).toHaveLength(2);
    expect(picture.querySelectorAll(".occlusion-box-asked")).toHaveLength(1);
    expect(screen.queryByText("Aorta")).toBeNull();

    await user.keyboard("[Space]");

    expect(
      await screen.findByRole("img", { name: "Picture with the box asked about uncovered." }),
    ).toBeTruthy();
    expect(screen.getAllByText("Aorta").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /help/i })).toBeNull();
  });
});
