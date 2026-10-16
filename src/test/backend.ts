import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Deck, DeckDetail, Note, NoteSummary } from "../api";

const CREATED = "2026-10-01T09:00:00Z";

type Internals = { invoke: (cmd: string, ...rest: unknown[]) => Promise<unknown> };

/**
 * Stands in for the Rust core: one normal deck ("Biology", with one card and
 * nothing due) and one note ("Mitosis"). Adding a card succeeds; any command
 * it doesn't know rejects as a failure. `calls` lists every command the app
 * sent, Tauri's own included (listening for events, closing the window).
 */
export function fakeBackend() {
  const deck: DeckDetail = {
    id: 1,
    name: "Biology",
    description: null,
    cardCount: 1,
    dueCount: 0,
    cards: [{ id: 10, front: "What is a cell?", back: "The basic unit of life." }],
    deletedCards: [],
  };
  const summary: Deck = {
    id: deck.id,
    name: deck.name,
    description: null,
    isSample: false,
    dueCount: 0,
  };
  const note: Note = {
    id: 5,
    title: "Mitosis",
    body: "Prophase, metaphase, anaphase, telophase.",
    createdAt: CREATED,
    updatedAt: CREATED,
  };
  const noteSummary: NoteSummary = {
    id: note.id,
    title: note.title,
    createdAt: CREATED,
    updatedAt: CREATED,
  };

  mockWindows("main");
  mockIPC(
    (cmd, args) => {
      switch (cmd) {
        case "get_decks":
          return [summary];
        case "get_deck_detail":
          return deck;
        case "get_archived_decks":
          return [];
        case "get_notes":
          return [noteSummary];
        case "get_deleted_notes":
          return [];
        case "get_note":
          return note;
        case "create_flashcard": {
          const { front, back } = args as { front: string; back: string };
          deck.cards = [...deck.cards, { id: 11, front, back }];
          deck.cardCount = deck.cards.length;
          return deck;
        }
        case "plugin:window|destroy":
          return null;
      }
      return Promise.reject({ kind: "failed", message: `The fake backend has no ${cmd}.` });
    },
    { shouldMockEvents: true },
  );

  // Record every command, including the event ones the mock answers itself.
  const calls: string[] = [];
  const internals = (window as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
  const invoke = internals.invoke;
  internals.invoke = (cmd, ...rest) => {
    calls.push(cmd);
    return invoke(cmd, ...rest);
  };

  return { calls };
}
