import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  createNote,
  getModels,
  stopTutor,
  summarizeNote,
  toCommandError,
  type Note,
  type SummaryBullet,
  type SummaryKeyTerm,
} from "./api";
import { SelectField } from "./forms";
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
const MODES = [{ id: "summary", label: "Summarise" }] as const;
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

function choose(messages: Partial<Extract<Stage, { step: "choose" }>> = {}): Stage {
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
          part={part ? `part ${partIndex + 1} of your note` : null}
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

function ChooseModel({
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
        A model in Ollama, on this computer, reads the note and writes up to 10 bullet points and 10
        key terms, in the note's language, each with the passage of your note it comes from. Points
        whose passage isn't in your note are left out. The note doesn't leave this computer, and
        nothing is saved unless you save the summary as a new note.
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
            hint={`This note is ${parts.noteLength.toLocaleString("en-US")} characters long, more than the ${SUGGESTION_NOTE_MAX_CHARS.toLocaleString("en-US")} a model reads at once, so it's split into ${parts.labels.length} parts where paragraphs end. A summary covers one part at a time; come back for the others.`}
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
          hint="One of the models in Ollama on this computer. Larger models usually write better summaries, but take longer."
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
            Summarise
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
          : `Writing a summary${part ? ` of ${part}` : ""} with ${model}… This can take a minute or more, depending on the model and this computer.`}
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
