import { mockConvertFileSrc, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type {
  CardText,
  CommandError,
  Deck,
  DeckDetail,
  ImportedFile,
  Note,
  NoteSummary,
  Source,
  SourceImport,
} from "../api";

export const CREATED = "2026-10-01T09:00:00Z";

/** FSRS's 21 default parameters, as `get_scheduling` sends them. */
export const DEFAULTS = [
  0.212, 1.2931, 2.3065, 8.2956, 6.4133, 0.8334, 3.0194, 0.001, 1.8722, 0.1666, 0.796, 1.4835,
  0.0614, 0.2629, 1.6483, 0.6014, 1.8729, 0.5425, 0.0912, 0.0658, 0.1542,
];

type Args = Record<string, unknown>;
/** Answers one command: a value, a promise of one, or a rejected promise for a refusal. */
export type Handler = (args: Args) => unknown;

type Internals = { invoke: (cmd: string, args?: unknown, ...rest: unknown[]) => Promise<unknown> };

/** A command refusal, as Rust sends it. */
export function refuse(
  kind: CommandError["kind"],
  message: string,
  field?: CommandError["field"],
  index?: number,
) {
  const error: CommandError = { kind, message };
  if (field) error.field = field;
  if (index !== undefined) error.index = index;
  return Promise.reject(error);
}

/** A kept source, as `import_source` and `get_source` send it: a PDF of three pages. */
export function source(overrides: Partial<Source> = {}): Source {
  return {
    id: 3,
    title: "Lecture 3",
    kind: "pdf",
    fileName: "Lecture 3.pdf",
    pageCount: 3,
    asset: {
      id: 7,
      sha256: "a".repeat(64),
      mediaType: "application/pdf",
      size: 2_400_000,
      createdAt: CREATED,
    },
    createdAt: CREATED,
    updatedAt: CREATED,
    deletedAt: null,
    ...overrides,
  };
}

/** What `import_source` sends for a file whose text reads as `file`: kept as source 3. */
export function keptSource(file: ImportedFile): SourceImport {
  const { status: _read, fileName, ...text } = file;
  return {
    status: "imported",
    source: source({
      title: file.title,
      kind: file.kind,
      fileName,
      pageCount: file.pages ?? 1,
    }),
    text,
  };
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
 * nothing due) and one note ("Mitosis"). Adding cards succeeds. Ollama has one
 * model ("llama3.2:3b"), which suggests one card and writes a one-question
 * quiz. Scheduling uses FSRS's defaults, with no reviews to optimize from yet,
 * aiming for 90%. `overrides`
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
    get_today: () => ({ reviews: 0, cards: 0 }),
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
    create_flashcards: (args) => {
      const { cards } = args as { cards: CardText[] };
      deck.cards = [...deck.cards, ...cards.map((card, n) => ({ id: 20 + n, ...card }))];
      deck.cardCount = deck.cards.length;
      return deck;
    },
    get_models: () => ["llama3.2:3b"],
    suggest_cards: () => ({
      status: "suggested",
      cards: [
        {
          front: "What are the phases of mitosis?",
          back: "Prophase, metaphase, anaphase, telophase.",
          source: "Prophase, metaphase, anaphase, telophase.",
          unfamiliar: [],
        },
      ],
      leftOut: 0,
      unsupported: 0,
    }),
    stop_suggesting: () => null,
    get_card_help: (args) => ({
      status: "written",
      help: { kind: args.kind, text: "Mighty Mito makes the power.", unfamiliar: [] },
    }),
    stop_card_help: () => null,
    quiz_note: () => ({
      status: "written",
      questions: [
        {
          question: "Which phase comes first in mitosis?",
          answer: "Prophase",
          distractors: ["Metaphase", "Anaphase", "Telophase"],
          source: "Prophase, metaphase, anaphase, telophase.",
          unfamiliar: [],
        },
      ],
      leftOut: 0,
      unsupported: 0,
    }),
    stop_quiz: () => null,
    get_scheduling: () => ({
      version: 0,
      personal: false,
      parameters: DEFAULTS,
      defaults: DEFAULTS,
      reviewsUsed: null,
      appliedAt: null,
      usableReviews: 0,
      minimumReviews: 400,
    }),
    stop_optimizing: () => null,
    get_retention: () => ({
      version: 0,
      global: 90,
      default: 90,
      minimum: 70,
      maximum: 97,
      decks: [{ id: deck.id, name: deck.name, percent: null }],
      archivedOwn: 0,
    }),
    // No note comes from a kept source unless a test says so.
    get_note_source: () => null,
    locate_quotes: () => null,
    "plugin:window|destroy": () => null,
    ...overrides,
  };

  mockWindows("main");
  mockConvertFileSrc("linux");
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
