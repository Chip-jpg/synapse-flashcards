import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Flashcard } from "./api";
import { ReviewSession } from "./ReviewSession";
import { deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const CARDS: Flashcard[] = [
  { id: 1, front: "What is a cell?", back: "The basic unit of life.", reps: 0 },
  { id: 2, front: "What is DNA?", back: "The molecule that carries genes.", reps: 3 },
];

/**
 * Opens a review of `CARDS`: each rating saved moves on to the next card, and
 * after the last one the session is complete. `review`, if given, runs before
 * a rating is saved; a refusal from it leaves the card unreviewed.
 */
function startSession(review?: Handler) {
  let reviewed = 0;
  const backend = fakeBackend({
    get_session_card: () =>
      reviewed < CARDS.length
        ? {
            status: "due",
            card: CARDS[reviewed],
            progress: { reviewed, remaining: CARDS.length - reviewed },
          }
        : { status: "completed", cardsReviewed: reviewed },
    review_card: async (args) => {
      if (review) await review(args);
      reviewed += 1;
      return null;
    },
  });
  const onExit = vi.fn();
  render(<ReviewSession sessionId={7} deckName="Biology" onExit={onExit} />);
  return {
    onExit,
    /** Every rating the app sent, saved or not. */
    ratings: () => backend.argsOf("review_card").map((args) => args.rating),
    argsOf: backend.argsOf,
  };
}

/** Waits for `card` to be shown, then reveals its answer with Space. */
async function reveal(user: UserEvent, card: Flashcard) {
  await screen.findByText(card.front);
  await user.keyboard("[Space]");
  await screen.findByText(card.back);
}

describe("reviewing a card", () => {
  it("shows only the question first, and the number keys do nothing until it's revealed", async () => {
    const session = startSession();
    const user = userEvent.setup();
    await screen.findByText(CARDS[0].front);

    await user.keyboard("3");

    expect(session.ratings()).toEqual([]);
    expect(screen.queryByText(CARDS[0].back)).toBeNull();
    expect(screen.queryByRole("button", { name: "Good" })).toBeNull();
  });

  it.each([
    ["Space", "[Space]"],
    ["Enter", "{Enter}"],
  ])("reveals the answer with %s, and moves focus to it", async (_name, keys) => {
    startSession();
    const user = userEvent.setup();
    await screen.findByText(CARDS[0].front);

    await user.keyboard(keys);

    const answer = await screen.findByText(CARDS[0].back);
    expect(document.activeElement).toBe(answer.closest("section"));
    expect(screen.getByRole("button", { name: "Good" })).toBeTruthy();
  });

  it("rates the revealed card with a number key, then shows the next one", async () => {
    const session = startSession();
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);

    await user.keyboard("3");

    expect(await screen.findByText(CARDS[1].front)).toBeTruthy();
    // The card's reps go with the rating, so Rust can refuse a stale one.
    expect(session.argsOf("review_card")).toEqual([
      { sessionId: 7, cardId: 1, expectedReps: 0, rating: 3 },
    ]);
  });

  it.each([1, 2, 3, 4])("sends rating %i for that number key", async (rating) => {
    const session = startSession();
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);

    await user.keyboard(String(rating));

    await screen.findByText(CARDS[1].front);
    expect(session.ratings()).toEqual([rating]);
  });

  it("ignores number keys pressed with a modifier, or repeated by a held key", async () => {
    const session = startSession();
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);

    await user.keyboard("{Control>}3{/Control}");
    await user.keyboard("{Alt>}3{/Alt}");
    await user.keyboard("{Meta>}3{/Meta}");
    await user.keyboard("{Shift>}3{/Shift}");
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "3", repeat: true });

    expect(session.ratings()).toEqual([]);
    expect(screen.getByText(CARDS[0].back)).toBeTruthy();
  });

  it("rates one card while a number key is held down", async () => {
    const session = startSession();
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);

    await user.keyboard("{3>5/}");

    await screen.findByText(CARDS[1].front);
    expect(session.ratings()).toEqual([3]);
  });

  it("sends one rating, however often it's pressed while saving", async () => {
    const save = deferred();
    const session = startSession(() => save.promise);
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);

    await user.keyboard("3");
    await user.keyboard("4");
    await user.click(screen.getByRole("button", { name: "Good" }));
    await user.click(screen.getByRole("button", { name: "Easy" }));

    expect(session.ratings()).toEqual([3]);
    expect(screen.getByText("Saving…")).toBeTruthy();

    await act(async () => save.resolve(null));
    expect(await screen.findByText(CARDS[1].front)).toBeTruthy();
    expect(session.ratings()).toEqual([3]);
  });

  it("rates once when Enter presses a focused rating button", async () => {
    const session = startSession();
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);
    screen.getByRole("button", { name: "Hard" }).focus();

    await user.keyboard("{Enter}");

    await screen.findByText(CARDS[1].front);
    expect(session.ratings()).toEqual([2]);
  });

  it("keeps the card when saving fails, and a second rating can succeed", async () => {
    let failing = true;
    const session = startSession(() =>
      failing ? refuse("failed", "The rating couldn't be saved.") : null,
    );
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);

    await user.keyboard("3");

    expect((await screen.findByRole("alert")).textContent).toBe(
      "The rating couldn't be saved. Choose a rating to try again.",
    );
    expect(screen.getByText(CARDS[0].front)).toBeTruthy();

    failing = false;
    await user.keyboard("4");

    expect(await screen.findByText(CARDS[1].front)).toBeTruthy();
    expect(session.ratings()).toEqual([3, 4]);
  });

  it("moves on without an error when the rating turns out to be stale", async () => {
    // The card was already rated (in another window, say), so Rust refuses
    // this rating as stale and the session has moved on.
    let reviewed = 0;
    fakeBackend({
      get_session_card: () => ({
        status: "due",
        card: CARDS[reviewed],
        progress: { reviewed, remaining: CARDS.length - reviewed },
      }),
      review_card: () => {
        reviewed = 1;
        return refuse("stale", "That card was already reviewed.");
      },
    });
    render(<ReviewSession sessionId={7} deckName="Biology" onExit={vi.fn()} />);
    const user = userEvent.setup();
    await reveal(user, CARDS[0]);

    await user.keyboard("3");

    expect(await screen.findByText(CARDS[1].front)).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("a review session", () => {
  it("shows how far it has got, and ends with a summary", async () => {
    const session = startSession();
    const user = userEvent.setup();

    expect(await screen.findByText("Card 1 of 2")).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "0 of 2 cards reviewed" })).toBeTruthy();

    await reveal(user, CARDS[0]);
    await user.keyboard("3");
    expect(await screen.findByText("Card 2 of 2")).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "1 of 2 cards reviewed" })).toBeTruthy();

    await reveal(user, CARDS[1]);
    await user.keyboard("4");

    const summary = await screen.findByText(/You reviewed 2 cards\./);
    expect(screen.getByText("Session complete.")).toBeTruthy();
    expect(document.activeElement).toBe(summary);

    await user.click(screen.getByRole("button", { name: "Back to Study Desk" }));
    expect(session.onExit).toHaveBeenCalledOnce();
  });
});
