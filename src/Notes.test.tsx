import { act, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Deck, DeletedNote, NoteSummary } from "./api";
import App from "./App";
import { NotesArea } from "./Notes";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const MITOSIS: NoteSummary = { id: 5, title: "Mitosis", createdAt: CREATED, updatedAt: CREATED };
const MEIOSIS: DeletedNote = {
  id: 6,
  title: "Meiosis",
  createdAt: CREATED,
  updatedAt: CREATED,
  deletedAt: CREATED,
};

function openNotes(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend(handlers);
  render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
  return backend;
}

describe("deleting a note", () => {
  it("asks first, with focus on the question, and Keep note puts focus back", async () => {
    const notes = openNotes();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Open note" }));

    await user.click(await screen.findByRole("button", { name: "Delete note" }));
    expect(document.activeElement).toBe(screen.getByText(/^Delete this note\?/));

    await user.click(screen.getByRole("button", { name: "Keep note" }));

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Delete note" }));
    expect(notes.argsOf("delete_note")).toEqual([]);
  });

  it("goes back to the library, which says where the note went", async () => {
    let deleted = false;
    const notes = openNotes({
      get_notes: () => (deleted ? [] : [MITOSIS]),
      get_deleted_notes: () => (deleted ? [{ ...MITOSIS, deletedAt: CREATED }] : []),
      delete_note: () => {
        deleted = true;
        return null;
      },
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await user.click(await screen.findByRole("button", { name: "Delete note" }));

    await user.click(screen.getByRole("button", { name: "Delete note" }));

    const notice = await screen.findByText(
      "Note deleted. It's kept in Deleted notes, where you can restore it.",
    );
    expect(document.activeElement).toBe(notice);
    expect(notes.argsOf("delete_note")).toEqual([{ noteId: 5 }]);
    expect(screen.getByRole("button", { name: "Restore note" })).toBeTruthy();
  });
});

describe("restoring a note", () => {
  it("puts it back in the library, and says so", async () => {
    let restored = false;
    openNotes({
      get_notes: () => (restored ? [MITOSIS, MEIOSIS] : [MITOSIS]),
      get_deleted_notes: () => (restored ? [] : [MEIOSIS]),
      restore_note: () => {
        restored = true;
        return null;
      },
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore note" }));

    const notice = await screen.findByText("Meiosis restored. It's back in your notes.");
    expect(document.activeElement).toBe(notice);
    expect(screen.getAllByRole("button", { name: "Open note" })).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Restore note" })).toBeNull();
  });

  it("sends one restore, however often it's pressed while restoring", async () => {
    const restoring = deferred();
    const notes = openNotes({
      get_deleted_notes: () => [MEIOSIS],
      restore_note: () => restoring.promise,
    });
    const user = userEvent.setup();

    const restore = await screen.findByRole("button", { name: "Restore note" });
    await user.click(restore);
    await user.click(restore);

    expect(screen.getByText("Restoring…")).toBeTruthy();
    expect(notes.argsOf("restore_note")).toEqual([{ noteId: 6 }]);
    await act(async () => restoring.resolve(null));
  });

  it("reloads the library and says why when the note wasn't deleted after all", async () => {
    let reloaded = false;
    const notes = openNotes({
      get_notes: () => (reloaded ? [MITOSIS, MEIOSIS] : [MITOSIS]),
      get_deleted_notes: () => (reloaded ? [] : [MEIOSIS]),
      restore_note: () => {
        reloaded = true;
        return refuse("stale", "That note isn't deleted.");
      },
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Restore note" }));

    const notice = await screen.findByText(
      "That note wasn't deleted after all. Here's your library as it is now.",
    );
    expect(document.activeElement).toBe(notice);
    expect(notes.argsOf("get_deleted_notes")).toHaveLength(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("searching notes", () => {
  const CELLS: NoteSummary = { id: 7, title: "Cell biology", createdAt: CREATED, updatedAt: CREATED };
  const OSMOSIS: NoteSummary = { id: 8, title: "Osmosis", createdAt: CREATED, updatedAt: CREATED };

  /** The titles the library lists, in order. */
  function listed() {
    return screen.queryAllByRole("heading", { level: 3 }).map((heading) => heading.textContent);
  }

  /** A library of three notes; Rust's search answers as `search` says. */
  function openLibrary(search: Handler) {
    return openNotes({ get_notes: () => [MITOSIS, CELLS, OSMOSIS], search_notes: search });
  }

  it("narrows the library to what Rust finds, in Rust's order, and clearing brings it all back", async () => {
    const notes = openLibrary(({ query }) =>
      String(query).toLowerCase() === "mitosis" ? [MITOSIS] : [OSMOSIS, CELLS],
    );
    const user = userEvent.setup();
    const field = await screen.findByLabelText("Search notes");
    expect(listed()).toEqual(["Mitosis", "Cell biology", "Osmosis"]);

    await user.type(field, "  MITOSIS ");

    await vi.waitFor(() => expect(listed()).toEqual(["Mitosis"]));
    expect(screen.getByText("1 note matches.")).toBeTruthy();
    // Sent trimmed, one request per change.
    const sent = notes.argsOf("search_notes");
    expect(sent[sent.length - 1]).toEqual({ query: "MITOSIS" });

    await user.click(screen.getByRole("button", { name: "Clear" }));

    expect(listed()).toEqual(["Mitosis", "Cell biology", "Osmosis"]);
    expect((field as HTMLInputElement).value).toBe("");
    expect(document.activeElement).toBe(field);
    expect(screen.queryByRole("button", { name: "Clear" })).toBeNull();

    await user.type(field, "o");
    await vi.waitFor(() => expect(listed()).toEqual(["Osmosis", "Cell biology"]));
    expect(screen.getByText("2 notes match.")).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(listed()).toEqual(["Mitosis", "Cell biology", "Osmosis"]);
  });

  it("says plainly when nothing matches", async () => {
    openLibrary(() => []);
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText("Search notes"), "kinetochore");

    expect(
      await screen.findByText(
        "No notes match “kinetochore”. Try other words, or clear the search to see every note.",
      ),
    ).toBeTruthy();
    expect(listed()).toEqual([]);
  });

  it("only searches typed text: a blank query lists every note without asking", async () => {
    const notes = openLibrary(() => []);
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText("Search notes"), "   ");

    expect(listed()).toEqual(["Mitosis", "Cell biology", "Osmosis"]);
    expect(notes.argsOf("search_notes")).toEqual([]);
  });

  it("shows the newest query's answer even when an older one arrives last", async () => {
    const first = deferred<NoteSummary[]>();
    const second = deferred<NoteSummary[]>();
    openLibrary(({ query }) => (query === "o" ? first.promise : second.promise));
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText("Search notes"), "os");
    expect(screen.getByText("Searching…")).toBeTruthy();
    await act(async () => second.resolve([OSMOSIS]));
    await act(async () => first.resolve([MITOSIS, CELLS, OSMOSIS]));

    expect(listed()).toEqual(["Osmosis"]);
  });

  it("explains a failed search and keeps the field", async () => {
    openLibrary(() => refuse("failed", "Synapse couldn't search your notes."));
    const user = userEvent.setup();

    await user.type(await screen.findByLabelText("Search notes"), "x");

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Synapse couldn't search your notes.",
    );
    expect((screen.getByLabelText("Search notes") as HTMLInputElement).value).toBe("x");
  });

  it("has no search field while the library is empty", async () => {
    openNotes({ get_notes: () => [] });

    expect(await screen.findByText("No notes yet. Choose New note to write one.")).toBeTruthy();
    expect(screen.queryByLabelText("Search notes")).toBeNull();
  });

  it("isn't an unsaved change: leaving asks nothing", async () => {
    fakeBackend({ search_notes: () => [MITOSIS] });
    render(<App />);
    const user = userEvent.setup();
    const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));
    await user.click(sidebar.getByRole("button", { name: "Notes" }));
    await user.type(await screen.findByLabelText("Search notes"), "mito");
    await screen.findByText("1 note matches.");

    await user.click(sidebar.getByRole("button", { name: "Study Desk" }));

    const question = screen.getByRole("alertdialog", { hidden: true }) as HTMLDialogElement;
    expect(question.open).toBe(false);
    expect(screen.queryByLabelText("Search notes")).toBeNull();
  });
});

describe("writing a card from a note", () => {
  const BIOLOGY: Deck = { id: 1, name: "Biology", description: null, isSample: false, dueCount: 0 };
  const CHEMISTRY: Deck = { id: 2, name: "Chemistry", description: null, isSample: false, dueCount: 0 };
  const SAMPLE: Deck = { id: 9, name: "Sample deck", description: null, isSample: true, dueCount: 3 };

  async function openCardForm(user: UserEvent) {
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await user.click(await screen.findByRole("button", { name: "Create card from note" }));
    return (await screen.findByLabelText("Deck")) as HTMLSelectElement;
  }

  function choices(select: HTMLSelectElement) {
    return within(select)
      .getAllByRole("option")
      .map((option) => option.textContent);
  }

  it("offers only the user's own active decks, and asks for one before saving", async () => {
    const notes = openNotes({ get_decks: () => [SAMPLE, BIOLOGY] });
    const user = userEvent.setup();
    const deck = await openCardForm(user);
    expect(choices(deck)).toEqual(["Choose a deck", "Biology"]);

    await user.type(screen.getByLabelText("Front"), "Name the phases.");
    await user.type(screen.getByLabelText("Back"), "Prophase to telophase.");
    await user.click(screen.getByRole("button", { name: "Save card" }));

    expect(await screen.findByText("Choose a deck for this card.")).toBeTruthy();
    expect(document.activeElement).toBe(deck);
    expect(notes.argsOf("create_flashcard")).toEqual([]);
  });

  it("shows a refusal about the deck under the deck list, and refreshes the choices", async () => {
    // Biology is archived after the form opened, so Rust refuses the card.
    let archived = false;
    const notes = openNotes({
      get_decks: () => (archived ? [CHEMISTRY] : [BIOLOGY, CHEMISTRY]),
      create_flashcard: () => {
        archived = true;
        return refuse("stale", "That deck is archived, so it can't take new cards.");
      },
    });
    const user = userEvent.setup();
    const deck = await openCardForm(user);
    await user.selectOptions(deck, "Biology");
    await user.type(screen.getByLabelText("Front"), "Name the phases.");
    await user.type(screen.getByLabelText("Back"), "Prophase to telophase.");

    await user.click(screen.getByRole("button", { name: "Save card" }));

    expect(await screen.findByText("That deck is archived, so it can't take new cards.")).toBeTruthy();
    expect(document.activeElement).toBe(deck);
    await vi.waitFor(() => expect(choices(deck)).toEqual(["Choose a deck", "Chemistry"]));
    expect(deck.value).toBe("");
    // What was written is kept.
    expect((screen.getByLabelText("Front") as HTMLTextAreaElement).value).toBe("Name the phases.");
    expect(notes.argsOf("create_flashcard")).toEqual([
      { deckId: 1, front: "Name the phases.", back: "Prophase to telophase." },
    ]);
  });
});
