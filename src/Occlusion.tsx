import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
} from "react";
import {
  choosePicture,
  createOcclusion,
  getOcclusion,
  keptFileUrl,
  storePicture,
  toCommandError,
  updateOcclusion,
  type Asset,
  type ClozeChanges,
  type DeckCard,
  type DeckDetail as Deck,
  type OcclusionBox,
  type OcclusionMode,
} from "./api";
import { FormActions, TextField, useFormSave } from "./forms";
import { Message } from "./ui";
import { useUnsavedChanges } from "./unsaved";

/** The smallest box drawn with the mouse, as a fraction of the picture's side. */
const MIN_DRAWN = 0.01;
/** How far an arrow key moves or resizes a box, as a fraction of the picture. */
const STEP = 0.01;
/** The size of a box added with the button. */
const NEW_BOX = { width: 0.2, height: 0.1 };

/** A box with a key that stays the same while it is edited. */
type EditorBox = OcclusionBox & { key: number };

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** A fraction as a CSS percentage. */
function percent(value: number): string {
  return `${(value * 100).toFixed(3)}%`;
}

/**
 * A picture with boxes over it, as review shows an image occlusion card. The
 * boxes numbered `asked` are the card's own: covered and marked with "?" on
 * the question, outlined with their labels on the answer. With "hideAll"
 * every other box stays covered; with "hideOne" the others aren't drawn.
 */
export function OcclusionPicture({
  picture,
  boxes,
  asked,
  mode,
  revealed,
}: {
  picture: Asset;
  boxes: OcclusionBox[];
  asked: number;
  mode: OcclusionMode;
  revealed: boolean;
}) {
  const own = boxes.filter((shape) => shape.number === asked).length;
  const description = revealed
    ? `Picture with the ${own === 1 ? "box" : "boxes"} asked about uncovered.`
    : `Picture with ${own === 1 ? "one box" : `${own} boxes`} to recall, marked with a question mark.`;
  return (
    <div className="occlusion-stage occlusion-review" role="img" aria-label={description}>
      <img className="occlusion-image" src={keptFileUrl(picture.sha256)} alt="" draggable={false} />
      {boxes.map((shape, index) => {
        const isOwn = shape.number === asked;
        if (!isOwn && mode === "hideOne") return null;
        const className = isOwn
          ? revealed
            ? "occlusion-box occlusion-box-shown"
            : "occlusion-box occlusion-box-asked"
          : "occlusion-box";
        return (
          <div
            key={index}
            className={className}
            style={{
              left: percent(shape.left),
              top: percent(shape.top),
              width: percent(shape.width),
              height: percent(shape.height),
            }}
          >
            {isOwn && !revealed && <span aria-hidden="true">?</span>}
            {isOwn && revealed && shape.label !== "" && (
              <span className="occlusion-box-label">{shape.label}</span>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** "1 card" or "3 cards". */
function cards(count: number): string {
  return `${count} ${count === 1 ? "card" : "cards"}`;
}

/** What saving the boxes did, in words. */
export function describeOcclusionChanges(changes: ClozeChanges, editing: boolean): string {
  if (!editing) return `Picture added: it made ${cards(changes.added)}.`;
  const parts = [
    changes.kept > 0 && `kept ${cards(changes.kept)}`,
    changes.restored > 0 && `brought back ${cards(changes.restored)} from Deleted cards`,
    changes.added > 0 && `added ${changes.added} new`,
    changes.deleted > 0 && `deleted ${cards(changes.deleted)} whose boxes were removed`,
  ].filter((part): part is string => typeof part === "string");
  const listed =
    parts.length <= 1
      ? (parts[0] ?? "changed nothing")
      : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
  return `Picture saved: it ${listed}.`;
}

type Loaded =
  | { status: "loading" }
  | { status: "error"; message: string }
  | {
      status: "ready";
      picture: Asset | null;
      header: string;
      mode: OcclusionMode;
      boxes: OcclusionBox[];
      nextNumber: number;
    };

/**
 * Adding an image occlusion to `deck`, or editing the one `card` was made
 * from. `kindChoice` (adding only) is the choice of kind of card, shown at
 * the top as on the other card forms.
 */
export function OcclusionForm({
  deck,
  card,
  kindChoice,
  onCancel,
  onSaved,
}: {
  deck: Deck;
  card?: DeckCard;
  kindChoice?: ReactNode;
  onCancel: () => void;
  onSaved: (deck: Deck, notice: string) => void;
}) {
  const cardId = card?.id ?? null;
  const [loaded, setLoaded] = useState<Loaded>(
    cardId === null
      ? { status: "ready", picture: null, header: "", mode: "hideAll", boxes: [], nextNumber: 1 }
      : { status: "loading" },
  );
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (cardId === null) return;
    let current = true;
    getOcclusion(cardId).then(
      (view) => {
        if (current) setLoaded({ status: "ready", ...view });
      },
      (err) => {
        if (current) setLoaded({ status: "error", message: toCommandError(err).message });
      },
    );
    return () => {
      current = false;
    };
  }, [cardId, attempt]);

  return (
    <div className="card occlusion-form-card">
      <h2 id="card-form-heading" className="form-title">
        {card ? "Edit a picture's cards in " : "Add a card to "}
        {deck.name}
      </h2>
      {kindChoice}
      {loaded.status === "loading" && (
        <p className="loading" role="status">
          Loading the picture…
        </p>
      )}
      {loaded.status === "error" && (
        <Message focus tone="alert" text={loaded.message}>
          <div className="actions">
            <button
              type="button"
              className="button"
              onClick={() => {
                setLoaded({ status: "loading" });
                setAttempt((n) => n + 1);
              }}
            >
              Retry
            </button>
            <button type="button" className="button" onClick={onCancel}>
              Cancel
            </button>
          </div>
        </Message>
      )}
      {loaded.status === "ready" && (
        <OcclusionEditor
          deck={deck}
          cardId={cardId}
          initial={loaded}
          onCancel={onCancel}
          onSaved={onSaved}
        />
      )}
    </div>
  );
}

/** A box being drawn, moved, or resized with the mouse. */
type Drag =
  | { kind: "draw"; startX: number; startY: number; key: number | null }
  | { kind: "move"; key: number; offsetX: number; offsetY: number }
  | { kind: "resize"; key: number };

function OcclusionEditor({
  deck,
  cardId,
  initial,
  onCancel,
  onSaved,
}: {
  deck: Deck;
  cardId: number | null;
  initial: Extract<Loaded, { status: "ready" }>;
  onCancel: () => void;
  onSaved: (deck: Deck, notice: string) => void;
}) {
  const editing = cardId !== null;
  const [picture, setPicture] = useState<Asset | null>(initial.picture);
  const [header, setHeader] = useState(initial.header);
  const [mode, setMode] = useState<OcclusionMode>(initial.mode);
  const keyRef = useRef(0);
  const [boxes, setBoxes] = useState<EditorBox[]>(() =>
    initial.boxes.map((shape) => ({ ...shape, key: keyRef.current++ })),
  );
  // A ref, so boxes added between two renders still get numbers of their own.
  const nextNumberRef = useRef(initial.nextNumber);
  const [selected, setSelected] = useState<number | null>(null);
  const [pictureError, setPictureError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const stageRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<Drag | null>(null);

  const startState = useMemo(
    () =>
      JSON.stringify({
        picture: initial.picture?.id ?? null,
        header: initial.header,
        mode: initial.mode,
        boxes: initial.boxes,
      }),
    [initial],
  );
  const plainBoxes = boxes.map(({ key: _key, ...shape }) => shape);
  useUnsavedChanges(
    JSON.stringify({ picture: picture?.id ?? null, header, mode, boxes: plainBoxes }) !==
      startState,
  );

  const numbers = [...new Set(boxes.map((shape) => shape.number))].sort((a, b) => a - b);

  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async (): Promise<{ deck: Deck; notice: string }> => {
      if (cardId !== null) {
        const saved = await updateOcclusion(cardId, header, mode, plainBoxes);
        return { deck: saved.deck, notice: describeOcclusionChanges(saved.changes, true) };
      }
      if (!picture) {
        throw { kind: "invalid", message: "Choose a picture first.", field: "boxes" };
      }
      const saved = await createOcclusion(deck.id, picture.id, header, mode, plainBoxes);
      const added = saved.cardCount - deck.cardCount;
      return {
        deck: saved,
        notice: describeOcclusionChanges(
          { cards: added, kept: 0, restored: 0, added, deleted: 0 },
          false,
        ),
      };
    },
    (saved) => onSaved(saved.deck, saved.notice),
    { header: "occlusion-header", boxes: "occlusion-boxes" },
  );

  function changeBoxes(update: (previous: EditorBox[]) => EditorBox[]) {
    setBoxes(update);
    edited("boxes");
  }

  function updateBox(key: number, change: Partial<OcclusionBox>) {
    changeBoxes((previous) =>
      previous.map((shape) => (shape.key === key ? { ...shape, ...change } : shape)),
    );
  }

  function removeBox(key: number) {
    changeBoxes((previous) => previous.filter((shape) => shape.key !== key));
    setSelected(null);
  }

  /** A number no box or card has had, for a card of its own. */
  function newNumber(): number {
    return nextNumberRef.current++;
  }

  /** A new box with a number of its own, so it makes a card of its own. */
  function addBox(position: { left: number; top: number; width: number; height: number }) {
    const key = keyRef.current++;
    const number = newNumber();
    changeBoxes((previous) => [...previous, { ...position, number, label: "", key }]);
    setSelected(key);
    return key;
  }

  async function keepPicture(choose: () => Promise<Asset | null>) {
    if (adding) return;
    setAdding(true);
    setPictureError(null);
    try {
      const asset = await choose();
      if (asset) {
        setPicture(asset);
        edited("boxes");
      }
    } catch (err) {
      setPictureError(toCommandError(err).message);
    }
    setAdding(false);
  }

  function onPaste(event: ClipboardEvent<HTMLDivElement>) {
    if (editing) return;
    const file = [...event.clipboardData.files].find((item) => item.type.startsWith("image/"));
    if (!file) return;
    event.preventDefault();
    void keepPicture(async () => storePicture(new Uint8Array(await file.arrayBuffer())));
  }

  /** Where a pointer is, as fractions of the picture. */
  function pointAt(event: PointerEvent): { x: number; y: number } | null {
    const rect = stageRef.current?.getBoundingClientRect();
    if (!rect || rect.width === 0 || rect.height === 0) return null;
    return {
      x: clamp((event.clientX - rect.left) / rect.width, 0, 1),
      y: clamp((event.clientY - rect.top) / rect.height, 0, 1),
    };
  }

  function onStagePointerDown(event: PointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;
    const point = pointAt(event);
    if (!point) return;
    event.preventDefault();
    stageRef.current?.setPointerCapture?.(event.pointerId);
    dragRef.current = { kind: "draw", startX: point.x, startY: point.y, key: null };
  }

  function onBoxPointerDown(event: PointerEvent, shape: EditorBox, resize: boolean) {
    if (event.button !== 0) return;
    event.stopPropagation();
    const point = pointAt(event);
    if (!point) return;
    event.preventDefault();
    stageRef.current?.setPointerCapture?.(event.pointerId);
    setSelected(shape.key);
    dragRef.current = resize
      ? { kind: "resize", key: shape.key }
      : { kind: "move", key: shape.key, offsetX: point.x - shape.left, offsetY: point.y - shape.top };
  }

  function onPointerMove(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag) return;
    const point = pointAt(event);
    if (!point) return;
    if (drag.kind === "draw") {
      const left = Math.min(drag.startX, point.x);
      const top = Math.min(drag.startY, point.y);
      const width = Math.abs(point.x - drag.startX);
      const height = Math.abs(point.y - drag.startY);
      if (drag.key === null) {
        // The box appears once the mouse has moved far enough to mean it.
        if (width < MIN_DRAWN && height < MIN_DRAWN) return;
        dragRef.current = { ...drag, key: addBox({ left, top, width, height }) };
      } else {
        updateBox(drag.key, { left, top, width, height });
      }
    } else if (drag.kind === "move") {
      const shape = boxes.find((item) => item.key === drag.key);
      if (!shape) return;
      updateBox(drag.key, {
        left: clamp(point.x - drag.offsetX, 0, 1 - shape.width),
        top: clamp(point.y - drag.offsetY, 0, 1 - shape.height),
      });
    } else {
      const shape = boxes.find((item) => item.key === drag.key);
      if (!shape) return;
      updateBox(drag.key, {
        width: clamp(point.x - shape.left, MIN_DRAWN, 1 - shape.left),
        height: clamp(point.y - shape.top, MIN_DRAWN, 1 - shape.top),
      });
    }
  }

  function onPointerUp() {
    const drag = dragRef.current;
    dragRef.current = null;
    if (drag?.kind !== "draw" || drag.key === null) return;
    // A box too thin to hide anything is a slip of the mouse.
    const key = drag.key;
    changeBoxes((previous) =>
      previous.filter(
        (shape) => shape.key !== key || (shape.width >= MIN_DRAWN && shape.height >= MIN_DRAWN),
      ),
    );
  }

  /** Arrow keys move the focused box; with Shift they resize it. Delete removes it. */
  function onBoxKeyDown(event: KeyboardEvent, shape: EditorBox) {
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-STEP, 0],
      ArrowRight: [STEP, 0],
      ArrowUp: [0, -STEP],
      ArrowDown: [0, STEP],
    };
    if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault();
      removeBox(shape.key);
      stageRef.current?.focus();
      return;
    }
    const move = moves[event.key];
    if (!move) return;
    event.preventDefault();
    const [dx, dy] = move;
    if (event.shiftKey) {
      updateBox(shape.key, {
        width: clamp(shape.width + dx, MIN_DRAWN, 1 - shape.left),
        height: clamp(shape.height + dy, MIN_DRAWN, 1 - shape.top),
      });
    } else {
      updateBox(shape.key, {
        left: clamp(shape.left + dx, 0, 1 - shape.width),
        top: clamp(shape.top + dy, 0, 1 - shape.height),
      });
    }
  }

  const boxesError = fieldError("boxes");

  return (
    <form
      className="occlusion-form"
      aria-labelledby="card-form-heading"
      noValidate
      onSubmit={submit}
    >
      {!editing && (
        <div className="field">
          <p className="field-label" id="occlusion-picture-label">
            Picture
          </p>
          <p className="field-hint" id="occlusion-picture-hint">
            A PNG, JPEG, GIF or WebP picture up to 20 MB. Choose a file, or paste one here.
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-small"
              aria-describedby="occlusion-picture-hint"
              aria-disabled={adding}
              onClick={() =>
                void keepPicture(async () => {
                  const chosen = await choosePicture();
                  return chosen.status === "added" ? chosen.asset : null;
                })
              }
            >
              {picture ? "Choose another picture" : "Choose a picture"}
            </button>
          </div>
          {pictureError && (
            <p className="field-error" role="alert">
              {pictureError}
            </p>
          )}
        </div>
      )}

      {picture ? (
        <div className="field">
          <p className="field-label" id="occlusion-boxes-label">
            Boxes
          </p>
          <p className="field-hint" id="occlusion-boxes-hint">
            Drag over the picture to hide a part. Each box is its own card; give boxes the same card
            to hide them together. Drag a box to move it, or its corner to resize it. With a box
            focused, arrow keys move it, Shift with arrow keys resizes it, and Delete removes it.
            {editing &&
              " Boxes you keep keep their schedules; removing every box of a card deletes it."}
          </p>
          <div
            id="occlusion-boxes"
            ref={stageRef}
            className="occlusion-stage occlusion-editor"
            tabIndex={-1}
            aria-labelledby="occlusion-boxes-label"
            aria-describedby={boxesError ? "occlusion-boxes-error" : "occlusion-boxes-hint"}
            onPointerDown={onStagePointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onPointerCancel={onPointerUp}
            onPaste={onPaste}
          >
            <img
              className="occlusion-image"
              src={keptFileUrl(picture.sha256)}
              alt=""
              draggable={false}
            />
            {boxes.map((shape) => (
              <div
                key={shape.key}
                role="button"
                tabIndex={0}
                aria-label={`Box for card ${shape.number}${shape.label ? `, ${shape.label}` : ""}`}
                aria-pressed={selected === shape.key}
                className={
                  selected === shape.key
                    ? "occlusion-box occlusion-box-edit occlusion-box-selected"
                    : "occlusion-box occlusion-box-edit"
                }
                style={{
                  left: percent(shape.left),
                  top: percent(shape.top),
                  width: percent(shape.width),
                  height: percent(shape.height),
                }}
                onPointerDown={(event) => onBoxPointerDown(event, shape, false)}
                onFocus={() => setSelected(shape.key)}
                onKeyDown={(event) => onBoxKeyDown(event, shape)}
              >
                <span aria-hidden="true">{shape.number}</span>
                <span
                  className="occlusion-handle"
                  aria-hidden="true"
                  onPointerDown={(event) => onBoxPointerDown(event, shape, true)}
                />
              </div>
            ))}
          </div>
          {boxesError && (
            <p id="occlusion-boxes-error" className="field-error">
              {boxesError}
            </p>
          )}
          <div className="deck-actions">
            <button
              type="button"
              className="button button-small"
              onClick={() =>
                addBox({ left: 0.4, top: 0.45, width: NEW_BOX.width, height: NEW_BOX.height })
              }
            >
              Add a box
            </button>
          </div>
          <p className="cloze-preview" role="status">
            {boxes.length === 0
              ? "No boxes yet."
              : `${boxes.length} ${boxes.length === 1 ? "box makes" : "boxes make"} ${cards(numbers.length)}.`}
          </p>
          {boxes.length > 0 && (
            <ol className="occlusion-list" aria-label="Boxes">
              {boxes.map((shape, index) => (
                <li
                  key={shape.key}
                  className={
                    selected === shape.key
                      ? "occlusion-list-item occlusion-list-item-selected"
                      : "occlusion-list-item"
                  }
                >
                  <label className="occlusion-list-card">
                    <span className="field-label">{`Box ${index + 1}`}</span>
                    <select
                      className="field-input"
                      value={shape.number}
                      aria-label={`Card for box ${index + 1}`}
                      onChange={(event) => {
                        const value = Number(event.target.value);
                        if (value === 0) {
                          updateBox(shape.key, { number: newNumber() });
                        } else {
                          updateBox(shape.key, { number: value });
                        }
                      }}
                    >
                      {numbers.map((number) => (
                        <option key={number} value={number}>{`Card ${number}`}</option>
                      ))}
                      <option value={0}>A new card</option>
                    </select>
                  </label>
                  <label className="occlusion-list-label">
                    <span className="field-label">Label</span>
                    <input
                      className="field-input"
                      type="text"
                      value={shape.label}
                      placeholder="What's under it (optional)"
                      aria-label={`Label for box ${index + 1}`}
                      onFocus={() => setSelected(shape.key)}
                      onChange={(event) => updateBox(shape.key, { label: event.target.value })}
                    />
                  </label>
                  <button
                    type="button"
                    className="button button-danger-soft button-small"
                    aria-label={`Remove box ${index + 1}`}
                    onClick={() => removeBox(shape.key)}
                  >
                    Remove
                  </button>
                </li>
              ))}
            </ol>
          )}
        </div>
      ) : (
        <div
          className="occlusion-paste"
          tabIndex={0}
          aria-label="Paste a picture here"
          onPaste={onPaste}
        >
          {adding ? "Keeping the picture…" : "No picture yet. Choose one, or paste one here."}
          {boxesError && (
            <p id="occlusion-boxes-error" className="field-error">
              {boxesError}
            </p>
          )}
        </div>
      )}

      <TextField
        id="occlusion-header"
        label="Header"
        hint="Shown above the picture on every card, such as what the picture is. Optional. Up to 500 characters."
        value={header}
        error={fieldError("header")}
        onChange={(value) => {
          setHeader(value);
          edited("header");
        }}
      />

      <fieldset className="field kind-choice">
        <legend className="field-label">When asking</legend>
        <label className="kind-choice-option">
          <input
            type="radio"
            name="occlusion-mode"
            value="hideAll"
            checked={mode === "hideAll"}
            onChange={() => setMode("hideAll")}
          />
          <span>
            <strong>Hide all</strong>: every box is covered, and the one asked about is marked.
          </span>
        </label>
        <label className="kind-choice-option">
          <input
            type="radio"
            name="occlusion-mode"
            value="hideOne"
            checked={mode === "hideOne"}
            onChange={() => setMode("hideOne")}
          />
          <span>
            <strong>Hide one</strong>: only the box asked about is covered.
          </span>
        </label>
      </fieldset>

      <FormActions
        saveLabel={editing ? "Save changes" : "Save cards"}
        saving={saving}
        error={formError}
        onCancel={onCancel}
      />
    </form>
  );
}
