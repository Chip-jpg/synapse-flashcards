import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  createFlashcards,
  getModels,
  quizNote,
  stopQuiz,
  toCommandError,
  type CommandError,
  type Deck,
  type DeckDetail,
  type Note,
  type QuizQuestion,
} from "./api";
import { loadDeckChoices, type DeckChoices } from "./deckChoices";
import { SelectField, useFormSave } from "./forms";
import { CheckIcon, SparkleIcon } from "./icons";
import { noteParts, SUGGESTION_NOTE_MAX_CHARS } from "./noteParts";
import { partLabel, preferredModel, rememberModel } from "./SuggestCards";
import { Message, useFocusOnMount } from "./ui";
import { useUnsavedChanges } from "./unsaved";

/** The keys that choose an option, in the order the options are shown. */
const OPTION_KEYS = ["1", "2", "3", "4"];

/** "1 card", "3 cards". */
function cardCount(count: number): string {
  return count === 1 ? "1 card" : `${count} cards`;
}

/** "1 question", "3 questions". */
function questionCount(count: number): string {
  return count === 1 ? "1 question" : `${count} questions`;
}

/** A question as it's asked: its options in a shuffled order, and which is correct. */
export type AskedQuestion = QuizQuestion & { options: string[]; correct: number };

/**
 * Puts a question's correct answer among its wrong ones in a random order, so
 * where it lands says nothing. `random` returns a number in [0, 1).
 */
export function shuffled(question: QuizQuestion, random: () => number = Math.random): AskedQuestion {
  const options = [question.answer, ...question.distractors];
  // Fisher–Yates.
  for (let i = options.length - 1; i > 0; i--) {
    const j = Math.floor(random() * (i + 1));
    [options[i], options[j]] = [options[j], options[i]];
  }
  return { ...question, options, correct: options.indexOf(question.answer) };
}

/**
 * Whether a key press is someone typing or choosing in a list, which the
 * option keys must leave alone.
 */
function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  return target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT";
}

function isButton(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.tagName === "BUTTON";
}

type Setup =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; models: string[] };

/**
 * A multiple-choice quiz on a note, written by a model in Ollama on this
 * computer and taken one question at a time. Nothing is saved, except
 * questions the user missed and chooses to add as cards.
 *
 * Leaving counts as unsaved changes only while some question has been missed
 * (and not yet added as a card): the quiz itself can always be written again,
 * but which questions the user got wrong can't.
 */
export function Quiz({
  note,
  onCancel,
  onSaved,
}: {
  note: Note;
  onCancel: () => void;
  /** Missed questions were added as cards: `count` of them, into `deck`. */
  onSaved: (deck: DeckDetail, count: number) => void;
}) {
  const [setup, setSetup] = useState<Setup>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    getModels()
      .then((models) => ({ status: "ready" as const, models }))
      .catch((err) => ({ status: "error" as const, message: toCommandError(err).message }))
      .then((next) => {
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

  if (setup.models.length === 0) {
    return (
      <Message
        focus
        tone="status"
        text="Ollama is running, but it has no models on this computer that can write a quiz. Download a model in Ollama, then check again."
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
    <Quizzer
      note={note}
      models={setup.models}
      onCancel={onCancel}
      onSaved={onSaved}
      onNoModels={() => setSetup({ status: "ready", models: [] })}
    />
  );
}

type ChooseFocus = "part" | "model" | "start" | "notice";

type ChooseStage = {
  step: "choose";
  focus: ChooseFocus;
  notice: string | null;
  modelError: string | null;
  error: string | null;
};

/** Choosing a model, waiting for the quiz, taking it, or looking back on it. */
type Stage =
  | ChooseStage
  | { step: "writing"; model: string }
  | {
      step: "taking";
      model: string;
      questions: AskedQuestion[];
      /** The question shown. */
      current: number;
      /** The option chosen for each question answered so far, in order. */
      picked: number[];
      leftOut: number;
      unsupported: number;
    }
  | { step: "summary"; questions: AskedQuestion[]; picked: number[] };

function choose(
  focus: ChooseFocus,
  messages: Partial<Pick<ChooseStage, "notice" | "modelError" | "error">> = {}
): ChooseStage {
  return { step: "choose", focus, notice: null, modelError: null, error: null, ...messages };
}

/** How many of the answered questions were missed. */
function missedCount(questions: AskedQuestion[], picked: number[]): number {
  return picked.filter((option, index) => option !== questions[index].correct).length;
}

function Quizzer({
  note,
  models: initialModels,
  onCancel,
  onSaved,
  onNoModels,
}: {
  note: Note;
  models: string[];
  onCancel: () => void;
  onSaved: (deck: DeckDetail, count: number) => void;
  onNoModels: () => void;
}) {
  // A note too long to use whole is offered in parts, as for suggestions.
  const parts = useMemo(() => noteParts(note.body), [note.body]);
  const split = parts.length > 1;
  const [partIndex, setPartIndex] = useState(0);
  const part = split ? parts[partIndex] : null;
  const [models, setModels] = useState(initialModels);
  const [model, setModel] = useState(() => preferredModel(initialModels));
  const [stage, setStage] = useState<Stage>(() => choose(split ? "part" : "model"));
  // Bumped with every stage, so each one remounts and takes focus.
  const [round, setRound] = useState(0);
  const writingRef = useRef(false);
  const shownRef = useRef(true);

  // Which questions were missed can't be got back once the screen is left.
  const missed =
    stage.step === "taking" || stage.step === "summary"
      ? missedCount(stage.questions, stage.picked)
      : 0;
  useUnsavedChanges(missed > 0);

  useEffect(() => {
    shownRef.current = true;
    return () => {
      shownRef.current = false;
      // Leaving while the quiz is written: stop it, so the model isn't kept busy.
      if (writingRef.current) stopQuiz().catch(() => {});
    };
  }, []);

  function show(next: Stage) {
    setStage(next);
    setRound((n) => n + 1);
  }

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

  async function start(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (writingRef.current) return;
    if (model === "") {
      show(choose("model", { modelError: "Choose a model to write the quiz with." }));
      return;
    }
    writingRef.current = true;
    show({ step: "writing", model });

    let next: Stage;
    try {
      const outcome = await quizNote(note.id, model, part ?? undefined);
      if (outcome.status === "stopped") {
        next = choose("notice", { notice: "Stopped. No quiz was written." });
      } else {
        rememberModel(model);
        next = {
          step: "taking",
          model,
          questions: outcome.questions.map((question) => shuffled(question)),
          current: 0,
          picked: [],
          leftOut: outcome.leftOut,
          unsupported: outcome.unsupported,
        };
      }
    } catch (err) {
      const failure = toCommandError(err);
      const fresh = failure.kind === "stale" ? await refreshModels() : null;
      next =
        fresh !== null && !fresh.includes(model)
          ? choose("model", { modelError: failure.message })
          : choose("start", { error: failure.message });
    }
    writingRef.current = false;
    if (shownRef.current) show(next);
  }

  return (
    <section className="card" aria-labelledby="quiz-heading">
      <h2 id="quiz-heading" className="form-title">
        {`Quiz on ${note.title}`}
      </h2>

      {stage.step === "choose" && (
        <>
          <section className="note-context" tabIndex={0} aria-labelledby="quiz-context-label">
            <p id="quiz-context-label" className="side-label">
              {part ? `Your note, part ${partIndex + 1} of ${parts.length}` : "Your note"}
            </p>
            <p className="note-body">{part ? part.text : note.body}</p>
          </section>
          <p id="quiz-hint" className="field-hint">
            A model in Ollama, on this computer, reads the note and writes up to 8 multiple-choice
            questions about it, each with the passage of your note its answer comes from.
            Questions that can't be traced back to your note are left out. The note doesn't leave
            this computer, and nothing is saved, though you can add questions you miss as cards.
          </p>
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
            onStart={start}
            onCancel={onCancel}
          />
        </>
      )}
      {stage.step === "writing" && (
        <Writing
          key={round}
          model={stage.model}
          part={part ? `part ${partIndex + 1} of your note` : null}
          onStop={() => stopQuiz().catch(() => {})}
        />
      )}
      {stage.step === "taking" && (
        <TakeQuestion
          // A fresh question, and with it fresh focus and a fresh key listener.
          key={`${round}-${stage.current}`}
          intro={
            stage.current === 0
              ? introText(stage.model, stage.questions.length, stage.leftOut, stage.unsupported)
              : null
          }
          question={stage.questions[stage.current]}
          position={stage.current}
          count={stage.questions.length}
          chosen={stage.picked[stage.current] ?? null}
          onPick={(option) =>
            setStage((current) =>
              current.step === "taking" && current.picked.length === current.current
                ? { ...current, picked: [...current.picked, option] }
                : current
            )
          }
          onNext={() => {
            if (stage.current + 1 >= stage.questions.length) {
              show({ step: "summary", questions: stage.questions, picked: stage.picked });
            } else {
              setStage({ ...stage, current: stage.current + 1 });
            }
          }}
        />
      )}
      {stage.step === "summary" && (
        <Summary
          key={round}
          questions={stage.questions}
          picked={stage.picked}
          onSaved={onSaved}
          onAgain={() => show(choose("start"))}
          onDone={onCancel}
        />
      )}
    </section>
  );
}

/** What the model wrote, and what was left out, said before the first question. */
function introText(model: string, count: number, leftOut: number, unsupported: number): string {
  const unsupportedText =
    unsupported === 0
      ? ""
      : unsupported === 1
        ? " 1 more question was left out because it couldn't be traced back to your note."
        : ` ${unsupported} more questions were left out because they couldn't be traced back to your note.`;
  const leftOutText =
    leftOut === 0
      ? ""
      : leftOut === 1
        ? " 1 more was left out because it was empty, too long, a repeat, or didn't have four different answers."
        : ` ${leftOut} more were left out because they were empty, too long, repeats, or didn't have four different answers.`;
  return `${model} wrote ${questionCount(count)}.${unsupportedText}${leftOutText}`;
}

type PartChoice = {
  labels: string[];
  index: number;
  noteLength: number;
  onChoose: (index: number) => void;
};

function ChooseModel({
  parts,
  models,
  model,
  stage,
  onModel,
  onStart,
  onCancel,
}: {
  parts: PartChoice | null;
  models: string[];
  model: string;
  stage: ChooseStage;
  onModel: (model: string) => void;
  onStart: (event: FormEvent<HTMLFormElement>) => void;
  onCancel: () => void;
}) {
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(
    stage.focus === "notice" && stage.notice !== null
  );
  const startRef = useFocusOnMount<HTMLButtonElement>(stage.focus === "start");

  return (
    <form className="suggest-step" aria-label="Choose a model" noValidate onSubmit={onStart}>
      {stage.notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {stage.notice}
        </p>
      )}
      {parts && (
        <SelectField
          id="quiz-part"
          label="Part of the note"
          hint={`This note is ${parts.noteLength.toLocaleString("en-US")} characters long, more than the ${SUGGESTION_NOTE_MAX_CHARS.toLocaleString("en-US")} a model reads at once, so it's split into ${parts.labels.length} parts where paragraphs end. A quiz covers one part at a time; come back for the others.`}
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
        id="quiz-model"
        label="Model"
        hint="One of the models in Ollama on this computer. Larger models usually write better questions, but take longer."
        describedBy="quiz-hint"
        value={model}
        options={models.map((name) => ({ value: name, label: name }))}
        placeholder="Choose a model"
        error={stage.modelError ?? undefined}
        required
        autoFocus={stage.focus === "model"}
        onChange={onModel}
      />
      <div className="form-actions">
        <button ref={startRef} type="submit" className="button button-primary">
          <SparkleIcon />
          Start quiz
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
function Writing({
  model,
  part,
  onStop,
}: {
  model: string;
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
          : `Writing a quiz${part ? ` on ${part}` : ""} with ${model}… This can take a minute or more, depending on the model and this computer.`}
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

/**
 * One question: its options, chosen with a click or 1–4. Once answered, the
 * right option is marked (and the one chosen, if wrong), with the passage of
 * the note it comes from, and Enter, Space or *Next question* moves on.
 */
function TakeQuestion({
  intro,
  question,
  position,
  count,
  chosen,
  onPick,
  onNext,
}: {
  /** Said above the first question: what the model wrote. */
  intro: string | null;
  question: AskedQuestion;
  position: number;
  count: number;
  /** The option chosen, once answered. */
  chosen: number | null;
  onPick: (option: number) => void;
  onNext: () => void;
}) {
  const questionRef = useFocusOnMount<HTMLElement>(intro === null);
  const introRef = useFocusOnMount<HTMLParagraphElement>(intro !== null);
  const feedbackRef = useRef<HTMLParagraphElement>(null);
  const answered = chosen !== null;
  const last = position + 1 === count;

  // Once answered, focus goes to what happened, so it's read out and a
  // second key press can't answer again.
  useEffect(() => {
    if (answered) feedbackRef.current?.focus();
  }, [answered]);

  // As in a review session: on the document, since focus usually sits on
  // the question or the feedback, which aren't controls. The parent keys
  // this component by question, so a listener never outlives its question.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return;
      if (event.repeat || isTypingTarget(event.target)) return;
      if (!answered) {
        const option = OPTION_KEYS.indexOf(event.key);
        if (option === -1 || option >= question.options.length) return;
        event.preventDefault();
        onPick(option);
        return;
      }
      if (event.key !== "Enter" && event.key !== " ") return;
      // A focused button already presses itself.
      if (isButton(event.target)) return;
      event.preventDefault();
      onNext();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [answered]);

  const right = chosen === question.correct;

  return (
    <div className="review quiz">
      {intro && (
        <p ref={introRef} className="notice" role="status" tabIndex={-1}>
          {intro}
        </p>
      )}
      <div className="progress">
        <p id="quiz-progress" className="progress-label">
          {`Question ${position + 1} of ${count}`}
        </p>
        <progress
          className="progress-bar"
          value={position + (answered ? 1 : 0)}
          max={count}
          aria-label={`${position + (answered ? 1 : 0)} of ${count} questions answered`}
        />
      </div>

      <div className="review-card">
        <section
          ref={questionRef}
          className="card-side"
          tabIndex={-1}
          aria-labelledby="quiz-progress quiz-question-label"
        >
          <h3 id="quiz-question-label" className="side-label">
            Question
          </h3>
          <p className="card-text">{question.question}</p>
        </section>
      </div>

      <div className="rating" role="group" aria-labelledby="quiz-prompt">
        <p id="quiz-prompt" className="rating-prompt">
          {answered ? "Your answer" : "Choose the right answer"}
        </p>
        <div className="quiz-options">
          {question.options.map((option, index) => {
            const mark = !answered
              ? ""
              : index === question.correct
                ? " quiz-option-right"
                : index === chosen
                  ? " quiz-option-wrong"
                  : " quiz-option-other";
            return (
              // `aria-disabled` once answered, so focus isn't dropped.
              <button
                key={index}
                type="button"
                className={`button quiz-option${mark}`}
                aria-disabled={answered}
                aria-keyshortcuts={OPTION_KEYS[index]}
                aria-pressed={answered ? index === chosen : undefined}
                onClick={() => {
                  if (!answered) onPick(index);
                }}
              >
                <span className="kbd" aria-hidden="true">
                  {OPTION_KEYS[index]}
                </span>
                <span className="quiz-option-text">{option}</span>
                {answered && index === question.correct && (
                  <span className="quiz-option-mark">
                    <CheckIcon />
                    Right answer
                  </span>
                )}
                {answered && index === chosen && !right && (
                  <span className="quiz-option-mark">Your answer</span>
                )}
              </button>
            );
          })}
        </div>
        {!answered && (
          <p className="shortcut-hint" aria-hidden="true">
            Press <span className="kbd">1</span>–<span className="kbd">4</span> to answer
          </p>
        )}
      </div>

      {answered && (
        <div className="quiz-feedback">
          <p
            ref={feedbackRef}
            className={right ? "notice" : "quiz-missed"}
            role="status"
            tabIndex={-1}
          >
            {right ? "Right." : `Not quite. The right answer is: ${question.answer}`}
          </p>
          <figure className="draft-source">
            <figcaption className="side-label">From your note</figcaption>
            <blockquote className="draft-quote">{question.source}</blockquote>
            {question.unfamiliar.length > 0 && (
              <p className="draft-check">
                {`Not in your note: ${question.unfamiliar.join(", ")}. The answer may say more than your note does.`}
              </p>
            )}
          </figure>
          <div className="reveal">
            <button
              type="button"
              className="button button-primary"
              aria-keyshortcuts="Enter Space"
              onClick={onNext}
            >
              {last ? "See results" : "Next question"}
            </button>
            <p className="shortcut-hint" aria-hidden="true">
              or press <span className="kbd">Enter</span>
            </p>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * The end of the quiz: the score, and each missed question, which can be
 * added as a card (the question on the front, the right answer on the back)
 * to a chosen deck, all at once through `createFlashcards`.
 */
function Summary({
  questions,
  picked,
  onSaved,
  onAgain,
  onDone,
}: {
  questions: AskedQuestion[];
  picked: number[];
  onSaved: (deck: DeckDetail, count: number) => void;
  onAgain: () => void;
  onDone: () => void;
}) {
  const scoreRef = useFocusOnMount<HTMLParagraphElement>(true);
  const missed = questions
    .map((question, index) => ({ question, index, chosen: picked[index] }))
    .filter(({ question, chosen }) => chosen !== question.correct);
  const right = questions.length - missed.length;

  return (
    <div className="suggest-step quiz-summary">
      <p ref={scoreRef} className="notice" role="status" tabIndex={-1}>
        {missed.length === 0
          ? `You got all ${questionCount(questions.length)} right.`
          : `You got ${right} of ${questionCount(questions.length)} right.`}
      </p>
      {missed.length > 0 && <AddMissed missed={missed} onSaved={onSaved} />}
      <div className="form-actions">
        <button type="button" className="button" onClick={onAgain}>
          New quiz
        </button>
        <button type="button" className="button" onClick={onDone}>
          Back to note
        </button>
      </div>
    </div>
  );
}

type Missed = { question: AskedQuestion; index: number; chosen: number };

function AddMissed({
  missed,
  onSaved,
}: {
  missed: Missed[];
  onSaved: (deck: DeckDetail, count: number) => void;
}) {
  const [choices, setChoices] = useState<DeckChoices>({ status: "loading" });
  // Every missed question is ticked to start with.
  const [ticked, setTicked] = useState<Set<number>>(() => new Set(missed.map((each) => each.index)));
  const [deckId, setDeckId] = useState("");

  function loadChoices() {
    loadDeckChoices().then((next) => {
      setChoices(next);
      if (next.status === "ready") {
        setDeckId((current) =>
          next.decks.some((deck) => String(deck.id) === current) ? current : ""
        );
      }
    });
  }

  useEffect(loadChoices, []);

  const chosen = missed.filter((each) => ticked.has(each.index));

  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      if (chosen.length === 0) {
        const none: CommandError = {
          kind: "invalid",
          message: "Tick at least one question to add as a card.",
        };
        throw none;
      }
      if (deckId === "") {
        const unchosen: CommandError = {
          kind: "invalid",
          message: "Choose a deck for these cards.",
          field: "deck",
        };
        throw unchosen;
      }
      const sent = chosen;
      try {
        const deck = await createFlashcards(
          Number(deckId),
          sent.map(({ question }) => ({ front: question.question, back: question.answer }))
        );
        return { deck, count: sent.length };
      } catch (err) {
        const error = toCommandError(err);
        if (error.index !== undefined) {
          // About one question's text: there's no field to fix, so say which.
          const position = missed.indexOf(sent[error.index]) + 1;
          const refused: CommandError = {
            kind: error.kind,
            message: `Missed question ${position}: ${error.message}`,
          };
          throw refused;
        }
        if (error.field || error.kind === "failed") throw err;
        // A refusal about no card is about the deck: archived or gone since
        // the list loaded. Nothing was saved.
        loadChoices();
        const refused: CommandError = { ...error, field: "deck" };
        throw refused;
      }
    },
    ({ deck, count }) => onSaved(deck, count),
    (field) => (field === "deck" ? "quiz-deck" : undefined)
  );

  function toggle(index: number, on: boolean) {
    if (saving) return;
    setTicked((current) => {
      const next = new Set(current);
      if (on) next.add(index);
      else next.delete(index);
      return next;
    });
  }

  return (
    <form className="suggest-step" aria-label="Missed questions" noValidate onSubmit={submit}>
      <h3 className="draft-label">Missed questions</h3>
      <ol className="card-list">
        {missed.map(({ question, index, chosen: answer }, position) => {
          const labelId = `missed-${index}-label`;
          return (
            <li key={index}>
              <div className="card-item" role="group" aria-labelledby={labelId}>
                <p id={labelId} className="quiz-missed-question">
                  {`${position + 1}. ${question.question}`}
                </p>
                <dl className="quiz-answers">
                  <dt>Your answer</dt>
                  <dd>{question.options[answer]}</dd>
                  <dt>Right answer</dt>
                  <dd>{question.answer}</dd>
                </dl>
                <figure className="draft-source">
                  <figcaption className="side-label">From your note</figcaption>
                  <blockquote className="draft-quote">{question.source}</blockquote>
                </figure>
                <label className="check-field">
                  <input
                    type="checkbox"
                    checked={ticked.has(index)}
                    aria-disabled={saving}
                    onChange={(event) => toggle(index, event.target.checked)}
                  />
                  <span>Add as a card</span>
                </label>
              </div>
            </li>
          );
        })}
      </ol>

      {choices.status === "loading" && (
        <p className="loading" role="status">
          Loading your decks…
        </p>
      )}
      {choices.status === "error" && (
        <p className="message-error" role="alert">
          {choices.message}
        </p>
      )}
      {choices.status === "ready" && choices.decks.length === 0 && (
        <p className="field-hint">
          You don't have a deck to add cards to yet. Create one on the Study Desk first; the Sample
          deck can't take new cards.
        </p>
      )}
      {choices.status === "ready" && choices.decks.length > 0 && (
        <>
          <SelectField
            id="quiz-deck"
            label="Deck"
            hint="Each card has the question on the front and the right answer on the back, and is due for review straight away."
            value={deckId}
            options={choices.decks.map((deck: Deck) => ({ value: String(deck.id), label: deck.name }))}
            placeholder="Choose a deck"
            error={fieldError("deck")}
            required
            onChange={(value) => {
              setDeckId(value);
              edited("deck");
            }}
          />
          <div className="form-actions">
            {/* `aria-disabled` rather than `disabled`, so keyboard focus stays put. */}
            <button type="submit" className="button button-primary" aria-disabled={saving}>
              {`Add ${cardCount(chosen.length)}`}
            </button>
          </div>
          <p className="form-status" role="status">
            {saving ? "Saving…" : ""}
          </p>
        </>
      )}
      {formError && (
        <p className="message-error" role="alert">
          {formError}
        </p>
      )}
    </form>
  );
}
