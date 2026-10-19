import { render, screen, waitFor } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Flashcard } from "./api";
import { helpNote } from "./CardHelp";
import { ReviewSession } from "./ReviewSession";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const HARD: Flashcard = {
  id: 4,
  front: "What does the mitochondrion do?",
  back: "It releases energy from food for the cell.",
  reps: 3,
};
const NEXT: Flashcard = { id: 5, front: "What is DNA?", back: "The molecule of genes.", reps: 0 };

/**
 * Opens a review of `HARD` then `NEXT`. `againStreak` is what Rust says about
 * `HARD`; help is offered from three. `overrides` replace backend commands.
 */
function startSession(againStreak: number, overrides: Record<string, Handler> = {}) {
  const cards = [HARD, NEXT];
  let reviewed = 0;
  const backend = fakeBackend({
    get_session_card: () =>
      reviewed < cards.length
        ? {
            status: "due",
            card: cards[reviewed],
            progress: { reviewed, remaining: cards.length - reviewed },
            againStreak: reviewed === 0 ? againStreak : 0,
            offerHelp: reviewed === 0 && againStreak >= 3,
          }
        : { status: "completed", cardsReviewed: reviewed },
    review_card: () => {
      reviewed += 1;
      return null;
    },
    create_note: (args) => {
      const { title, body } = args as { title: string; body: string };
      return { id: 9, title, body, createdAt: CREATED, updatedAt: CREATED };
    },
    ...overrides,
  });
  render(<ReviewSession sessionId={7} deckName="Biology" onExit={vi.fn()} />);
  return backend;
}

async function reveal(user: UserEvent) {
  await screen.findByText(HARD.front);
  await user.keyboard("[Space]");
  await screen.findByText(HARD.back);
}

describe("help with a card", () => {
  it("is offered after three Agains in a row, only once the answer is shown", async () => {
    startSession(3);
    const user = userEvent.setup();
    await screen.findByText(HARD.front);
    expect(screen.queryByRole("button", { name: "Get help with this card" })).toBeNull();

    await reveal(user);

    expect(screen.getByText(/answered Again on this card 3 times in a row/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Get help with this card" })).toBeTruthy();
  });

  it("can be asked for on any card after reveal, without being offered", async () => {
    startSession(2);
    const user = userEvent.setup();
    await reveal(user);

    expect(screen.queryByText(/times in a row/)).toBeNull();
    expect(screen.getByRole("button", { name: "Get help with this card" })).toBeTruthy();
  });

  it("asks the chosen model for the chosen kind of help, and shows it as a draft", async () => {
    const backend = startSession(3);
    const user = userEvent.setup();
    await reveal(user);

    await user.click(screen.getByRole("button", { name: "Get help with this card" }));
    await user.click(await screen.findByRole("radio", { name: /Analogy/ }));
    await user.click(screen.getByRole("button", { name: "Write help" }));

    expect(await screen.findByText("Mighty Mito makes the power.")).toBeTruthy();
    expect(backend.argsOf("get_card_help")).toEqual([
      { cardId: HARD.id, model: "llama3.2:3b", kind: "analogy" },
    ]);
    // A draft: nothing saved yet.
    expect(backend.argsOf("create_note")).toEqual([]);
  });

  it("is kept only when added as a note, through the normal note path", async () => {
    const backend = startSession(3);
    const user = userEvent.setup();
    await reveal(user);
    await user.click(screen.getByRole("button", { name: "Get help with this card" }));
    await user.click(await screen.findByRole("button", { name: "Write help" }));
    await screen.findByText("Mighty Mito makes the power.");

    await user.click(screen.getByRole("button", { name: "Add as a note" }));

    expect(await screen.findByText(`Added to your notes as “Help: ${HARD.front}”.`)).toBeTruthy();
    expect(backend.argsOf("create_note")).toEqual([
      {
        title: `Help: ${HARD.front}`,
        body: `Mnemonic\n\nMighty Mito makes the power.\n\nFrom the card\nQuestion: ${HARD.front}\nAnswer: ${HARD.back}`,
      },
    ]);
    expect(screen.queryByRole("button", { name: "Add as a note" })).toBeNull();
  });

  it("never holds up rating: rating while help is written stops it and moves on", async () => {
    const pending = deferred();
    const backend = startSession(3, { get_card_help: () => pending.promise });
    const user = userEvent.setup();
    await reveal(user);
    await user.click(screen.getByRole("button", { name: "Get help with this card" }));
    await user.click(await screen.findByRole("button", { name: "Write help" }));
    await screen.findByText(/Writing a mnemonic with llama3.2:3b/);

    await user.click(screen.getByRole("button", { name: "Good" }));

    expect(await screen.findByText(NEXT.front)).toBeTruthy();
    expect(backend.argsOf("review_card").map((args) => args.rating)).toEqual([3]);
    await waitFor(() => expect(backend.calls).toContain("stop_card_help"));
    pending.resolve({ status: "stopped" });
  });

  it("can be stopped, going back to the choice", async () => {
    const pending = deferred();
    const backend = startSession(3, {
      get_card_help: () => pending.promise,
      stop_card_help: () => {
        pending.resolve({ status: "stopped" });
        return null;
      },
    });
    const user = userEvent.setup();
    await reveal(user);
    await user.click(screen.getByRole("button", { name: "Get help with this card" }));
    await user.click(await screen.findByRole("button", { name: "Write help" }));

    await user.click(await screen.findByRole("button", { name: "Stop" }));

    expect(await screen.findByRole("button", { name: "Write help" })).toBeTruthy();
    expect(backend.calls).toContain("stop_card_help");
  });

  it("says when Ollama can't be reached, and reviewing carries on as before", async () => {
    const backend = startSession(3, {
      get_models: () => refuse("failed", "Synapse couldn't reach Ollama on this computer."),
    });
    const user = userEvent.setup();
    await reveal(user);

    await user.click(screen.getByRole("button", { name: "Get help with this card" }));

    expect(await screen.findByText("Synapse couldn't reach Ollama on this computer.")).toBeTruthy();
    await user.keyboard("1");
    expect(await screen.findByText(NEXT.front)).toBeTruthy();
    expect(backend.argsOf("review_card").map((args) => args.rating)).toEqual([1]);
  });

  it("shows why help couldn't be written, and offers the models there now", async () => {
    startSession(3, {
      get_card_help: () => refuse("stale", "That model isn't in Ollama any more. Choose another."),
    });
    const user = userEvent.setup();
    await reveal(user);
    await user.click(screen.getByRole("button", { name: "Get help with this card" }));
    await user.click(await screen.findByRole("button", { name: "Write help" }));

    expect(
      await screen.findByText("That model isn't in Ollama any more. Choose another.")
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Write help" })).toBeTruthy();
  });
});

describe("the note that keeps help", () => {
  it("is titled after the card on one line, within a note title's limit", () => {
    const card = { ...HARD, front: `Why\n  ${"x".repeat(300)}?` };
    const note = helpNote(card, { kind: "question", text: "Asked another way?", unfamiliar: [] });
    expect(note.title.startsWith("Help: Why xxx")).toBe(true);
    expect(Array.from(note.title)).toHaveLength(200);
    expect(note.body.startsWith("Reframed question\n\nAsked another way?")).toBe(true);
  });
});
