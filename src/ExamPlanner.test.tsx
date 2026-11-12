import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Exam, ExamPlan } from "./api";
import App from "./App";
import { ExamPlanner } from "./ExamPlanner";
import { CREATED, fakeBackend, refuse, type Handler } from "./test/backend";

function exam(overrides: Partial<Exam> = {}): Exam {
  return {
    id: 1,
    title: "Biology midterm",
    examDate: "2099-12-01",
    deckId: 1,
    tagId: null,
    scopeName: "Biology",
    scopeArchived: false,
    createdAt: CREATED,
    ...overrides,
  };
}

function plan(overrides: Partial<ExamPlan> = {}): ExamPlan {
  return {
    examId: 1,
    title: "Biology midterm",
    examDate: "2099-12-01",
    status: "upcoming",
    daysLeft: 10,
    cards: 40,
    newCards: 12,
    dueBeforeExam: 20,
    belowTarget: 7,
    dailyGoal: 5,
    readiness: 63.4,
    ...overrides,
  };
}

function openPlanner(
  handlers: Record<string, Handler> = {},
  props: { onStart?: () => void; onStartTag?: (tag: unknown) => void } = {},
) {
  const backend = fakeBackend(handlers);
  render(<ExamPlanner onStart={props.onStart ?? vi.fn()} onStartTag={props.onStartTag ?? vi.fn()} />);
  return backend;
}

describe("the Exams screen", () => {
  it("is in the sidebar and opens from it", async () => {
    const user = userEvent.setup();
    fakeBackend();
    render(<App />);
    const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));

    await user.click(sidebar.getByRole("button", { name: "Exams" }));

    expect(await screen.findByRole("heading", { name: "Exams", level: 2 })).toBeTruthy();
    expect(screen.getByText(/No exams yet/)).toBeTruthy();
  });

  it("shows each exam's plan: days left, goal, counts and readiness", async () => {
    openPlanner({ list_exams: () => [exam()], get_exam_plan: () => plan() });

    const item = await screen.findByRole("article", { name: "Biology midterm" });
    const within_ = within(item);
    expect(within_.getByText(/· In 10 days$/)).toBeTruthy();
    expect(within_.getByText("5 reviews")).toBeTruthy();
    expect(within_.getByText("Today's goal")).toBeTruthy();
    expect(within_.getByText("40")).toBeTruthy();
    expect(within_.getByText("12")).toBeTruthy();
    expect(within_.getByText("20")).toBeTruthy();
    expect(within_.getByText("7")).toBeTruthy();
    const bar = within_.getByRole("progressbar", { name: "Readiness" });
    expect(bar.getAttribute("aria-valuenow")).toBe("63");
  });

  it("keeps finished exams collapsed", async () => {
    openPlanner({
      list_exams: () => [exam({ id: 1 }), exam({ id: 2, title: "Old quiz", examDate: "2020-01-01" })],
      get_exam_plan: (args) =>
        args.examId === 2
          ? plan({ examId: 2, title: "Old quiz", status: "finished", daysLeft: 0 })
          : plan(),
    });

    expect(await screen.findByRole("article", { name: "Biology midterm" })).toBeTruthy();
    const finished = screen.getByText("Finished").closest("details")!;
    expect(finished.open).toBe(false);
    expect(finished.contains(screen.getByText("Old quiz"))).toBe(true);
  });

  it("adds an exam for a deck", async () => {
    const user = userEvent.setup();
    const created = vi.fn((args: Record<string, unknown>) =>
      exam({ title: String(args.title), examDate: String(args.examDate) }),
    );
    const { argsOf } = openPlanner({ create_exam: created });

    await user.click(await screen.findByRole("button", { name: "Add exam" }));
    await user.type(screen.getByLabelText("Title"), "Finals");
    fireEvent.change(screen.getByLabelText("Exam date"), { target: { value: "2099-06-01" } });
    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Add exam" }));

    await waitFor(() => expect(argsOf("create_exam")).toHaveLength(1));
    expect(argsOf("create_exam")[0]).toEqual({
      title: "Finals",
      examDate: "2099-06-01",
      deckId: 1,
      tagId: null,
    });
    expect(await screen.findByText(/Exam “Finals” added\./)).toBeTruthy();
  });

  it("adds an exam for a tag", async () => {
    const user = userEvent.setup();
    const { argsOf } = openPlanner({
      get_tags: () => [{ id: 4, name: "bio::cells", cards: 3, notes: 0 }],
      create_exam: () => exam({ deckId: null, tagId: 4, scopeName: "bio::cells" }),
    });

    await user.click(await screen.findByRole("button", { name: "Add exam" }));
    await user.type(screen.getByLabelText("Title"), "Cells quiz");
    fireEvent.change(screen.getByLabelText("Exam date"), { target: { value: "2099-06-01" } });
    await user.click(screen.getByRole("radio", { name: "A tag" }));
    await user.selectOptions(screen.getByLabelText("Tag"), "bio::cells");
    await user.click(screen.getByRole("button", { name: "Add exam" }));

    await waitFor(() => expect(argsOf("create_exam")).toHaveLength(1));
    expect(argsOf("create_exam")[0]).toMatchObject({ deckId: null, tagId: 4 });
  });

  it("shows a refusal on the field it is about, and keeps what was typed", async () => {
    const user = userEvent.setup();
    openPlanner({
      create_exam: () => refuse("invalid", "That isn't a date.", "examDate"),
    });

    await user.click(await screen.findByRole("button", { name: "Add exam" }));
    await user.type(screen.getByLabelText("Title"), "Finals");
    await user.selectOptions(await screen.findByLabelText("Deck"), "Biology");
    await user.click(screen.getByRole("button", { name: "Add exam" }));

    const date = screen.getByLabelText("Exam date");
    expect(await screen.findByText("That isn't a date.")).toBeTruthy();
    expect(date.getAttribute("aria-invalid")).toBe("true");
    await waitFor(() => expect(document.activeElement).toBe(date));
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("Finals");
  });

  it("shows a scope refusal under the deck choice", async () => {
    const user = userEvent.setup();
    openPlanner({
      create_exam: () => refuse("invalid", "Choose a deck or a tag.", "examScope"),
    });

    await user.click(await screen.findByRole("button", { name: "Add exam" }));
    await user.type(screen.getByLabelText("Title"), "Finals");
    fireEvent.change(screen.getByLabelText("Exam date"), { target: { value: "2099-06-01" } });
    await user.click(screen.getByRole("button", { name: "Add exam" }));

    expect(await screen.findByText("Choose a deck or a tag.")).toBeTruthy();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Deck")));
  });

  it("edits only the title and date", async () => {
    const user = userEvent.setup();
    const { argsOf } = openPlanner({
      list_exams: () => [exam()],
      get_exam_plan: () => plan(),
      update_exam: (args) => exam({ title: String(args.title) }),
    });

    await user.click(await screen.findByRole("button", { name: "Edit" }));
    const title = screen.getByLabelText("Title");
    await user.clear(title);
    await user.type(title, "Midterm 2");
    await user.click(screen.getByRole("button", { name: "Save exam" }));

    await waitFor(() => expect(argsOf("update_exam")).toHaveLength(1));
    expect(argsOf("update_exam")[0]).toEqual({
      examId: 1,
      title: "Midterm 2",
      examDate: "2099-12-01",
    });
  });

  it("asks before deleting, and deletes on confirming", async () => {
    const user = userEvent.setup();
    let exams = [exam()];
    const { argsOf } = openPlanner({
      list_exams: () => exams,
      get_exam_plan: () => plan(),
      delete_exam: () => {
        exams = [];
        return null;
      },
    });

    await user.click(await screen.findByRole("button", { name: "Delete" }));
    expect(argsOf("delete_exam")).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Keep exam" }));
    expect(screen.queryByRole("group", { name: /Delete/ })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Delete" }));
    await user.click(screen.getByRole("button", { name: "Delete exam" }));

    expect(await screen.findByText(/Exam “Biology midterm” deleted\./)).toBeTruthy();
    expect(argsOf("delete_exam")).toEqual([{ examId: 1 }]);
  });

  it("starts a review of the exam's deck with Study now", async () => {
    const user = userEvent.setup();
    const onStart = vi.fn();
    openPlanner(
      {
        list_exams: () => [exam()],
        get_exam_plan: () => plan(),
        start_session: () => ({ status: "started", sessionId: 77 }),
      },
      { onStart },
    );

    await user.click(await screen.findByRole("button", { name: "Study now" }));

    await waitFor(() => expect(onStart).toHaveBeenCalledWith(77, "Biology"));
  });

  it("starts a review of the exam's tag with Study now", async () => {
    const user = userEvent.setup();
    const onStartTag = vi.fn();
    openPlanner(
      {
        list_exams: () => [exam({ deckId: null, tagId: 4, scopeName: "bio::cells" })],
        get_exam_plan: () => plan(),
      },
      { onStartTag },
    );

    await user.click(await screen.findByRole("button", { name: "Study now" }));

    expect(onStartTag).toHaveBeenCalledWith({ id: 4, name: "bio::cells" });
  });

  it("counts an unsaved form as unsaved changes", async () => {
    const user = userEvent.setup();
    fakeBackend();
    render(<App />);
    const sidebar = within(screen.getByRole("navigation", { name: "Synapse" }));
    await user.click(sidebar.getByRole("button", { name: "Exams" }));
    await user.click(await screen.findByRole("button", { name: "Add exam" }));
    await user.type(screen.getByLabelText("Title"), "Fin");

    await user.click(sidebar.getByRole("button", { name: "Study Desk" }));

    expect(await screen.findByText("Leave without saving?")).toBeTruthy();
  });
});
