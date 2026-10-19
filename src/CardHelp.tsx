import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  createNote,
  getCardHelp,
  getModels,
  stopCardHelp,
  toCommandError,
  type CardHelp as Help,
  type Flashcard,
  type HelpKind,
} from "./api";
import { SelectField } from "./forms";
import { SparkleIcon } from "./icons";
import { preferredModel, rememberModel } from "./SuggestCards";
import { useFocusOnMount } from "./ui";

/** The kinds of help, in the order offered, with what each is called. */
const KINDS: { value: HelpKind; label: string; hint: string }[] = [
  { value: "mnemonic", label: "Mnemonic", hint: "an acronym, rhyme or image to recall it by" },
  { value: "explanation", label: "Simpler explanation", hint: "the answer in plainer words" },
  { value: "analogy", label: "Analogy", hint: "the idea compared with something familiar" },
  { value: "question", label: "Reframed question", hint: "the question asked another way" },
];

function kindLabel(kind: HelpKind): string {
  return KINDS.find((each) => each.value === kind)?.label ?? kind;
}

/** A note's longest title, as Rust allows (`notes.rs`). */
const NOTE_TITLE_MAX_CHARS = 200;

/** The note that keeps help: titled after the card, with the card under the help. */
export function helpNote(card: Flashcard, help: Help): { title: string; body: string } {
  const front = card.front.split(/\s+/).filter(Boolean).join(" ");
  const title = Array.from(`Help: ${front}`).slice(0, NOTE_TITLE_MAX_CHARS).join("").trim();
  const body = `${kindLabel(help.kind)}\n\n${help.text}\n\nFrom the card\nQuestion: ${card.front}\nAnswer: ${card.back}`;
  return { title, body };
}

type Stage =
  | { step: "closed" }
  | { step: "loading" }
  | { step: "unavailable"; message: string }
  | { step: "noModels" }
  | { step: "choose"; models: string[]; error: string | null }
  | { step: "writing"; models: string[]; model: string; kind: HelpKind }
  | { step: "written"; models: string[]; model: string; help: Help };

/**
 * Help with the card being reviewed, once its answer is shown: a model in
 * Ollama on this computer writes a mnemonic, a simpler explanation, an analogy
 * or the question asked another way, from the card's front and back only. It's
 * a draft; nothing is saved unless the user adds it as a note.
 *
 * `offered` (Rust's call, after Again three times in a row) shows it as a
 * suggestion; otherwise it's a quiet button. Rating never waits on it: the
 * rating buttons stay as they are, and rating the card replaces this panel,
 * which stops any help still being written.
 */
export function CardHelp({
  card,
  offered,
  againStreak,
}: {
  card: Flashcard;
  offered: boolean;
  againStreak: number;
}) {
  const [stage, setStage] = useState<Stage>({ step: "closed" });
  const [model, setModel] = useState("");
  const [kind, setKind] = useState<HelpKind>("mnemonic");
  const writing = useRef(false);
  writing.current = stage.step === "writing";

  // Moving on to the next card mid-request: stop it, so the model isn't kept busy.
  useEffect(
    () => () => {
      if (writing.current) stopCardHelp().catch(() => {});
    },
    []
  );

  async function open() {
    setStage({ step: "loading" });
    try {
      const models = await getModels();
      if (models.length === 0) {
        setStage({ step: "noModels" });
        return;
      }
      setModel((current) => (models.includes(current) ? current : preferredModel(models)));
      setStage({ step: "choose", models, error: null });
    } catch (err) {
      setStage({ step: "unavailable", message: toCommandError(err).message });
    }
  }

  async function write(models: string[], chosen: string, chosenKind: HelpKind) {
    setStage({ step: "writing", models, model: chosen, kind: chosenKind });
    try {
      const outcome = await getCardHelp(card.id, chosen, chosenKind);
      if (outcome.status === "stopped") {
        setStage({ step: "choose", models, error: null });
        return;
      }
      rememberModel(chosen);
      setStage({ step: "written", models, model: chosen, help: outcome.help });
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // The model may have left Ollama: offer the ones there now.
        try {
          const fresh = await getModels();
          if (fresh.length === 0) {
            setStage({ step: "noModels" });
            return;
          }
          setModel((current) => (fresh.includes(current) ? current : ""));
          setStage({ step: "choose", models: fresh, error: failure.message });
          return;
        } catch {
          // Show the first problem; the user can try again.
        }
      }
      setStage({ step: "choose", models, error: failure.message });
    }
  }

  if (stage.step === "closed") {
    return offered ? (
      <div className="confirm confirm-calm card-help-offer">
        <p className="confirm-question">
          You've answered Again on this card {againStreak} times in a row. A model in Ollama on
          this computer can help you remember it in another way.
        </p>
        <div className="deck-actions">
          <button type="button" className="button button-tonal button-small" onClick={open}>
            <SparkleIcon />
            Get help with this card
          </button>
        </div>
      </div>
    ) : (
      <button type="button" className="button button-quiet button-small" onClick={open}>
        <SparkleIcon />
        Get help with this card
      </button>
    );
  }

  return (
    <section className="card-help" aria-labelledby="card-help-heading">
      <h3 id="card-help-heading" className="draft-label">
        Help with this card
      </h3>
      {stage.step === "loading" && (
        <p className="loading" role="status">
          Checking Ollama…
        </p>
      )}
      {stage.step === "unavailable" && (
        <Problem
          text={stage.message}
          onRetry={open}
          onClose={() => setStage({ step: "closed" })}
        />
      )}
      {stage.step === "noModels" && (
        <Problem
          text="Ollama is running, but it has no models on this computer that can write. Download a model in Ollama, then check again."
          onRetry={open}
          onClose={() => setStage({ step: "closed" })}
        />
      )}
      {stage.step === "choose" && (
        <Choose
          models={stage.models}
          model={model}
          kind={kind}
          error={stage.error}
          onModel={setModel}
          onKind={setKind}
          onWrite={(chosen, chosenKind) => write(stage.models, chosen, chosenKind)}
          onClose={() => setStage({ step: "closed" })}
        />
      )}
      {stage.step === "writing" && (
        <Writing
          model={stage.model}
          kind={stage.kind}
          onStop={() => stopCardHelp().catch(() => {})}
        />
      )}
      {stage.step === "written" && (
        <Written
          card={card}
          model={stage.model}
          help={stage.help}
          onAgain={() => setStage({ step: "choose", models: stage.models, error: null })}
          onClose={() => setStage({ step: "closed" })}
        />
      )}
    </section>
  );
}

function Problem({
  text,
  onRetry,
  onClose,
}: {
  text: string;
  onRetry: () => void;
  onClose: () => void;
}) {
  const ref = useFocusOnMount<HTMLParagraphElement>(true);
  return (
    <>
      <p ref={ref} className="message-error" role="alert" tabIndex={-1}>
        {text}
      </p>
      <div className="deck-actions">
        <button type="button" className="button button-small" onClick={onRetry}>
          Check again
        </button>
        <button type="button" className="button button-quiet button-small" onClick={onClose}>
          Close
        </button>
      </div>
    </>
  );
}

function Choose({
  models,
  model,
  kind,
  error,
  onModel,
  onKind,
  onWrite,
  onClose,
}: {
  models: string[];
  model: string;
  kind: HelpKind;
  error: string | null;
  onModel: (model: string) => void;
  onKind: (kind: HelpKind) => void;
  onWrite: (model: string, kind: HelpKind) => void;
  onClose: () => void;
}) {
  const [modelError, setModelError] = useState<string | null>(null);
  const errorRef = useFocusOnMount<HTMLParagraphElement>(error !== null);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (model === "") {
      setModelError("Choose a model to write help with.");
      return;
    }
    onWrite(model, kind);
  }

  return (
    <form className="suggest-step" aria-label="Choose help" noValidate onSubmit={submit}>
      <p id="card-help-hint" className="field-hint">
        The model reads only this card's question and answer, on this computer. Nothing is saved
        unless you add the help as a note. Models still get things wrong, so check what it writes
        against your material.
      </p>
      <fieldset className="field card-help-kinds">
        <legend className="field-label">Kind of help</legend>
        {KINDS.map((each) => (
          <label key={each.value} className="card-help-kind">
            <input
              type="radio"
              name="card-help-kind"
              value={each.value}
              checked={kind === each.value}
              onChange={() => onKind(each.value)}
            />
            <span>
              <strong>{each.label}</strong>: {each.hint}
            </span>
          </label>
        ))}
      </fieldset>
      <SelectField
        id="card-help-model"
        label="Model"
        hint="One of the models in Ollama on this computer."
        describedBy="card-help-hint"
        value={model}
        options={models.map((name) => ({ value: name, label: name }))}
        placeholder="Choose a model"
        error={modelError ?? undefined}
        required
        autoFocus={error === null}
        onChange={(value) => {
          setModelError(null);
          onModel(value);
        }}
      />
      <div className="form-actions">
        <button type="submit" className="button button-primary button-small">
          <SparkleIcon />
          Write help
        </button>
        <button type="button" className="button button-quiet button-small" onClick={onClose}>
          Close
        </button>
      </div>
      {error && (
        <p ref={errorRef} className="message-error" role="alert" tabIndex={-1}>
          {error}
        </p>
      )}
    </form>
  );
}

/** While the model works. Focus goes to the status, so a held Enter can't stop it. */
function Writing({ model, kind, onStop }: { model: string; kind: HelpKind; onStop: () => void }) {
  const statusRef = useFocusOnMount<HTMLParagraphElement>(true);
  const [stopping, setStopping] = useState(false);
  return (
    <div className="confirm confirm-calm suggest-waiting">
      <p ref={statusRef} className="confirm-question" role="status" tabIndex={-1}>
        {stopping
          ? "Stopping…"
          : `Writing a ${kindLabel(kind).toLowerCase()} with ${model}… You can rate the card meanwhile.`}
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

/** The help, for the user to read, and keep as a note if they want it. */
function Written({
  card,
  model,
  help,
  onAgain,
  onClose,
}: {
  card: Flashcard;
  model: string;
  help: Help;
  onAgain: () => void;
  onClose: () => void;
}) {
  const textRef = useFocusOnMount<HTMLDivElement>(true);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [saved, setSaved] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  async function addNote() {
    if (savingRef.current || saved !== null) return;
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    const note = helpNote(card, help);
    try {
      const created = await createNote(note.title, note.body);
      setSaved(created.title);
    } catch (err) {
      setSaveError(toCommandError(err).message);
    }
    savingRef.current = false;
    setSaving(false);
  }

  return (
    <>
      <div ref={textRef} className="draft-source" tabIndex={-1} aria-label={kindLabel(help.kind)}>
        <p className="draft-label">
          {kindLabel(help.kind)} · {model}
        </p>
        <p className="draft-quote">{help.text}</p>
        {help.unfamiliar.length > 0 && (
          <p className="draft-check">Not on the card: {help.unfamiliar.join(", ")}</p>
        )}
      </div>
      <p className="field-hint">
        Written by a model from this card only. It's a draft: nothing is saved unless you add it
        as a note.
      </p>
      {saved !== null ? (
        <p className="notice" role="status">
          Added to your notes as “{saved}”.
        </p>
      ) : null}
      <div className="deck-actions">
        {saved === null && (
          <button
            type="button"
            className="button button-tonal button-small"
            aria-disabled={saving}
            onClick={addNote}
          >
            {saving ? "Adding…" : "Add as a note"}
          </button>
        )}
        <button type="button" className="button button-small" onClick={onAgain}>
          Try another
        </button>
        <button type="button" className="button button-quiet button-small" onClick={onClose}>
          Close
        </button>
      </div>
      {saveError && (
        <p className="message-error" role="alert">
          {saveError}
        </p>
      )}
    </>
  );
}
