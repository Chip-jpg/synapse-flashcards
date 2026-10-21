import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Asset,
  DeckDetail as Deck,
  Note,
  OcclusionBox,
  Recognition,
} from "./api";
import { DeckDetail } from "./DeckDetail";
import { NotesArea } from "./Notes";
import { CREATED, fakeBackend, refuse, type Handler } from "./test/backend";

const PHOTO: Asset = {
  id: 7,
  sha256: "7".repeat(64),
  mediaType: "image/jpeg",
  size: 2048,
  createdAt: CREATED,
};

const SECOND: Asset = { ...PHOTO, id: 8, sha256: "8".repeat(64) };

function reading(assetId: number, lines: Recognition["lines"]): Recognition {
  return {
    assetId,
    page: 0,
    engine: "ocrs",
    model: "ocrs 0.13",
    text: lines.map((line) => line.text).join("\n"),
    lines,
    width: 800,
    height: 600,
    createdAt: CREATED,
  };
}

/** A `recognise_picture` that waits until `stop_recognition` is called. */
function waitingForStop() {
  let stop = () => {};
  return {
    recognise_picture: () =>
      new Promise((resolve) => {
        stop = () => resolve({ status: "stopped" });
      }),
    stop_recognition: () => {
      stop();
      return null;
    },
  } satisfies Record<string, Handler>;
}

beforeEach(() => {
  localStorage.clear();
  mockConvertFileSrc("linux");
});

afterEach(() => clearMocks());

function note(pictures: Asset[]): Note {
  return {
    id: 5,
    title: "Cells",
    body: "Notes on cells.",
    createdAt: CREATED,
    updatedAt: CREATED,
    pictures,
  };
}

async function openNote(
  pictures: Asset[],
  handlers: Record<string, Handler> = {},
) {
  const backend = fakeBackend({
    get_note: () => note(pictures),
    update_note: (args) => ({ ...note(pictures), body: args.body as string }),
    ...handlers,
  });
  const view = render(
    <NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />,
  );
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "Open note" }));
  await screen.findByRole("heading", { name: "Cells" });
  return { backend, user, view };
}

describe("text in a picture of a note", () => {
  it("is read in the form, shown as recognised, and added to the note only when asked", async () => {
    const { backend, user } = await openNote([PHOTO, SECOND]);

    await user.click(screen.getByRole("button", { name: "Edit note" }));
    await user.click(
      screen.getByRole("button", { name: "Text in picture 2 of 2" }),
    );

    const panel = await screen.findByRole("region", {
      name: "Text in picture 2 of 2",
    });
    expect(
      within(panel).getByText("Recognised text: check it for mistakes"),
    ).toBeTruthy();
    expect(
      within(panel).getByText(/built-in reader, which doesn't read accents/),
    ).toBeTruthy();
    expect(within(panel).getByText("Mitochondria make ATP.")).toBeTruthy();
    expect(backend.argsOf("recognise_picture")).toEqual([
      { assetId: SECOND.id },
    ]);
    // Reading wrote nothing to the note.
    expect((screen.getByLabelText("Note") as HTMLTextAreaElement).value).toBe(
      "Notes on cells.",
    );

    await user.click(
      within(panel).getByRole("button", { name: "Add to note" }),
    );
    expect((screen.getByLabelText("Note") as HTMLTextAreaElement).value).toBe(
      "Notes on cells.\n\nMitochondria make ATP.",
    );
    expect(
      within(panel).getByText("Added to the end of the note."),
    ).toBeTruthy();
    expect(backend.calls).not.toContain("update_note");

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(backend.argsOf("update_note")).toEqual([
      {
        noteId: 5,
        title: "Cells",
        body: "Notes on cells.\n\nMitochondria make ATP.",
        pictures: [PHOTO.id, SECOND.id],
      },
    ]);
  });

  it("can be copied from the note itself, which offers nothing to add it to", async () => {
    // user-event stands in a clipboard of its own.
    const { user } = await openNote([PHOTO]);

    await user.click(
      screen.getByRole("button", { name: "Text in picture in the note" }),
    );
    const panel = await screen.findByRole("region", {
      name: "Text in picture in the note",
    });
    expect(
      within(panel).queryByRole("button", { name: "Add to note" }),
    ).toBeNull();

    await user.click(within(panel).getByRole("button", { name: "Copy" }));
    expect(await navigator.clipboard.readText()).toBe("Mitochondria make ATP.");
    expect(within(panel).getByText("Copied.")).toBeTruthy();

    await user.click(within(panel).getByRole("button", { name: "Close" }));
    expect(
      screen.queryByRole("region", { name: /Text in picture/ }),
    ).toBeNull();
  });

  it("says when no text was found, and when reading is refused", async () => {
    let refused = false;
    const { user } = await openNote([PHOTO], {
      recognise_picture: (args) =>
        refused
          ? refuse(
              "invalid",
              "Synapse couldn't read that picture. It may be damaged.",
            )
          : {
              status: "read",
              recognition: reading(args.assetId as number, []),
            },
    });

    await user.click(
      screen.getByRole("button", { name: "Text in picture in the note" }),
    );
    expect(await screen.findByText("No text found.")).toBeTruthy();

    refused = true;
    await user.click(
      screen.getByRole("button", { name: "Text in picture in the note" }),
    );
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Synapse couldn't read that picture. It may be damaged.",
    );
  });

  it("can be stopped, and is stopped when the screen goes", async () => {
    const { backend, user, view } = await openNote([PHOTO], waitingForStop());

    await user.click(
      screen.getByRole("button", { name: "Text in picture in the note" }),
    );
    expect(
      await screen.findByText("Reading the text in picture in the note…"),
    ).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Stopped. Nothing was read.")).toBeTruthy();
    expect(backend.argsOf("stop_recognition")).toHaveLength(1);

    await user.click(
      screen.getByRole("button", { name: "Text in picture in the note" }),
    );
    await screen.findByText("Reading the text in picture in the note…");
    view.unmount();
    expect(backend.argsOf("stop_recognition")).toHaveLength(2);
  });
});

describe("text in a picture on a card", () => {
  it("is added to the side the picture is on", async () => {
    const card: Deck = {
      id: 1,
      name: "Biology",
      description: null,
      cardCount: 1,
      dueCount: 0,
      cards: [
        {
          id: 10,
          front: "What makes ATP?",
          back: "",
          pictures: { front: [], back: [PHOTO] },
        },
      ],
      deletedCards: [],
    };
    fakeBackend({ get_deck_detail: () => card });
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

    await user.click(await screen.findByRole("button", { name: "Edit card" }));
    const back = screen.getByRole("group", { name: "Pictures on the back" });
    await user.click(
      within(back).getByRole("button", { name: "Text in picture" }),
    );
    await user.click(
      await within(back).findByRole("button", { name: "Add to back" }),
    );

    expect((screen.getByLabelText("Back") as HTMLTextAreaElement).value).toBe(
      "Mitochondria make ATP.",
    );
    expect((screen.getByLabelText("Front") as HTMLTextAreaElement).value).toBe(
      "What makes ATP?",
    );
  });
});

describe("a note from a photo", () => {
  function openLibrary(handlers: Record<string, Handler> = {}) {
    const backend = fakeBackend({
      get_notes: () => [],
      choose_picture: () => ({ status: "added", asset: PHOTO }),
      create_note: (args) => ({
        ...note([PHOTO]),
        title: args.title as string,
        body: args.body as string,
      }),
      ...handlers,
    });
    render(
      <NotesArea
        onBack={vi.fn()}
        onOpenDeck={vi.fn()}
        onOpenSource={vi.fn()}
      />,
    );
    return { backend, user: userEvent.setup() };
  }

  it("opens a new note with the text read and the photo attached", async () => {
    const { backend, user } = openLibrary();

    await user.click(
      await screen.findByRole("button", { name: "Note from a photo" }),
    );

    const summary = await screen.findByText(
      /text Synapse recognised in the photo/,
    );
    await waitFor(() => expect(document.activeElement).toBe(summary));
    expect(
      screen.getByRole("heading", { name: "New note from a photo" }),
    ).toBeTruthy();
    expect((screen.getByLabelText("Note") as HTMLTextAreaElement).value).toBe(
      "Mitochondria make ATP.",
    );
    expect(
      screen.getByRole("img", { name: "Picture" }).getAttribute("src"),
    ).toBe(`synapse://localhost/${PHOTO.sha256}`);
    expect(backend.calls).not.toContain("create_note");

    await user.type(screen.getByLabelText("Title"), "Energy");
    await user.click(screen.getByRole("button", { name: "Save note" }));
    expect(backend.argsOf("create_note")).toEqual([
      { title: "Energy", body: "Mitochondria make ATP.", pictures: [PHOTO.id] },
    ]);
  });

  it("still opens with the photo when it has no text, or reading was stopped", async () => {
    const stopping = waitingForStop();
    let stop = false;
    const { user } = openLibrary({
      recognise_picture: (args) =>
        stop
          ? stopping.recognise_picture()
          : {
              status: "read",
              recognition: reading(args.assetId as number, []),
            },
      stop_recognition: stopping.stop_recognition,
    });

    await user.click(
      await screen.findByRole("button", { name: "Note from a photo" }),
    );
    expect(await screen.findByText(/No text found in the photo/)).toBeTruthy();
    expect((screen.getByLabelText("Note") as HTMLTextAreaElement).value).toBe(
      "",
    );
    expect(screen.getByRole("img", { name: "Picture" })).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    const again = await screen.findByRole("button", {
      name: "Note from a photo",
    });
    await waitFor(() => expect(document.activeElement).toBe(again));

    stop = true;
    await user.click(again);
    await user.click(await screen.findByRole("button", { name: "Stop" }));
    expect(
      await screen.findByText(/Reading stopped before any text was found/),
    ).toBeTruthy();
    expect(screen.getByRole("img", { name: "Picture" })).toBeTruthy();
  });

  it("does nothing when no photo is chosen", async () => {
    const { backend, user } = openLibrary({
      choose_picture: () => ({ status: "cancelled" }),
    });

    await user.click(
      await screen.findByRole("button", { name: "Note from a photo" }),
    );
    await waitFor(() => expect(backend.calls).toContain("choose_picture"));
    expect(backend.calls).not.toContain("recognise_picture");
    expect(screen.getByRole("heading", { name: "Notes" })).toBeTruthy();
  });
});

describe("finding labels for picture cards", () => {
  const empty: Deck = {
    id: 1,
    name: "Biology",
    description: null,
    cardCount: 0,
    dueCount: 0,
    cards: [],
    deletedCards: [],
  };

  async function openOcclusion(handlers: Record<string, Handler> = {}) {
    const backend = fakeBackend({
      get_deck_detail: () => empty,
      choose_picture: () => ({ status: "added", asset: PHOTO }),
      create_occlusion: () => empty,
      ...handlers,
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
    await screen.findByRole("button", { name: "Find labels" });
    return { backend, user };
  }

  it("puts a labelled box over each line, a little larger than the text", async () => {
    const { backend, user } = await openOcclusion();

    await user.click(screen.getByRole("button", { name: "Find labels" }));
    expect(
      await screen.findByText(/Added 1 box, one over each line/),
    ).toBeTruthy();
    expect(
      screen.getByText(/labels are recognised text and can have mistakes/),
    ).toBeTruthy();
    expect(
      (screen.getByLabelText("Label for box 1") as HTMLInputElement).value,
    ).toBe("Mitochondria make ATP.");

    // Finding them again adds nothing over the same text.
    await user.click(screen.getByRole("button", { name: "Find labels" }));
    expect(
      await screen.findByText("Every piece of text found already has a box."),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Save cards" }));
    const [box] = backend.argsOf("create_occlusion")[0].boxes as OcclusionBox[];
    // The line is at 0.1, 0.2, 0.5 × 0.08 of an 800 × 600 picture.
    expect(box).toEqual({
      number: 1,
      left: 0.088,
      top: 0.184,
      width: 0.524,
      height: 0.112,
      label: "Mitochondria make ATP.",
    });
  });

  it("splits a line where its words are far apart, as labels side by side", async () => {
    // "Cell membrane" and "Nucleus" on one row of a diagram, read as one line.
    const line = {
      text: "Cell membrane Nucleus",
      box: { left: 0.05, top: 0.1, width: 0.85, height: 0.05 },
      words: [
        {
          text: "Cell",
          box: { left: 0.05, top: 0.1, width: 0.08, height: 0.05 },
        },
        {
          text: "membrane",
          box: { left: 0.14, top: 0.1, width: 0.15, height: 0.05 },
        },
        {
          text: "Nucleus",
          box: { left: 0.78, top: 0.1, width: 0.12, height: 0.05 },
        },
      ],
    };
    await openOcclusion({
      recognise_picture: (args) => ({
        status: "read",
        recognition: reading(args.assetId as number, [line]),
      }),
    });
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Find labels" }));

    expect(await screen.findByText(/Added 2 boxes/)).toBeTruthy();
    expect(
      (screen.getByLabelText("Label for box 1") as HTMLInputElement).value,
    ).toBe("Cell membrane");
    expect(
      (screen.getByLabelText("Label for box 2") as HTMLInputElement).value,
    ).toBe("Nucleus");
  });

  it("can put a box over each word instead, each its own card", async () => {
    await openOcclusion();
    const user = userEvent.setup();

    await user.selectOptions(
      screen.getByRole("combobox", { name: /One box per/ }),
      "word",
    );
    await user.click(screen.getByRole("button", { name: "Find labels" }));

    expect(
      await screen.findByText(/Added 3 boxes, one over each word/),
    ).toBeTruthy();
    expect(screen.getByText("3 boxes make 3 cards.")).toBeTruthy();
    expect(
      (screen.getByLabelText("Label for box 3") as HTMLInputElement).value,
    ).toBe("ATP.");
  });

  it("keeps to 200 boxes and 200-character labels", async () => {
    const lines = Array.from({ length: 205 }, (_, index) => ({
      text: index === 0 ? "x".repeat(250) : `Line ${index}`,
      box: { left: 0, top: index / 205, width: 0.5, height: 0.004 },
      words: [],
    }));
    await openOcclusion({
      recognise_picture: (args) => ({
        status: "read",
        recognition: reading(args.assetId as number, lines),
      }),
    });
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Find labels" }));

    expect(
      await screen.findByText(/Added 200 boxes.*5 more didn't fit/),
    ).toBeTruthy();
    expect(
      (screen.getByLabelText("Label for box 1") as HTMLInputElement).value,
    ).toHaveLength(200);
    expect(screen.queryByLabelText("Label for box 201")).toBeNull();
    // 200 boxes, each with a choice of 200 cards, are slow to draw in jsdom.
  }, 30_000);

  it("can be stopped, adding nothing", async () => {
    const { backend, user } = await openOcclusion(waitingForStop());

    await user.click(screen.getByRole("button", { name: "Find labels" }));
    await user.click(await screen.findByRole("button", { name: "Stop" }));

    expect(
      await screen.findByText("Stopped. No boxes were added."),
    ).toBeTruthy();
    expect(screen.getByText("No boxes yet.")).toBeTruthy();
    expect(backend.argsOf("stop_recognition")).toHaveLength(1);
  });
});
