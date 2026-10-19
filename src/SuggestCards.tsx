import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  createFlashcards,
  getModels,
  stopSuggesting,
  suggestCards,
  toCommandError,
  type CommandError,
  type Deck,
  type DeckDetail,
  type Note,
  type SuggestedCard,
} from "./api";
import { loadDeckChoices } from "./deckChoices";
import { FormActions, SelectField, TextField, useFormSave } from "./forms";
import { SparkleIcon, TrashIcon } from "./icons";
import { noteParts, SUGGESTION_NOTE_MAX_CHARS, type NotePart } from "./noteParts";
import { Message, useFocusOnMount } from "./ui";
import { useUnsavedChanges } from "./unsaved";

/** Where the model last used is remembered, in this computer's web storage. */
const MODEL_KEY = "synapse.suggestionModel";

/** "1 card", "3 cards". */
function cardCount(count: number): string {
  return count === 1 ? "1 card" : `${count} cards`;
}

// Remembering the model is only a convenience: if storage isn't available,
// the user simply chooses again.
function rememberedModel(): string | null {
  try {
    return localStorage.getItem(MODEL_KEY);
  } catch {
    return null;
  }
}

export function rememberModel(model: string) {
  try {
    localStorage.setItem(MODEL_KEY, model);
  } catch {
    // As above.
  }
}

/** The model to offer first: the one used last, or the only one there is. */
export function preferredModel(models: string[]): string {
  const remembered = rememberedModel();
  if (remembered !== null && models.includes(remembered)) return remembered;
  return models.length === 1 ? models[0] : "";
}

type Setup =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; decks: Deck[]; models: string[] };

/** Loads the decks cards can go into, then Ollama's models. Never rejects. */
async function loadSetup(): Promise<Setup> {
  const choices = await loadDeckChoices();
  if (choices.status === "error") return choices;
  try {
    return { status: "ready", decks: choices.decks, models: await getModels() };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * Having a model in Ollama, on this computer, suggest cards from a note. The
 * suggestions are drafts: the user checks and edits them, removes any they
 * don't want, and saves the rest into a deck they choose, all at once and by
 * the same rules as cards they write themselves. Nothing is saved before
 * that, and the cards aren't linked to the note.
 */
export function SuggestCards({
  note,
  onCancel,
  onSaved,
}: {
  note: Note;
  onCancel: () => void;
  /** The cards were saved: `count` of them, into `deck`. */
  onSaved: (deck: DeckDetail, count: number) => void;
}) {
  const [setup, setSetup] = useState<Setup>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    loadSetup().then((next) => {
      if (current) setSetup(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  function retry() {
    setSetup({ status: "loading" });
    setAttempt((n) => n + 1);
  }

  if (setup.status === "loading") {
    return (
      <p className="loading" role="status">
        Looking for Ollama…
      </p>
    );
  }

  const back = (
    <button type="button" className="button" onClick={onCancel}>
      Back to note
    </button>
  );

  if (setup.status === "error") {
    return (
      <Message focus tone="alert" text={setup.message}>
        <div className="actions">
          <button type="button" className="button" onClick={retry}>
            Try again
          </button>
          {back}
        </div>
      </Message>
    );
  }

  if (setup.decks.length === 0) {
    return (
      <Message
        focus
        tone="status"
        text="You don't have a deck to add cards to yet. Create one on the Study Desk first; the Sample deck can't take new cards."
      >
        {back}
      </Message>
    );
  }

  if (setup.models.length === 0) {
    return (
      <Message
        focus
        tone="status"
        text="Ollama is running, but it has no models on this computer that can write cards. Download a model in Ollama, then check again."
      >
        <div className="actions">
          <button type="button" className="button" onClick={retry}>
            Check again
          </button>
          {back}
        </div>
      </Message>
    );
  }

  return (
    <Suggester
      note={note}
      decks={setup.decks}
      models={setup.models}
      onCancel={onCancel}
      onSaved={onSaved}
      onNoDecks={() => setSetup({ ...setup, decks: [] })}
      onNoModels={() => setSetup({ ...setup, models: [] })}
    />
  );
}

/** A suggested card being checked. `key` tells drafts apart as some are removed. */
type Draft = SuggestedCard & { key: number };

/** What takes focus when choosing a model appears. */
type ChooseFocus = "part" | "model" | "suggest" | "notice";

type ChooseStage = {
  step: "choose";
  focus: ChooseFocus;
  notice: string | null;
  /** About the model chosen, shown under the list of models. */
  modelError: string | null;
  /** Any other reason nothing was suggested. */
  error: string | null;
};

/** How a part of a long note is offered: where it is, and how it begins. */
function partLabel(part: NotePart, index: number, count: number): string {
  const firstLine = part.text.split("\n", 1)[0].replace(/\s+/gu, " ").trim();
  const begins = Array.from(firstLine);
  const opening = begins.length > 60 ? `${begins.slice(0, 60).join("").trimEnd()}…` : firstLine;
  return `Part ${index + 1} of ${count}: ${opening}`;
}

/** Choosing a model, waiting for its suggestions, or checking them. */
type Stage =
  | ChooseStage
  | { step: "suggesting"; model: string }
  | {
      step: "review";
      model: string;
      drafts: Draft[];
      leftOut: number;
      unsupported: number;
    };

/** Choosing a model, with `focus` on mount and any of its messages. */
function choose(
  focus: ChooseFocus,
  messages: Partial<Pick<ChooseStage, "notice" | "modelError" | "error">> = {}
): ChooseStage {
  return { step: "choose", focus, notice: null, modelError: null, error: null, ...messages };
}

function Suggester({
  note,
  decks,
  models: initialModels,
  onCancel,
  onSaved,
  onNoDecks,
  onNoModels,
}: {
  note: Note;
  decks: Deck[];
  models: string[];
  onCancel: () => void;
  onSaved: (deck: DeckDetail, count: number) => void;
  onNoDecks: () => void;
  onNoModels: () => void;
}) {
  // A note too long to use whole is offered in parts, one used at a time.
  const parts = useMemo(() => noteParts(note.body), [note.body]);
  const split = parts.length > 1;
  const [partIndex, setPartIndex] = useState(0);
  const part = split ? parts[partIndex] : null;
  const [models, setModels] = useState(initialModels);
  const [model, setModel] = useState(() => preferredModel(initialModels));
  const [stage, setStage] = useState<Stage>(() => choose(split ? "part" : "model"));
  // Bumped with every stage, so each one remounts and takes focus.
  const [round, setRound] = useState(0);
  // Blocks a second suggestion immediately, before the re-render lands, and
  // says whether one is still running when this screen closes.
  const suggestingRef = useRef(false);
  const shownRef = useRef(true);
  const lastKey = useRef(0);
  // Suggested cards are lost if the user leaves before saving them.
  useUnsavedChanges(stage.step === "review");

  useEffect(() => {
    shownRef.current = true;
    return () => {
      shownRef.current = false;
      // Leaving mid-suggestion: stop it, so the model isn't kept busy.
      if (suggestingRef.current) stopSuggesting().catch(() => {});
    };
  }, []);

  function show(next: Stage) {
    setStage(next);
    setRound((n) => n + 1);
  }

  /**
   * Reloads the models after one turned out to be gone, so it can't be
   * chosen again. Resolves with the fresh list, or null if it couldn't load.
   */
  async function refreshModels(): Promise<string[] | null> {
    try {
      const fresh = await getModels();
      if (!shownRef.current) return fresh;
      if (fresh.length === 0) {
        onNoModels();
        return fresh;
      }
      setModels(fresh);
      setModel((current) => (fresh.includes(current) ? current : ""));
      return fresh;
    } catch {
      return null;
    }
  }

  async function suggest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (suggestingRef.current) return;
    if (model === "") {
      show(choose("model", { modelError: "Choose a model to suggest cards with." }));
      return;
    }
    suggestingRef.current = true;
    show({ step: "suggesting", model });

    let next: Stage;
    try {
      const outcome = await suggestCards(note.id, model, part ?? undefined);
      if (outcome.status === "stopped") {
        next = choose("notice", { notice: "Stopped. No cards were suggested." });
      } else {
        rememberModel(model);
        next = {
          step: "review",
          model,
          drafts: outcome.cards.map((card) => ({ ...card, key: ++lastKey.current })),
          leftOut: outcome.leftOut,
          unsupported: outcome.unsupported,
        };
      }
    } catch (err) {
      const failure = toCommandError(err);
      // Out of date: the model has left Ollama (or the note was deleted).
      const fresh = failure.kind === "stale" ? await refreshModels() : null;
      next =
        fresh !== null && !fresh.includes(model)
          ? choose("model", { modelError: failure.message })
          : choose("suggest", { error: failure.message });
    }
    suggestingRef.current = false;
    if (shownRef.current) show(next);
  }

  return (
    <section className="card" aria-labelledby="suggest-heading">
      <h2 id="suggest-heading" className="form-title">
        {`Suggest cards from ${note.title}`}
      </h2>

      {/* Focusable so keyboard users can scroll a long note. */}
      <section className="note-context" tabIndex={0} aria-labelledby="suggest-context-label">
        <p id="suggest-context-label" className="side-label">
          {part ? `Your note, part ${partIndex + 1} of ${parts.length}` : "Your note"}
        </p>
        <p className="note-body">{part ? part.text : note.body}</p>
      </section>
      <p id="suggest-hint" className="field-hint">
        A model in Ollama, on this computer, reads the note and suggests up to 10 cards, each with
        the passage of your note it comes from. Cards that can't be traced back to your note are
        left out. The note doesn't leave this computer, and nothing is saved until you save the
        cards. Models still get things wrong, so check each card against its passage before you
        save it.
      </p>

      {stage.step === "choose" && (
        <ChooseModel
          key={round}
          parts={
            split
              ? {
                  labels: parts.map((each, index) => partLabel(each, index, parts.length)),
                  index: partIndex,
                  noteLength: Array.from(note.body).length,
                  onChoose: setPartIndex,
                }
              : null
          }
          models={models}
          model={model}
          stage={stage}
          onModel={(value) => {
            setModel(value);
            setStage((current) =>
              current.step === "choose" ? { ...current, modelError: null } : current
            );
          }}
          onSuggest={suggest}
          onCancel={onCancel}
        />
      )}
      {stage.step === "suggesting" && (
        <Waiting
          key={round}
          model={stage.model}
          part={part ? `part ${partIndex + 1} of your note` : null}
          onStop={() => stopSuggesting().catch(() => {})}
        />
      )}
      {stage.step === "review" && (
        <ReviewSuggestions
          key={round}
          model={stage.model}
          drafts={stage.drafts}
          leftOut={stage.leftOut}
          unsupported={stage.unsupported}
          decks={decks}
          onSaved={onSaved}
          onCancel={onCancel}
          onNoDecks={onNoDecks}
          onAllRemoved={() =>
            show(
              choose("notice", {
                notice:
                  "You removed every suggested card, so there's nothing to save. Suggest again, or go back to the note.",
              })
            )
          }
        />
      )}
    </section>
  );
}

/** The parts of a long note, offered while choosing a model. */
type PartChoice = {
  labels: string[];
  index: number;
  /** In characters, as the user is told. */
  noteLength: number;
  onChoose: (index: number) => void;
};

function ChooseModel({
  parts,
  models,
  model,
  stage,
  onModel,
  onSuggest,
  onCancel,
}: {
  parts: PartChoice | null;
  models: string[];
  model: string;
  stage: ChooseStage;
  onModel: (model: string) => void;
  onSuggest: (event: FormEvent<HTMLFormElement>) => void;
  onCancel: () => void;
}) {
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(
    stage.focus === "notice" && stage.notice !== null
  );
  const suggestRef = useFocusOnMount<HTMLButtonElement>(stage.focus === "suggest");

  return (
    <form className="suggest-step" aria-label="Choose a model" noValidate onSubmit={onSuggest}>
      {stage.notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {stage.notice}
        </p>
      )}
      {parts && (
        <SelectField
          id="suggest-part"
          label="Part of the note"
          hint={`This note is ${parts.noteLength.toLocaleString("en-US")} characters long, more than the ${SUGGESTION_NOTE_MAX_CHARS.toLocaleString("en-US")} a model reads at once, so it's split into ${parts.labels.length} parts where paragraphs end. Cards are suggested from one part at a time; come back for the others.`}
          value={String(parts.index)}
          options={parts.labels.map((label, index) => ({ value: String(index), label }))}
          placeholder="Choose a part"
          required
          autoFocus={stage.focus === "part"}
          onChange={(value) => {
            if (value !== "") parts.onChoose(Number(value));
          }}
        />
      )}
      <SelectField
        id="suggest-model"
        label="Model"
        hint="One of the models in Ollama on this computer. Larger models usually write better cards, but take longer."
        describedBy="suggest-hint"
        value={model}
        options={models.map((name) => ({ value: name, label: name }))}
        placeholder="Choose a model"
        error={stage.modelError ?? undefined}
        required
        autoFocus={stage.focus === "model"}
        onChange={onModel}
      />
      <div className="form-actions">
        <button ref={suggestRef} type="submit" className="button button-primary">
          <SparkleIcon />
          Suggest cards
        </button>
        <button type="button" className="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
      {stage.error && (
        <p className="message-error" role="alert">
          {stage.error}
        </p>
      )}
    </form>
  );
}

/** While the model works. Focus goes to the status, so a held Enter can't stop it. */
function Waiting({
  model,
  part,
  onStop,
}: {
  model: string;
  /** Which part of the note, if only part of it is read. */
  part: string | null;
  onStop: () => void;
}) {
  const statusRef = useFocusOnMount<HTMLParagraphElement>(true);
  const [stopping, setStopping] = useState(false);

  return (
    <div className="confirm confirm-calm suggest-waiting">
      <p ref={statusRef} className="confirm-question" role="status" tabIndex={-1}>
        {stopping
          ? "Stopping…"
          : `Suggesting cards${part ? ` from ${part}` : ""} with ${model}… This can take a minute or more, depending on the model and this computer.`}
      </p>
      <div className="deck-actions">
        <button
          type="button"
          className="button button-small"
          aria-disabled={stopping}
          onClick={() => {
            if (stopping) return;
            setStopping(true);
            onStop();
          }}
        >
          Stop
        </button>
      </div>
    </div>
  );
}

/** A confirmation after removing a draft. `count` re-announces a repeat. */
type Removal = { count: number; text: string };

/**
 * Checking the suggestions: each is an editable front and back, and can be
 * removed. Saving adds every card left to the chosen deck at once. Rust
 * checks them as it checks typed cards, and saves all or none.
 */
function ReviewSuggestions({
  model,
  drafts: suggested,
  leftOut,
  unsupported,
  decks: initialDecks,
  onSaved,
  onCancel,
  onNoDecks,
  onAllRemoved,
}: {
  model: string;
  drafts: Draft[];
  leftOut: number;
  unsupported: number;
  decks: Deck[];
  onSaved: (deck: DeckDetail, count: number) => void;
  onCancel: () => void;
  /** Reloading the choices found none left, so there's nothing to save into. */
  onNoDecks: () => void;
  onAllRemoved: () => void;
}) {
  const [drafts, setDrafts] = useState(suggested);
  const [decks, setDecks] = useState(initialDecks);
  // An explicit choice: no deck is picked until the user picks one.
  const [deckId, setDeckId] = useState("");
  const [removal, setRemoval] = useState<Removal | null>(null);
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(true);
  const removalRef = useRef<HTMLParagraphElement>(null);

  // After a removal, focus goes to what happened rather than to a neighbour's
  // Remove button, so a repeated press can't remove a second card.
  useEffect(() => {
    if (removal) removalRef.current?.focus();
  }, [removal]);

  /** Reloads the deck choices in place, keeping the cards (as in CardFromNoteForm). */
  function refreshDecks() {
    loadDeckChoices().then((next) => {
      if (next.status !== "ready") return;
      if (next.decks.length === 0) {
        onNoDecks();
        return;
      }
      setDecks(next.decks);
      setDeckId((current) =>
        next.decks.some((deck) => String(deck.id) === current) ? current : ""
      );
    });
  }

  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      if (deckId === "") {
        const unchosen: CommandError = {
          kind: "invalid",
          message: "Choose a deck for these cards.",
          field: "deck",
        };
        throw unchosen;
      }
      const sent = drafts;
      try {
        const deck = await createFlashcards(
          Number(deckId),
          sent.map(({ front, back }) => ({ front, back }))
        );
        return { deck, count: sent.length };
      } catch (err) {
        const error = toCommandError(err);
        if (error.index !== undefined) {
          // Point at the draft itself, by its key, so the error stays with
          // that card whichever others are removed afterwards.
          const refused: CommandError = { ...error, index: sent[error.index]?.key };
          throw refused;
        }
        if (error.field || error.kind === "failed") throw err;
        // A refusal about no card is about the deck: it was archived or is
        // gone since the list loaded. Nothing was saved.
        refreshDecks();
        const refused: CommandError = { ...error, field: "deck" };
        throw refused;
      }
    },
    ({ deck, count }) => onSaved(deck, count),
    (field, key) =>
      field === "deck" ? "suggest-deck" : key === undefined ? undefined : `draft-${key}-${field}`
  );

  function change(key: number, side: "front" | "back", value: string) {
    setDrafts((current) =>
      current.map((draft) => (draft.key === key ? { ...draft, [side]: value } : draft))
    );
    edited(side, key);
  }

  function remove(key: number) {
    if (saving) return;
    const left = drafts.filter((draft) => draft.key !== key);
    if (left.length === 0) {
      onAllRemoved();
      return;
    }
    setDrafts(left);
    setRemoval((previous) => ({
      count: (previous?.count ?? 0) + 1,
      text: `Card removed. ${cardCount(left.length)} left.`,
    }));
  }

  const unsupportedText =
    unsupported === 0
      ? ""
      : unsupported === 1
        ? " 1 more suggestion was left out because it couldn't be traced back to your note."
        : ` ${unsupported} more suggestions were left out because they couldn't be traced back to your note.`;
  const leftOutText =
    leftOut === 0
      ? ""
      : leftOut === 1
        ? " 1 more was left out because it was empty, too long, or a repeat."
        : ` ${leftOut} more were left out because they were empty, too long, or repeats.`;

  return (
    <form className="suggest-step" aria-label="Suggested cards" noValidate onSubmit={submit}>
      <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
        {`${model} suggested ${cardCount(suggested.length)}. Check each one against the passage it comes from, edit or remove any you don't want, then choose a deck and save.${unsupportedText}${leftOutText}`}
      </p>
      <p className="field-hint">
        Each card needs a front (the question) and a back (the answer), each up to 2,000
        characters.
      </p>

      <ol className="card-list">
        {drafts.map((draft, position) => {
          const labelId = `draft-${draft.key}-label`;
          return (
            <li key={draft.key}>
              {/* A group, so its two fields are read out under the card's name. */}
              <div className="card-item" role="group" aria-labelledby={labelId}>
                <h3 id={labelId} className="draft-label">
                  {`Card ${position + 1}`}
                </h3>
                <TextField
                  id={`draft-${draft.key}-front`}
                  label="Front"
                  value={draft.front}
                  error={fieldError("front", draft.key)}
                  required
                  multiline
                  rows={2}
                  onChange={(value) => change(draft.key, "front", value)}
                />
                <TextField
                  id={`draft-${draft.key}-back`}
                  label="Back"
                  value={draft.back}
                  error={fieldError("back", draft.key)}
                  required
                  multiline
                  rows={3}
                  onChange={(value) => change(draft.key, "back", value)}
                />
                <figure className="draft-source">
                  <figcaption className="side-label">From your note</figcaption>
                  <blockquote className="draft-quote">{draft.source}</blockquote>
                  {draft.unfamiliar.length > 0 && (
                    <p className="draft-check">
                      {`Not in your note: ${draft.unfamiliar.join(", ")}. The suggested answer may say more than your note does.`}
                    </p>
                  )}
                </figure>
                <div className="deck-actions item-actions">
                  <button
                    type="button"
                    className="button button-quiet button-small"
                    aria-describedby={labelId}
                    aria-disabled={saving}
                    onClick={() => remove(draft.key)}
                  >
                    <TrashIcon />
                    Remove card
                  </button>
                </div>
              </div>
            </li>
          );
        })}
      </ol>
      {removal && (
        <p ref={removalRef} className="notice" role="status" tabIndex={-1}>
          {removal.text}
        </p>
      )}

      <SelectField
        id="suggest-deck"
        label="Deck"
        hint="The cards are added to this deck and are due for review straight away."
        value={deckId}
        options={decks.map((deck) => ({ value: String(deck.id), label: deck.name }))}
        placeholder="Choose a deck"
        error={fieldError("deck")}
        required
        onChange={(value) => {
          setDeckId(value);
          edited("deck");
        }}
      />

      <FormActions
        saveLabel={`Save ${cardCount(drafts.length)}`}
        saving={saving}
        error={formError}
        onCancel={onCancel}
      />
    </form>
  );
}
