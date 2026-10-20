import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { FileImport, ImportedFile } from "./api";
import { NotesArea } from "./Notes";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const LECTURE: ImportedFile = {
  status: "read",
  fileName: "Lecture 3.pdf",
  kind: "pdf",
  title: "Lecture 3",
  body: "Mitosis is cell division.\n\nMetaphase: chromosomes line up.",
  pages: 3,
  pagesWithoutText: [2],
};

function openNotes(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({
    create_note: (args) => {
      const { title, body } = args as { title: string; body: string };
      return { id: 9, title, body, createdAt: CREATED, updatedAt: CREATED };
    },
    get_note: () => ({
      id: 9,
      title: "Lecture 3",
      body: LECTURE.body,
      createdAt: CREATED,
      updatedAt: CREATED,
    }),
    ...handlers,
  });
  render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} />);
  return { ...backend, user: userEvent.setup() };
}

describe("importing a file as a note", () => {
  it("opens the file's text as a new note to check, and saves what the user keeps", async () => {
    const { user, argsOf } = openNotes({ import_file: () => LECTURE });
    await user.click(await screen.findByRole("button", { name: "Import file" }));

    expect(await screen.findByRole("heading", { name: "New note from Lecture 3.pdf" })).toBeTruthy();
    const summary = screen.getByText(/^Synapse read the text of Lecture 3\.pdf \(3 pages\)\./);
    expect(document.activeElement).toBe(summary);
    expect(summary.textContent).toContain("The file itself isn't kept.");
    expect(
      screen.getByText(
        "Page 2 had no text Synapse could read. They may be scanned images, so check whether anything is missing."
      )
    ).toBeTruthy();
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("Lecture 3");
    const body = screen.getByLabelText("Note") as HTMLTextAreaElement;
    expect(body.value).toBe(LECTURE.body);

    await user.type(body, "\nAnaphase: chromatids separate.");
    await user.click(screen.getByRole("button", { name: "Save note" }));

    expect(await screen.findByText("Note saved.")).toBeTruthy();
    expect(argsOf("create_note")).toEqual([
      {
        title: "Lecture 3",
        body: `${LECTURE.body}\nAnaphase: chromatids separate.`,
        pictures: [],
      },
    ]);
  });

  it("counts slides for a presentation, and names several pages without text", async () => {
    const { user } = openNotes({
      import_file: () => ({
        ...LECTURE,
        fileName: "Talk.pptx",
        kind: "powerPoint",
        pages: 12,
        pagesWithoutText: [],
      }),
    });
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    expect(await screen.findByText(/^Synapse read the text of Talk\.pptx \(12 slides\)\./)).toBeTruthy();
    expect(screen.queryByText(/had no text Synapse could read/)).toBeNull();
  });

  it("lists the pages of a PDF that had no text", async () => {
    const { user } = openNotes({
      import_file: () => ({ ...LECTURE, pages: 9, pagesWithoutText: [2, 5, 9] }),
    });
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    expect(await screen.findByText(/^Pages 2, 5, and 9 had no text Synapse could read\./)).toBeTruthy();
  });

  it("warns while the text is too long for a note, until it's shortened", async () => {
    const { user } = openNotes({
      import_file: () => ({ ...LECTURE, body: "x".repeat(100_001), pages: null, kind: "text" }),
    });
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    expect(
      await screen.findByText(/^This text is 100,001 characters long, but a note can hold 100,000\./)
    ).toBeTruthy();

    const body = screen.getByLabelText("Note");
    await user.clear(body);
    await user.type(body, "Just the part I need.");

    expect(screen.queryByText(/characters long, but a note can hold/)).toBeNull();
  });

  it("says when the text is too long to suggest cards from whole", async () => {
    const { user } = openNotes({
      import_file: () => ({ ...LECTURE, body: "x".repeat(12_001), pages: null, kind: "text" }),
    });
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    expect(
      await screen.findByText(/^At 12,001 characters, .* Suggest cards will offer the note in parts/)
    ).toBeTruthy();

    const body = screen.getByLabelText("Note");
    await user.clear(body);
    await user.type(body, "Just the part I need.");
    expect(screen.queryByText(/will offer the note in parts/)).toBeNull();
  });

  it("changes nothing when the file window is closed", async () => {
    const { user, argsOf } = openNotes({ import_file: (): FileImport => ({ status: "cancelled" }) });
    const button = await screen.findByRole("button", { name: "Import file" });
    await user.click(button);

    expect(button.getAttribute("aria-disabled")).toBe("false");
    expect(screen.getByRole("heading", { name: "Notes" })).toBeTruthy();
    expect(argsOf("create_note")).toEqual([]);
  });

  it("says why a file can't be imported, and stays in the library", async () => {
    const { user } = openNotes({
      import_file: () => refuse("invalid", "That file has no text Synapse can import."),
    });
    await user.click(await screen.findByRole("button", { name: "Import file" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "That file has no text Synapse can import."
    );
    expect(screen.getByRole("heading", { name: "Notes" })).toBeTruthy();
  });

  it("opens one file window however often Import file is pressed", async () => {
    const answer = deferred<FileImport>();
    const { user, argsOf } = openNotes({ import_file: () => answer.promise });
    const button = await screen.findByRole("button", { name: "Import file" });

    await user.click(button);
    await user.click(button);

    expect(argsOf("import_file")).toHaveLength(1);
    expect(screen.getByText("Choose a file. Synapse then reads its text…")).toBeTruthy();
  });

  it("goes back to the library on Cancel, with focus on Import file", async () => {
    const { user, argsOf } = openNotes({ import_file: () => LECTURE });
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));

    expect(document.activeElement).toBe(await screen.findByRole("button", { name: "Import file" }));
    expect(argsOf("create_note")).toEqual([]);
  });
});
