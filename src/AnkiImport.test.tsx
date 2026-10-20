import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AnkiImported, Deck, ExportOutcome, ReadAnkiFile } from "./api";
import App from "./App";
import { deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const DECKS: Deck[] = [
  { id: 1, name: "Biology", description: null, isSample: false, dueCount: 0 },
];

const FILE: ReadAnkiFile = {
  status: "read",
  token: 7,
  fileName: "Languages.apkg",
  decks: [
    {
      ankiName: "Biology",
      name: "Biology (2)",
      cards: 1,
      reversible: 0,
      examples: [{ front: "What is a cell?", back: "The basic unit of life." }],
    },
    {
      ankiName: "French",
      name: "French",
      cards: 4,
      reversible: 2,
      examples: [{ front: "chat", back: "cat" }],
    },
  ],
  notes: 9,
  skipped: { imageOcclusion: 2, badCloze: 1, otherNoteType: 1, empty: 1, tooLong: 0, noCard: 0 },
  mediaRemoved: 1,
  extraRemoved: 2,
  reviewed: 3,
};

const IMPORTED: AnkiImported = {
  decks: [
    { id: 4, name: "Biology (2)", cards: 1 },
    { id: 5, name: "French", cards: 4 },
  ],
  cards: 5,
};

function sidebar() {
  return screen.getByRole("navigation", { name: "Synapse", hidden: true });
}

/** Starts Synapse and opens Backup & restore. */
async function openBackup(user: UserEvent, handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({
    get_decks: () => DECKS,
    cancel_anki_import: () => null,
    ...handlers,
  });
  render(<App />);
  await screen.findByRole("button", { name: "Create deck" });
  await user.click(within(sidebar()).getByRole("button", { name: "Backup & restore" }));
  await screen.findByRole("heading", { name: "Decks from Anki" });
  return backend;
}

/** Chooses a file and waits for the preview's summary. */
async function preview(user: UserEvent) {
  await user.click(screen.getByRole("button", { name: "Choose an Anki file" }));
  return screen.findByText(/^Read 9 notes from Languages\.apkg/);
}

describe("importing decks from Anki", () => {
  it("shows what the file holds, then adds it as new decks", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, {
      read_anki_package: () => FILE,
      import_anki_package: () => IMPORTED,
    });

    const summary = await preview(user);

    await waitFor(() => expect(document.activeElement).toBe(summary));
    expect(summary.textContent).toBe(
      "Read 9 notes from Languages.apkg. 5 cards ready to add in 2 new decks. Nothing has been added yet.",
    );
    expect(
      screen.getByText(
        "Left out: 2 notes of image occlusion, 1 note with cloze deletions Synapse can't read (nested, or with an empty answer), 1 note of a type that isn't one front and one back, or cloze, 1 note with an empty side (perhaps only a picture).",
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "2 cloze notes have text in another field, such as Back Extra, which is left out.",
      ),
    ).toBeTruthy();
    expect(
      screen.getByText("1 card has a picture or sound, which is left out. Their text is kept."),
    ).toBeTruthy();
    expect(
      screen.getByText("3 of these notes were studied in Anki. The cards still start as new here."),
    ).toBeTruthy();
    const rows = within(screen.getByRole("region", { name: "Decks to add" }))
      .getAllByRole("row")
      .slice(1)
      .map((row) => within(row).getAllByRole("cell").map((cell) => cell.textContent));
    expect(rows).toEqual([
      ["Biology", "Biology (2)", "1", "What is a cell?"],
      ["French", "French", "4", "chat"],
    ]);
    expect(
      screen.getByRole("checkbox", { name: /Also add the reversed card/ }),
    ).toHaveProperty("checked", false);

    await user.click(screen.getByRole("button", { name: "Add 5 cards in 2 new decks" }));

    expect(backend.argsOf("import_anki_package")).toEqual([{ token: 7, bothDirections: false }]);
    const done = await screen.findByText(
      "Added 5 cards in 2 new decks: Biology (2), French. They're on the Study Desk, ready to review.",
    );
    await waitFor(() => expect(document.activeElement).toBe(done));
    expect(screen.queryByRole("region", { name: "Decks to add" })).toBeNull();
  });

  it("adds the reversed cards when asked", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, {
      read_anki_package: () => FILE,
      import_anki_package: () => ({ ...IMPORTED, cards: 7 }),
    });
    await preview(user);

    await user.click(
      screen.getByRole("checkbox", {
        name: "Also add the reversed card, with the back as the question, for the 2 notes that have one in Anki",
      }),
    );
    expect(screen.getAllByRole("cell").map((cell) => cell.textContent)).toContain("6");
    await user.click(screen.getByRole("button", { name: "Add 7 cards in 2 new decks" }));

    expect(backend.argsOf("import_anki_package")).toEqual([{ token: 7, bothDirections: true }]);
  });

  it("offers no reversed cards when no note has one", async () => {
    const user = userEvent.setup();
    await openBackup(user, {
      read_anki_package: () => ({
        ...FILE,
        decks: [FILE.decks[0]],
        skipped: { imageOcclusion: 0, badCloze: 0, otherNoteType: 0, empty: 0, tooLong: 0, noCard: 0 },
        mediaRemoved: 0,
        extraRemoved: 0,
        reviewed: 0,
      }),
    });
    await user.click(screen.getByRole("button", { name: "Choose an Anki file" }));

    await screen.findByText(/1 card ready to add in 1 new deck\./);
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByText(/^Left out/)).toBeNull();
    expect(screen.queryByText(/studied in Anki/)).toBeNull();
    expect(screen.queryByText(/Back Extra/)).toBeNull();
  });

  it("sends one save however often Add is pressed, and the spreadsheet decks reload", async () => {
    const user = userEvent.setup();
    const saving = deferred<AnkiImported>();
    const backend = await openBackup(user, {
      read_anki_package: () => FILE,
      import_anki_package: () => saving.promise,
    });
    await preview(user);
    const loads = backend.argsOf("get_decks").length;

    const add = screen.getByRole("button", { name: "Add 5 cards in 2 new decks" });
    await user.click(add);
    await user.click(add);
    expect(screen.getByText("Adding the decks…")).toBeTruthy();
    expect(backend.argsOf("import_anki_package")).toHaveLength(1);

    await act(async () => saving.resolve(IMPORTED));
    await screen.findByText(/^Added 5 cards/);
    await waitFor(() => expect(backend.argsOf("get_decks").length).toBe(loads + 1));
  });

  it("says when nothing in the file can become a card", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, {
      read_anki_package: () => ({ ...FILE, decks: [], mediaRemoved: 0, reviewed: 0 }),
    });
    await user.click(screen.getByRole("button", { name: "Choose an Anki file" }));

    await screen.findByText(
      "Read 9 notes from Languages.apkg. None of them can become cards here, so there's nothing to add.",
    );
    expect(screen.queryByRole("button", { name: /^Add/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(backend.argsOf("cancel_anki_import")).toEqual([{ token: 7 }]);
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Choose an Anki file" }),
      ),
    );
  });

  it("shows a refused file, and does nothing when the window is closed", async () => {
    const user = userEvent.setup();
    let answer: () => unknown = () =>
      refuse("invalid", "That file isn't an Anki package. In Anki, export your decks as an .apkg file, then choose that.");
    await openBackup(user, { read_anki_package: () => answer() });

    await user.click(screen.getByRole("button", { name: "Choose an Anki file" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/^That file isn't an Anki package/);

    answer = () => ({ status: "cancelled" });
    await user.click(screen.getByRole("button", { name: "Choose an Anki file" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.queryByRole("region", { name: "Decks to add" })).toBeNull();
  });

  it("drops a preview Rust no longer holds", async () => {
    const user = userEvent.setup();
    await openBackup(user, {
      read_anki_package: () => FILE,
      import_anki_package: () =>
        refuse("stale", "This preview is out of date. Choose the file again."),
    });
    await preview(user);

    await user.click(screen.getByRole("button", { name: "Add 5 cards in 2 new decks" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "This preview is out of date. Choose the file again.",
    );
    expect(screen.queryByRole("region", { name: "Decks to add" })).toBeNull();
  });

  it("keeps the preview when saving fails", async () => {
    const user = userEvent.setup();
    await openBackup(user, {
      read_anki_package: () => FILE,
      import_anki_package: () =>
        refuse("failed", "Synapse couldn't save the cards. Nothing was added."),
    });
    await preview(user);

    await user.click(screen.getByRole("button", { name: "Add 5 cards in 2 new decks" }));

    await screen.findByText("Synapse couldn't save the cards. Nothing was added.");
    expect(screen.getByRole("region", { name: "Decks to add" })).toBeTruthy();
  });

  it("asks before leaving a preview, and Cancel drops it without asking", async () => {
    const user = userEvent.setup();
    const backend = await openBackup(user, { read_anki_package: () => FILE });
    await preview(user);

    await user.click(within(sidebar()).getByRole("button", { name: "Study Desk" }));
    expect(screen.getByRole("alertdialog", { name: "Leave without saving?" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(screen.getByRole("region", { name: "Decks to add" })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("region", { name: "Decks to add" })).toBeNull();
    expect(backend.argsOf("cancel_anki_import")).toEqual([{ token: 7 }]);
    await user.click(within(sidebar()).getByRole("button", { name: "Study Desk" }));
    await screen.findByRole("button", { name: "Create deck" });
  });

  it("waits while a backup is being exported", async () => {
    const user = userEvent.setup();
    const exporting = deferred<ExportOutcome>();
    const backend = await openBackup(user, {
      export_data: () => exporting.promise,
      read_anki_package: () => FILE,
    });

    await user.click(screen.getByRole("button", { name: "Export data" }));
    await user.click(screen.getByRole("button", { name: "Choose an Anki file" }));
    expect(backend.argsOf("read_anki_package")).toEqual([]);

    await act(async () => exporting.resolve({ status: "cancelled" }));
    await user.click(screen.getByRole("button", { name: "Choose an Anki file" }));
    expect(backend.argsOf("read_anki_package")).toEqual([{}]);
  });
});
