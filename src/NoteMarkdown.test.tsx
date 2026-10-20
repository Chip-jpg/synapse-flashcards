import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Note } from "./api";
import { NotesArea } from "./Notes";
import { CREATED, fakeBackend } from "./test/backend";

const BODY = [
  "# Cell division",
  "",
  "Mitosis has **four** phases, see [the notes](https://example.org/mitosis).",
  "",
  "- Prophase",
  "- [x] Metaphase",
  "",
  "| Phase | Chromosomes |",
  "| --- | --- |",
  "| Anaphase | pulled apart |",
  "",
  '<img src="https://example.org/x.png" onerror="alert(1)"> ![A cell](https://example.org/cell.png)',
  "",
  "```",
  "*not formatted*",
  "```",
].join("\n");

function note(body: string): Note {
  return { id: 5, title: "Mitosis", body, createdAt: CREATED, updatedAt: CREATED };
}

async function openNote(body: string) {
  const backend = fakeBackend({ get_note: () => note(body) });
  render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "Open note" }));
  await screen.findByRole("heading", { name: "Mitosis" });
  return { backend, user };
}

beforeEach(() => {
  localStorage.clear();
});

describe("reading a note written in Markdown", () => {
  it("shows it formatted, with no HTML read and nothing to open or load", async () => {
    await openNote(BODY);

    expect(screen.getByRole("button", { name: "Formatted" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
    // Below the note's own title, the h2.
    expect(screen.getByRole("heading", { level: 3, name: "Cell division" })).toBeTruthy();
    expect(screen.getByText("four").tagName).toBe("STRONG");
    const items = within(screen.getAllByRole("list")[0]).getAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual(["Prophase", "☑Metaphase"]);
    expect(screen.getByRole("img", { name: "Done" })).toBeTruthy();
    expect(screen.getByRole("cell", { name: "pulled apart" })).toBeTruthy();
    expect(screen.getByText("*not formatted*").tagName).toBe("CODE");

    // The link's address is shown as text; it isn't a link.
    expect(screen.getByText("the notes").parentElement?.textContent).toBe(
      "the notes (https://example.org/mitosis)",
    );
    expect(screen.queryAllByRole("link")).toEqual([]);
    // HTML stays text, and the picture is only described.
    const article = screen.getByRole("heading", { name: "Mitosis" }).closest("article")!;
    expect(article.querySelector("img, a, script")).toBeNull();
    expect(screen.getByText(/<img src=/)).toBeTruthy();
    expect(screen.getByText("Picture: A cell")).toBeTruthy();
  });

  it("shows the stored text as it is with Plain text, and remembers the choice", async () => {
    const { user } = await openNote(BODY);

    await user.click(screen.getByRole("button", { name: "Plain text" }));

    expect(screen.getByRole("button", { name: "Plain text" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
    expect(screen.queryByRole("heading", { name: "Cell division" })).toBeNull();
    const plain = document.querySelector("p.note-body")!;
    expect(plain.textContent).toBe(BODY);
    expect(localStorage.getItem("synapse.noteFormat")).toBe("plain");

    // Back in the library and in again: still plain.
    await user.click(screen.getByRole("button", { name: /^Back to/ }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    expect(
      (await screen.findByRole("button", { name: "Plain text" })).getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("offers no choice for a note with no Markdown in it, shown exactly as typed", async () => {
    const body = "Prophase, metaphase.\n    Then anaphase.\n\n\nTelophase, 2 * 3.";
    await openNote(body);

    expect(screen.queryByRole("group", { name: "Show the note" })).toBeNull();
    expect(document.querySelector("p.note-body")!.textContent).toBe(body);
  });

  it("edits the stored text, not the formatted one", async () => {
    const { backend, user } = await openNote(BODY);

    await user.click(screen.getByRole("button", { name: "Edit note" }));

    expect((screen.getByLabelText("Note") as HTMLTextAreaElement).value).toBe(BODY);
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(backend.argsOf("update_note")).toEqual([
      { noteId: 5, title: "Mitosis", body: BODY, pictures: [] },
    ]);
  });
});
