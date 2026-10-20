import { emit } from "@tauri-apps/api/event";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import { fakeBackend, keptSource } from "./test/backend";

function sidebar() {
  return within(screen.getByRole("navigation", { name: "Synapse" }));
}

/** The discard question. It's always in the page; `open` says whether it's showing. */
function question() {
  return screen.getByRole("alertdialog", { hidden: true }) as HTMLDialogElement;
}

/** Presses one of the question's buttons, then lets its "close" event arrive. */
async function answer(user: UserEvent, name: "Discard changes" | "Keep editing") {
  await user.click(within(question()).getByRole("button", { name, hidden: true }));
  await act(() => new Promise((resolve) => setTimeout(resolve)));
}

async function openDeck(user: UserEvent) {
  await user.click((await screen.findAllByRole("button", { name: "Open deck" }))[0]);
}

async function openNote(user: UserEvent) {
  await user.click(sidebar().getByRole("button", { name: "Notes" }));
  await user.click(await screen.findByRole("button", { name: "Open note" }));
}

/** Each form, opened from launch, and a field in it to type into. */
const FORMS: { form: string; open: (user: UserEvent) => Promise<void>; field: string }[] = [
  {
    form: "creating a deck",
    open: async (user) => user.click(await screen.findByRole("button", { name: "Create deck" })),
    field: "Deck name",
  },
  {
    form: "renaming a deck",
    open: async (user) => {
      await openDeck(user);
      await user.click(await screen.findByRole("button", { name: "Rename deck" }));
    },
    field: "Deck name",
  },
  {
    form: "adding a card",
    open: async (user) => {
      await openDeck(user);
      await user.click(await screen.findByRole("button", { name: "Add card" }));
    },
    field: "Front",
  },
  {
    form: "editing a card",
    open: async (user) => {
      await openDeck(user);
      await user.click(await screen.findByRole("button", { name: "Edit card" }));
    },
    field: "Back",
  },
  {
    form: "writing a note",
    open: async (user) => {
      await user.click(sidebar().getByRole("button", { name: "Notes" }));
      await user.click(await screen.findByRole("button", { name: "New note" }));
    },
    field: "Title",
  },
  {
    form: "editing a note",
    open: async (user) => {
      await openNote(user);
      await user.click(await screen.findByRole("button", { name: "Edit note" }));
    },
    field: "Note",
  },
  {
    form: "writing a card from a note",
    open: async (user) => {
      await openNote(user);
      await user.click(await screen.findByRole("button", { name: "Create card from note" }));
    },
    field: "Front",
  },
  {
    form: "checking suggested cards",
    open: async (user) => {
      await suggest(user);
    },
    field: "Front",
  },
];

/** Opens the note, and has the fake model suggest its one card. */
async function suggest(user: UserEvent) {
  await openNote(user);
  await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
  await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
  await screen.findByRole("group", { name: "Card 1" });
}

describe("leaving a form with unsaved changes", () => {
  it.each(FORMS)("asks first when $form", async ({ open, field }) => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await open(user);
    await user.type(await screen.findByLabelText(field), " more");

    await user.click(sidebar().getByRole("button", { name: "Archived decks" }));

    expect(question().open).toBe(true);
    expect(within(question()).getByRole("heading", { name: "Leave without saving?", hidden: true })).toBeTruthy();
    expect(document.activeElement).toBe(question());
  });

  it("asks first when cards were suggested, even if none was changed", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await suggest(user);

    await user.click(sidebar().getByRole("button", { name: "Study Desk" }));

    expect(question().open).toBe(true);
  });

  it("asks first when a file's text was imported, even if none was changed", async () => {
    fakeBackend({
      import_source: () =>
        keptSource({
          status: "read",
          fileName: "Lecture 3.pdf",
          kind: "pdf",
          title: "Lecture 3",
          body: "Mitosis is cell division.",
          pages: 1,
          pagesWithoutText: [],
        }),
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    await screen.findByRole("heading", { name: "New note from Lecture 3.pdf" });

    await user.click(sidebar().getByRole("button", { name: "Study Desk" }));

    expect(question().open).toBe(true);
  });

  it("leaves without asking while only a model is being chosen", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await openNote(user);
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await screen.findByLabelText("Model");

    await user.click(sidebar().getByRole("button", { name: "Archived decks" }));

    expect(question().open).toBe(false);
    expect(await screen.findByRole("heading", { name: "Archived decks" })).toBeTruthy();
  });

  it("leaves an untouched form without asking", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Create deck" }));

    await user.click(sidebar().getByRole("button", { name: "Notes" }));

    expect(question().open).toBe(false);
    expect(await screen.findByRole("heading", { name: "Notes" })).toBeTruthy();
  });

  it("doesn't ask once the text is back to how the form opened", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await openDeck(user);
    await user.click(await screen.findByRole("button", { name: "Rename deck" }));
    const name = await screen.findByLabelText("Deck name");
    await user.type(name, "s");
    await user.type(name, "{Backspace}");

    await user.click(sidebar().getByRole("button", { name: "Notes" }));

    expect(question().open).toBe(false);
    expect(await screen.findByRole("heading", { name: "Notes" })).toBeTruthy();
  });

  it("doesn't ask once the form is saved", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await openDeck(user);
    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.type(await screen.findByLabelText("Front"), "What is DNA?");
    await user.type(screen.getByLabelText("Back"), "The molecule that carries genes.");
    await user.click(screen.getByRole("button", { name: "Save card" }));
    expect(await screen.findByText("Card added.")).toBeTruthy();

    await user.click(sidebar().getByRole("button", { name: "Notes" }));

    expect(question().open).toBe(false);
    expect(await screen.findByRole("heading", { name: "Notes" })).toBeTruthy();
  });

  it("stays on the form, with its text, after Keep editing", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Create deck" }));
    await user.type(await screen.findByLabelText("Deck name"), "Chemistry");
    const notes = sidebar().getByRole("button", { name: "Notes" });

    await user.click(notes);
    await answer(user, "Keep editing");

    expect(question().open).toBe(false);
    expect((screen.getByLabelText("Deck name") as HTMLInputElement).value).toBe("Chemistry");
    // Focus goes back to the sidebar button that was pressed.
    expect(document.activeElement).toBe(notes);

    // Pressing it again asks again.
    await user.click(notes);
    expect(question().open).toBe(true);
  });

  it("treats Escape as Keep editing", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Create deck" }));
    await user.type(await screen.findByLabelText("Deck name"), "Chemistry");

    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.keyboard("{Escape}");
    await act(() => new Promise((resolve) => setTimeout(resolve)));

    expect(question().open).toBe(false);
    expect((screen.getByLabelText("Deck name") as HTMLInputElement).value).toBe("Chemistry");
  });

  it("goes where the user was heading after Discard changes", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Create deck" }));
    await user.type(await screen.findByLabelText("Deck name"), "Chemistry");

    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await answer(user, "Discard changes");

    expect(await screen.findByRole("heading", { name: "Notes" })).toBeTruthy();
    expect(screen.queryByLabelText("Deck name")).toBeNull();

    // Nothing is unsaved any more, so the sidebar works without asking.
    await user.click(sidebar().getByRole("button", { name: "Study Desk" }));
    expect(question().open).toBe(false);
    expect(await screen.findByRole("button", { name: "Create deck" })).toBeTruthy();
  });
});

describe("closing the window with unsaved changes", () => {
  /** Waits until the app is holding the window's close, then asks to close it. */
  async function requestClose(calls: string[]) {
    await waitFor(() => expect(calls).toContain("plugin:event|listen"));
    await act(() => emit("tauri://close-requested"));
  }

  it("doesn't hold the window when nothing is unsaved", async () => {
    const { calls } = fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Create deck" }));
    await screen.findByLabelText("Deck name");

    // With no listener, Tauri closes the window by itself, as before.
    expect(calls).not.toContain("plugin:event|listen");
  });

  it("asks first, and closes the window after Discard changes", async () => {
    const { calls } = fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Create deck" }));
    await user.type(await screen.findByLabelText("Deck name"), "Chemistry");

    await requestClose(calls);

    expect(question().open).toBe(true);
    expect(
      within(question()).getByRole("heading", { name: "Close Synapse without saving?", hidden: true })
    ).toBeTruthy();
    expect(calls).not.toContain("plugin:window|destroy");

    await answer(user, "Discard changes");

    await waitFor(() => expect(calls).toContain("plugin:window|destroy"));
  });

  it("keeps the window open after Keep editing", async () => {
    const { calls } = fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Create deck" }));
    const name = await screen.findByLabelText("Deck name");
    await user.type(name, "Chemistry");

    await requestClose(calls);
    await answer(user, "Keep editing");

    expect(question().open).toBe(false);
    expect(calls).not.toContain("plugin:window|destroy");
    expect((name as HTMLInputElement).value).toBe("Chemistry");
    // Focus goes back to where the user was typing.
    expect(document.activeElement).toBe(name);
  });
});
