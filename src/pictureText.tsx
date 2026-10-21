import { useEffect, useRef, useState } from "react";
import {
  stopRecognition,
  toCommandError,
  type Asset,
  type Recognition,
} from "./api";
import { Agreement, ReadWith, readPicture, useReader, type Reader } from "./ReadWith";

/**
 * Reading the text in a kept picture, and showing what was read. Text
 * recognition makes mistakes (the built-in engine reads no accents and little
 * handwriting), so what it read is always shown as recognised text to check,
 * never as if it were typed. Nothing here saves anything: text the user adds
 * goes into a form, which saves it with everything else.
 */

/** Where reading a picture has got to. */
export type PictureReadState =
  | { status: "idle" }
  | { status: "reading"; assetId: number }
  | {
      status: "read";
      assetId: number;
      recognition: Recognition;
      /** How far a model's text agrees with the built-in reader's, if both read it. */
      agreement: number | null;
    }
  | { status: "stopped"; assetId: number }
  | { status: "error"; assetId: number; message: string };

/** The picture a reading is about, if any. */
export function readingAssetId(state: PictureReadState): number | null {
  return state.status === "idle" ? null : state.assetId;
}

/**
 * The newest reading started on this screen. Rust runs one reading at a time
 * and starting one stops the last, so a screen that goes away stops its
 * reading only if no other has started since.
 */
let newestRun = 0;

/**
 * Reads pictures one at a time, with the reader chosen in `readWith` (the
 * built-in engine, or a vision model in Ollama). `stop` ends the reading in
 * progress; leaving the screen does too.
 */
export function usePictureReading() {
  const [state, setState] = useState<PictureReadState>({ status: "idle" });
  const readWith = useReader();
  const runRef = useRef<number | null>(null);

  useEffect(
    () => () => {
      const run = runRef.current;
      runRef.current = null;
      if (run !== null && run === newestRun)
        void stopRecognition().catch(() => {});
    },
    [],
  );

  /**
   * Reads `assetId`, with `reader` or else the one chosen. Resolves with how
   * it went, or with null when nobody is waiting for it any more (the screen
   * went, or another picture was asked for).
   */
  async function read(
    assetId: number,
    reader: Reader = readWith.reader,
  ): Promise<PictureReadState | null> {
    const run = ++newestRun;
    runRef.current = run;
    setState({ status: "reading", assetId });
    let next: PictureReadState;
    try {
      const reading = await readPicture(reader, assetId);
      next =
        reading.status === "read"
          ? {
              status: "read",
              assetId,
              recognition: reading.recognition,
              agreement: reading.agreement,
            }
          : { status: "stopped", assetId };
    } catch (err) {
      const failure = toCommandError(err);
      next = { status: "error", assetId, message: failure.message };
      // The model has left Ollama: the list offered is out of date.
      if (failure.kind === "stale" && reader.engine === "ollama") void readWith.reload();
    }
    // Gone, or another picture was asked for meanwhile: that one is shown.
    if (runRef.current !== run) return null;
    runRef.current = null;
    setState(next);
    return next;
  }

  function stop() {
    void stopRecognition().catch(() => {});
  }

  return { state, read, stop, readWith, clear: () => setState({ status: "idle" }) };
}

export type PictureReading = ReturnType<typeof usePictureReading>;

/** Who read the text, said plainly. */
export function readBy(recognition: Recognition): string {
  return recognition.engine === "ocrs"
    ? "Read by Synapse's built-in reader, which doesn't read accents or handwriting well."
    : `Read by ${recognition.model} in Ollama.`;
}

/** Text added to the end of `existing`, a blank line after what's there. */
export function appendText(existing: string, added: string): string {
  const kept = existing.replace(/\s+$/, "");
  return kept === "" ? added : `${kept}\n\n${added}`;
}

/**
 * The text read from one picture, under it: reading (with *Stop*), what was
 * read (with *Copy* and, in a form, `add`), or why nothing was.
 */
export function PictureTextPanel({
  reading,
  name,
  add,
}: {
  reading: PictureReading;
  /** What the picture is called: "Picture 2 of 3". */
  name: string;
  /** Puts the text into the form beside the picture: "Add to note". */
  add?: { label: string; done: string; onAdd: (text: string) => void };
}) {
  const { state } = reading;
  const [said, setSaid] = useState("");
  const assetId = readingAssetId(state);
  // A new reading starts with nothing said about the last one.
  useEffect(() => setSaid(""), [assetId, state.status]);

  if (state.status === "idle") return null;

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setSaid("Copied.");
    } catch {
      setSaid("Synapse couldn't copy it. Select the text and copy it instead.");
    }
  }

  const close = (
    <button
      type="button"
      className="button button-small button-quiet"
      onClick={reading.clear}
    >
      Close
    </button>
  );

  return (
    <section
      className="picture-text"
      aria-label={`Text in ${name.toLowerCase()}`}
    >
      {state.status === "reading" && (
        <div className="picture-text-row">
          <p className="form-status" role="status">
            {`Reading the text in ${name.toLowerCase()}…`}
          </p>
          <button
            type="button"
            className="button button-small"
            onClick={reading.stop}
          >
            Stop
          </button>
        </div>
      )}
      {state.status === "stopped" && (
        <div className="picture-text-row">
          <p role="status">Stopped. Nothing was read.</p>
          {close}
        </div>
      )}
      {state.status === "error" && (
        <div className="picture-text-row">
          <p className="message-error" role="alert">
            {state.message}
          </p>
          {close}
        </div>
      )}
      {state.status === "read" &&
        (state.recognition.text.trim() === "" ? (
          <div className="picture-text-row">
            <p role="status">No text found.</p>
            {close}
          </div>
        ) : (
          <>
            <p className="field-label" role="status">
              Recognised text: check it for mistakes
            </p>
            <p className="field-hint">{readBy(state.recognition)}</p>
            <Agreement percent={state.agreement} />
            <p className="picture-text-body">{state.recognition.text}</p>
            <div className="picture-text-row">
              {add && (
                <button
                  type="button"
                  className="button button-small button-tonal"
                  onClick={() => {
                    add.onAdd(state.recognition.text);
                    setSaid(add.done);
                  }}
                >
                  {add.label}
                </button>
              )}
              <button
                type="button"
                className="button button-small"
                onClick={() => void copy(state.recognition.text)}
              >
                Copy
              </button>
              {close}
            </div>
          </>
        ))}
      {state.status !== "reading" && assetId !== null && (
        <div className="picture-text-again">
          <ReadWith id={`read-with-${assetId}`} state={reading.readWith} brief />
          <button
            type="button"
            className="button button-small"
            onClick={() => void reading.read(assetId)}
          >
            Read again
          </button>
        </div>
      )}
      <p className="form-status" role="status">
        {said}
      </p>
    </section>
  );
}

/** The *Text in picture* button for one picture. */
export function TextInPictureButton({
  picture,
  name,
  reading,
  disabled = false,
}: {
  picture: Asset;
  name: string;
  reading: PictureReading;
  disabled?: boolean;
}) {
  const busy =
    reading.state.status === "reading" && reading.state.assetId === picture.id;
  return (
    <button
      type="button"
      className="button button-small"
      aria-label={`Text in ${name.toLowerCase()}`}
      disabled={disabled || busy}
      onClick={() => void reading.read(picture.id)}
    >
      Text in picture
    </button>
  );
}
