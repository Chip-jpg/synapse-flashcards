import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { clearMocks, mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Asset, DeckDetail as Deck, Note } from "./api";
import { DeckDetail } from "./DeckDetail";
import { NotesArea } from "./Notes";
import { ReviewSession } from "./ReviewSession";
import { CREATED, fakeBackend, refuse, type Handler } from "./test/backend";

function picture(id: number): Asset {
  return {
    id,
    sha256: String(id).repeat(64).slice(0, 64),
    mediaType: "image/png",
    size: 100,
    createdAt: CREATED,
  };
}

const FIRST = picture(1);
const SECOND = picture(2);
const PASTED = picture(3);

const url = (asset: Asset) => `synapse://localhost/${asset.sha256}`;

function note(pictures: Asset[]): Note {
  return {
    id: 5,
    title: "Mitosis",
    body: "Four phases.",
    createdAt: CREATED,
    updatedAt: CREATED,
    pictures,
  };
}

/** Pastes a picture file onto `target`, as a clipboard holding one would. */
function pastePicture(target: Element) {
  const file = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "image.png", {
    type: "image/png",
  });
  fireEvent.paste(target, { clipboardData: { files: [file], types: ["Files"] } });
}

beforeEach(() => {
  localStorage.clear();
  mockConvertFileSrc("linux");
});

afterEach(() => clearMocks());

async function openNote(pictures: Asset[], handlers: Record<string, Handler> = {}) {
  let saved = note(pictures);
  const backend = fakeBackend({
    get_note: () => saved,
    update_note: (args) => {
      saved = {
        ...saved,
        pictures: (args.pictures as number[]).map(
          (id) => [FIRST, SECOND, PASTED].find((asset) => asset.id === id)!,
        ),
      };
      return saved;
    },
    ...handlers,
  });
  render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "Open note" }));
  await screen.findByRole("heading", { name: "Mitosis" });
  return { backend, user };
}

describe("pictures in a note", () => {
  it("are shown under the text, in order, from Synapse's own store", async () => {
    await openNote([FIRST, SECOND]);

    const gallery = screen.getByRole("list", { name: "Pictures in the note" });
    const images = within(gallery).getAllByRole("img") as HTMLImageElement[];
    expect(images.map((image) => image.getAttribute("src"))).toEqual([url(FIRST), url(SECOND)]);
    expect(images.map((image) => image.alt)).toEqual([
      "Picture in the note 1 of 2",
      "Picture in the note 2 of 2",
    ]);
  });

  it("can be added, pasted, reordered and removed, and are saved with the note", async () => {
    const { backend, user } = await openNote([FIRST], {
      choose_picture: () => ({ status: "added", asset: SECOND }),
      store_picture: () => PASTED,
    });

    await user.click(screen.getByRole("button", { name: "Edit note" }));
    await user.click(screen.getByRole("button", { name: "Add picture" }));
    expect(await screen.findByRole("img", { name: "Picture 2 of 2" })).toBeTruthy();

    // A picture pasted while typing in the note lands in its pictures.
    pastePicture(screen.getByLabelText("Note"));
    expect(await screen.findByRole("img", { name: "Picture 3 of 3" })).toBeTruthy();
    expect(backend.argsOf("store_picture")).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "Move picture 3 of 3 earlier" }));
    await user.click(screen.getByRole("button", { name: "Remove picture 1 of 3" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Add picture" }));

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(backend.argsOf("update_note")).toEqual([
      { noteId: 5, title: "Mitosis", body: "Four phases.", pictures: [PASTED.id, SECOND.id] },
    ]);
    const gallery = await screen.findByRole("list", { name: "Pictures in the note" });
    expect(
      within(gallery)
        .getAllByRole("img")
        .map((image) => image.getAttribute("src")),
    ).toEqual([url(PASTED), url(SECOND)]);
  });

  it("count as unsaved changes", async () => {
    const { user } = await openNote([FIRST]);

    await user.click(screen.getByRole("button", { name: "Edit note" }));
    await user.click(screen.getByRole("button", { name: "Remove picture" }));
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    // Cancel discards on purpose, without asking; the note is as it was.
    expect(await screen.findByRole("img", { name: "Picture in the note" })).toBeTruthy();
  });

  it("show why one couldn't be added, and a refused save points at them", async () => {
    const { backend, user } = await openNote([FIRST], {
      choose_picture: () => refuse("invalid", "That file isn't a PNG, JPEG, GIF or WebP picture."),
      update_note: () => refuse("invalid", "A note can show at most 20 pictures.", "pictures"),
    });

    await user.click(screen.getByRole("button", { name: "Edit note" }));
    const add = screen.getByRole("button", { name: "Add picture" });
    await user.click(add);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "That file isn't a PNG, JPEG, GIF or WebP picture.",
    );

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("A note can show at most 20 pictures.")).toBeTruthy();
    expect(document.activeElement).toBe(add);
    expect(backend.argsOf("update_note")).toHaveLength(1);
  });

  it("stands in a short note for a picture that can't be loaded", async () => {
    await openNote([FIRST]);

    fireEvent.error(screen.getByRole("img", { name: "Picture in the note" }));

    expect(screen.getByRole("img", { name: "Picture in the note" }).textContent).toBe(
      "This picture couldn't be shown.",
    );
  });
});

function deck(): Deck {
  return {
    id: 1,
    name: "Anatomy",
    description: null,
    cardCount: 1,
    dueCount: 0,
    cards: [
      {
        id: 10,
        front: "Name the bone",
        back: "Femur",
        pictures: { front: [FIRST], back: [] },
      },
    ],
    deletedCards: [],
  };
}

describe("pictures on a card", () => {
  it("are shown small in the deck's card list", async () => {
    fakeBackend({ get_deck_detail: () => deck() });
    render(
      <DeckDetail
        deckId={1}
        justCreated={false}
        onBack={vi.fn()}
        onStart={vi.fn()}
        onArchived={vi.fn()}
      />,
    );

    const image = await screen.findByRole("img", { name: "Picture on the front" });
    expect(image.getAttribute("src")).toBe(url(FIRST));
  });

  it("are added to either side and saved with the card", async () => {
    const backend = fakeBackend({
      get_deck_detail: () => deck(),
      choose_picture: () => ({ status: "added", asset: SECOND }),
      store_picture: () => PASTED,
      update_flashcard: () => deck(),
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

    await user.click(await screen.findByRole("button", { name: "Edit card" }));
    const front = screen.getByRole("group", { name: "Pictures on the front" });
    const back = screen.getByRole("group", { name: "Pictures on the back" });
    expect(within(front).getByRole("img", { name: "Picture" })).toBeTruthy();

    await user.click(within(back).getByRole("button", { name: "Add picture" }));
    await within(back).findByRole("img", { name: "Picture" });
    // Pasting into the front's text adds to the front.
    pastePicture(screen.getByLabelText("Front"));
    await within(front).findByRole("img", { name: "Picture 2 of 2" });

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(backend.argsOf("update_flashcard")).toEqual([
      {
        cardId: 10,
        front: "Name the bone",
        back: "Femur",
        pictures: { front: [FIRST.id, PASTED.id], back: [SECOND.id] },
      },
    ]);
  });

  it("show in review: the front's with the question, the back's with the answer", async () => {
    fakeBackend({
      get_session_card: () => ({
        status: "due",
        card: { id: 10, front: "Name the bone", back: "Femur", reps: 0 },
        progress: { reviewed: 0, remaining: 1 },
        againStreak: 0,
        offerHelp: false,
        pictures: { front: [FIRST], back: [SECOND] },
      }),
    });
    render(<ReviewSession sessionId={7} deckName="Anatomy" onExit={vi.fn()} />);
    const user = userEvent.setup();

    const question = await screen.findByRole("img", { name: "Picture with the question" });
    expect(question.getAttribute("src")).toBe(url(FIRST));
    expect(screen.queryByRole("img", { name: "Picture with the answer" })).toBeNull();

    await user.keyboard("[Space]");
    const answer = await screen.findByRole("img", { name: "Picture with the answer" });
    expect(answer.getAttribute("src")).toBe(url(SECOND));
  });
});
