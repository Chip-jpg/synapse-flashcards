import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Note, NoteSummary, SearchHit, TagSummary } from "./api";
import App from "./App";
import { NotesArea } from "./Notes";
import { CREATED, fakeBackend, keptSource, refuse, type Handler } from "./test/backend";
import { addTags, hasTag, sameTags, splitTags, tagsIn } from "./tagInput";

const LATER = "2026-10-02T09:00:00Z";

const MITOSIS: Note = {
  id: 5,
  title: "Mitosis",
  body: "Prophase, metaphase, anaphase, telophase.",
  createdAt: CREATED,
  updatedAt: CREATED,
  tags: ["bio::cells", "exam"],
};

function summary(note: Note): NoteSummary {
  const { id, title, createdAt, updatedAt, tags } = note;
  return { id, title, createdAt, updatedAt, tags };
}

const ATOMS: NoteSummary = {
  id: 6,
  title: "Atoms",
  createdAt: CREATED,
  updatedAt: CREATED,
  tags: ["chem"],
};
const PLAIN: NoteSummary = { id: 7, title: "Plain", createdAt: CREATED, updatedAt: CREATED };

const TAGS: TagSummary[] = [
  { id: 1, name: "bio::cells", cards: 3, notes: 1 },
  { id: 2, name: "chem", cards: 0, notes: 1 },
  { id: 3, name: "exam", cards: 1, notes: 1 },
];

function openNotes(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({
    get_notes: () => [summary(MITOSIS), ATOMS, PLAIN],
    get_note: () => MITOSIS,
    get_tags: () => TAGS,
    ...handlers,
  });
  render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
  return { ...backend, user: userEvent.setup() };
}

function chipNames(list: HTMLElement): string[] {
  return within(list)
    .getAllByRole("listitem")
    .map((item) => item.querySelector(".tag-name")?.textContent ?? "");
}

describe("tag helpers", () => {
  it("split typed text into words, keep each name once and match a tag or one below it", () => {
    expect(splitTags(" bio, exam  june,")).toEqual(["bio", "exam", "june"]);
    expect(addTags(["Bio"], ["bio", "exam", "EXAM"])).toEqual(["Bio", "exam"]);
    expect(sameTags(["a", "B"], ["b", "A"])).toBe(true);
    expect(sameTags(["a"], ["a", "b"])).toBe(false);
    expect(hasTag(["Bio::cells"], "bio")).toBe(true);
    expect(hasTag(["biology"], "bio")).toBe(false);
    expect(hasTag(undefined, "bio")).toBe(false);
    expect(tagsIn([{ tags: ["b", "A"] }, { tags: ["a"] }, {}])).toEqual([
      { name: "A", count: 2 },
      { name: "b", count: 1 },
    ]);
  });
});

describe("tags on a note", () => {
  it("are typed as chips, suggested from the tags in use, and saved with a new note", async () => {
    const { user, argsOf } = openNotes({
      create_note: (args) => ({
        id: 9,
        title: args.title,
        body: args.body,
        createdAt: CREATED,
        updatedAt: CREATED,
        tags: args.tags,
      }),
    });
    await user.click(await screen.findByRole("button", { name: "New note" }));
    await user.type(screen.getByLabelText("Title"), "Cells");
    await user.type(screen.getByLabelText("Note"), "They divide.");

    const field = screen.getByLabelText("Tags") as HTMLInputElement;
    await waitFor(() =>
      expect(
        [...(field.list?.querySelectorAll("option") ?? [])].map((option) => option.value),
      ).toEqual(["bio::cells", "chem", "exam"]),
    );
    // A space or comma ends a tag; Enter does too, without saving the form.
    await user.type(field, "bio, exam Bio{Enter}");
    const added = screen.getByRole("list", { name: "Tags added" });
    expect(chipNames(added)).toEqual(["bio", "exam"]);
    expect(field.value).toBe("");
    expect(argsOf("create_note")).toEqual([]);
    // Suggestions leave out the tags already added.
    expect([...(field.list?.querySelectorAll("option") ?? [])].map((o) => o.value)).toEqual([
      "bio::cells",
      "chem",
    ]);

    // Backspace in the empty box takes the last one off; × takes one off.
    await user.type(field, "{Backspace}");
    expect(chipNames(added)).toEqual(["bio"]);
    await user.type(field, "late");
    await user.click(screen.getByRole("button", { name: "Remove tag bio" }));
    expect(screen.queryByRole("list", { name: "Tags added" })).not.toBeNull();
    // A tag still being typed is kept when the box is left (here, for Save).
    await user.click(screen.getByRole("button", { name: "Save note" }));

    expect(await screen.findByText("Note saved.")).toBeTruthy();
    expect(argsOf("create_note")).toEqual([
      { title: "Cells", body: "They divide.", pictures: [], tags: ["late"] },
    ]);
  });

  it("show a refused tag under the field, which takes focus", async () => {
    const { user } = openNotes({
      create_note: () =>
        refuse(
          "invalid",
          "A tag is one word: no spaces or commas. Use - or :: to join words.",
          "tags",
        ),
    });
    await user.click(await screen.findByRole("button", { name: "New note" }));
    await user.type(screen.getByLabelText("Title"), "Cells");
    await user.type(screen.getByLabelText("Note"), "Text");
    await user.type(screen.getByLabelText("Tags"), "x{Enter}");
    await user.click(screen.getByRole("button", { name: "Save note" }));

    expect(await screen.findByText(/^A tag is one word/)).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Tags"));
  });

  it("are shown on the note, kept unless changed when it's edited, and count as unsaved", async () => {
    const { user, argsOf } = openNotes({
      update_note: (args) => ({ ...MITOSIS, tags: args.tags ?? MITOSIS.tags, updatedAt: LATER }),
    });
    await user.click((await screen.findAllByRole("button", { name: "Open note" }))[0]);
    await screen.findByRole("heading", { level: 2, name: "Mitosis" });
    expect(chipNames(screen.getByRole("list", { name: "Tags" }))).toEqual(["bio::cells", "exam"]);

    // Saved unchanged, tags are left out, so Rust keeps them.
    await user.click(screen.getByRole("button", { name: "Edit note" }));
    expect(chipNames(screen.getByRole("list", { name: "Tags added" }))).toEqual([
      "bio::cells",
      "exam",
    ]);
    await user.type(screen.getByLabelText("Title"), "!");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByRole("button", { name: "Edit note" });
    expect(argsOf("update_note")[0].tags).toBeUndefined();

    // Changed, they're sent.
    await user.click(screen.getByRole("button", { name: "Edit note" }));
    await user.click(screen.getByRole("button", { name: "Remove tag exam" }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByRole("button", { name: "Edit note" });
    expect(argsOf("update_note")[1].tags).toEqual(["bio::cells"]);
  });

  it("count as an unsaved change on their own", async () => {
    fakeBackend({ get_notes: () => [summary(MITOSIS)], get_note: () => MITOSIS, get_tags: () => [] });
    render(<App />);
    const user = userEvent.setup();
    const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));
    await user.click(sidebar.getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await user.click(await screen.findByRole("button", { name: "Edit note" }));
    const dialog = () => screen.getByRole("alertdialog", { hidden: true }) as HTMLDialogElement;

    await user.click(screen.getByRole("button", { name: "Remove tag exam" }));
    await user.click(sidebar.getByRole("button", { name: "Study Desk" }));
    expect(dialog().open).toBe(true);
  });

  it("are saved with a note from a source", async () => {
    const { user, argsOf } = openNotes({
      import_source: () =>
        keptSource({
          status: "read",
          fileName: "Lecture.pdf",
          kind: "pdf",
          title: "Lecture",
          body: "Text",
          pages: 1,
          pagesWithoutText: [],
        }),
      create_note_from_source: (args) => ({
        id: 9,
        title: args.title,
        body: args.body,
        createdAt: CREATED,
        updatedAt: CREATED,
        tags: args.tags,
      }),
    });
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    await user.type(await screen.findByLabelText("Tags"), "lecture ");
    await user.click(screen.getByRole("button", { name: "Save note" }));

    expect(await screen.findByText("Note saved.")).toBeTruthy();
    expect(argsOf("create_note_from_source")[0]).toMatchObject({
      sourceId: 3,
      tags: ["lecture"],
    });
  });

  it("are shown on a deleted note being read", async () => {
    const { user } = openNotes({
      get_notes: () => [],
      get_deleted_notes: () => [{ ...summary(MITOSIS), deletedAt: LATER }],
      get_deleted_note: () => ({ ...MITOSIS, deletedAt: LATER, pictures: [] }),
    });
    await user.click(await screen.findByRole("button", { name: "Read note" }));
    await screen.findByRole("heading", { level: 2, name: "Mitosis" });
    expect(chipNames(screen.getByRole("list", { name: "Tags" }))).toEqual(["bio::cells", "exam"]);
  });
});

describe("the note library", () => {
  it("shows each note's tags and narrows to one tag, or a tag below it", async () => {
    const { user, calls } = openNotes();
    await screen.findByRole("heading", { name: "Mitosis" });
    const items = () =>
      screen.getAllByRole("article").map((item) => within(item).getByRole("heading").textContent);
    expect(items()).toEqual(["Mitosis", "Atoms", "Plain"]);

    const filter = screen.getByLabelText("Tag") as HTMLSelectElement;
    expect([...filter.options].map((option) => option.textContent)).toEqual([
      "Every tag",
      "bio::cells (1)",
      "chem (1)",
      "exam (1)",
    ]);
    await user.selectOptions(filter, "chem");
    expect(items()).toEqual(["Atoms"]);
    expect(screen.getByText("1 note shown.")).toBeTruthy();

    await user.selectOptions(filter, "Every tag");
    expect(items()).toEqual(["Mitosis", "Atoms", "Plain"]);
    // Filtering is done here: nothing is asked of Rust.
    expect(calls.filter((cmd) => cmd === "search_notes")).toEqual([]);
  });

  it("combines the tag with a search, and says when nothing has both", async () => {
    const { user } = openNotes({ search_notes: () => [summary(MITOSIS)] });
    await screen.findByRole("heading", { name: "Mitosis" });
    await user.selectOptions(screen.getByLabelText("Tag"), "chem");
    await user.type(screen.getByLabelText("Search notes"), "mito");

    expect(
      await screen.findByText(
        "No notes tagged chem match “mito”. Try other words, or show every tag.",
      ),
    ).toBeTruthy();
  });

  it("offers no tag filter when no note has tags", async () => {
    openNotes({ get_notes: () => [PLAIN] });
    await screen.findByRole("heading", { name: "Plain" });
    expect(screen.queryByLabelText("Tag")).toBeNull();
  });
});

const TAGGED_CARD: SearchHit = {
  kind: "card",
  cardId: 10,
  deckId: 1,
  deckName: "Biology",
  front: [{ text: "What is a cell?", hit: false }],
  back: [{ text: "The basic unit of life.", hit: false }],
  tags: ["bio::cells"],
};

describe("searching by tag", () => {
  async function openSearch(overrides: Record<string, Handler> = {}) {
    const user = userEvent.setup();
    const backend = fakeBackend({
      search_everything: () => ({ results: [TAGGED_CARD], more: false }),
      get_tags: () => TAGS,
      ...overrides,
    });
    render(<App />);
    const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));
    await user.click(sidebar.getByRole("button", { name: "Search" }));
    await screen.findByRole("heading", { name: "Search", level: 2 });
    return { user, sidebar, ...backend };
  }

  it("searches a chosen tag alone, and shows each result's tags", async () => {
    const { user, argsOf } = await openSearch();
    await user.selectOptions(await screen.findByLabelText("Tag"), "bio::cells");

    expect(await screen.findByText("1 result.")).toBeTruthy();
    expect(argsOf("search_everything").slice(-1)[0]).toEqual({
      query: "",
      scope: "all",
      deckId: null,
      tag: "bio::cells",
    });
    const result = screen.getAllByRole("article")[0];
    expect(chipNames(within(result).getByRole("list", { name: "Tags" }))).toEqual(["bio::cells"]);
  });

  it("passes a typed #tag to Rust with the words", async () => {
    const { user, argsOf } = await openSearch({
      search_everything: () => ({ results: [], more: false }),
    });
    await user.type(screen.getByLabelText("Search for"), "cell #bio");
    expect(await screen.findByText("Nothing matches “cell #bio”.")).toBeTruthy();
    expect(argsOf("search_everything").slice(-1)[0]).toMatchObject({ query: "cell #bio", tag: null });

    await user.selectOptions(screen.getByLabelText("Tag"), "exam");
    expect(await screen.findByText("Nothing tagged exam matches “cell #bio”.")).toBeTruthy();
  });
});

describe("the Tags screen", () => {
  async function openTags(overrides: Record<string, Handler> = {}) {
    const user = userEvent.setup();
    const backend = fakeBackend({ get_tags: () => TAGS, ...overrides });
    render(<App />);
    const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));
    await user.click(sidebar.getByRole("button", { name: "Tags" }));
    await screen.findByRole("heading", { name: "Tags", level: 2 });
    return { user, sidebar, ...backend };
  }

  it("lists every tag with its counts, and says when there are none", async () => {
    const { sidebar } = await openTags();
    expect(sidebar.getByRole("button", { name: "Tags" }).getAttribute("aria-current")).toBe(
      "page",
    );
    const rows = screen.getAllByRole("article");
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining("bio::cells3 cards · 1 note"),
      expect.stringContaining("chem1 note"),
      expect.stringContaining("exam1 card · 1 note"),
    ]);
  });

  it("says when there are no tags", async () => {
    await openTags({ get_tags: () => [] });
    expect(screen.getByText(/^No tags yet\./)).toBeTruthy();
  });

  it("renames a tag, and says so", async () => {
    const { user, argsOf } = await openTags({
      rename_tag: () => [{ ...TAGS[0], name: "biology::cells" }, TAGS[1], TAGS[2]],
    });
    await user.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    const field = screen.getByLabelText("New name for bio::cells") as HTMLInputElement;
    expect(document.activeElement).toBe(field);
    await user.clear(field);
    await user.type(field, "biology::cells");
    await user.click(screen.getByRole("button", { name: "Rename tag" }));

    const notice = await screen.findByText(
      "bio::cells is now biology::cells, on every card and note that had it.",
    );
    expect(document.activeElement).toBe(notice);
    expect(argsOf("rename_tag")).toEqual([{ tagId: 1, name: "biology::cells" }]);
    expect(screen.getByText("biology::cells")).toBeTruthy();
  });

  it("shows a refused name under the field, and Cancel puts focus back on Rename", async () => {
    const { user } = await openTags({
      rename_tag: () => refuse("invalid", "Another tag already has that name.", "tags"),
    });
    await user.click(screen.getAllByRole("button", { name: "Rename" })[1]);
    const field = screen.getByLabelText("New name for chem");
    await user.clear(field);
    await user.type(field, "EXAM");
    await user.click(screen.getByRole("button", { name: "Rename tag" }));

    expect(await screen.findByText("Another tag already has that name.")).toBeTruthy();
    expect(document.activeElement).toBe(field);

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(document.activeElement).toBe(screen.getAllByRole("button", { name: "Rename" })[1]);
  });

  it("reloads and explains when the tag is gone", async () => {
    let gone = false;
    const { user } = await openTags({
      get_tags: () => (gone ? TAGS.slice(1) : TAGS),
      rename_tag: () => {
        gone = true;
        return refuse("stale", "That tag is no longer there.");
      },
    });
    await user.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    await user.type(screen.getByLabelText("New name for bio::cells"), "x");
    await user.click(screen.getByRole("button", { name: "Rename tag" }));

    expect(
      await screen.findByText("That tag is no longer there. Here are your tags as they are now."),
    ).toBeTruthy();
    expect(screen.getAllByRole("article")).toHaveLength(2);
  });

  it("counts a rename being typed as unsaved", async () => {
    const { user, sidebar } = await openTags();
    await user.click(screen.getAllByRole("button", { name: "Rename" })[0]);
    await user.type(screen.getByLabelText("New name for bio::cells"), "x");
    await user.click(sidebar.getByRole("button", { name: "Notes" }));
    // The question is always in the page; `open` says whether it's showing.
    expect((screen.getByRole("alertdialog", { hidden: true }) as HTMLDialogElement).open).toBe(
      true,
    );
    expect(screen.getByRole("heading", { name: "Tags", level: 2 })).toBeTruthy();
  });

  it("finds a tag in Search", async () => {
    const { user, argsOf } = await openTags({
      search_everything: () => ({ results: [TAGGED_CARD], more: false }),
    });
    await user.click(screen.getAllByRole("button", { name: "Find in Search" })[0]);

    await screen.findByRole("heading", { name: "Search", level: 2 });
    expect((screen.getByLabelText("Tag") as HTMLSelectElement).value).toBe("bio::cells");
    expect(await screen.findByText("1 result.")).toBeTruthy();
    expect(argsOf("search_everything").slice(-1)[0]).toMatchObject({ tag: "bio::cells" });
  });
});
