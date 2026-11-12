import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  createFlashcards,
  createNote,
  getModels,
  gradeAnswer,
  socraticFollowup,
  stopTutor,
  summarizeNote,
  toCommandError,
  tutorQuestions,
  type CommandError,
  type Deck,
  type MissedPoint,
  type Note,
  type OpenQuestion,
  type SummaryBullet,
  type SummaryKeyTerm,
  type Verdict,
} from "./api";
import { loadDeckChoices, type DeckChoices } from "./deckChoices";
import { SelectField, TextField, useFormSave } from "./forms";
import { SparkleIcon } from "./icons";
import { noteParts, SUGGESTION_NOTE_MAX_CHARS } from "./noteParts";
import { partLabel, preferredModel, rememberModel } from "./SuggestCards";
import { Message, useFocusOnMount } from "./ui";
import { useUnsavedChanges } from "./unsaved";

/** The longest a note's title may be, in characters. */
const NOTE_TITLE_MAX_CHARS = 200;
const SUMMARY_PREFIX = "Summary: ";

/**
 * The ways the tutor can work on a note. Each is a panel that gets the note
 * and the local models; later modes (explaining it, Socratic follow-up) are
 * added here and shown as tabs.
 */
const MODES = [
  { id: "summary", label: "Summarise" },
  { id: "explain", label: "Explain it" },
] as const;
type TutorMode = (typeof MODES)[number]["id"];

type Setup =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; models: string[] };

/**
 * The tutor for a note: a local model in Ollama, on this computer, works on
 * the note and everything it writes is checked against the note. Nothing is
 * saved unless the user chooses to.
 */
export function Tutor({
  note,
  onCancel,
  onSaved,
}: {
  note: Note;
  onCancel: () => void;
  /** A summary was saved as a new note. */
  onSaved: (saved: Note) => void;
}) {
  const [setup, setSetup] = useState<Setup>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  const [mode, setMode] = useState<TutorMode>("summary");

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
        text="Ollama is running, but it has no models on this computer that can summarise a note. Download a model in Ollama, then check again."
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
    <section className="card" aria-labelledby="tutor-heading">
      <h2 id="tutor-heading" className="form-title">
        {`Tutor: ${note.title}`}
      </h2>
      {MODES.length > 1 && (
        <div role="tablist" aria-label="Tutor mode" className="actions">
          {MODES.map((each) => (
            <button
              key={each.id}
              type="button"
              role="tab"
              className="button button-small"
              aria-selected={mode === each.id}
              onClick={() => setMode(each.id)}
            >
              {each.label}
            </button>
          ))}
        </div>
      )}
      {mode === "summary" && (
        <Summariser
          note={note}
          models={setup.models}
          onCancel={onCancel}
          onSaved={onSaved}
          onNoModels={() => setSetup({ status: "ready", models: [] })}
        />
      )}
      {mode === "explain" && (
        <Explainer
          note={note}
          models={setup.models}
          onCancel={onCancel}
          onNoModels={() => setSetup({ status: "ready", models: [] })}
        />
      )}
    </section>
  );
}

type Stage =
  | { step: "choose"; notice: string | null; modelError: string | null; error: string | null }
  | { step: "writing"; model: string }
  | {
      step: "shown";
      model: string;
      bullets: SummaryBullet[];
      keyTerms: SummaryKeyTerm[];
      dropped: number;
      leftOut: number;
      saveError: string | null;
    };

type ChooseStage = Extract<Stage, { step: "choose" }>;

function choose(messages: Partial<ChooseStage> = {}): ChooseStage {
  return { step: "choose", notice: null, modelError: null, error: null, ...messages };
}

/** "1 point", "3 points". */
function pointCount(count: number): string {
  return count === 1 ? "1 point" : `${count} points`;
}

/** What the note-to-be holds: the bullets, then the key terms. */
export function summaryBody(bullets: SummaryBullet[], keyTerms: SummaryKeyTerm[]): string {
  const lines = bullets.map((bullet) => `- ${bullet.text}`).join("\n");
  if (keyTerms.length === 0) return lines;
  return `${lines}\n\nKey terms: ${keyTerms.map((each) => each.term).join(", ")}`;
}

/** "Summary: <title>", cut so it fits a note's title. */
export function summaryTitle(title: string): string {
  return `${SUMMARY_PREFIX}${Array.from(title)
    .slice(0, NOTE_TITLE_MAX_CHARS - SUMMARY_PREFIX.length)
    .join("")}`.trim();
}

/**
 * A summary of a note: choose a model (and a part, for a long note), wait
 * (with Stop), read the points with the passages they come from, and
 * optionally save it as a new note. Until it's saved or left, it's a draft and
 * counts as unsaved changes.
 */
function Summariser({
  note,
  models: initialModels,
  onCancel,
  onSaved,
  onNoModels,
}: {
  note: Note;
  models: string[];
  onCancel: () => void;
  onSaved: (saved: Note) => void;
  onNoModels: () => void;
}) {
  const parts = useMemo(() => noteParts(note.body), [note.body]);
  const split = parts.length > 1;
  const [partIndex, setPartIndex] = useState(0);
  const part = split ? parts[partIndex] : null;
  const [models, setModels] = useState(initialModels);
  const [model, setModel] = useState(() => preferredModel(initialModels));
  const [stage, setStage] = useState<Stage>(() => choose());
  // Bumped with every stage, so each one remounts and takes focus.
  const [round, setRound] = useState(0);
  const writingRef = useRef(false);
  const savingRef = useRef(false);
  const shownRef = useRef(true);

  useUnsavedChanges(stage.step === "shown");

  useEffect(() => {
    shownRef.current = true;
    return () => {
      shownRef.current = false;
      // Leaving while the summary is written: stop it, so the model isn't kept busy.
      if (writingRef.current) stopTutor().catch(() => {});
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
      show(choose({ modelError: "Choose a model to write the summary with." }));
      return;
    }
    writingRef.current = true;
    show({ step: "writing", model });

    let next: Stage;
    try {
      const outcome = await summarizeNote(note.id, model, part ?? undefined);
      if (outcome.status === "stopped") {
        next = choose({ notice: "Stopped. No summary was written." });
      } else {
        rememberModel(model);
        next = {
          step: "shown",
          model,
          bullets: outcome.bullets,
          keyTerms: outcome.keyTerms,
          dropped: outcome.dropped,
          leftOut: outcome.leftOut,
          saveError: null,
        };
      }
    } catch (err) {
      const failure = toCommandError(err);
      const fresh = failure.kind === "stale" ? await refreshModels() : null;
      next =
        fresh !== null && !fresh.includes(model)
          ? choose({ modelError: failure.message })
          : choose({ error: failure.message });
    }
    writingRef.current = false;
    if (shownRef.current) show(next);
  }

  async function save(): Promise<void> {
    if (stage.step !== "shown" || savingRef.current) return;
    savingRef.current = true;
    try {
      const saved = await createNote(
        summaryTitle(note.title),
        summaryBody(stage.bullets, stage.keyTerms)
      );
      if (shownRef.current) onSaved(saved);
    } catch (err) {
      const message = toCommandError(err).message;
      setStage((current) => (current.step === "shown" ? { ...current, saveError: message } : current));
    } finally {
      savingRef.current = false;
    }
  }

  return (
    <>
      {stage.step === "choose" && (
        <ChooseModel
          key={round}
          copy={SUMMARY_COPY}
          note={note}
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
          part={part ? { index: partIndex, count: parts.length, text: part.text } : null}
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
      )}
      {stage.step === "writing" && (
        <Writing
          key={round}
          model={stage.model}
          activity={`Writing a summary${part ? ` of part ${partIndex + 1} of your note` : ""}`}
          onStop={() => stopTutor().catch(() => {})}
        />
      )}
      {stage.step === "shown" && (
        <Shown
          key={round}
          stage={stage}
          onSave={save}
          onAgain={() => show(choose())}
          onDone={onCancel}
        />
      )}
    </>
  );
}

type PartChoice = {
  labels: string[];
  index: number;
  noteLength: number;
  onChoose: (index: number) => void;
};

/** The words of the choose step, which differ by mode. */
type ChooseCopy = { intro: string; partRule: string; modelHint: string; submit: string };

const SUMMARY_COPY: ChooseCopy = {
  intro:
    "A model in Ollama, on this computer, reads the note and writes up to 10 bullet points and 10 key terms, in the note's language, each with the passage of your note it comes from. Points whose passage isn't in your note are left out. The note doesn't leave this computer, and nothing is saved unless you save the summary as a new note.",
  partRule: "A summary covers one part at a time; come back for the others.",
  modelHint:
    "One of the models in Ollama on this computer. Larger models usually write better summaries, but take longer.",
  submit: "Summarise",
};

const EXPLAIN_COPY: ChooseCopy = {
  intro:
    "A model in Ollama, on this computer, asks up to 5 questions about the note. You answer each in your own words, and the model checks your answer against the note only, showing what you got right, what you missed (with the passage of your note) and what was wrong. Questions whose passage isn't in your note are left out. The note and your answers don't leave this computer, and nothing is saved unless you add missed points as cards.",
  partRule: "Questions cover one part at a time; come back for the others.",
  modelHint:
    "One of the models in Ollama on this computer. Larger models usually ask and grade better, but take longer.",
  submit: "Ask me questions",
};

function ChooseModel({
  copy,
  note,
  parts,
  part,
  models,
  model,
  stage,
  onModel,
  onStart,
  onCancel,
}: {
  copy: ChooseCopy;
  note: Note;
  parts: PartChoice | null;
  part: { index: number; count: number; text: string } | null;
  models: string[];
  model: string;
  stage: Extract<Stage, { step: "choose" }>;
  onModel: (model: string) => void;
  onStart: (event: FormEvent<HTMLFormElement>) => void;
  onCancel: () => void;
}) {
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(stage.notice !== null);

  return (
    <>
      <section className="note-context" tabIndex={0} aria-labelledby="tutor-context-label">
        <p id="tutor-context-label" className="side-label">
          {part ? `Your note, part ${part.index + 1} of ${part.count}` : "Your note"}
        </p>
        <p className="note-body">{part ? part.text : note.body}</p>
      </section>
      <p id="tutor-hint" className="field-hint">
        {copy.intro}
      </p>
      <form
        className="suggest-step"
        aria-label="Choose a model"
        noValidate
        onSubmit={onStart}
      >
        {stage.notice && (
          <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
            {stage.notice}
          </p>
        )}
        {parts && (
          <SelectField
            id="tutor-part"
            label="Part of the note"
            hint={`This note is ${parts.noteLength.toLocaleString("en-US")} characters long, more than the ${SUGGESTION_NOTE_MAX_CHARS.toLocaleString("en-US")} a model reads at once, so it's split into ${parts.labels.length} parts where paragraphs end. ${copy.partRule}`}
            value={String(parts.index)}
            options={parts.labels.map((label, index) => ({ value: String(index), label }))}
            placeholder="Choose a part"
            required
            onChange={(value) => {
              if (value !== "") parts.onChoose(Number(value));
            }}
          />
        )}
        <SelectField
          id="tutor-model"
          label="Model"
          hint={copy.modelHint}
          describedBy="tutor-hint"
          value={model}
          options={models.map((name) => ({ value: name, label: name }))}
          placeholder="Choose a model"
          error={stage.modelError ?? undefined}
          required
          onChange={onModel}
        />
        <div className="form-actions">
          <button type="submit" className="button button-primary">
            <SparkleIcon />
            {copy.submit}
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
    </>
  );
}

/** While the model works. Focus goes to the status, so a held Enter can't stop it. */
function Writing({
  model,
  activity,
  onStop,
}: {
  model: string;
  /** What the model is doing, such as "Writing a summary". */
  activity: string;
  onStop: () => void;
}) {
  const statusRef = useFocusOnMount<HTMLParagraphElement>(true);
  const [stopping, setStopping] = useState(false);

  return (
    <div className="confirm confirm-calm suggest-waiting">
      <p ref={statusRef} className="confirm-question" role="status" tabIndex={-1}>
        {stopping
          ? "Stopping…"
          : `${activity} with ${model}… This can take a minute or more, depending on the model and this computer.`}
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

/** What was left out, said above the summary. */
function leftOutText(dropped: number, leftOut: number): string {
  const droppedText =
    dropped === 0
      ? ""
      : dropped === 1
        ? " 1 point was left out because its passage couldn't be found in your note."
        : ` ${dropped} points were left out because their passages couldn't be found in your note.`;
  const leftOutText =
    leftOut === 0
      ? ""
      : leftOut === 1
        ? " 1 more was left out because it was empty, too long or a repeat."
        : ` ${leftOut} more were left out because they were empty, too long or repeats.`;
  return `${droppedText}${leftOutText}`;
}

function Shown({
  stage,
  onSave,
  onAgain,
  onDone,
}: {
  stage: Extract<Stage, { step: "shown" }>;
  onSave: () => Promise<void>;
  onAgain: () => void;
  onDone: () => void;
}) {
  const introRef = useFocusOnMount<HTMLParagraphElement>(true);
  const [saving, setSaving] = useState(false);

  return (
    <div className="suggest-step">
      <p ref={introRef} className="notice" role="status" tabIndex={-1}>
        {`${stage.model} wrote ${pointCount(stage.bullets.length)} and ${stage.keyTerms.length} key ${stage.keyTerms.length === 1 ? "term" : "terms"}.${leftOutText(stage.dropped, stage.leftOut)} Check them against your note: nothing is saved yet.`}
      </p>
      <h3 className="side-label">Summary</h3>
      <ul className="summary-list">
        {stage.bullets.map((bullet, index) => (
          <li key={index}>
            <p>{bullet.text}</p>
            <p className="field-hint">{`From your note: “${bullet.quote}”`}</p>
          </li>
        ))}
      </ul>
      {stage.keyTerms.length > 0 && (
        <>
          <h3 className="side-label">Key terms</h3>
          <ul className="summary-list">
            {stage.keyTerms.map((each, index) => (
              <li key={index}>
                <p>
                  <strong>{each.term}</strong>
                </p>
                <p className="field-hint">{`From your note: “${each.quote}”`}</p>
              </li>
            ))}
          </ul>
        </>
      )}
      {stage.saveError && (
        <p className="message-error" role="alert">
          {stage.saveError}
        </p>
      )}
      <div className="form-actions">
        <button
          type="button"
          className="button button-primary"
          aria-disabled={saving}
          onClick={() => {
            if (saving) return;
            setSaving(true);
            // On success the panel is replaced; on failure it can be tried again.
            onSave().finally(() => setSaving(false));
          }}
        >
          Save as note
        </button>
        <button type="button" className="button" onClick={onAgain}>
          Summarise again
        </button>
        <button type="button" className="button" onClick={onDone}>
          Discard
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Explain it: open questions answered in the student's own words, graded
// against the note.
// ---------------------------------------------------------------------------

/** The most characters an answer may have (`ANSWER_MAX_CHARS` in `tutor.rs`). */
export const ANSWER_MAX_CHARS = 4_000;

type ExplainStage =
  | ChooseStage
  | { step: "writing"; model: string }
  | {
      step: "practice";
      model: string;
      questions: OpenQuestion[];
      index: number;
      dropped: number;
      leftOut: number;
    };

/** "1 card", "3 cards". */
function cardCount(count: number): string {
  return count === 1 ? "1 card" : `${count} cards`;
}

const VERDICT_LABELS: Record<Verdict, string> = {
  right: "Right",
  partly: "Partly right",
  wrong: "Not right",
};

/** What was left out of the questions, said above the first one. */
function questionsLeftOut(dropped: number, leftOut: number): string {
  const droppedText =
    dropped === 0
      ? ""
      : dropped === 1
        ? " 1 question was left out because its passage couldn't be found in your note."
        : ` ${dropped} questions were left out because their passages couldn't be found in your note.`;
  const leftOutText =
    leftOut === 0
      ? ""
      : leftOut === 1
        ? " 1 more was left out because it was empty, too long or a repeat."
        : ` ${leftOut} more were left out because they were empty, too long or repeats.`;
  return `${droppedText}${leftOutText}`;
}

/**
 * "Explain it": choose a model (and a part, for a long note), get questions,
 * answer each in your own words, and see it graded against the note. Missed
 * points can be added as cards. Nothing else is stored.
 */
function Explainer({
  note,
  models: initialModels,
  onCancel,
  onNoModels,
}: {
  note: Note;
  models: string[];
  onCancel: () => void;
  onNoModels: () => void;
}) {
  const parts = useMemo(() => noteParts(note.body), [note.body]);
  const split = parts.length > 1;
  const [partIndex, setPartIndex] = useState(0);
  const part = split ? parts[partIndex] : null;
  const [models, setModels] = useState(initialModels);
  const [model, setModel] = useState(() => preferredModel(initialModels));
  const [stage, setStage] = useState<ExplainStage>(() => choose());
  // Bumped with every stage, so each one remounts and takes focus.
  const [round, setRound] = useState(0);
  const writingRef = useRef(false);
  const shownRef = useRef(true);

  useEffect(() => {
    shownRef.current = true;
    return () => {
      shownRef.current = false;
      // Leaving while the questions are written: stop it, so the model isn't kept busy.
      if (writingRef.current) stopTutor().catch(() => {});
    };
  }, []);

  function show(next: ExplainStage) {
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

  /** The model or the note's part went stale: back to the choice, saying why. */
  async function backToChoice(failure: CommandError, usedModel: string) {
    const fresh = failure.kind === "stale" ? await refreshModels() : null;
    if (!shownRef.current) return;
    show(
      fresh !== null && !fresh.includes(usedModel)
        ? choose({ modelError: failure.message })
        : choose({ error: failure.message })
    );
  }

  async function start(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (writingRef.current) return;
    if (model === "") {
      show(choose({ modelError: "Choose a model to ask the questions with." }));
      return;
    }
    writingRef.current = true;
    show({ step: "writing", model });

    try {
      const outcome = await tutorQuestions(note.id, model, part ?? undefined);
      writingRef.current = false;
      if (!shownRef.current) return;
      if (outcome.status === "stopped") {
        show(choose({ notice: "Stopped. No questions were written." }));
      } else {
        rememberModel(model);
        show({
          step: "practice",
          model,
          questions: outcome.questions,
          index: 0,
          dropped: outcome.dropped,
          leftOut: outcome.leftOut,
        });
      }
    } catch (err) {
      writingRef.current = false;
      await backToChoice(toCommandError(err), model);
    }
  }

  return (
    <>
      {stage.step === "choose" && (
        <ChooseModel
          key={round}
          copy={EXPLAIN_COPY}
          note={note}
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
          part={part ? { index: partIndex, count: parts.length, text: part.text } : null}
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
      )}
      {stage.step === "writing" && (
        <Writing
          key={round}
          model={stage.model}
          activity={`Writing questions${part ? ` about part ${partIndex + 1} of your note` : ""}`}
          onStop={() => stopTutor().catch(() => {})}
        />
      )}
      {stage.step === "practice" && (
        <Practice
          key={`${round}-${stage.index}`}
          note={note}
          part={part}
          model={stage.model}
          question={stage.questions[stage.index]}
          position={stage.index + 1}
          total={stage.questions.length}
          leftOut={stage.index === 0 ? questionsLeftOut(stage.dropped, stage.leftOut) : ""}
          onNext={() =>
            setStage((current) =>
              current.step === "practice" && current.index + 1 < current.questions.length
                ? { ...current, index: current.index + 1, dropped: 0, leftOut: 0 }
                : current
            )
          }
          onAgain={() => show(choose())}
          onDone={onCancel}
          onStale={(failure) => backToChoice(failure, stage.model)}
        />
      )}
    </>
  );
}

type Graded = {
  score: number;
  verdict: Verdict;
  correct: string[];
  missed: MissedPoint[];
  wrong: string[];
  dropped: number;
};

/**
 * One question: the answer box, then the grade. A typed answer not yet graded,
 * or graded misses not yet added as cards, count as unsaved changes.
 */
function Practice({
  note,
  part,
  model,
  question,
  position,
  total,
  leftOut,
  onNext,
  onAgain,
  onDone,
  onStale,
}: {
  note: Note;
  part: { start: number; end: number } | null;
  model: string;
  question: OpenQuestion;
  position: number;
  total: number;
  leftOut: string;
  onNext: () => void;
  onAgain: () => void;
  onDone: () => void;
  onStale: (failure: CommandError) => void;
}) {
  const [answer, setAnswer] = useState("");
  const [phase, setPhase] = useState<"answering" | "grading" | "graded">("answering");
  const [graded, setGraded] = useState<Graded | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [added, setAdded] = useState<{ deck: string; count: number } | null>(null);
  const gradingRef = useRef(false);
  const shownRef = useRef(true);
  const resultRef = useFocusOnMount<HTMLParagraphElement>(phase === "graded");

  useUnsavedChanges(
    (phase !== "graded" && answer.trim() !== "") ||
      (graded !== null && graded.missed.length > 0 && added === null)
  );

  useEffect(() => {
    shownRef.current = true;
    return () => {
      shownRef.current = false;
      // Leaving while the answer is graded: stop it, so the model isn't kept busy.
      if (gradingRef.current) stopTutor().catch(() => {});
    };
  }, []);

  async function check(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (gradingRef.current) return;
    setNotice(null);
    if (answer.trim() === "") {
      setError("Write your answer first.");
      return;
    }
    if (Array.from(answer.trim()).length > ANSWER_MAX_CHARS) {
      setError(`Answers can be at most ${ANSWER_MAX_CHARS.toLocaleString("en-US")} characters.`);
      return;
    }
    setError(null);
    gradingRef.current = true;
    setPhase("grading");
    try {
      const outcome = await gradeAnswer(note.id, model, question, answer, part ?? undefined);
      gradingRef.current = false;
      if (!shownRef.current) return;
      if (outcome.status === "stopped") {
        setNotice("Stopped. Your answer wasn't graded.");
        setPhase("answering");
      } else {
        setGraded(outcome);
        setPhase("graded");
      }
    } catch (err) {
      gradingRef.current = false;
      if (!shownRef.current) return;
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale(failure);
        return;
      }
      setError(failure.message);
      setPhase("answering");
    }
  }

  return (
    <div className="suggest-step">
      {leftOut !== "" && <p className="field-hint">{leftOut.trim()}</p>}
      <h3 className="side-label">{`Question ${position} of ${total}`}</h3>
      <p className="quiz-missed-question">{question.question}</p>

      {phase === "answering" && (
        <form aria-label="Answer the question" noValidate onSubmit={check}>
          {notice && (
            <p className="notice" role="status">
              {notice}
            </p>
          )}
          <TextField
            id="tutor-answer"
            label="Your answer"
            hint={`In your own words. The model grades it against your note only. Up to ${ANSWER_MAX_CHARS.toLocaleString("en-US")} characters.`}
            value={answer}
            onChange={(value) => {
              setAnswer(value);
              setError(null);
            }}
            error={error ?? undefined}
            required
            multiline
            rows={6}
            autoFocus
          />
          <div className="form-actions">
            <button type="submit" className="button button-primary">
              Check my answer
            </button>
            <button type="button" className="button" onClick={onDone}>
              Back to note
            </button>
          </div>
        </form>
      )}

      {phase !== "answering" && (
        <section aria-label="Your answer">
          <h4 className="side-label">Your answer</h4>
          <p className="note-body">{answer.trim()}</p>
        </section>
      )}

      {phase === "grading" && (
        <Writing
          model={model}
          activity="Checking your answer"
          onStop={() => stopTutor().catch(() => {})}
        />
      )}

      {phase === "graded" && graded && (
        <>
          <p ref={resultRef} className="notice" role="status" tabIndex={-1}>
            {`${VERDICT_LABELS[graded.verdict]}: ${graded.score} out of 100.`}
          </p>
          <GradeDetails graded={graded} question={question} />
          {added && (
            <p className="notice" role="status">
              {`Added ${cardCount(added.count)} to ${added.deck}.`}
            </p>
          )}
          {graded.missed.length > 0 && added === null && (
            <AddMisses
              question={question.question}
              missed={graded.missed}
              onAdded={(deck, count) => setAdded({ deck, count })}
            />
          )}
          <FollowUp
            note={note}
            part={part}
            model={model}
            question={question}
            answer={answer.trim()}
            onStale={onStale}
          />
          <div className="form-actions">
            {position < total && (
              <button type="button" className="button button-primary" onClick={onNext}>
                Next question
              </button>
            )}
            <button type="button" className="button" onClick={onAgain}>
              New questions
            </button>
            <button type="button" className="button" onClick={onDone}>
              Back to note
            </button>
          </div>
        </>
      )}
    </div>
  );
}

/**
 * Optional Socratic follow-up after a graded answer: one guiding question
 * from the model, with the passage of the note it is about. Only asked when
 * the student presses the button; nothing is saved.
 */
function FollowUp({
  note,
  part,
  model,
  question,
  answer,
  onStale,
}: {
  note: Note;
  part: { start: number; end: number } | null;
  model: string;
  question: OpenQuestion;
  answer: string;
  onStale: (failure: CommandError) => void;
}) {
  const [phase, setPhase] = useState<"idle" | "asking" | "done">("idle");
  const [found, setFound] = useState<{ question: string; quote: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const askingRef = useRef(false);
  const shownRef = useRef(true);

  useEffect(() => {
    shownRef.current = true;
    return () => {
      shownRef.current = false;
      if (askingRef.current) stopTutor().catch(() => {});
    };
  }, []);

  async function ask() {
    if (askingRef.current) return;
    askingRef.current = true;
    setError(null);
    setPhase("asking");
    try {
      const outcome = await socraticFollowup(note.id, model, question, answer, part ?? undefined);
      askingRef.current = false;
      if (!shownRef.current) return;
      if (outcome.status === "stopped") {
        setPhase("idle");
      } else {
        setFound({ question: outcome.question, quote: outcome.quote });
        setPhase("done");
      }
    } catch (err) {
      askingRef.current = false;
      if (!shownRef.current) return;
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale(failure);
        return;
      }
      setError(failure.message);
      setPhase("idle");
    }
  }

  return (
    <section aria-label="Follow-up question">
      {phase === "asking" && (
        <Writing
          model={model}
          activity="Thinking of a follow-up question"
          onStop={() => stopTutor().catch(() => {})}
        />
      )}
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
      {phase === "done" && found && (
        <>
          <h4 className="side-label">A question to think about</h4>
          <p className="quiz-missed-question">{found.question}</p>
          <p className="field-hint">{`From your note: “${found.quote}”`}</p>
        </>
      )}
      {(phase === "idle" || phase === "done") && (
        <div className="form-actions">
          <button type="button" className="button" onClick={ask}>
            {phase === "done" ? "Ask me another follow-up" : "Ask me a follow-up"}
          </button>
        </div>
      )}
    </section>
  );
}

function GradeDetails({ graded, question }: { graded: Graded; question: OpenQuestion }) {
  const nothing =
    graded.correct.length === 0 && graded.missed.length === 0 && graded.wrong.length === 0;
  return (
    <>
      {nothing && <p className="field-hint">The model gave no details.</p>}
      {graded.correct.length > 0 && (
        <>
          <h4 className="side-label">What you got right</h4>
          <ul className="summary-list">
            {graded.correct.map((each, index) => (
              <li key={index}>{each}</li>
            ))}
          </ul>
        </>
      )}
      {graded.missed.length > 0 && (
        <>
          <h4 className="side-label">What you missed</h4>
          <ul className="summary-list">
            {graded.missed.map((each, index) => (
              <li key={index}>
                <p>{each.point}</p>
                <p className="field-hint">{`From your note: “${each.quote}”`}</p>
              </li>
            ))}
          </ul>
        </>
      )}
      {graded.dropped > 0 && (
        <p className="field-hint">
          {graded.dropped === 1
            ? "1 missed point was left out because its passage couldn't be found in your note."
            : `${graded.dropped} missed points were left out because their passages couldn't be found in your note.`}
        </p>
      )}
      {graded.wrong.length > 0 && (
        <>
          <h4 className="side-label">What was wrong</h4>
          <ul className="summary-list">
            {graded.wrong.map((each, index) => (
              <li key={index}>{each}</li>
            ))}
          </ul>
        </>
      )}
      <h4 className="side-label">The answer from your note</h4>
      <p>{question.reference}</p>
      <p className="field-hint">{`From your note: “${question.quote}”`}</p>
    </>
  );
}

/**
 * Missed points as cards (the question on the front, the missed point on the
 * back) for a chosen deck, all at once through `createFlashcards`.
 */
function AddMisses({
  question,
  missed,
  onAdded,
}: {
  question: string;
  missed: MissedPoint[];
  onAdded: (deckName: string, count: number) => void;
}) {
  const [choices, setChoices] = useState<DeckChoices>({ status: "loading" });
  // Every missed point is ticked to start with.
  const [ticked, setTicked] = useState<Set<number>>(() => new Set(missed.map((_, index) => index)));
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

  const chosen = missed.filter((_, index) => ticked.has(index));

  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      if (chosen.length === 0) {
        const none: CommandError = {
          kind: "invalid",
          message: "Tick at least one missed point to add as a card.",
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
          sent.map((each) => ({ front: question, back: each.point }))
        );
        return { deck, count: sent.length };
      } catch (err) {
        const error = toCommandError(err);
        if (error.index !== undefined) {
          // About one card's text: there's no field to fix, so say which.
          const position = missed.indexOf(sent[error.index]) + 1;
          const refused: CommandError = {
            kind: error.kind,
            message: `Missed point ${position}: ${error.message}`,
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
    ({ deck, count }) => onAdded(deck.name, count),
    (field) => (field === "deck" ? "tutor-deck" : undefined)
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
    <form className="suggest-step" aria-label="Missed points" noValidate onSubmit={submit}>
      <h4 className="draft-label">Add missed points as cards</h4>
      <ul className="card-list">
        {missed.map((each, index) => (
          <li key={index}>
            <label className="check-field">
              <input
                type="checkbox"
                checked={ticked.has(index)}
                aria-disabled={saving}
                onChange={(event) => toggle(index, event.target.checked)}
              />
              <span>{each.point}</span>
            </label>
          </li>
        ))}
      </ul>

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
            id="tutor-deck"
            label="Deck"
            hint="Each card has the question on the front and the missed point on the back, and is due for review straight away."
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
