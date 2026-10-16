import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ArchivedDeck } from "./api";
import { ArchivedDecks } from "./ArchivedDecks";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const CHEMISTRY: ArchivedDeck = {
  id: 3,
  name: "Chemistry",
  description: null,
  cardCount: 4,
  archivedAt: CREATED,
};

function openArchived(handlers: Record<string, Handler> = {}) {
  const backend = fakeBackend({ get_archived_decks: () => [CHEMISTRY], ...handlers });
  render(<ArchivedDecks />);
  return backend;
}

describe("unarchiving a deck", () => {
  it("asks first, with focus on the question, and Keep archived puts focus back", async () => {
    const archived = openArchived();
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Unarchive deck" }));
    expect(document.activeElement).toBe(screen.getByText(/^Unarchive Chemistry\?/));

    await user.click(screen.getByRole("button", { name: "Keep archived" }));

    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Unarchive deck" }));
    expect(archived.argsOf("unarchive_deck")).toEqual([]);
  });

  it("takes the deck off the list, and says where it went", async () => {
    let unarchived = false;
    const archived = openArchived({
      get_archived_decks: () => (unarchived ? [] : [CHEMISTRY]),
      unarchive_deck: () => {
        unarchived = true;
        return null;
      },
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Unarchive deck" }));
    await user.click(screen.getByRole("button", { name: "Unarchive deck" }));

    const notice = await screen.findByText(
      "Chemistry unarchived. It's back on your Study Desk with its cards and review history.",
    );
    expect(document.activeElement).toBe(notice);
    expect(archived.argsOf("unarchive_deck")).toEqual([{ deckId: 3 }]);
    expect(screen.queryByRole("button", { name: "Unarchive deck" })).toBeNull();
  });

  it("sends one unarchive, however often it's confirmed while unarchiving", async () => {
    const unarchiving = deferred();
    const archived = openArchived({ unarchive_deck: () => unarchiving.promise });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Unarchive deck" }));
    const confirm = screen.getByRole("button", { name: "Unarchive deck" });
    await user.click(confirm);
    await user.click(confirm);

    expect(screen.getByText("Unarchiving…")).toBeTruthy();
    expect(archived.argsOf("unarchive_deck")).toHaveLength(1);
    await act(async () => unarchiving.resolve(null));
  });

  it("reloads the list and says why when the deck wasn't archived after all", async () => {
    let reloaded = false;
    const archived = openArchived({
      get_archived_decks: () => (reloaded ? [] : [CHEMISTRY]),
      unarchive_deck: () => {
        reloaded = true;
        return refuse("stale", "That deck isn't archived.");
      },
    });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "Unarchive deck" }));
    await user.click(screen.getByRole("button", { name: "Unarchive deck" }));

    const notice = await screen.findByText(
      "That deck wasn't archived after all. Here are your archived decks as they are now.",
    );
    expect(document.activeElement).toBe(notice);
    expect(archived.argsOf("get_archived_decks")).toHaveLength(2);
  });
});
