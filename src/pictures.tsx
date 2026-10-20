import { useRef, useState, type ClipboardEvent, type DragEvent } from "react";
import { choosePicture, keptFileUrl, storePicture, toCommandError, type Asset } from "./api";
import { PictureIcon, TrashIcon } from "./icons";

/**
 * Pictures in notes and on cards: showing a kept picture, a list of them, and
 * a form field that adds, orders and removes them. A picture always comes
 * from an `Asset` that Rust returned, through `keptFileUrl`; nothing typed in
 * a note or card is ever turned into an address.
 */

/**
 * One kept picture. If it can't be loaded (its file went missing), a short
 * note stands in its place instead of a broken image.
 */
export function KeptPicture({
  asset,
  alt,
  className,
}: {
  asset: Asset;
  alt: string;
  className?: string;
}) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <span className="picture-missing" role="img" aria-label={alt}>
        This picture couldn't be shown.
      </span>
    );
  }
  return (
    <img
      className={className ?? "kept-picture"}
      src={keptFileUrl(asset.sha256)}
      alt={alt}
      onError={() => setFailed(true)}
    />
  );
}

/** Whether two lists hold the same pictures in the same order. */
export function samePictures(a: Asset[], b: Asset[]): boolean {
  return a.length === b.length && a.every((picture, index) => picture.id === b[index].id);
}

/** "Picture 2 of 3" (or "Picture" alone), naming what it belongs to. */
function pictureName(label: string, index: number, count: number): string {
  return count === 1 ? label : `${label} ${index + 1} of ${count}`;
}

/**
 * A read-only list of pictures, in order, such as a note's under its text or
 * a card side's in review. Shows nothing when there are none.
 */
export function PictureGallery({
  pictures,
  label,
  listLabel,
  compact = false,
}: {
  pictures: Asset[] | undefined;
  /** What each picture is called to a screen reader: "Picture on the front". */
  label: string;
  /** What the list is called: "Pictures on the front". */
  listLabel: string;
  /** Small thumbnails, for a list of cards. */
  compact?: boolean;
}) {
  if (!pictures || pictures.length === 0) return null;
  return (
    <ul
      className={compact ? "picture-gallery picture-gallery-compact" : "picture-gallery"}
      aria-label={listLabel}
    >
      {pictures.map((picture, index) => (
        <li key={picture.id}>
          <KeptPicture asset={picture} alt={pictureName(label, index, pictures.length)} />
        </li>
      ))}
    </ul>
  );
}

/** The picture files on a clipboard or in a drop, if any. */
function pictureFiles(data: DataTransfer | null): File[] {
  if (!data) return [];
  return Array.from(data.files).filter((file) => file.type.startsWith("image/"));
}

async function bytesOf(file: File): Promise<Uint8Array> {
  return new Uint8Array(await file.arrayBuffer());
}

/**
 * Adding pictures to a list: from a file chosen in the OS dialog, or from
 * pasted or dropped files. Each is kept by Rust first (which checks its bytes
 * and size) and then added to the list; the list itself is only saved with
 * the note or card. `onPaste` can also be put on the text fields beside the
 * list, so a picture pasted while typing lands in it.
 */
export function usePictureAdder({
  pictures,
  onChange,
  max,
}: {
  pictures: Asset[];
  onChange: (pictures: Asset[]) => void;
  max: number;
}) {
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second add immediately, before the re-render lands.
  const addingRef = useRef(false);
  // The list as it is now, so pictures added one after another all stay.
  const latest = useRef(pictures);
  latest.current = pictures;

  function append(added: Asset[]) {
    const current = latest.current;
    const fresh = added.filter(
      (asset, index) =>
        !current.some((picture) => picture.id === asset.id) &&
        added.findIndex((other) => other.id === asset.id) === index
    );
    const room = max - current.length;
    if (fresh.length > room) {
      setError(`At most ${max} pictures fit here.`);
    }
    if (fresh.length > 0 && room > 0) onChange([...current, ...fresh.slice(0, room)]);
  }

  async function run(work: () => Promise<Asset[]>) {
    if (addingRef.current) return;
    if (latest.current.length >= max) {
      setError(`At most ${max} pictures fit here.`);
      return;
    }
    addingRef.current = true;
    setAdding(true);
    setError(null);
    try {
      append(await work());
    } catch (err) {
      setError(toCommandError(err).message);
    }
    addingRef.current = false;
    setAdding(false);
  }

  function choose() {
    void run(async () => {
      const choice = await choosePicture();
      return choice.status === "added" ? [choice.asset] : [];
    });
  }

  function addFiles(files: File[]) {
    void run(async () => {
      const added: Asset[] = [];
      for (const file of files) added.push(await storePicture(await bytesOf(file)));
      return added;
    });
  }

  /** Takes pasted pictures; pasted text is left to the field. */
  function onPaste(event: ClipboardEvent) {
    const files = pictureFiles(event.clipboardData);
    if (files.length === 0) return;
    event.preventDefault();
    addFiles(files);
  }

  function onDrop(event: DragEvent) {
    const files = pictureFiles(event.dataTransfer);
    if (files.length === 0) return;
    event.preventDefault();
    addFiles(files);
  }

  return { adding, error, choose, onPaste, onDrop, clearError: () => setError(null) };
}

export type PictureAdder = ReturnType<typeof usePictureAdder>;

/**
 * A form's pictures: the ones chosen so far, each with *Move earlier* and
 * *Remove*, and *Add picture*. Pictures can also be pasted or dropped onto
 * it. `adder` comes from `usePictureAdder`, so the form can share its paste
 * handling with its text fields.
 */
export function PictureField({
  id,
  label,
  hint,
  pictures,
  onChange,
  adder,
  error,
  disabled = false,
}: {
  id: string;
  label: string;
  hint?: string;
  pictures: Asset[];
  onChange: (pictures: Asset[]) => void;
  adder: PictureAdder;
  /** A save refused because of the pictures. */
  error?: string;
  disabled?: boolean;
}) {
  const hintId = hint === undefined ? undefined : `${id}-hint`;
  const errorId = `${id}-error`;
  const shownError = error ?? adder.error ?? undefined;
  const described = [shownError ? errorId : undefined, hintId].filter(Boolean).join(" ");

  function move(index: number) {
    const next = [...pictures];
    [next[index - 1], next[index]] = [next[index], next[index - 1]];
    onChange(next);
  }

  function remove(index: number) {
    adder.clearError();
    onChange(pictures.filter((_, other) => other !== index));
    // Focus stays in the field, on the button that adds pictures.
    document.getElementById(id)?.focus();
  }

  return (
    <div
      className="field picture-field"
      role="group"
      aria-labelledby={`${id}-label`}
      onPaste={adder.onPaste}
      onDragOver={(event) => event.preventDefault()}
      onDrop={adder.onDrop}
    >
      <span id={`${id}-label`} className="field-label">
        {label}
      </span>
      {hint !== undefined && (
        <p id={hintId} className="field-hint">
          {hint}
        </p>
      )}
      {pictures.length > 0 && (
        <ol className="picture-choices">
          {pictures.map((picture, index) => {
            const name = pictureName("Picture", index, pictures.length);
            return (
              <li key={picture.id} className="picture-choice">
                <KeptPicture asset={picture} alt={name} className="picture-thumb" />
                <div className="picture-choice-actions">
                  {index > 0 && (
                    <button
                      type="button"
                      className="button button-small"
                      aria-label={`Move ${name.toLowerCase()} earlier`}
                      disabled={disabled}
                      onClick={() => move(index)}
                    >
                      Move earlier
                    </button>
                  )}
                  <button
                    type="button"
                    className="button button-small button-quiet"
                    aria-label={`Remove ${name.toLowerCase()}`}
                    disabled={disabled}
                    onClick={() => remove(index)}
                  >
                    <TrashIcon />
                    Remove
                  </button>
                </div>
              </li>
            );
          })}
        </ol>
      )}
      <div className="picture-add">
        <button
          id={id}
          type="button"
          className="button button-small"
          aria-describedby={described || undefined}
          aria-invalid={shownError ? true : undefined}
          disabled={disabled || adder.adding}
          onClick={adder.choose}
        >
          <PictureIcon />
          {adder.adding ? "Adding picture…" : "Add picture"}
        </button>
        <span className="field-hint">or paste or drop one here</span>
      </div>
      {shownError && (
        <p id={errorId} className="field-error" role="alert">
          {shownError}
        </p>
      )}
    </div>
  );
}
