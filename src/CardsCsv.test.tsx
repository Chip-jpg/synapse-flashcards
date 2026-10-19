import { act, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Deck, ExportOutcome, ReadCardsFile } from "./api";
import App from "./App";
import { deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const DECKS: Deck[] = [
  { id: 1, name: "Biology", description: null, isSample: false, dueCount: 0 },
  { id: 2, name: "Sample deck", description: null, isSample: true, dueCount: 1 },
];

const FILE: ReadCardsFile = {
  status: "read",
  fileName: "biology.csv",
  delimiter: "comma",
  header: true,
  rows: [
    { line: 2, front: "What is DNA?", back: "A molecule.", problem: null, duplicate: null },
    {
      line: 3,
      front: "What is a cell?",
      back: "Again.",
      problem: null,
      duplicate: { kind: "inDeck" },
    },
    {
      line: 4,
      front: "Only a front",
      back: "",
      problem: { field: "back", message: "This row has no back: put the back in a second column." },
      duplicate: null,
    },
    { line: 5, front: "What is RNA?", back: "Another\nmolecule.", problem: null, duplicate: null },
  ],
  rowsWithExtraColumns: 0,
};

function sidebar() {
  return screen.getByRole("navigation", { name: "Synapse", hidden: true });
}

/** Starts Synapse and opens Backup & restore, with the deck lists loaded. */
async function openBackup(user: UserEvent, handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({ get_decks: () => DECKS, ...handlers });
  render(<App />);
  await screen.findByRole("button", { name: "Create deck" });
  await user.click(within(sidebar()).getByRole("button", { name: "Backup & restore" }));
  await screen.findByRole("heading", { name: "Cards in a spreadsheet" });
  return backend;
}

function importGroup() {
  return screen.getByRole("group", { name: "Import cards" });
}

function exportGroup() {
  return screen.getByRole("group", { name: "Export cards" });
}

/** Chooses Biology and a file, and waits for the preview. */
async function preview(user: UserEvent) {
  await user.selectOptions(within(importGroup()).getByLabelText("Deck"), "Biology");
  await user.click(screen.getByRole("button", { name: "Choose a CSV file" }));
  return screen.findByText(/^Read 4 rows from biology\.csv/);
}

describe("importing cards from a file", () => {
  it("offers only decks that take cards, and exports from any active deck", async () => {
    const user = userEvent.setup();
    await openBackup(user);

    const importChoices = within(within(importGroup()).getByLabelText("Deck"))
      .getAllByRole("option")
      .map((option) => option.textContent);
    expect(importChoices).toEqual(["Choose a deck", "Biology"]);
    const exportChoices = within(within(exportGroup()).getByLabelText("Deck"))
      .getAllByRole("option")
      .map((option) => option.textContent);
    expect(exportChoices).toEqual(["Choose a deck", "Biology", "Sample deck"]);
  });

  it("asks for a deck before opening the file window", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, { read_cards_csv: () => FILE });

    await user.click(screen.getByRole("button", { name: "Choose a CSV file" }));

    expect(screen.getByText("Choose the deck to add the cards to.")).toBeTruthy();
    expect(document.activeElement).toBe(within(importGroup()).getByLabelText("Deck"));
    expect(backend.argsOf("read_cards_csv")).toEqual([]);
  });

  it("shows every row, then adds the ready ones and leaves duplicates out", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, {
      read_cards_csv: () => FILE,
      import_cards_csv: () => ({ added: 2, skipped: 0, cardCount: 3 }),
    });

    const summary = await preview(user);

    expect(document.activeElement).toBe(summary);
    expect(summary.textContent).toBe(
      "Read 4 rows from biology.csv, separated by commas, after a header row. 2 cards ready to add, 1 already in the deck or repeated, 1 row with problems, which won't be added. Nothing has been added to Biology yet.",
    );
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows.map((row) => within(row).getAllByRole("cell")[3].textContent)).toEqual([
      "Ready",
      "Already in this deck: left out",
      "This row has no back: put the back in a second column.",
      "Ready",
    ]);
    expect(
      screen.getByRole("checkbox", { name: /Leave out cards whose front is already/ }),
    ).toHaveProperty("checked", true);

    await user.click(screen.getByRole("button", { name: "Add 2 cards to Biology" }));

    expect(backend.argsOf("import_cards_csv")).toEqual([
      {
        deckId: 1,
        cards: [
          { front: "What is DNA?", back: "A molecule." },
          { front: "What is RNA?", back: "Another\nmolecule." },
        ],
        skipDuplicates: true,
      },
    ]);
    const done = await screen.findByText("Added 2 cards to Biology. It now has 3 cards.");
    expect(document.activeElement).toBe(done);
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("adds duplicates too when asked", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, {
      read_cards_csv: () => FILE,
      import_cards_csv: () => ({ added: 3, skipped: 0, cardCount: 4 }),
    });
    await preview(user);

    await user.click(screen.getByRole("checkbox", { name: /Leave out cards/ }));
    expect(screen.getByText("Already in this deck: added anyway")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Add 3 cards to Biology" }));

    const [sent] = backend.argsOf("import_cards_csv");
    expect(sent.skipDuplicates).toBe(false);
    expect((sent.cards as unknown[]).length).toBe(3);
  });

  it("sends one save however often Add is pressed", async () => {
    const user = userEvent.setup();
    const saving = deferred();
    const backend = await openBackup(user, {
      read_cards_csv: () => FILE,
      import_cards_csv: () => saving.promise,
    });
    await preview(user);

    const add = screen.getByRole("button", { name: "Add 2 cards to Biology" });
    await user.click(add);
    await user.click(add);
    expect(screen.getByText("Adding the cards…")).toBeTruthy();
    expect(backend.argsOf("import_cards_csv")).toHaveLength(1);

    await act(async () => saving.resolve({ added: 2, skipped: 0, cardCount: 3 }));
    await screen.findByText(/^Added 2 cards to Biology/);
  });

  it("names the line of a card Rust refuses, and keeps the preview", async () => {
    const user = userEvent.setup();
    await openBackup(user, {
      read_cards_csv: () => FILE,
      import_cards_csv: () =>
        refuse("invalid", "The back can be at most 2,000 characters.", "back", 1),
    });
    await preview(user);

    await user.click(screen.getByRole("button", { name: "Add 2 cards to Biology" }));

    expect(
      (await screen.findByRole("alert")).textContent,
    ).toBe("Line 5: The back can be at most 2,000 characters.");
    expect(screen.getByRole("table")).toBeTruthy();
  });

  it("says when a file has nothing new to add", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, {
      read_cards_csv: () => ({
        ...FILE,
        rows: [FILE.rows[1]],
      }),
    });
    await user.selectOptions(within(importGroup()).getByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Choose a CSV file" }));

    await screen.findByText("There's nothing new to add from this file.");
    await user.click(screen.getByRole("button", { name: "Add 0 cards to Biology" }));
    expect(backend.argsOf("import_cards_csv")).toEqual([]);
  });

  it("shows a refused file and leaves nothing to save", async () => {
    const user = userEvent.setup();
    await openBackup(user, {
      read_cards_csv: () => refuse("invalid", "That file has no cards in it."),
    });
    await user.selectOptions(within(importGroup()).getByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Choose a CSV file" }));

    expect((await screen.findByRole("alert")).textContent).toBe("That file has no cards in it.");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("asks before leaving a preview, and Cancel drops it without asking", async () => {
    const user = userEvent.setup();
    await openBackup(user, { read_cards_csv: () => FILE });
    await preview(user);

    await user.click(within(sidebar()).getByRole("button", { name: "Study Desk" }));
    expect(screen.getByRole("alertdialog", { name: "Leave without saving?" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(screen.getByRole("table")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("table")).toBeNull();
    await user.click(within(sidebar()).getByRole("button", { name: "Study Desk" }));
    await screen.findByRole("button", { name: "Create deck" });
  });

  it("waits while a backup is being exported", async () => {
    const user = userEvent.setup();
    const exporting = deferred<ExportOutcome>();
    const backend = await openBackup(user, {
      export_data: () => exporting.promise,
      read_cards_csv: () => FILE,
    });
    await user.selectOptions(within(importGroup()).getByLabelText("Deck"), "Biology");

    await user.click(screen.getByRole("button", { name: "Export data" }));
    await user.click(screen.getByRole("button", { name: "Choose a CSV file" }));
    expect(backend.argsOf("read_cards_csv")).toEqual([]);

    await act(async () => exporting.resolve({ status: "cancelled" }));
    await user.click(screen.getByRole("button", { name: "Choose a CSV file" }));
    expect(backend.argsOf("read_cards_csv")).toEqual([{ deckId: 1 }]);
  });
});

describe("exporting a deck's cards", () => {
  it("saves the chosen deck and says under what name", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, {
      export_cards_csv: () => ({ status: "saved", fileName: "Sample deck.csv" }),
    });

    await user.click(screen.getByRole("button", { name: "Export as CSV" }));
    expect(screen.getByText("Choose the deck to export.")).toBeTruthy();
    expect(backend.argsOf("export_cards_csv")).toEqual([]);

    await user.selectOptions(within(exportGroup()).getByLabelText("Deck"), "Sample deck");
    await user.click(screen.getByRole("button", { name: "Export as CSV" }));

    expect(backend.argsOf("export_cards_csv")).toEqual([{ deckId: 2 }]);
    await screen.findByText("Cards saved as Sample deck.csv.");
  });

  it("shows why a deck couldn't be exported", async () => {
    const user = userEvent.setup();
    await openBackup(user, {
      export_cards_csv: () => refuse("invalid", "This deck has no cards to export."),
    });
    await user.selectOptions(within(exportGroup()).getByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Export as CSV" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "This deck has no cards to export.",
    );
  });
});
