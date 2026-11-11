import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { GradeOutcome, Note, QuestionsOutcome, SummaryOutcome } from "./api";
import { NotesArea } from "./Notes";
import { summaryBody, summaryTitle } from "./Tutor";
import { deferred, fakeBackend, refuse, type Handler } from "./test/backend";

/** Opens the Mitosis note and chooses *Summarise* on it. */
async function openTutor(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend(handlers);
  const user = userEvent.setup();
  const view = render(<NotesArea onBack={vi.fn()} onOpenDeck={vi.fn()} onOpenSource={vi.fn()} />);
  await user.click(await screen.findByRole("button", { name: "Open note" }));
  await user.click(await screen.findByRole("button", { name: "Summarise" }));
  return { ...backend, ...view, user };
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the summary text", () => {
  it("lists the bullets, then the key terms", () => {
    expect(
      summaryBody(
        [
          { text: "One.", quote: "q" },
          { text: "Two.", quote: "q" },
        ],
        [
          { term: "A", quote: "q" },
          { term: "B", quote: "q" },
        ]
      )
    ).toBe("- One.\n- Two.\n\nKey terms: A, B");
    expect(summaryBody([{ text: "One.", quote: "q" }], [])).toBe("- One.");
  });

  it("makes a title that fits a note's", () => {
    expect(summaryTitle("Mitosis")).toBe("Summary: Mitosis");
    expect(Array.from(summaryTitle("x".repeat(500))).length).toBe(200);
  });
});

describe("summarising a note", () => {
  it("shows the bullets and key terms with the passages they come from", async () => {
    const { user, calls } = await openTutor();
    await user.click(await screen.findByRole("button", { name: /^Summarise$/ }));

    expect(await screen.findByText(/wrote 1 point and 1 key term\./)).toBeTruthy();
    expect(screen.getByText("Mitosis makes two identical cells.")).toBeTruthy();
    expect(
      screen.getByText("From your note: “It produces two genetically identical daughter cells.”")
    ).toBeTruthy();
    expect(screen.getByText("Mitosis")).toBeTruthy();
    expect(calls).toContain("summarize_note");
  });

  it("says how many points were left out", async () => {
    const { user } = await openTutor({
      summarize_note: () => ({
        status: "written",
        bullets: [{ text: "Kept.", quote: "Kept." }],
        keyTerms: [],
        dropped: 2,
        leftOut: 1,
      }),
    });
    await user.click(await screen.findByRole("button", { name: /^Summarise$/ }));
    expect(
      await screen.findByText(/2 points were left out because their passages couldn't be found/)
    ).toBeTruthy();
    expect(screen.getByText(/1 more was left out/)).toBeTruthy();
  });

  it("can be stopped while it's written, which tells Ollama", async () => {
    const answer = deferred<SummaryOutcome>();
    const { user, calls } = await openTutor({
      summarize_note: () => answer.promise,
      stop_tutor: () => {
        answer.resolve({ status: "stopped" });
        return null;
      },
    });
    await user.click(await screen.findByRole("button", { name: /^Summarise$/ }));
    const waiting = await screen.findByText(/^Writing a summary with llama3\.2:3b…/);
    await waitFor(() => expect(document.activeElement).toBe(waiting));

    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Stopped. No summary was written.")).toBeTruthy();
    expect(calls).toContain("stop_tutor");
  });

  it("stops writing when the screen is left", async () => {
    const answer = deferred<SummaryOutcome>();
    const { user, calls, unmount } = await openTutor({ summarize_note: () => answer.promise });
    await user.click(await screen.findByRole("button", { name: /^Summarise$/ }));
    await screen.findByText(/^Writing a summary/);
    unmount();
    expect(calls.filter((cmd) => cmd === "stop_tutor")).toHaveLength(1);
    await act(async () => answer.resolve({ status: "stopped" }));
  });

  it("saves the summary as a new note called Summary: <title>", async () => {
    let saved: { title: string; body: string } | null = null;
    const { user } = await openTutor({
      create_note: (args) => {
        saved = { title: String(args.title), body: String(args.body) };
        return { ...(args as object), id: 99 } as unknown as Note;
      },
    });
    await user.click(await screen.findByRole("button", { name: /^Summarise$/ }));
    await user.click(await screen.findByRole("button", { name: "Save as note" }));

    expect(await screen.findByText(/Summary saved as a new note: Summary: /)).toBeTruthy();
    expect(saved).not.toBeNull();
    const { title, body } = saved as unknown as { title: string; body: string };
    expect(title.startsWith("Summary: ")).toBe(true);
    expect(body).toBe("- Mitosis makes two identical cells.\n\nKey terms: Mitosis");
  });

  it("keeps the draft and says why when saving fails", async () => {
    const { user } = await openTutor({
      create_note: () => refuse("failed", "Synapse couldn't save the note."),
    });
    await user.click(await screen.findByRole("button", { name: /^Summarise$/ }));
    await user.click(await screen.findByRole("button", { name: "Save as note" }));

    expect(await screen.findByText("Synapse couldn't save the note.")).toBeTruthy();
    expect(screen.getByText("Mitosis makes two identical cells.")).toBeTruthy();
  });

  it("shows why no summary was written", async () => {
    const { user } = await openTutor({
      summarize_note: () => refuse("failed", "The model's answer didn't include a usable summary."),
    });
    await user.click(await screen.findByRole("button", { name: /^Summarise$/ }));
    expect(
      await screen.findByText("The model's answer didn't include a usable summary.")
    ).toBeTruthy();
  });

  it("says so when Ollama can't be reached", async () => {
    const { user } = await openTutor({
      get_models: () => refuse("failed", "Synapse couldn't reach Ollama on this computer."),
    });
    expect(await screen.findByText("Synapse couldn't reach Ollama on this computer.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Back to note" }));
    expect(await screen.findByRole("button", { name: "Summarise" })).toBeTruthy();
  });

  it("says so when Ollama has no model", async () => {
    await openTutor({ get_models: () => [] });
    expect(await screen.findByText(/it has no models on this computer/)).toBeTruthy();
  });
});

const MISSED_POINT = "The two daughter cells are genetically identical.";

/** Opens the *Explain it* tab and asks for questions. */
async function openExplain(handlers: Record<string, Handler> = {}) {
  const opened = await openTutor(handlers);
  await opened.user.click(await screen.findByRole("tab", { name: "Explain it" }));
  await opened.user.click(await screen.findByRole("button", { name: "Ask me questions" }));
  await screen.findByText("Question 1 of 2");
  return opened;
}

/** Types an answer and has it checked. */
async function answerAndCheck(user: ReturnType<typeof userEvent.setup>, text: string) {
  await user.type(screen.getByLabelText("Your answer"), text);
  await user.click(screen.getByRole("button", { name: "Check my answer" }));
}

describe("explaining it", () => {
  it("asks questions, grades the answer and shows what was missed with its quote", async () => {
    const { user, argsOf } = await openExplain();
    expect(screen.getByText("Why are the daughter cells identical?")).toBeTruthy();

    await answerAndCheck(user, "Because the cell copies itself.");

    const result = await screen.findByText("Partly right: 55 out of 100.");
    await waitFor(() => expect(document.activeElement).toBe(result));
    expect(screen.getByText("You said the cells divide.")).toBeTruthy();
    // Listed under what was missed, and offered as a card.
    expect(screen.getAllByText(MISSED_POINT)).toHaveLength(2);
    expect(
      screen.getAllByText("From your note: “It produces two genetically identical daughter cells.”")
        .length
    ).toBeGreaterThan(0);
    expect(screen.getByText("Mitosis is not the division of the whole organism.")).toBeTruthy();
    expect(argsOf("grade_answer")).toEqual([
      {
        noteId: 5,
        model: "llama3.2:3b",
        question: "Why are the daughter cells identical?",
        reference: "Mitosis produces two genetically identical daughter cells.",
        quote: "It produces two genetically identical daughter cells.",
        answer: "Because the cell copies itself.",
      },
    ]);
  });

  it("moves on to the next question with an empty answer box", async () => {
    const { user } = await openExplain();
    await answerAndCheck(user, "Some answer.");
    await user.click(await screen.findByRole("button", { name: "Next question" }));

    expect(await screen.findByText("Question 2 of 2")).toBeTruthy();
    expect((screen.getByLabelText("Your answer") as HTMLTextAreaElement).value).toBe("");
    await answerAndCheck(user, "Again.");
    await screen.findByText("Partly right: 55 out of 100.");
    // The last question has no next one.
    expect(screen.queryByRole("button", { name: "Next question" })).toBeNull();
  });

  it("won't grade a blank answer, and sends nothing", async () => {
    const { user, calls } = await openExplain();
    await user.click(screen.getByRole("button", { name: "Check my answer" }));
    expect(await screen.findByText("Write your answer first.")).toBeTruthy();
    expect(calls).not.toContain("grade_answer");
  });

  it("says how many questions were left out", async () => {
    const { user } = await openTutor({
      tutor_questions: () => ({
        status: "written",
        questions: [{ question: "Q?", reference: "A.", quote: "It produces two." }],
        dropped: 2,
        leftOut: 1,
      }),
    });
    await user.click(await screen.findByRole("tab", { name: "Explain it" }));
    await user.click(await screen.findByRole("button", { name: "Ask me questions" }));
    expect(
      await screen.findByText(/2 questions were left out because their passages couldn't be found/)
    ).toBeTruthy();
    expect(screen.getByText(/1 more was left out/)).toBeTruthy();
  });

  it("shows why grading failed and keeps the typed answer", async () => {
    const { user } = await openExplain({
      grade_answer: () => refuse("failed", "The model's answer wasn't usable."),
    });
    await answerAndCheck(user, "My words.");
    expect(await screen.findByText("The model's answer wasn't usable.")).toBeTruthy();
    expect((screen.getByLabelText("Your answer") as HTMLTextAreaElement).value).toBe("My words.");
  });

  it("can stop grading, which tells Ollama, and keeps the answer", async () => {
    const grading = deferred<GradeOutcome>();
    const { user, calls } = await openExplain({
      grade_answer: () => grading.promise,
      stop_tutor: () => {
        grading.resolve({ status: "stopped" });
        return null;
      },
    });
    await answerAndCheck(user, "My words.");
    const waiting = await screen.findByText(/^Checking your answer with llama3\.2:3b…/);
    await waitFor(() => expect(document.activeElement).toBe(waiting));

    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Stopped. Your answer wasn't graded.")).toBeTruthy();
    expect(calls).toContain("stop_tutor");
    expect((screen.getByLabelText("Your answer") as HTMLTextAreaElement).value).toBe("My words.");
  });

  it("stops grading when the screen is left", async () => {
    const grading = deferred<GradeOutcome>();
    const { user, calls, unmount } = await openExplain({ grade_answer: () => grading.promise });
    await answerAndCheck(user, "My words.");
    await screen.findByText(/^Checking your answer/);
    unmount();
    expect(calls.filter((cmd) => cmd === "stop_tutor")).toHaveLength(1);
    await act(async () => grading.resolve({ status: "stopped" }));
  });

  it("stops writing questions when the screen is left", async () => {
    const asking = deferred<QuestionsOutcome>();
    const opened = await openTutor({ tutor_questions: () => asking.promise });
    await opened.user.click(await screen.findByRole("tab", { name: "Explain it" }));
    await opened.user.click(await screen.findByRole("button", { name: "Ask me questions" }));
    await screen.findByText(/^Writing questions/);
    opened.unmount();
    expect(opened.calls.filter((cmd) => cmd === "stop_tutor")).toHaveLength(1);
    await act(async () => asking.resolve({ status: "stopped" }));
  });

  it("adds the missed points as cards: the question on the front, the point on the back", async () => {
    const { user, argsOf } = await openExplain();
    await answerAndCheck(user, "Because the cell copies itself.");
    await screen.findByText("Partly right: 55 out of 100.");

    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Add 1 card" }));

    expect(await screen.findByText("Added 1 card to Biology.")).toBeTruthy();
    expect(argsOf("create_flashcards")).toEqual([
      {
        deckId: 1,
        cards: [{ front: "Why are the daughter cells identical?", back: MISSED_POINT }],
      },
    ]);
    // Done: the form is gone.
    expect(screen.queryByRole("button", { name: "Add 1 card" })).toBeNull();
  });

  it("asks for a deck before adding, and adds nothing when none is ticked", async () => {
    const { user, argsOf } = await openExplain();
    await answerAndCheck(user, "Because the cell copies itself.");
    await screen.findByText("Partly right: 55 out of 100.");

    await user.click(await screen.findByRole("button", { name: "Add 1 card" }));
    expect(await screen.findByText("Choose a deck for these cards.")).toBeTruthy();
    await user.selectOptions(screen.getByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("checkbox", { name: MISSED_POINT }));
    await user.click(screen.getByRole("button", { name: "Add 0 cards" }));
    expect(await screen.findByText("Tick at least one missed point to add as a card.")).toBeTruthy();
    expect(argsOf("create_flashcards")).toEqual([]);
  });

  it("offers no cards when nothing was missed", async () => {
    const { user } = await openExplain({
      grade_answer: () => ({
        status: "written",
        score: 95,
        verdict: "right",
        correct: ["All of it."],
        missed: [],
        wrong: [],
        dropped: 0,
      }),
    });
    await answerAndCheck(user, "Everything.");
    expect(await screen.findByText("Right: 95 out of 100.")).toBeTruthy();
    expect(screen.queryByLabelText("Deck")).toBeNull();
  });
});

describe("explaining it and unsaved changes", () => {
  function sidebar() {
    return within(screen.getByRole("navigation", { name: "Synapse" }));
  }
  function question() {
    return screen.getByRole("alertdialog", { hidden: true }) as HTMLDialogElement;
  }

  async function openExplainInApp() {
    const backend = fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    await user.click(sidebar().getByRole("button", { name: "Notes" }));
    await user.click(await screen.findByRole("button", { name: "Open note" }));
    await user.click(await screen.findByRole("button", { name: "Summarise" }));
    await user.click(await screen.findByRole("tab", { name: "Explain it" }));
    await user.click(await screen.findByRole("button", { name: "Ask me questions" }));
    await screen.findByText("Question 1 of 2");
    return { ...backend, user };
  }

  it("doesn't ask before leaving while nothing has been typed", async () => {
    const { user } = await openExplainInApp();
    await user.click(sidebar().getByRole("button", { name: "Study Desk" }));
    expect(question().open).toBe(false);
  });

  it("asks before leaving once an answer is typed", async () => {
    const { user } = await openExplainInApp();
    await user.type(screen.getByLabelText("Your answer"), "Half an answer");
    await user.click(sidebar().getByRole("button", { name: "Study Desk" }));
    expect(question().open).toBe(true);
  });

  it("asks before leaving while graded misses haven't been added as cards", async () => {
    const { user } = await openExplainInApp();
    await answerAndCheck(user, "Because the cell copies itself.");
    await screen.findByText("Partly right: 55 out of 100.");
    await user.click(sidebar().getByRole("button", { name: "Study Desk" }));
    expect(question().open).toBe(true);
  });

  it("doesn't ask once the misses are added", async () => {
    const { user } = await openExplainInApp();
    await answerAndCheck(user, "Because the cell copies itself.");
    await screen.findByText("Partly right: 55 out of 100.");
    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Add 1 card" }));
    await screen.findByText("Added 1 card to Biology.");
    await user.click(sidebar().getByRole("button", { name: "Study Desk" }));
    expect(question().open).toBe(false);
  });
});
