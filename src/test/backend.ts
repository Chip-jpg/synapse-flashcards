import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { CommandError, Deck, DeckDetail, Note, NoteSummary } from "../api";

export const CREATED = "2026-10-01T09:00:00Z";

type Args = Record<string, unknown>;
/** Answers one command: a value, a promise of one, or a rejected promise for a refusal. */
export type Handler = (args: Args) => unknown;

type Internals = { invoke: (cmd: string, args?: unknown, ...rest: unknown[]) => Promise<unknown> };

/** A command refusal, as Rust sends it. */
export function refuse(kind: CommandError["kind"], message: string, field?: CommandError["field"]) {
  const error: CommandError = field ? { kind, message, field } : { kind, message };
  return Promise.reject(error);
}

/** A promise the test settles itself, to hold the app in a "saving" or "checking" state. */
export function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/**
 * Stands in for the Rust core: one normal deck ("Biology", with one card and
 * nothing due) and one note ("Mitosis"). Adding a card succeeds. `overrides`
 * replace or add commands; any command left unanswered rejects as a failure.
 * `calls` lists every command the app sent, Tauri's own included (listening
 * for events, closing the window), and `argsOf` the arguments of one.
 */
export function fakeBackend(overrides: Record<string, Handler> = {}) {
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

  const handlers: Record<string, Handler> = {
    get_decks: () => [summary],
    get_deck_detail: () => deck,
    get_archived_decks: () => [],
    get_notes: () => [noteSummary],
    get_deleted_notes: () => [],
    get_note: () => note,
    create_flashcard: (args) => {
      const { front, back } = args as { front: string; back: string };
      deck.cards = [...deck.cards, { id: 11, front, back }];
      deck.cardCount = deck.cards.length;
      return deck;
    },
    "plugin:window|destroy": () => null,
    ...overrides,
  };

  mockWindows("main");
  mockIPC(
    (cmd, args) => {
      const handler = handlers[cmd];
      if (handler) return handler((args ?? {}) as Args);
      return refuse("failed", `The fake backend has no ${cmd}.`);
    },
    { shouldMockEvents: true },
  );

  // Record every command, including the event ones the mock answers itself.
  const calls: string[] = [];
  const sent: { cmd: string; args: Args }[] = [];
  const internals = (window as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
  const invoke = internals.invoke;
  internals.invoke = (cmd, args, ...rest) => {
    calls.push(cmd);
    sent.push({ cmd, args: (args ?? {}) as Args });
    return invoke(cmd, args, ...rest);
  };

  return {
    calls,
    /** The arguments of each `cmd` the app sent, oldest first. */
    argsOf: (cmd: string) => sent.filter((call) => call.cmd === cmd).map((call) => call.args),
  };
}
