import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Note, SummaryOutcome } from "./api";
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
