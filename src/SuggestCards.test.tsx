import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CardText, SuggestedCard, SuggestOutcome } from "./api";
import { NotesArea } from "./Notes";
import { deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const PHASES: SuggestedCard = {
  front: "What are the phases of mitosis?",
  back: "Prophase, metaphase, anaphase, telophase.",
  source: "Prophase, metaphase, anaphase, telophase.",
  unfamiliar: [],
};
const EQUATOR: SuggestedCard = {
  front: "Where do chromosomes line up in metaphase?",
  back: "At the cell's equator.",
  source: "In metaphase, chromosomes line up at the cell's equator.",
  unfamiliar: [],
};
const RESULT: SuggestedCard = {
  front: "What does mitosis produce?",
  back: "Two identical daughter cells, unlike meiosis.",
  source: "Mitosis produces two identical daughter cells.",
  unfamiliar: ["unlike", "meiosis"],
};

function suggested(cards: SuggestedCard[], leftOut = 0, unsupported = 0): SuggestOutcome {
  return { status: "suggested", cards, leftOut, unsupported };
}

/** What is saved of a suggested card: its text only. */
function text({ front, back }: CardText): CardText {
  return { front, back };
}

/** Opens the Mitosis note and chooses *Suggest cards* on it. */
async function openSuggestions(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend(handlers);
  const user = userEvent.setup();
  const view = render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} />);
  await user.click(await screen.findByRole("button", { name: "Open note" }));
  await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
  return { ...backend, ...view, user };
}

function modelList() {
  return screen.getByLabelText("Model") as HTMLSelectElement;
}

beforeEach(() => {
  localStorage.clear();
});

describe("suggesting cards from a note", () => {
  it("asks the chosen model, then saves the checked cards into the chosen deck", async () => {
    const { user, argsOf } = await openSuggestions({
      get_models: () => ["llama3.2:3b", "qwen3:4b"],
      suggest_cards: () => suggested([PHASES, EQUATOR, RESULT], 1, 2),
    });

    // Two models and none used before, so the user chooses.
    expect(document.activeElement).toBe(await screen.findByLabelText("Model"));
    expect(modelList().value).toBe("");
    await user.selectOptions(modelList(), "qwen3:4b");
    await user.click(screen.getByRole("button", { name: "Suggest cards" }));

    const notice = await screen.findByText(/^qwen3:4b suggested 3 cards\./);
    expect(document.activeElement).toBe(notice);
    expect(notice.textContent).toContain(
      "2 more suggestions were left out because they couldn't be traced back to your note."
    );
    expect(notice.textContent).toContain("1 more was left out because it was empty");
    expect(argsOf("suggest_cards")).toEqual([{ noteId: 5, model: "qwen3:4b" }]);

    // Edit the first card, and remove the second.
    const firstFront = screen.getAllByLabelText("Front")[0];
    await user.clear(firstFront);
    await user.type(firstFront, "Name the four phases of mitosis.");
    const second = screen.getByRole("group", { name: "Card 2" });
    await user.click(within(second).getByRole("button", { name: "Remove card" }));
    expect(document.activeElement).toBe(screen.getByText("Card removed. 2 cards left."));
    expect(screen.queryByRole("group", { name: "Card 3" })).toBeNull();

    await user.selectOptions(screen.getByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Save 2 cards" }));

    expect(
      await screen.findByText("2 cards added to Biology. They're due for review now.")
    ).toBe(document.activeElement);
    expect(screen.getByRole("button", { name: "Open Biology" })).toBeTruthy();
    expect(argsOf("create_flashcards")).toEqual([
      { deckId: 1, cards: [text({ ...PHASES, front: "Name the four phases of mitosis." }), text(RESULT)] },
    ]);
  });

  it("asks for a model before suggesting, and offers the one that worked next time", async () => {
    const { user, argsOf, unmount } = await openSuggestions({
      get_models: () => ["llama3.2:3b", "qwen3:4b"],
    });

    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));

    expect(screen.getByText("Choose a model to suggest cards with.")).toBeTruthy();
    expect(document.activeElement).toBe(modelList());
    expect(argsOf("suggest_cards")).toEqual([]);

    await user.selectOptions(modelList(), "qwen3:4b");
    expect(screen.queryByText("Choose a model to suggest cards with.")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Suggest cards" }));
    await screen.findByText(/^qwen3:4b suggested 1 card\./);
    unmount();

    await openSuggestions({ get_models: () => ["llama3.2:3b", "qwen3:4b"] });
    await screen.findByLabelText("Model");
    expect(modelList().value).toBe("qwen3:4b");
  });

  it("offers the only model there is without asking", async () => {
    await openSuggestions();
    await screen.findByLabelText("Model");
    expect(modelList().value).toBe("llama3.2:3b");
  });

  it("says when Ollama can't be reached, and can try again", async () => {
    let running = false;
    const { user } = await openSuggestions({
      get_models: () =>
        running
          ? ["llama3.2:3b"]
          : refuse("failed", "Synapse couldn't reach Ollama on this computer."),
    });

    const message = await screen.findByText("Synapse couldn't reach Ollama on this computer.");
    expect(document.activeElement).toBe(message);

    running = true;
    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByLabelText("Model")).toBeTruthy();
  });

  it("says when Ollama has no models, and can check again", async () => {
    let models: string[] = [];
    const { user } = await openSuggestions({ get_models: () => models });

    const message = await screen.findByText(/^Ollama is running, but it has no models/);
    expect(document.activeElement).toBe(message);

    models = ["llama3.2:3b"];
    await user.click(screen.getByRole("button", { name: "Check again" }));

    expect(await screen.findByLabelText("Model")).toBeTruthy();
  });

  it("offers a note too long to use whole in parts, and suggests from the one chosen", async () => {
    const page = (n: number) => `Page ${n}\n${"Cells divide and grow. ".repeat(250).trim()}`;
    const body = [1, 2, 3].map(page).join("\n\n");
    const { user, argsOf } = await openSuggestions({
      get_note: () => ({
        id: 5,
        title: "Cell biology",
        body,
        createdAt: "2026-10-01T09:00:00Z",
        updatedAt: "2026-10-01T09:00:00Z",
      }),
    });

    const partList = (await screen.findByLabelText("Part of the note")) as HTMLSelectElement;
    expect(document.activeElement).toBe(partList);
    expect(screen.getByText(/split into 2 parts where paragraphs end/)).toBeTruthy();
    const options = within(partList)
      .getAllByRole("option")
      .map((option) => option.textContent);
    expect(options).toEqual(["Choose a part", "Part 1 of 2: Page 1", "Part 2 of 2: Page 3"]);
    expect(partList.value).toBe("0");
    expect(screen.getByText("Your note, part 1 of 2")).toBeTruthy();

    await user.selectOptions(partList, "1");
    expect(screen.getByText("Your note, part 2 of 2")).toBeTruthy();
    expect(document.querySelector(".note-context .note-body")?.textContent).toBe(page(3));
    await user.click(screen.getByRole("button", { name: "Suggest cards" }));
    await screen.findByText(/^llama3\.2:3b suggested 1 card\./);

    const start = Array.from(body).length - Array.from(page(3)).length;
    expect(argsOf("suggest_cards")).toEqual([
      { noteId: 5, model: "llama3.2:3b", part: { start, end: Array.from(body).length } },
    ]);
  });

  it("goes back to the note on Cancel, with focus on Suggest cards", async () => {
    const { user } = await openSuggestions();
    await user.click(await screen.findByRole("button", { name: "Cancel" }));

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Suggest cards" }));
    expect(screen.getByRole("heading", { name: "Mitosis" })).toBeTruthy();
  });
});

describe("waiting for a suggestion", () => {
  it("shows the model at work, and Stop ends it with nothing suggested", async () => {
    const answer = deferred<SuggestOutcome>();
    const { user, calls } = await openSuggestions({
      suggest_cards: () => answer.promise,
      stop_suggesting: () => {
        answer.resolve({ status: "stopped" });
        return null;
      },
    });
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));

    expect(document.activeElement).toBe(screen.getByText(/^Suggesting cards with llama3\.2:3b…/));
    // A second press while it works sends nothing more.
    expect(screen.queryByRole("button", { name: "Suggest cards" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Stop" }));

    const notice = await screen.findByText("Stopped. No cards were suggested.");
    expect(document.activeElement).toBe(notice);
    expect(calls.filter((cmd) => cmd === "stop_suggesting")).toHaveLength(1);
    expect(calls.filter((cmd) => cmd === "suggest_cards")).toHaveLength(1);
  });

  it("asks once however quickly Suggest is pressed twice", async () => {
    const answer = deferred<SuggestOutcome>();
    const { argsOf } = await openSuggestions({ suggest_cards: () => answer.promise });
    const form = await screen.findByRole("form", { name: "Choose a model" });

    // Both before React re-renders, as a held Enter can.
    act(() => {
      fireEvent.submit(form);
      fireEvent.submit(form);
    });

    await screen.findByText(/^Suggesting cards with/);
    expect(argsOf("suggest_cards")).toHaveLength(1);
  });

  it("stops the model if the screen closes while it works", async () => {
    const answer = deferred<SuggestOutcome>();
    const { user, calls, unmount } = await openSuggestions({
      suggest_cards: () => answer.promise,
    });
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await screen.findByText(/^Suggesting cards with/);
    expect(calls).not.toContain("stop_suggesting");

    unmount();

    expect(calls).toContain("stop_suggesting");
    // The late answer lands on nothing.
    await act(async () => answer.resolve(suggested([PHASES])));
  });

  it("keeps the model chosen and says why when a suggestion fails", async () => {
    const { user } = await openSuggestions({
      get_models: () => ["llama3.2:3b", "qwen3:4b"],
      suggest_cards: () =>
        refuse("failed", "The model took too long to answer, so Synapse stopped waiting."),
    });
    await user.selectOptions(await screen.findByLabelText("Model"), "qwen3:4b");
    await user.click(screen.getByRole("button", { name: "Suggest cards" }));

    expect(
      (await screen.findByRole("alert")).textContent
    ).toBe("The model took too long to answer, so Synapse stopped waiting.");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Suggest cards" }));
    expect(modelList().value).toBe("qwen3:4b");
  });

  it("takes a model that has left Ollama off the list", async () => {
    let gone = false;
    const { user } = await openSuggestions({
      get_models: () => (gone ? ["qwen3:4b"] : ["llama3.2:3b", "qwen3:4b"]),
      suggest_cards: () => {
        gone = true;
        return refuse("stale", "That model isn't in Ollama any more. Choose another.");
      },
    });
    await user.selectOptions(await screen.findByLabelText("Model"), "llama3.2:3b");
    await user.click(screen.getByRole("button", { name: "Suggest cards" }));

    expect(
      await screen.findByText("That model isn't in Ollama any more. Choose another.")
    ).toBeTruthy();
    expect(document.activeElement).toBe(modelList());
    expect(modelList().value).toBe("");
    const options = Array.from(modelList().options).map((option) => option.value);
    expect(options).toEqual(["", "qwen3:4b"]);
  });
});

describe("checking suggested cards", () => {
  it("shows each card's passage from the note, and any words the note doesn't use", async () => {
    const { user } = await openSuggestions({
      suggest_cards: () => suggested([EQUATOR, RESULT]),
    });
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));

    const first = await screen.findByRole("group", { name: "Card 1" });
    expect(within(first).getByText(EQUATOR.source)).toBeTruthy();
    expect(within(first).queryByText(/^Not in your note/)).toBeNull();

    const second = screen.getByRole("group", { name: "Card 2" });
    expect(within(second).getByText(RESULT.source)).toBeTruthy();
    expect(
      within(second).getByText(
        "Not in your note: unlike, meiosis. The suggested answer may say more than your note does."
      )
    ).toBeTruthy();
  });

  it("keeps a refused card's error with that card, and gives it focus", async () => {
    const { user, argsOf } = await openSuggestions({
      suggest_cards: () => suggested([PHASES, EQUATOR, RESULT]),
      create_flashcards: () => refuse("invalid", "Enter the back of the card.", "back", 1),
    });
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    const first = await screen.findByRole("group", { name: "Card 1" });
    await user.click(within(first).getByRole("button", { name: "Remove card" }));
    await user.selectOptions(screen.getByLabelText("Deck"), "Biology");

    await user.click(screen.getByRole("button", { name: "Save 2 cards" }));

    // Index 1 of what was sent: the card now shown second, once the first
    // was removed.
    expect(argsOf("create_flashcards")[0].cards).toEqual([text(EQUATOR), text(RESULT)]);
    const refused = screen.getAllByLabelText("Back")[1];
    expect(await screen.findByText("Enter the back of the card.")).toBeTruthy();
    expect(document.activeElement).toBe(refused);
    expect(refused.getAttribute("aria-invalid")).toBe("true");
    expect(screen.getAllByLabelText("Back")[0].getAttribute("aria-invalid")).toBeNull();

    await user.type(refused, " Fixed.");
    expect(screen.queryByText("Enter the back of the card.")).toBeNull();
  });

  it("asks for a deck before saving", async () => {
    const { user, argsOf } = await openSuggestions();
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await user.click(await screen.findByRole("button", { name: "Save 1 card" }));

    expect(screen.getByText("Choose a deck for these cards.")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Deck"));
    expect(argsOf("create_flashcards")).toEqual([]);
  });

  it("shows a deck refusal under the deck list, and reloads the decks", async () => {
    const { user, calls } = await openSuggestions({
      create_flashcards: () => refuse("stale", "This deck has been archived."),
    });
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    const loads = calls.filter((cmd) => cmd === "get_decks").length;

    await user.click(screen.getByRole("button", { name: "Save 1 card" }));

    expect(await screen.findByText("This deck has been archived.")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Deck"));
    await waitFor(() =>
      expect(calls.filter((cmd) => cmd === "get_decks").length).toBe(loads + 1)
    );
  });

  it("saves once however often Save is pressed", async () => {
    const saved = deferred();
    const { user, argsOf } = await openSuggestions({ create_flashcards: () => saved.promise });
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    const save = screen.getByRole("button", { name: "Save 1 card" });

    await user.click(save);
    await user.click(save);

    expect(argsOf("create_flashcards")).toHaveLength(1);
  });

  it("goes back to choosing a model once every card is removed", async () => {
    const { user, argsOf } = await openSuggestions();
    await user.click(await screen.findByRole("button", { name: "Suggest cards" }));
    await user.click(await screen.findByRole("button", { name: "Remove card" }));

    expect(document.activeElement).toBe(
      screen.getByText(/^You removed every suggested card, so there's nothing to save\./)
    );
    expect(screen.getByLabelText("Model")).toBeTruthy();
    expect(argsOf("create_flashcards")).toEqual([]);
  });
});
