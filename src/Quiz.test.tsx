import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { Note, QuizOutcome, QuizQuestion } from "./api";
import { NotesArea } from "./Notes";
import { shuffled } from "./Quiz";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const FIRST: QuizQuestion = {
  question: "Which phase comes first in mitosis?",
  answer: "Prophase",
  distractors: ["Metaphase", "Anaphase", "Telophase"],
  source: "Prophase, metaphase, anaphase, telophase.",
  unfamiliar: [],
};
const EQUATOR: QuizQuestion = {
  question: "Where do chromosomes line up in metaphase?",
  answer: "At the cell's equator",
  distractors: ["At the poles", "In the nucleus", "Outside the cell"],
  source: "In metaphase, chromosomes line up at the cell's equator.",
  unfamiliar: ["outside"],
};

function written(questions: QuizQuestion[], leftOut = 0, unsupported = 0): QuizOutcome {
  return { status: "written", questions, leftOut, unsupported };
}

/** Opens the Mitosis note and chooses *Quiz me* on it. */
async function openQuiz(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend(handlers);
  const user = userEvent.setup();
  const view = render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} />);
  await user.click(await screen.findByRole("button", { name: "Open note" }));
  await user.click(await screen.findByRole("button", { name: "Quiz me" }));
  return { ...backend, ...view, user };
}

/** Starts the quiz with the only model there is, and waits for the first question. */
async function start(user: UserEvent) {
  await user.click(await screen.findByRole("button", { name: "Start quiz" }));
  await screen.findByText("Question 1 of", { exact: false });
}

function option(name: string) {
  return screen.getByRole("button", { name: new RegExp(`^\\d?\\s*${name}`) });
}

beforeEach(() => {
  localStorage.clear();
  // With every draw 0, the shuffle puts the wrong answers first, in order,
  // and the right one last: option 4.
  vi.spyOn(Math, "random").mockReturnValue(0);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("shuffling a question's options", () => {
  it("keeps every option once and knows where the right one went", () => {
    const draws = [0.9, 0.1, 0.6];
    const asked = shuffled(FIRST, () => draws.shift()!);
    expect([...asked.options].sort()).toEqual(
      [FIRST.answer, ...FIRST.distractors].sort()
    );
    expect(asked.options[asked.correct]).toBe("Prophase");
    expect(asked.options).not.toEqual([FIRST.answer, ...FIRST.distractors]);
  });
});

describe("taking a quiz on a note", () => {
  it("asks one question at a time, shows the passage after each answer, then adds a missed one as a card", async () => {
    const { user, argsOf } = await openQuiz({
      quiz_note: () => written([FIRST, EQUATOR], 1, 2),
    });

    // The only model is offered without asking.
    expect((await screen.findByLabelText("Model") as HTMLSelectElement).value).toBe("llama3.2:3b");
    await start(user);
    expect(argsOf("quiz_note")).toEqual([{ noteId: 5, model: "llama3.2:3b" }]);

    const intro = screen.getByText(/^llama3\.2:3b wrote 2 questions\./);
    await waitFor(() => expect(document.activeElement).toBe(intro));
    expect(intro.textContent).toContain(
      "2 more questions were left out because they couldn't be traced back to your note."
    );
    expect(intro.textContent).toContain("1 more was left out because it was empty");
    expect(screen.getByText("Question 1 of 2")).toBeTruthy();
    expect(screen.getByText(FIRST.question)).toBeTruthy();
    // No passage until the question is answered.
    expect(screen.queryByText(FIRST.source)).toBeNull();
    const options = within(screen.getByRole("group", { name: "Choose the right answer" }))
      .getAllByRole("button")
      .map((button) => button.textContent);
    expect(options).toEqual(["1Metaphase", "2Anaphase", "3Telophase", "4Prophase"]);

    // 1 chooses Metaphase: wrong.
    await user.keyboard("1");
    const missed = await screen.findByText("Not quite. The right answer is: Prophase");
    await waitFor(() => expect(document.activeElement).toBe(missed));
    expect(screen.getByText(FIRST.source)).toBeTruthy();
    expect(option("Metaphase").getAttribute("aria-pressed")).toBe("true");
    expect(option("Metaphase").textContent).toContain("Your answer");
    expect(option("Prophase").textContent).toContain("Right answer");
    // Answered already: another number changes nothing.
    await user.keyboard("4");
    expect(screen.getByText("Not quite. The right answer is: Prophase")).toBeTruthy();

    // Enter moves on.
    await user.keyboard("{Enter}");
    expect(await screen.findByText("Question 2 of 2")).toBeTruthy();
    await user.click(option("At the cell's equator"));
    await screen.findByText("Right.");
    expect(screen.getByText("Not in your note: outside. The answer may say more than your note does.")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "See results" }));

    const score = await screen.findByText("You got 1 of 2 questions right.");
    await waitFor(() => expect(document.activeElement).toBe(score));
    const item = screen.getByRole("group", { name: `1. ${FIRST.question}` });
    expect(within(item).getByText("Metaphase")).toBeTruthy();
    expect(within(item).getByText("Prophase")).toBeTruthy();
    expect(within(item).getByRole("checkbox", { name: "Add as a card" })).toHaveProperty("checked", true);
    // The question answered right isn't offered.
    expect(screen.queryByRole("group", { name: new RegExp(EQUATOR.question) })).toBeNull();

    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Add 1 card" }));

    expect(await screen.findByText("1 card added to Biology. It's due for review now.")).toBeTruthy();
    expect(argsOf("create_flashcards")).toEqual([
      { deckId: 1, cards: [{ front: FIRST.question, back: "Prophase" }] },
    ]);
  });

  it("ignores modified, held and unused number keys", async () => {
    const { user } = await openQuiz();
    await start(user);

    fireEvent.keyDown(document.body, { key: "1", ctrlKey: true });
    fireEvent.keyDown(document.body, { key: "2", altKey: true });
    fireEvent.keyDown(document.body, { key: "3", repeat: true });
    fireEvent.keyDown(document.body, { key: "5" });
    expect(screen.queryByText(/^Not quite|^Right\./)).toBeNull();

    // Nothing to type into here, so press the right one for real.
    await user.keyboard("4");
    expect(await screen.findByText("Right.")).toBeTruthy();
  });

  it("says when every question was right, with nothing to add", async () => {
    const { user, argsOf } = await openQuiz();
    await start(user);
    await user.keyboard("4");
    await user.click(await screen.findByRole("button", { name: "See results" }));

    expect(await screen.findByText("You got all 1 question right.")).toBeTruthy();
    expect(screen.queryByRole("form", { name: "Missed questions" })).toBeNull();
    expect(screen.queryByLabelText("Deck")).toBeNull();

    await user.click(screen.getByRole("button", { name: "New quiz" }));
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Start quiz" }))
    );
    expect(argsOf("create_flashcards")).toEqual([]);
  });

  it("adds only the ticked missed questions, and asks for a deck and at least one", async () => {
    const { user, argsOf } = await openQuiz({ quiz_note: () => written([FIRST, EQUATOR]) });
    await start(user);
    await user.keyboard("1");
    await user.keyboard("{Enter}");
    await screen.findByText("Question 2 of 2");
    await user.keyboard("2");
    await user.click(await screen.findByRole("button", { name: "See results" }));
    await screen.findByText("You got 0 of 2 questions right.");

    await user.click(screen.getByRole("button", { name: "Add 2 cards" }));
    expect(await screen.findByText("Choose a deck for these cards.")).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Deck")));

    const boxes = screen.getAllByRole("checkbox", { name: "Add as a card" });
    await user.click(boxes[0]);
    await user.click(boxes[1]);
    await user.selectOptions(screen.getByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Add 0 cards" }));
    expect(await screen.findByText("Tick at least one question to add as a card.")).toBeTruthy();
    expect(argsOf("create_flashcards")).toEqual([]);

    await user.click(boxes[1]);
    await user.click(screen.getByRole("button", { name: "Add 1 card" }));
    await screen.findByText("1 card added to Biology. It's due for review now.");
    expect(argsOf("create_flashcards")).toEqual([
      { deckId: 1, cards: [{ front: EQUATOR.question, back: EQUATOR.answer }] },
    ]);
  });

  it("keeps the missed questions when the deck is refused, and reloads the decks", async () => {
    let decksAsked = 0;
    const { user } = await openQuiz({
      get_decks: () => {
        decksAsked += 1;
        return [{ id: 1, name: "Biology", description: null, isSample: false, dueCount: 0 }];
      },
      create_flashcards: () => refuse("stale", "That deck was archived. Choose another."),
    });
    await start(user);
    await user.keyboard("1");
    await user.click(await screen.findByRole("button", { name: "See results" }));
    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    const asked = decksAsked;
    await user.click(screen.getByRole("button", { name: "Add 1 card" }));

    expect(await screen.findByText("That deck was archived. Choose another.")).toBeTruthy();
    await waitFor(() => expect(decksAsked).toBe(asked + 1));
    expect(screen.getByRole("group", { name: `1. ${FIRST.question}` })).toBeTruthy();
  });

  it("can be stopped while the quiz is written, which tells Ollama", async () => {
    const answer = deferred<QuizOutcome>();
    const { user, calls } = await openQuiz({
      quiz_note: () => answer.promise,
      stop_quiz: () => {
        answer.resolve({ status: "stopped" });
        return null;
      },
    });
    await user.click(await screen.findByRole("button", { name: "Start quiz" }));
    const waiting = await screen.findByText(/^Writing a quiz with llama3\.2:3b…/);
    await waitFor(() => expect(document.activeElement).toBe(waiting));

    await user.click(screen.getByRole("button", { name: "Stop" }));
    const stopped = await screen.findByText("Stopped. No quiz was written.");
    await waitFor(() => expect(document.activeElement).toBe(stopped));
    expect(calls).toContain("stop_quiz");
  });

  it("stops the quiz being written when the screen is left", async () => {
    const answer = deferred<QuizOutcome>();
    const { user, calls, unmount } = await openQuiz({ quiz_note: () => answer.promise });
    await user.click(await screen.findByRole("button", { name: "Start quiz" }));
    await screen.findByText(/^Writing a quiz/);
    unmount();
    expect(calls.filter((cmd) => cmd === "stop_quiz")).toHaveLength(1);
    await act(async () => answer.resolve({ status: "stopped" }));
  });

  it("shows why no quiz was written, and offers the list again when the model is gone", async () => {
    let models = ["llama3.2:3b", "qwen3:4b"];
    const { user } = await openQuiz({
      get_models: () => models,
      quiz_note: () => {
        models = ["qwen3:4b"];
        return refuse("stale", "That model isn't in Ollama any more. Choose another.");
      },
    });
    await user.selectOptions(await screen.findByLabelText("Model"), "llama3.2:3b");
    await user.click(screen.getByRole("button", { name: "Start quiz" }));

    expect(
      await screen.findByText("That model isn't in Ollama any more. Choose another.")
    ).toBeTruthy();
    const list = screen.getByLabelText("Model") as HTMLSelectElement;
    await waitFor(() => expect(document.activeElement).toBe(list));
    expect(Array.from(list.options).map((each) => each.value)).toEqual(["", "qwen3:4b"]);
  });

  it("reports a failure next to Start quiz, which keeps focus", async () => {
    const { user } = await openQuiz({
      quiz_note: () =>
        refuse("failed", "None of the questions the model wrote could be traced back to your note."),
    });
    await user.click(await screen.findByRole("button", { name: "Start quiz" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("could be traced back");
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Start quiz" }))
    );
  });

  it("offers a long note in parts and quizzes on the part chosen", async () => {
    const paragraph = "Cells divide. ".repeat(500).trim();
    const long: Note = {
      id: 5,
      title: "Mitosis",
      body: `${paragraph}\n\n${paragraph}`,
      createdAt: CREATED,
      updatedAt: CREATED,
    };
    const { user, argsOf } = await openQuiz({ get_note: () => long });
    const parts = (await screen.findByLabelText("Part of the note")) as HTMLSelectElement;
    expect(parts.options).toHaveLength(3);
    await user.selectOptions(parts, "1");
    await start(user);
    const length = Array.from(paragraph).length;
    expect(argsOf("quiz_note")).toEqual([
      { noteId: 5, model: "llama3.2:3b", part: { start: length + 2, end: 2 * length + 2 } },
    ]);
  });
});

describe("leaving a quiz", () => {
  function sidebar() {
    return within(screen.getByRole("navigation", { name: "Synapse" }));
  }

  function question() {
    return screen.getByRole("alertdialog", { hidden: true }) as HTMLDialogElement;
  }

  async function openAppQuiz() {
    fakeBackend({ quiz_note: () => written([FIRST, EQUATOR]) });
    const user = userEvent.setup();
    render(<App />);
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await user.click(await screen.findByRole("button", { name: "Quiz me" }));
    await start(user);
    return user;
  }

  it("leaves without asking while nothing has been missed", async () => {
    const user = await openAppQuiz();
    await user.keyboard("4");
    await screen.findByText("Right.");

    await user.click(sidebar().getByRole("button", { name: "Archived decks" }));
    expect(question().open).toBe(false);
    expect(await screen.findByRole("heading", { name: "Archived decks" })).toBeTruthy();
  });

  it("asks first once a question has been missed", async () => {
    const user = await openAppQuiz();
    await user.keyboard("1");
    await screen.findByText(/^Not quite/);

    await user.click(sidebar().getByRole("button", { name: "Archived decks" }));
    expect(question().open).toBe(true);
  });

  it("stops asking once the missed questions are added as cards", async () => {
    const user = await openAppQuiz();
    await user.keyboard("1");
    await user.keyboard("{Enter}");
    await screen.findByText("Question 2 of 2");
    await user.keyboard("4");
    await user.click(await screen.findByRole("button", { name: "See results" }));
    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Add 1 card" }));
    await screen.findByText("1 card added to Biology. It's due for review now.");

    await user.click(sidebar().getByRole("button", { name: "Archived decks" }));
    expect(question().open).toBe(false);
  });
});
