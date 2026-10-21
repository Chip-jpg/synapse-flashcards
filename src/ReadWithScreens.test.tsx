import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { Asset, DeckDetail as Deck, Note, SourcePage } from "./api";
import { DeckDetail } from "./DeckDetail";
import { NotesArea } from "./Notes";
import { CREATED, fakeBackend, sendOnChannel, source, type Handler } from "./test/backend";

// Where *Built in* or a vision model in Ollama is chosen on the screens that
// read text: a source's scanned pages, one page, a picture, and picture cards.

const MODEL = "qwen2.5vl:7b";
const ITALIAN = "La fotosintesi è il processo più importante.";

const PHOTO: Asset = {
  id: 7,
  sha256: "7".repeat(64),
  mediaType: "image/jpeg",
  size: 2048,
  createdAt: CREATED,
};

beforeEach(() => {
  localStorage.clear();
  mockConvertFileSrc("linux");
});

afterEach(() => localStorage.clear());

/** Lecture 3: page 1 typed, pages 2 and 3 scans; the built-in reader has read page 2. */
function openSources(handlers: Record<string, Handler> = {}) {
  const lecture = source({ pageCount: 3 });
  let pages: SourcePage[] = [
    { number: 1, text: "Mitosis is cell division.", recognised: null },
    {
      number: 2,
      text: "",
      recognised: {
        text: "La fotosintesi e il processo piu importante.",
        engine: "ocrs",
        model: "ocrs 0.13",
        createdAt: CREATED,
      },
    },
    { number: 3, text: "", recognised: null },
  ];
  const backend = fakeBackend({
    get_sources: () => [lecture],
    get_deleted_sources: () => [],
    get_source: () => lecture,
    get_source_pages: () => pages,
    get_source_notes: () => [],
    get_pages_to_recognise: () => [3],
    get_pages_to_recognise_with_model: () =>
      pages
        .filter((page) => page.text === "" && page.recognised?.engine !== "ollama")
        .map((page) => page.number),
    recognise_source_pages_with_model: (args) => {
      const asked = args.pages as number[];
      asked.forEach((number, index) => {
        pages = pages.map((page) =>
          page.number === number
            ? {
                ...page,
                recognised: { text: ITALIAN, engine: "ollama", model: MODEL, createdAt: CREATED },
              }
            : page,
        );
        sendOnChannel(args.onProgress, index, {
          page: number,
          done: index + 1,
          total: asked.length,
          problem: null,
        });
      });
      return {
        read: asked,
        skipped: [],
        stopped: false,
        agreement: asked.includes(2) ? [{ page: 2, percent: 30 }] : [],
      };
    },
    ...handlers,
  });
  render(<App />);
  return { ...backend, user: userEvent.setup() };
}

async function openLecture(user: ReturnType<typeof userEvent.setup>) {
  const nav = within(screen.getByRole("navigation", { name: "Synapse" }));
  await user.click(nav.getByRole("button", { name: "Sources" }));
  await user.click(await screen.findByRole("button", { name: "Open source" }));
  await screen.findByRole("heading", { name: "Lecture 3", level: 2 });
}

describe("choosing the reader for a source's scanned pages", () => {
  it("reads every page with a vision model chosen, including pages the built-in reader read", async () => {
    const { user, argsOf, calls } = openSources();
    await openLecture(user);

    // Built in: only page 3 is left.
    expect(screen.getByRole("button", { name: "Read scanned pages (1)" })).toBeTruthy();
    const choice = await screen.findByRole("combobox", { name: "Read with" });
    await waitFor(() => expect(within(choice).getAllByRole("option")).toHaveLength(2));
    await user.selectOptions(choice, MODEL);

    await user.click(await screen.findByRole("button", { name: "Read scanned pages (2)" }));
    expect(
      await screen.findByText(
        "Read 2 pages. On page 2, the model's text shares under half the words the built-in reader found: check it against the original.",
      ),
    ).toBeTruthy();
    expect(argsOf("recognise_source_pages_with_model")).toEqual([
      expect.objectContaining({ sourceId: 3, pages: [2, 3], model: MODEL }),
    ]);
    expect(calls).not.toContain("recognise_source_pages");
    expect(argsOf("get_pages_to_recognise_with_model")[0]).toEqual({
      sourceId: 3,
      model: MODEL,
    });

    // The page shows the model's text, labelled with who read it.
    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByText(ITALIAN)).toBeTruthy();
    const label = screen.getByText("Recognised text, check for mistakes.").parentElement!;
    expect(label.textContent).toContain(`${MODEL}, a vision model in Ollama read it`);
    expect(label.textContent).not.toContain("doesn't read accents");
  });

  it("reads one page with the reader chosen beside it", async () => {
    const { user, argsOf } = openSources();
    localStorage.setItem("synapse.readWith", MODEL);
    await openLecture(user);
    await user.click(screen.getByRole("button", { name: "Next page" }));
    await user.click(screen.getByRole("button", { name: "Read this page again" }));
    expect(await screen.findByText(ITALIAN)).toBeTruthy();
    expect(argsOf("recognise_source_pages_with_model")).toEqual([
      expect.objectContaining({ pages: [2], model: MODEL }),
    ]);
  });
});

function note(): Note {
  return {
    id: 5,
    title: "Cells",
    body: "Notes on cells.",
    createdAt: CREATED,
    updatedAt: CREATED,
    pictures: [PHOTO],
  };
}

describe("choosing the reader for a picture", () => {
  it("reads it again with a vision model, showing how far the two agree", async () => {
    const backend = fakeBackend({
      get_note: () => note(),
      recognise_picture_with_model: (args) => ({
        status: "read",
        recognition: {
          assetId: args.assetId,
          page: 0,
          engine: "ollama",
          model: args.model,
          text: ITALIAN,
          lines: [],
          width: null,
          height: null,
          createdAt: CREATED,
        },
        agreement: 92,
      }),
    });
    render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await user.click(await screen.findByRole("button", { name: "Edit note" }));
    await user.click(screen.getByRole("button", { name: "Text in picture" }));

    const panel = await screen.findByRole("region", { name: "Text in picture" });
    expect(await within(panel).findByText("Mitochondria make ATP.")).toBeTruthy();
    const choice = within(panel).getByRole("combobox", { name: "Read with" });
    await waitFor(() => expect(within(choice).getAllByRole("option")).toHaveLength(2));
    await user.selectOptions(choice, MODEL);
    await user.click(within(panel).getByRole("button", { name: "Read again" }));

    expect(await within(panel).findByText(ITALIAN)).toBeTruthy();
    expect(within(panel).getByText(`Read by ${MODEL} in Ollama.`)).toBeTruthy();
    expect(within(panel).getByText(/has 92% of the words the built-in reader found/)).toBeTruthy();
    expect(backend.argsOf("recognise_picture_with_model")).toEqual([
      { assetId: PHOTO.id, model: MODEL },
    ]);

    await user.click(within(panel).getByRole("button", { name: "Add to note" }));
    expect((screen.getByLabelText("Note") as HTMLTextAreaElement).value).toBe(
      `Notes on cells.\n\n${ITALIAN}`,
    );
  });

  it("makes a note from a photo with the reader chosen last", async () => {
    localStorage.setItem("synapse.readWith", MODEL);
    const backend = fakeBackend({
      get_notes: () => [],
      choose_picture: () => ({ status: "added", asset: PHOTO }),
    });
    render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Note from a photo" }));
    await screen.findByText(/The note below is the text Synapse recognised/);
    expect(backend.argsOf("recognise_picture_with_model")).toEqual([
      { assetId: PHOTO.id, model: MODEL },
    ]);
    expect(
      screen.getByText(new RegExp(`Read by ${MODEL.replace(".", "\\.")} in Ollama`)),
    ).toBeTruthy();
  });

  it("always finds labels for picture cards with the built-in reader, which says where text is", async () => {
    localStorage.setItem("synapse.readWith", MODEL);
    const empty: Deck = {
      id: 1,
      name: "Biology",
      description: null,
      cardCount: 0,
      dueCount: 0,
      cards: [],
      deletedCards: [],
    };
    const backend = fakeBackend({
      get_deck_detail: () => empty,
      choose_picture: () => ({ status: "added", asset: PHOTO }),
    });
    render(
      <DeckDetail
        deckId={1}
        justCreated={false}
        onBack={vi.fn()}
        onStart={vi.fn()}
        onArchived={vi.fn()}
      />,
    );
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Add card" }));
    await user.click(screen.getByRole("radio", { name: /Picture/ }));
    await user.click(screen.getByRole("button", { name: "Choose a picture" }));
    await user.click(await screen.findByRole("button", { name: "Find labels" }));
    expect(await screen.findByText(/Added 1 box, one over each line/)).toBeTruthy();
    expect(backend.argsOf("recognise_picture")).toEqual([{ assetId: PHOTO.id }]);
    expect(backend.calls).not.toContain("recognise_picture_with_model");
  });
});
