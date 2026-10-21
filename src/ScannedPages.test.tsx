import { act, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "./App";
import type { ImportedFile, Note, RecognisedText, SourcePage, SourceRecognition } from "./api";
import { NotesArea } from "./Notes";
import { readingSummary } from "./ScannedPages";
import {
  CREATED,
  deferred,
  fakeBackend,
  keptSource,
  sendOnChannel,
  source,
  type Handler,
} from "./test/backend";

const READ_AT = "2026-10-21T09:00:00Z";

function recognised(text: string, engine: RecognisedText["engine"] = "ocrs"): RecognisedText {
  return {
    text,
    engine,
    model: engine === "ocrs" ? "ocrs 0.13 (detection f15cfb56, recognition e484866d)" : "qwen2.5vl:7b",
    createdAt: READ_AT,
  };
}

/** Lecture 3: a PDF of four pages, 2 and 4 scans not read yet. */
function scannedPages(): SourcePage[] {
  return [
    { number: 1, text: "Mitosis is cell division.", recognised: null },
    { number: 2, text: "", recognised: null },
    { number: 3, text: "Metaphase: chromosomes line up.", recognised: null },
    { number: 4, text: "", recognised: null },
  ];
}

function sidebar() {
  return within(screen.getByRole("navigation", { name: "Synapse" }));
}

/**
 * The app with Lecture 3 kept. Reading pages fills in what `read` says each
 * page holds; a page not in `read` is skipped for having no picture.
 */
function openApp(handlers: Record<string, Handler> = {}, read: Record<number, string> = {}) {
  const lecture = source({ pageCount: 4 });
  let pages = scannedPages();
  const backend = fakeBackend({
    get_sources: () => [lecture],
    get_deleted_sources: () => [],
    get_source: () => lecture,
    get_source_pages: () => pages,
    get_source_notes: () => [],
    get_pages_to_recognise: () =>
      pages.filter((page) => page.text === "" && page.recognised === null).map((p) => p.number),
    recognise_source_pages: (args) => {
      const asked = args.pages as number[];
      const result: SourceRecognition = { read: [], skipped: [], stopped: false };
      asked.forEach((number, index) => {
        const text = read[number];
        if (text === undefined) {
          result.skipped.push({ page: number, problem: "noPicture" });
        } else {
          result.read.push(number);
          pages = pages.map((page) =>
            page.number === number ? { ...page, recognised: recognised(text) } : page,
          );
        }
        sendOnChannel(args.onProgress, index, {
          page: number,
          done: index + 1,
          total: asked.length,
          problem: text === undefined ? "noPicture" : null,
        });
      });
      return result;
    },
    create_note_from_source: (args) => {
      const { title, body } = args as { title: string; body: string };
      return { id: 9, title, body, createdAt: CREATED, updatedAt: CREATED } satisfies Note;
    },
    ...handlers,
  });
  render(<App />);
  return { ...backend, user: userEvent.setup() };
}

async function openLecture(user: UserEvent) {
  await user.click(sidebar().getByRole("button", { name: "Sources" }));
  await user.click(await screen.findByRole("button", { name: "Open source" }));
  return screen.findByRole("heading", { name: "Lecture 3", level: 2 });
}

describe("reading a source's scanned pages", () => {
  it("reads every page left, says how it went, and shows the text as recognised", async () => {
    const { user, argsOf } = openApp({}, { 2: "Prophase: chromosomes condense." });
    await openLecture(user);

    expect(screen.getByText(/^2 pages of this PDF have no text Synapse could read/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Read scanned pages (2)" }));

    const summary = await screen.findByText(
      "Read page 2. Skipped page 4: no picture to read.",
    );
    expect(document.activeElement).toBe(summary);
    expect(argsOf("recognise_source_pages")).toEqual([
      expect.objectContaining({ sourceId: 3, pages: [2, 4] }),
    ]);
    // Page 4 was skipped for having no picture, so it isn't offered again.
    expect(screen.queryByRole("button", { name: /^Read scanned pages/ })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByText("Prophase: chromosomes condense.")).toBeTruthy();
    const label = screen.getByText("Recognised text, check for mistakes.").parentElement!;
    expect(label.textContent).toContain("Synapse's built-in reader read it from the page's picture");
    expect(label.textContent).toContain("It doesn't read accents: è comes out as e.");
    expect(screen.getByRole("button", { name: "Read this page again" })).toBeTruthy();
  });

  it("says which page it's on, and stops when asked, keeping the pages read", async () => {
    const answer = deferred<SourceRecognition>();
    let channel: unknown;
    const { user, calls } = openApp({
      recognise_source_pages: (args) => {
        channel = args.onProgress;
        return answer.promise;
      },
    });
    await openLecture(user);
    await user.click(screen.getByRole("button", { name: "Read scanned pages (2)" }));

    const status = await screen.findByText("Reading page 1 of 2…");
    expect(document.activeElement).toBe(status);
    act(() => sendOnChannel(channel, 0, { page: 2, done: 1, total: 2, problem: null }));
    expect(screen.getByText("Reading page 2 of 2…")).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "1 of 2 pages read" })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(calls).toContain("stop_recognition");
    expect(screen.getByText("Stopping… The pages read so far are kept.")).toBeTruthy();

    await act(async () => answer.resolve({ read: [2], skipped: [], stopped: true }));
    expect(
      await screen.findByText(/^Stopped after reading 1 page\. Those pages are kept\./),
    ).toBeTruthy();
  });

  it("stops reading when the user leaves the source", async () => {
    const answer = deferred<SourceRecognition>();
    const { user, calls } = openApp({ recognise_source_pages: () => answer.promise });
    await openLecture(user);
    await user.click(screen.getByRole("button", { name: "Read scanned pages (2)" }));
    await screen.findByText("Reading page 1 of 2…");
    expect(calls).not.toContain("stop_recognition");

    await user.click(screen.getByRole("button", { name: "Back to sources" }));
    expect(calls).toContain("stop_recognition");
    await act(async () => answer.resolve({ read: [], skipped: [], stopped: true }));
  });

  it("reads one page again from the page viewer", async () => {
    const { user, argsOf } = openApp(
      {
        get_source_pages: () => [
          { number: 1, text: "Mitosis is cell division.", recognised: null },
          { number: 2, text: "", recognised: recognised("Prophase", "ollama") },
        ],
        get_pages_to_recognise: () => [],
        recognise_source_pages: () => ({ read: [2], skipped: [], stopped: false }),
      },
    );
    await openLecture(user);
    // Nothing left to read, so nothing is offered for the whole source.
    expect(screen.queryByRole("heading", { name: "Scanned pages" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(
      screen.getByText(/qwen2\.5vl:7b, a vision model in Ollama read it/),
    ).toBeTruthy();
    // Only the built-in reader is said to miss accents.
    expect(screen.queryByText(/doesn't read accents/)).toBeNull();

    await user.click(screen.getByRole("button", { name: "Read this page again" }));
    expect(await screen.findByText("Read page 2.")).toBeTruthy();
    expect(argsOf("recognise_source_pages")).toEqual([
      expect.objectContaining({ sourceId: 3, pages: [2] }),
    ]);
  });

  it("makes a note with the recognised text, and says it was recognised", async () => {
    const { user, argsOf } = openApp(
      {},
      { 2: "Prophase: chromosomes condense.", 4: "Telophase: two nuclei form." },
    );
    await openLecture(user);
    await user.click(screen.getByRole("button", { name: "Read scanned pages (2)" }));
    await screen.findByText("Read 2 pages.");

    await user.click(screen.getByRole("button", { name: "Make a note" }));
    const body = (await screen.findByLabelText("Note")) as HTMLTextAreaElement;
    expect(body.value).toBe(
      "Mitosis is cell division.\n\nProphase: chromosomes condense.\n\nMetaphase: chromosomes line up.\n\nTelophase: two nuclei form.",
    );
    expect(screen.getByText(/^Pages 2 and 4 were scanned, so their text was recognised/)).toBeTruthy();
    // Every page has text now.
    expect(screen.queryByText(/had no text Synapse could read/)).toBeNull();

    await user.click(screen.getByRole("button", { name: "Save note" }));
    expect(await screen.findByText("Note saved.")).toBeTruthy();
    expect(argsOf("create_note_from_source")[0]).toMatchObject({ sourceId: 3, body: body.value });
  });

  it("labels a recognised page opened from a suggested card's quote", async () => {
    // The fake backend's suggested card quotes this.
    const quote = "Prophase, metaphase, anaphase, telophase.";
    const { user } = openApp({
      get_source_pages: () => [
        { number: 1, text: "Mitosis is cell division.", recognised: null },
        { number: 2, text: "", recognised: recognised(quote) },
      ],
      get_note_source: () => 3,
      locate_quotes: () => ({
        sourceId: 3,
        title: "Lecture 3",
        kind: "pdf",
        pageCount: 2,
        spots: [{ page: 2, lastPage: 2 }],
      }),
      get_quoted_page: () => [{ text: quote, hit: true }],
    });
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));

    await user.click(await screen.findByRole("button", { name: "From page 2 of Lecture 3" }));
    const page = screen.getByRole("dialog", { name: "Page 2 of 2" });
    expect((await within(page).findByText(quote)).tagName).toBe("MARK");
    expect(await within(page).findByText("Recognised text, check for mistakes.")).toBeTruthy();
  });
});

const SCAN: ImportedFile = {
  status: "read",
  fileName: "Scan.pdf",
  kind: "pdf",
  title: "Scan",
  body: "",
  pages: 2,
  pagesWithoutText: [1, 2],
};

/** The notes area, where *Import file* keeps Scan.pdf, two scanned pages. */
function openNotes(handlers: Record<string, Handler> = {}) {
  const scan = source({ title: "Scan", fileName: "Scan.pdf", pageCount: 2 });
  let pages: SourcePage[] = [
    { number: 1, text: "", recognised: null },
    { number: 2, text: "", recognised: null },
  ];
  const backend = fakeBackend({
    import_source: () => keptSource(SCAN),
    get_source: () => scan,
    get_source_pages: () => pages,
    get_source_notes: () => [],
    get_pages_to_recognise: () =>
      pages.filter((page) => page.recognised === null).map((page) => page.number),
    recognise_source_pages: (args) => {
      pages = pages.map((page) => ({ ...page, recognised: recognised(`Page ${page.number} text.`) }));
      sendOnChannel(args.onProgress, 0, { page: 1, done: 1, total: 2, problem: null });
      return { read: [1, 2], skipped: [], stopped: false };
    },
    ...handlers,
  });
  render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
  return { ...backend, user: userEvent.setup() };
}

describe("importing a scanned PDF", () => {
  it("offers to read its pages first, then opens the note with their text", async () => {
    const { user, argsOf } = openNotes();
    await user.click(await screen.findByRole("button", { name: "Import file" }));

    const intro = await screen.findByText(/^Synapse kept Scan\.pdf under Sources, but none of its 2 pages/);
    expect(document.activeElement).toBe(intro);
    await user.click(screen.getByRole("button", { name: "Read scanned pages (2)" }));

    const body = (await screen.findByLabelText("Note")) as HTMLTextAreaElement;
    expect(body.value).toBe("Page 1 text.\n\nPage 2 text.");
    expect(screen.getByRole("heading", { name: "New note from Scan.pdf" })).toBeTruthy();
    expect(screen.getByText(/^Synapse kept Scan\.pdf under Sources and read its text/)).toBeTruthy();
    expect(screen.getByText(/^Pages 1 and 2 were scanned/)).toBeTruthy();
    expect(argsOf("recognise_source_pages")).toEqual([
      expect.objectContaining({ sourceId: 3, pages: [1, 2] }),
    ]);
  });

  it("lets the user write the note without reading, or go back", async () => {
    const { user, calls } = openNotes();
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("button", { name: "Import file" })).toBe(document.activeElement);

    await user.click(screen.getByRole("button", { name: "Import file" }));
    await user.click(await screen.findByRole("button", { name: "Write the note myself" }));
    expect(((await screen.findByLabelText("Note")) as HTMLTextAreaElement).value).toBe("");
    expect(calls).not.toContain("recognise_source_pages");
  });

  it("after a stop, carries on reading or writes the note with what was read", async () => {
    const { user, argsOf } = openNotes({
      recognise_source_pages: () => ({ read: [], skipped: [], stopped: true }),
    });
    await user.click(await screen.findByRole("button", { name: "Import file" }));
    await user.click(await screen.findByRole("button", { name: "Read scanned pages (2)" }));

    expect(await screen.findByText("Stopped before any page was read.")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Carry on reading (2)" }));
    expect(argsOf("recognise_source_pages")).toHaveLength(2);
    await screen.findByText("Stopped before any page was read.");
    // Nothing was read, so the note is written by hand.
    expect(screen.getByRole("button", { name: "Write the note myself" })).toBeTruthy();
  });
});

describe("what reading says", () => {
  const pages: SourcePage[] = [
    { number: 1, text: "", recognised: recognised("Text") },
    { number: 2, text: "", recognised: recognised("") },
    { number: 3, text: "", recognised: recognised("More") },
  ];

  it("counts pages read, empty and skipped, with plain reasons", () => {
    expect(
      readingSummary(
        {
          read: [3, 1, 2],
          skipped: [
            { page: 6, problem: "unreadablePicture" },
            { page: 5, problem: "noPicture" },
            { page: 4, problem: "noPicture" },
          ],
          stopped: false,
        },
        pages,
      ),
    ).toBe(
      "Read 3 pages. No text was found on page 2. Skipped page 6: its picture is stored in a form Synapse can't read, such as JPEG 2000. Skipped pages 4 and 5: no picture to read.",
    );
    expect(readingSummary({ read: [2], skipped: [], stopped: false }, pages)).toBe(
      "Read page 2, but no text was found on it.",
    );
    expect(readingSummary({ read: [1, 2], skipped: [], stopped: true }, pages)).toBe(
      "Stopped after reading 2 pages. Those pages are kept. No text was found on page 2.",
    );
  });
});
