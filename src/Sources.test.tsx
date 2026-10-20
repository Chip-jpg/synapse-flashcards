import { act, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { Note, NoteSummary, Source, SourcePage } from "./api";
import { CREATED, deferred, fakeBackend, refuse, source, type Handler } from "./test/backend";

const LATER = "2026-10-02T09:00:00Z";

const PAGES: SourcePage[] = [
  { number: 1, text: "Mitosis is cell division." },
  { number: 2, text: "" },
  { number: 3, text: "Metaphase: chromosomes line up." },
];

function sidebar() {
  return within(screen.getByRole("navigation", { name: "Synapse" }));
}

/**
 * The app with one kept source ("Lecture 3", a PDF of three pages, the
 * second without text) and nothing deleted. Saving a note from it gives
 * note 9.
 */
function openApp(handlers: Record<string, Handler> = {}) {
  const lecture = source();
  const backend = fakeBackend({
    get_sources: () => [lecture],
    get_deleted_sources: () => [],
    get_source: () => lecture,
    get_source_pages: () => PAGES,
    get_source_notes: () => [],
    create_note_from_source: (args) => {
      const { title, body } = args as { title: string; body: string };
      return { id: 9, title, body, createdAt: CREATED, updatedAt: CREATED } satisfies Note;
    },
    ...handlers,
  });
  render(<App />);
  return { ...backend, user: userEvent.setup() };
}

async function openSources(user: UserEvent) {
  await user.click(sidebar().getByRole("button", { name: "Sources" }));
  await screen.findByRole("heading", { name: "Sources" });
}

async function openLecture(user: UserEvent) {
  await openSources(user);
  await user.click(screen.getByRole("button", { name: "Open source" }));
  return screen.findByRole("heading", { name: "Lecture 3", level: 2 });
}

describe("the Sources screen", () => {
  it("lists each kept source with what it is", async () => {
    const { user } = openApp({
      get_deleted_sources: () => [
        source({
          id: 4,
          title: "Old slides",
          kind: "powerPoint",
          pageCount: 12,
          asset: { ...source().asset, size: 840_000 },
          deletedAt: LATER,
        }),
      ],
    });
    await openSources(user);

    const lecture = screen.getByRole("article", { name: "Lecture 3" });
    expect(within(lecture).getByText(/^PDF · 3 pages · 2\.4 MB · Added /)).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Deleted sources" })).toBeTruthy();
    expect(
      screen.getByText(/^PowerPoint presentation · 12 slides · 840 KB · Deleted /),
    ).toBeTruthy();
  });

  it("keeps a new source and opens it at its first page", async () => {
    const kept = source({ id: 6, title: "Week 2", fileName: "week2.pdf" });
    const { user, argsOf } = openApp({
      import_source: () => ({
        status: "imported",
        source: kept,
        text: {
          kind: "pdf",
          title: "Week 2",
          body: "Mitosis is cell division.",
          pages: 3,
          pagesWithoutText: [2],
        },
      }),
      get_source: () => kept,
    });
    await openSources(user);
    await user.click(screen.getByRole("button", { name: "Add source" }));

    const notice = await screen.findByText(/^Kept week2\.pdf\./);
    expect(document.activeElement).toBe(notice);
    expect(argsOf("get_source_pages")).toEqual([{ sourceId: 6 }]);
    expect(screen.getByRole("heading", { name: "Page 1 of 3" })).toBeTruthy();
    expect(screen.getByText("Mitosis is cell division.")).toBeTruthy();
  });

  it("leaves the list as it was when the file window is closed, and says why a file isn't kept", async () => {
    let answer: Handler = () => ({ status: "cancelled" });
    const { user, argsOf } = openApp({ import_source: (args) => answer(args) });
    await openSources(user);
    await user.click(screen.getByRole("button", { name: "Add source" }));
    expect(argsOf("import_source")).toHaveLength(1);
    expect(screen.getByRole("article", { name: "Lecture 3" })).toBeTruthy();

    answer = () => refuse("invalid", "That file is too large to keep: the limit is 50 MB.");
    await user.click(screen.getByRole("button", { name: "Add source" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "That file is too large to keep: the limit is 50 MB.",
    );
  });

  it("opens one file window however often Add source is pressed", async () => {
    const waiting = deferred();
    const { user, argsOf } = openApp({ import_source: () => waiting.promise });
    await openSources(user);
    const add = screen.getByRole("button", { name: "Add source" });
    await user.click(add);
    await user.click(add);
    expect(argsOf("import_source")).toHaveLength(1);
    expect(
      screen.getByText("Choose a file. Synapse then keeps it and reads its text…"),
    ).toBeTruthy();
  });

  it("shows a source's pages one at a time, and says when a page has no text", async () => {
    const { user } = openApp();
    await openLecture(user);

    expect(screen.getByText("Mitosis is cell division.")).toBeTruthy();
    const previous = screen.getByRole("button", { name: "Previous page" });
    expect(previous.getAttribute("aria-disabled")).toBe("true");

    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByRole("heading", { name: "Page 2 of 3" })).toBeTruthy();
    expect(screen.getByText(/^Synapse found no text on this page\./)).toBeTruthy();

    await user.selectOptions(screen.getByLabelText("Go to page"), "Page 3");
    expect(screen.getByText("Metaphase: chromosomes line up.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Next page" }).getAttribute("aria-disabled")).toBe(
      "true",
    );
  });

  it("calls a presentation's pages slides, and shows a one-page document whole", async () => {
    const slides = source({ kind: "powerPoint", fileName: "Talk.pptx", pageCount: 2 });
    const { user, argsOf } = openApp({
      get_source: () => slides,
      get_source_pages: () => [
        { number: 1, text: "Slide 1\nCells" },
        { number: 2, text: "Slide 2\nDivision" },
      ],
    });
    await openLecture(user);
    expect(screen.getByRole("heading", { name: "Slide 1 of 2" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Next slide" }));
    expect(screen.getByText(/Division/)).toBeTruthy();
    expect(argsOf("get_source")).toEqual([{ sourceId: 3 }]);
  });

  it("makes a note from a source's pages, saved with its link, and back on Cancel", async () => {
    const { user, argsOf } = openApp();
    await openLecture(user);

    await user.click(screen.getByRole("button", { name: "Make a note" }));
    expect(
      await screen.findByRole("heading", { name: "New note from Lecture 3.pdf" }),
    ).toBeTruthy();
    expect(
      screen.getByText(/^This is the text of Lecture 3\.pdf \(3 pages\), kept under Sources\./),
    ).toBeTruthy();
    expect(screen.getByText(/^Page 2 had no text Synapse could read\./)).toBeTruthy();
    const body = screen.getByLabelText("Note") as HTMLTextAreaElement;
    expect(body.value).toBe("Mitosis is cell division.\n\nMetaphase: chromosomes line up.");

    // Cancel goes back to the source.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await screen.findByRole("heading", { name: "Lecture 3", level: 2 });

    await user.click(screen.getByRole("button", { name: "Make a note" }));
    await user.click(await screen.findByRole("button", { name: "Save note" }));
    expect(await screen.findByText("Note saved.")).toBeTruthy();
    expect(argsOf("create_note_from_source")).toEqual([
      {
        sourceId: 3,
        title: "Lecture 3",
        body: "Mitosis is cell division.\n\nMetaphase: chromosomes line up.",
      },
    ]);
  });

  it("lists the notes written from a source, and opens one with the way back", async () => {
    const written: NoteSummary = {
      id: 9,
      title: "Mitosis notes",
      createdAt: CREATED,
      updatedAt: LATER,
    };
    const { user } = openApp({
      get_source_notes: () => [written],
      get_note: () => ({ ...written, body: "Mitosis is cell division." }),
      get_note_source: () => 3,
    });
    await openLecture(user);

    const listed = screen.getByRole("article", { name: "Mitosis notes" });
    expect(within(listed).getByText(/^Edited /)).toBeTruthy();
    await user.click(within(listed).getByRole("button", { name: "Open note" }));

    expect(await screen.findByRole("heading", { name: "Mitosis notes" })).toBeTruthy();
    expect(await screen.findByText(/^Written from Lecture 3\./)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Back to source" }));
    expect(await screen.findByRole("heading", { name: "Lecture 3", level: 2 })).toBeTruthy();
  });

  it("renames a source, and points at the title when it's refused", async () => {
    let rename: Handler = () => refuse("invalid", "Give the source a title.", "title");
    const { user, argsOf } = openApp({ rename_source: (args) => rename(args) });
    await openLecture(user);

    await user.click(screen.getByRole("button", { name: "Rename" }));
    const title = screen.getByLabelText("Title");
    expect(document.activeElement).toBe(title);
    await user.clear(title);
    await user.click(screen.getByRole("button", { name: "Save title" }));
    expect(await screen.findByText("Give the source a title.")).toBeTruthy();
    expect(document.activeElement).toBe(title);

    rename = (args) => source({ title: String(args.title).trim(), updatedAt: LATER });
    await user.type(title, "Cell division ");
    await user.click(screen.getByRole("button", { name: "Save title" }));
    expect(await screen.findByText("Source renamed.")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Cell division", level: 2 })).toBeTruthy();
    expect(argsOf("rename_source")[1]).toEqual({ sourceId: 3, title: "Cell division " });
  });

  it("deletes a source after asking, and restores it", async () => {
    const lecture = source();
    let deleted: Source[] = [];
    let active: Source[] = [lecture];
    const { user, argsOf } = openApp({
      get_sources: () => active,
      get_deleted_sources: () => deleted,
      delete_source: () => {
        active = [];
        deleted = [{ ...lecture, deletedAt: LATER }];
        return null;
      },
      restore_source: () => {
        active = [lecture];
        deleted = [];
        return null;
      },
    });
    await openLecture(user);

    await user.click(screen.getByRole("button", { name: "Delete source" }));
    expect(document.activeElement?.textContent).toMatch(/^Delete this source\?/);
    await user.click(screen.getByRole("button", { name: "Keep source" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Delete source" }));

    await user.click(screen.getByRole("button", { name: "Delete source" }));
    await user.click(screen.getByRole("button", { name: "Delete source" }));
    const notice = await screen.findByText(/^Lecture 3 deleted\./);
    expect(document.activeElement).toBe(notice);
    expect(argsOf("delete_source")).toEqual([{ sourceId: 3 }]);

    await user.click(screen.getByRole("button", { name: "Restore source" }));
    expect(await screen.findByText("Lecture 3 restored. It's back in your sources.")).toBeTruthy();
    expect(screen.getByRole("article", { name: "Lecture 3" })).toBeTruthy();
  });

  it("opens the original or saves a copy, and says what happened", async () => {
    let open: Handler = () => null;
    const { user, argsOf } = openApp({
      open_source_original: (args) => open(args),
      save_source_copy: () => ({ status: "saved", fileName: "Lecture 3.pdf" }),
    });
    await openLecture(user);

    await user.click(screen.getByRole("button", { name: "Open original" }));
    expect(await screen.findByText(/^Opened a copy of Lecture 3\.pdf\./)).toBeTruthy();
    expect(argsOf("open_source_original")).toEqual([{ sourceId: 3 }]);

    await user.click(screen.getByRole("button", { name: "Save a copy" }));
    expect(await screen.findByText("Saved a copy as Lecture 3.pdf.")).toBeTruthy();

    open = () =>
      refuse("failed", "Synapse couldn't open the original. Save a copy instead, and open that.");
    await user.click(screen.getByRole("button", { name: "Open original" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/^Synapse couldn't open/);
  });
});

describe("a note written from a source", () => {
  it("says which source, and opens it", async () => {
    const { user } = openApp({ get_note_source: () => 3 });
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));

    await screen.findByText(/^Written from Lecture 3\./);
    await user.click(screen.getByRole("button", { name: "Open source" }));
    expect(await screen.findByRole("heading", { name: "Lecture 3", level: 2 })).toBeTruthy();
  });

  it("says when its source was deleted", async () => {
    const { user } = openApp({
      get_note_source: () => 3,
      get_source: () => refuse("stale", "This source or note was deleted."),
    });
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    expect(await screen.findByText(/^Written from a source that's been deleted\./)).toBeTruthy();
  });

  it("says nothing for a typed note", async () => {
    const { user, argsOf } = openApp();
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await screen.findByRole("heading", { name: "Mitosis" });
    expect(argsOf("get_note_source")).toEqual([{ noteId: 5 }]);
    expect(argsOf("get_source")).toEqual([]);
    expect(screen.queryByText(/Written from/)).toBeNull();
  });
});

const QUOTE = "Prophase, metaphase, anaphase, telophase.";

/** The Mitosis note as written from Lecture 3, its quotes on page 2. */
const FROM_LECTURE: Record<string, Handler> = {
  get_note_source: () => 3,
  locate_quotes: (args) => ({
    sourceId: 3,
    title: "Lecture 3",
    kind: "pdf",
    pageCount: 3,
    spots: (args.quotes as string[]).map(() => ({ page: 2, lastPage: 2 })),
  }),
  get_quoted_page: (args) =>
    args.number === 2
      ? [
          { text: "The phases: ", hit: false },
          { text: QUOTE, hit: true },
        ]
      : [{ text: `Page ${String(args.number)} text.`, hit: false }],
};

describe("the page a suggestion or question came from", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.spyOn(Math, "random").mockReturnValue(0);
  });

  async function openNote(user: UserEvent) {
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
  }

  it("is shown on a suggested card, opening that page with the quote marked", async () => {
    const { user, argsOf } = openApp(FROM_LECTURE);
    await openNote(user);
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));

    const link = await screen.findByRole("button", { name: "From page 2 of Lecture 3" });
    expect(argsOf("locate_quotes")).toEqual([{ noteId: 5, quotes: [QUOTE] }]);
    const front = screen.getByLabelText("Front") as HTMLTextAreaElement;
    await user.type(front, " Name them.");

    await user.click(link);
    const page = screen.getByRole("dialog", { name: "Page 2 of 3" });
    expect(document.activeElement).toBe(page);
    expect((await within(page).findByText(QUOTE)).tagName).toBe("MARK");
    expect(argsOf("get_quoted_page")).toEqual([{ sourceId: 3, number: 2, quote: QUOTE }]);

    await user.click(within(page).getByRole("button", { name: "Next page" }));
    expect(await within(page).findByText("Page 3 text.")).toBeTruthy();
    expect(within(page).getByRole("heading", { name: "Page 3 of 3" })).toBeTruthy();

    await user.keyboard("{Escape}");
    await act(() => new Promise((resolve) => setTimeout(resolve)));
    expect(screen.queryByRole("dialog", { name: /^Page/ })).toBeNull();
    // The draft is as the user left it.
    expect(front.value).toBe("What are the phases of mitosis? Name them.");
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "From page 2 of Lecture 3" }),
    );
  });

  it("isn't shown when the note has no source, or the quote isn't in it", async () => {
    const { user, argsOf } = openApp({
      ...FROM_LECTURE,
      locate_quotes: () => ({
        sourceId: 3,
        title: "Lecture 3",
        kind: "pdf",
        pageCount: 3,
        spots: [null],
      }),
    });
    await openNote(user);
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await screen.findByLabelText("Front");
    await act(() => new Promise((resolve) => setTimeout(resolve)));
    expect(argsOf("locate_quotes")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: /^From page/ })).toBeNull();
  });

  it("is shown after a quiz answer, and Enter on the page doesn't move on", async () => {
    const { user } = openApp(FROM_LECTURE);
    await openNote(user);
    await user.click(await screen.findByRole("button", { name: "Quiz me" }));
    await user.click(await screen.findByRole("button", { name: "Start quiz" }));
    await screen.findByText("Question 1 of", { exact: false });
    await user.keyboard("4");

    await user.click(await screen.findByRole("button", { name: "From page 2 of Lecture 3" }));
    const page = screen.getByRole("dialog", { name: "Page 2 of 3" });
    await within(page).findByText(QUOTE);
    await user.keyboard("{Enter}");
    expect(screen.getByRole("dialog", { name: "Page 2 of 3" })).toBe(page);
    expect(screen.queryByText(/^You got/)).toBeNull();

    await user.click(within(page).getByRole("button", { name: "Close" }));
    await act(() => new Promise((resolve) => setTimeout(resolve)));
    await user.click(screen.getByRole("button", { name: /^(Next question|See results|Finish)/ }));
    expect(await screen.findByText(/^You got all 1 question right\./)).toBeTruthy();
  });
});
