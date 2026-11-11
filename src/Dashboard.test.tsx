import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TodaySummary } from "./api";
import { Dashboard } from "./Dashboard";
import { fakeBackend, refuse, type Handler } from "./test/backend";

/** Shows the Study Desk, with `get_today` answered by `today`. */
function openDesk(today: Handler) {
  const backend = fakeBackend({ get_today: today });
  const view = render(desk());
  return { ...backend, ...view };
}

function desk() {
  return (
    <Dashboard
      focusOnLoad={false}
      notice={null}
      fromNotes={false}
      onStart={vi.fn()}
      onOpen={vi.fn()}
      onCreated={vi.fn()}
      onOpenNotes={vi.fn()}
    />
  );
}

const counts = (reviews: number, cards: number): TodaySummary => ({ reviews, cards });

describe("today's reviews on the Study Desk", () => {
  it("says so when nothing has been reviewed yet today", async () => {
    openDesk(() => counts(0, 0));

    expect(await screen.findByText("No reviews yet today")).toBeTruthy();
    expect(screen.queryByText(/^Today:/)).toBeNull();
  });

  it("counts reviews and different cards separately", async () => {
    openDesk(() => counts(12, 9));

    expect(await screen.findByText("Today: 12 reviews · 9 cards")).toBeTruthy();
  });

  it("uses the singular for one", async () => {
    openDesk(() => counts(1, 1));

    expect(await screen.findByText("Today: 1 review · 1 card")).toBeTruthy();
  });

  it("asks again each time the desk loads", async () => {
    let reviews = 0;
    const desk1 = openDesk(() => counts(reviews, reviews));
    await screen.findByText("No reviews yet today");

    // Back from a review: the desk is shown afresh.
    desk1.unmount();
    reviews = 2;
    render(desk());

    expect(await screen.findByText("Today: 2 reviews · 2 cards")).toBeTruthy();
    expect(desk1.argsOf("get_today")).toHaveLength(2);
  });

  it("leaves the line out, and still shows the decks, if the count can't be read", async () => {
    openDesk(() => refuse("failed", "Synapse couldn't count today's reviews."));

    expect(await screen.findByRole("heading", { name: "Biology" })).toBeTruthy();
    expect(screen.queryByText(/today/i)).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("the next exam on the Study Desk", () => {
  const exam = {
    id: 1,
    title: "Biology midterm",
    examDate: "2099-12-01",
    deckId: 1,
    tagId: null,
    scopeName: "Biology",
    scopeArchived: false,
    createdAt: "2026-10-01T09:00:00Z",
  };
  const plan = {
    examId: 1,
    title: "Biology midterm",
    examDate: "2099-12-01",
    status: "upcoming" as const,
    daysLeft: 4,
    cards: 30,
    newCards: 5,
    dueBeforeExam: 12,
    belowTarget: 3,
    dailyGoal: 6,
    readiness: 71.6,
  };

  it("shows nothing when there are no exams", async () => {
    fakeBackend();
    render(desk());

    expect(await screen.findByRole("heading", { name: "Biology" })).toBeTruthy();
    expect(screen.queryByText(/Next exam/)).toBeNull();
  });

  it("shows the soonest upcoming exam with its goal and readiness, and opens the planner", async () => {
    const onOpenExams = vi.fn();
    const { argsOf } = fakeBackend({
      list_exams: () => [
        { ...exam, id: 9, title: "Old", examDate: "2020-01-01" },
        exam,
        { ...exam, id: 2, title: "Later", examDate: "2099-12-30" },
      ],
      get_exam_plan: () => plan,
    });
    render(
      <Dashboard
        focusOnLoad={false}
        notice={null}
        fromNotes={false}
        onStart={vi.fn()}
        onOpen={vi.fn()}
        onCreated={vi.fn()}
        onOpenNotes={vi.fn()}
        onOpenExams={onOpenExams}
      />,
    );

    expect(await screen.findByText("Next exam: Biology midterm")).toBeTruthy();
    expect(screen.getByText(/In 4 days · goal today: 6 reviews · readiness 72%/)).toBeTruthy();
    expect(argsOf("get_exam_plan")).toEqual([{ examId: 1 }]);
    (await screen.findByRole("button", { name: /Open planner/ })).click();
    expect(onOpenExams).toHaveBeenCalled();
  });

  it("leaves the card out if the exam's plan can't be read", async () => {
    fakeBackend({
      list_exams: () => [exam],
      get_exam_plan: () => refuse("failed", "No plan."),
    });
    render(desk());

    expect(await screen.findByRole("heading", { name: "Biology" })).toBeTruthy();
    expect(screen.queryByText(/Next exam/)).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
