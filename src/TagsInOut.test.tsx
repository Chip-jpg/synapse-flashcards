import { render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Deck, Flashcard, ReadAnkiFile, ReadCardsFile, TagDue } from "./api";
import App from "./App";
import { Dashboard } from "./Dashboard";
import { ReviewSession } from "./ReviewSession";
import { fakeBackend, refuse } from "./test/backend";

const DECKS: Deck[] = [
  { id: 1, name: "Biology", description: null, isSample: false, dueCount: 1 },
];

const TAGS: TagDue[] = [
  { id: 3, name: "exam", dueCount: 2, decks: 2 },
  { id: 4, name: "later", dueCount: 0, decks: 0 },
];

function desk(onStartTag = vi.fn()) {
  render(
    <Dashboard
      focusOnLoad={false}
      notice={null}
      fromNotes={false}
      onStart={vi.fn()}
      onStartTag={onStartTag}
      onOpen={vi.fn()}
      onCreated={vi.fn()}
      onOpenNotes={vi.fn()}
    />,
  );
  return onStartTag;
}

describe("reviewing a tag on the Study Desk", () => {
  it("lists the tags with cards due, and starts one", async () => {
    const user = userEvent.setup();
    fakeBackend({ get_decks: () => DECKS, get_tags_due: () => TAGS });
    const onStartTag = desk();

    const section = await screen.findByRole("region", { name: /Review by tag/ });
    expect(within(section).getByText("2 cards due in 2 decks")).toBeTruthy();
    // A tag with nothing due isn't offered.
    expect(within(section).queryByText("later")).toBeNull();
    await user.click(within(section).getByRole("button", { name: "Review tag" }));
    expect(onStartTag).toHaveBeenCalledWith({ id: 3, name: "exam" });
  });

  it("shows nothing when no tag has cards due, or the tags can't be read", async () => {
    fakeBackend({ get_decks: () => DECKS, get_tags_due: () => [TAGS[1]] });
    desk();
    await screen.findByRole("heading", { name: /Your decks/ });
    expect(screen.queryByRole("region", { name: /Review by tag/ })).toBeNull();
  });

  it("still shows the desk when the tags fail to load", async () => {
    fakeBackend({
      get_decks: () => DECKS,
      get_tags_due: () => refuse("failed", "Synapse couldn't load your tags."),
    });
    desk();
    expect(await screen.findByRole("heading", { name: /Your decks/ })).toBeTruthy();
    expect(screen.queryByRole("region", { name: /Review by tag/ })).toBeNull();
  });
});

const CARDS: (Flashcard & { deck: string })[] = [
  { id: 1, front: "What is a cell?", back: "The basic unit of life.", reps: 0, deck: "Biology" },
  { id: 2, front: "What is an ion?", back: "A charged atom.", reps: 2, deck: "Chemistry" },
];

describe("the review of a tag", () => {
  it("goes through the tag's cards from every deck, counting its own ratings", async () => {
    const user = userEvent.setup();
    let rated = 0;
    const backend = fakeBackend({
      get_tag_card: (args) => {
        expect(args.reviewed).toBe(rated);
        const card = CARDS[rated];
        return card
          ? {
              status: "due",
              card: { id: card.id, front: card.front, back: card.back, reps: card.reps },
              progress: { reviewed: args.reviewed, remaining: CARDS.length - rated },
              againStreak: 0,
              offerHelp: false,
              deck: card.deck,
            }
          : { status: "completed", cardsReviewed: args.reviewed };
      },
      review_tag_card: () => {
        rated += 1;
        return null;
      },
    });
    render(<ReviewSession tag={{ id: 3, name: "exam" }} onExit={vi.fn()} />);

    expect(screen.getByRole("heading", { name: "Reviewing tag exam" })).toBeTruthy();
    expect(await screen.findByText("From Biology")).toBeTruthy();
    await rate(user, "Good");
    expect(await screen.findByText("From Chemistry")).toBeTruthy();
    expect(screen.getByText("Card 2 of 2")).toBeTruthy();
    await rate(user, "Easy");

    expect(await screen.findByText(/No more cards with this tag are due/)).toBeTruthy();
    expect(screen.getByText(/You reviewed 2 cards/)).toBeTruthy();
    expect(backend.argsOf("review_tag_card")).toEqual([
      { tagId: 3, cardId: 1, expectedReps: 0, rating: 3 },
      { tagId: 3, cardId: 2, expectedReps: 2, rating: 4 },
    ]);
    expect(backend.calls).not.toContain("review_card");
  });

  it("doesn't count a stale rating as one made", async () => {
    const user = userEvent.setup();
    let stale = true;
    const backend = fakeBackend({
      get_tag_card: (args) =>
        stale
          ? {
              status: "due",
              card: CARDS[0],
              progress: { reviewed: args.reviewed, remaining: 1 },
              againStreak: 0,
              offerHelp: false,
              deck: "Biology",
            }
          : { status: "completed", cardsReviewed: args.reviewed },
      review_tag_card: () => {
        stale = false;
        return refuse("stale", "This card was already reviewed.");
      },
    });
    render(<ReviewSession tag={{ id: 3, name: "exam" }} onExit={vi.fn()} />);
    await screen.findByText("From Biology");
    await rate(user, "Good");

    expect(await screen.findByText(/You reviewed 0 cards/)).toBeTruthy();
    expect(backend.argsOf("get_tag_card").map((args) => args.reviewed)).toEqual([0, 0]);
  });
});

async function rate(user: UserEvent, rating: string) {
  await user.click(await screen.findByRole("button", { name: "Reveal answer" }));
  await user.click(screen.getByRole("button", { name: new RegExp(`^${rating}`) }));
}

function sidebar() {
  return screen.getByRole("navigation", { name: "Synapse", hidden: true });
}

const CSV: ReadCardsFile = {
  status: "read",
  fileName: "tagged.csv",
  delimiter: "comma",
  header: true,
  tagsColumn: true,
  rows: [
    {
      line: 2,
      front: "What is DNA?",
      back: "A molecule.",
      problem: null,
      duplicate: null,
      tags: ["bio", "exam"],
    },
    {
      line: 3,
      front: "Too long",
      back: "x",
      problem: { field: "tags", message: "A tag can be at most 60 characters." },
      duplicate: null,
      tags: [],
    },
  ],
  rowsWithExtraColumns: 0,
};

describe("tags in files", () => {
  it("shows a CSV file's tags column and sends each card's tags", async () => {
    const user = userEvent.setup();
    const backend = fakeBackend({
      get_decks: () => DECKS,
      read_cards_csv: () => CSV,
      import_cards_csv: () => ({ added: 1, skipped: 0, cardCount: 2 }),
    });
    render(<App />);
    await screen.findByRole("button", { name: "Create deck" });
    await user.click(within(sidebar()).getByRole("button", { name: "Backup & restore" }));
    const group = await screen.findByRole("group", { name: "Import cards" });
    await user.selectOptions(within(group).getByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Choose a CSV file" }));
    await screen.findByText(/^Read 2 rows from tagged\.csv/);

    const table = screen.getByRole("table");
    expect(within(table).getByRole("columnheader", { name: "Tags" })).toBeTruthy();
    expect(within(table).getByText("bio exam")).toBeTruthy();
    expect(within(table).getByText("A tag can be at most 60 characters.")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /^Add 1 card/ }));

    expect(await screen.findByText(/^Added 1 card to Biology/)).toBeTruthy();
    expect(backend.argsOf("import_cards_csv")[0].cards).toEqual([
      { front: "What is DNA?", back: "A molecule.", tags: ["bio", "exam"] },
    ]);
  });

  it("says how many tags come from an Anki package, and how many are left out", async () => {
    const user = userEvent.setup();
    const file: ReadAnkiFile = {
      status: "read",
      token: 1,
      fileName: "tags.apkg",
      decks: [
        {
          ankiName: "Tagged",
          name: "Tagged",
          cards: 2,
          reversible: 0,
          examples: [{ front: "Cell", back: "Unit of life" }],
        },
      ],
      notes: 2,
      skipped: {
        imageOcclusion: 0,
        badCloze: 0,
        otherNoteType: 0,
        empty: 0,
        tooLong: 0,
        noCard: 0,
      },
      mediaRemoved: 0,
      extraRemoved: 0,
      reviewed: 0,
      occlusions: 0,
      occlusionApproximated: 0,
      cardPictures: 0,
      picturesLeftOut: { missing: 0, unsupported: 0, tooLarge: 0, tooMany: 0 },
      tags: 3,
      tagsLeftOut: 2,
    };
    fakeBackend({ get_decks: () => DECKS, read_anki_package: () => file });
    render(<App />);
    await screen.findByRole("button", { name: "Create deck" });
    await user.click(within(sidebar()).getByRole("button", { name: "Backup & restore" }));
    await user.click(await screen.findByRole("button", { name: /Choose an Anki/ }));

    expect(await screen.findByText("The cards keep their notes' tags: 3 tags in all.")).toBeTruthy();
    expect(screen.getByText(/^2 tags are left out/)).toBeTruthy();
  });
});
