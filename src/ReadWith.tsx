import { useCallback, useEffect, useState } from "react";
import {
  getPagesToRecognise,
  getPagesToRecogniseWithModel,
  getVisionModels,
  recognisePicture,
  recognisePictureWithModel,
  recogniseSourcePages,
  recogniseSourcePagesWithModel,
  toCommandError,
  type ModelPictureReading,
  type ModelSourceRecognition,
  type PageProgress,
} from "./api";

/**
 * What reads text in pictures and scanned pages: the engine built into
 * Synapse, or a vision model in Ollama on this computer.
 */
export type Reader = { engine: "ocrs" } | { engine: "ollama"; model: string };

export const BUILT_IN: Reader = { engine: "ocrs" };

/** Where the reader last chosen is remembered, in this computer's web storage. */
const READER_KEY = "synapse.readWith";

/** The select's value for the built-in engine; a model's is its name. */
const BUILT_IN_VALUE = "";

// Remembering the reader is only a convenience: if storage isn't available,
// the built-in engine is used until the user chooses again.
function rememberedModel(): string | null {
  try {
    return localStorage.getItem(READER_KEY);
  } catch {
    return null;
  }
}

function rememberReader(reader: Reader) {
  try {
    if (reader.engine === "ollama") localStorage.setItem(READER_KEY, reader.model);
    else localStorage.removeItem(READER_KEY);
  } catch {
    // As above.
  }
}

/** Ollama's vision models, while loading them, or why there are none to offer. */
export type VisionModels =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; models: string[] }
  | { status: "unavailable"; message: string };

/**
 * The reader to use, and the vision models it can be chosen from. Starts
 * with the reader chosen last. Ollama isn't asked for its models until a
 * [`ReadWith`] choice is shown (or `reload` is called): then a remembered
 * model that has gone, or an Ollama that can't be reached, means the
 * built-in engine.
 */
export function useReader() {
  const [models, setModels] = useState<VisionModels>({ status: "idle" });
  const [reader, setReader] = useState<Reader>(() => {
    const remembered = rememberedModel();
    return remembered === null ? BUILT_IN : { engine: "ollama", model: remembered };
  });

  const load = useCallback(async () => {
    setModels({ status: "loading" });
    let found: string[];
    try {
      found = await getVisionModels();
    } catch (err) {
      setModels({ status: "unavailable", message: toCommandError(err).message });
      setReader(BUILT_IN);
      return;
    }
    setModels({ status: "ready", models: found });
    setReader((current) =>
      current.engine === "ollama" && found.includes(current.model) ? current : BUILT_IN,
    );
  }, []);

  const choose = useCallback((next: Reader) => {
    setReader(next);
    rememberReader(next);
  }, []);

  return { reader, models, choose, reload: load };
}

export type ReaderState = ReturnType<typeof useReader>;

/**
 * Choosing what reads the text: *Built in*, or one of Ollama's vision models.
 * Put it beside a *Read* action, and read with [`readPicture`] or
 * [`readSourcePages`] using `state.reader`. Showing it asks Ollama for its
 * models. `brief` leaves out the hint about each reader.
 */
export function ReadWith({
  id,
  state,
  disabled = false,
  brief = false,
}: {
  id: string;
  state: ReaderState;
  disabled?: boolean;
  brief?: boolean;
}) {
  const { reader, models, choose, reload } = state;
  useEffect(() => {
    if (models.status === "idle") void reload();
  }, [models.status, reload]);

  const names = models.status === "ready" ? models.models : [];
  // Until the list comes, the model chosen last stays shown as chosen.
  const offered =
    reader.engine === "ollama" && !names.includes(reader.model) ? [reader.model, ...names] : names;
  const hint =
    reader.engine === "ocrs"
      ? "The reader built into Synapse is quick, but reads letters without accents (è as e) and reads handwriting poorly."
      : "A vision model in Ollama, on this computer, reads accents and handwriting, but takes much longer, and can misread or make up words.";
  const looking = models.status === "idle" || models.status === "loading";
  return (
    <div className="read-with">
      <div className="field">
        <label htmlFor={id} className="field-label">
          Read with
        </label>
        {!brief && (
          <p id={`${id}-hint`} className="field-hint">
            {hint}
          </p>
        )}
        <select
          id={id}
          className="field-input"
          value={reader.engine === "ollama" ? reader.model : BUILT_IN_VALUE}
          disabled={disabled}
          aria-describedby={brief ? undefined : `${id}-hint`}
          onChange={(event) => {
            const value = event.target.value;
            choose(value === BUILT_IN_VALUE ? BUILT_IN : { engine: "ollama", model: value });
          }}
        >
          <option value={BUILT_IN_VALUE}>Built in</option>
          {offered.map((name) => (
            <option key={name} value={name}>
              {name} (Ollama)
            </option>
          ))}
        </select>
      </div>
      {looking && (
        <p className="field-hint" role="status">
          Looking for vision models in Ollama…
        </p>
      )}
      {!looking && names.length === 0 && (
        <p className="field-hint">
          {models.status === "unavailable"
            ? `${models.message} Until then, only the built-in reader is offered.`
            : "Ollama has no models that can read pictures. To read accents and handwriting, download one in Ollama, such as qwen2.5vl or gemma3, then check again."}{" "}
          <button type="button" className="button button-small" onClick={() => void reload()}>
            Check again
          </button>
        </p>
      )}
    </div>
  );
}

/** What reading a picture gave, by either reader. */
export type PictureRead = ModelPictureReading;

/**
 * Reads the text in a kept picture with `reader`. The built-in engine gives
 * no agreement. Rejects as the underlying command does: kind "stale" when a
 * model has left Ollama (reload the models then).
 */
export async function readPicture(reader: Reader, assetId: number): Promise<PictureRead> {
  if (reader.engine === "ollama") return recognisePictureWithModel(assetId, reader.model);
  const reading = await recognisePicture(assetId);
  return reading.status === "read" ? { ...reading, agreement: null } : reading;
}

/** The pages of a PDF source `readSourcePages` reads with `reader` when given none. */
export function pagesToRead(reader: Reader, sourceId: number): Promise<number[]> {
  return reader.engine === "ollama"
    ? getPagesToRecogniseWithModel(sourceId, reader.model)
    : getPagesToRecognise(sourceId);
}

/**
 * Reads pages of a PDF source with `reader`, saving each as it is read:
 * `pages`, or by default every page `pagesToRead` lists.
 */
export async function readSourcePages(
  reader: Reader,
  sourceId: number,
  pages: number[] | null,
  onProgress: (progress: PageProgress) => void,
): Promise<ModelSourceRecognition> {
  if (reader.engine === "ollama") {
    return recogniseSourcePagesWithModel(sourceId, pages, reader.model, onProgress);
  }
  const result = await recogniseSourcePages(sourceId, pages, onProgress);
  return { ...result, agreement: [] };
}

/**
 * How much of what the built-in reader found the model's text has too: a
 * hint that the model read the same thing, shown with its text. Nothing when
 * there's nothing to compare.
 */
export function Agreement({ percent }: { percent: number | null }) {
  if (percent === null) return null;
  const note =
    percent >= 80
      ? "they mostly agree."
      : percent >= 50
        ? "check the text closely."
        : "they disagree a lot: the model may have misread or made up text.";
  return (
    <p className="field-hint read-agreement">
      The model's text has {percent}% of the words the built-in reader found (ignoring accents):{" "}
      {note}
    </p>
  );
}
