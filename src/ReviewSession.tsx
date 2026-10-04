import { useEffect, useRef, useState } from "react";
import {
  getSessionCard,
  reviewCard,
  toCommandError,
  type Flashcard,
  type Rating,
  type SessionProgress,
} from "./api";
import { Message, useFocusOnMount } from "./ui";

type SessionState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "due"; card: Flashcard; progress: SessionProgress }
  | { status: "completed"; cardsReviewed: number };

/**
 * The four answer buttons, with the number key that presses each one and the
 * class that colours it.
 */
const RATINGS: { value: Rating; label: string; key: string; tone: string }[] = [
  { value: 1, label: "Again", key: "1", tone: "rating-again" },
  { value: 2, label: "Hard", key: "2", tone: "rating-hard" },
  { value: 3, label: "Good", key: "3", tone: "rating-good" },
  { value: 4, label: "Easy", key: "4", tone: "rating-easy" },
];

/** Fetches the session's next card and describes the outcome; never rejects. */
async function loadCard(sessionId: number): Promise<SessionState> {
  try {
    return await getSessionCard(sessionId);
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * Whether a key press is someone typing rather than answering a card. The
 * review screen has no such fields today; the guard keeps the shortcuts from
 * eating keystrokes if one is ever added to it.
 */
function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  return target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT";
}

function isButton(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.tagName === "BUTTON";
}

export function ReviewSession({
  sessionId,
  deckName,
  onExit,
}: {
  sessionId: number;
  deckName: string;
  onExit: () => void;
}) {
  const [state, setState] = useState<SessionState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or after a review).
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    loadCard(sessionId).then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [sessionId, attempt]);

  function reload() {
    setState({ status: "loading" });
    setAttempt((n) => n + 1);
  }

  // Everything in a session follows a button press, so each new state takes
  // focus (the question, the completion message, or the error).
  return (
    <section className="session" aria-labelledby="session-heading">
      <h2 id="session-heading" className="session-title">
        <span className="session-kicker">Reviewing</span> {deckName}
      </h2>

      {state.status === "loading" && (
        <p className="loading" role="status">
          Loading the next card…
        </p>
      )}

      {state.status === "error" && (
        <Message focus tone="alert" text={state.message}>
          <div className="actions">
            <button type="button" className="button" onClick={reload}>
              Retry
            </button>
            <button type="button" className="button" onClick={onExit}>
              Back to Study Desk
            </button>
          </div>
        </Message>
      )}

      {state.status === "due" && (
        <>
          <Progress progress={state.progress} />
          <Card
            key={`${state.card.id}:${state.card.reps}`}
            sessionId={sessionId}
            card={state.card}
            onReviewed={reload}
          />
        </>
      )}

      {state.status === "completed" && (
        <Message
          focus
          tone="status"
          done
          text={
            <>
              <strong className="message-title">Session complete.</strong>
              {` You reviewed ${state.cardsReviewed} ${state.cardsReviewed === 1 ? "card" : "cards"}. No more cards are due in this deck right now.`}
            </>
          }
        >
          <button type="button" className="button button-primary" onClick={onExit}>
            Back to Study Desk
          </button>
        </Message>
      )}
    </section>
  );
}

/**
 * How far the session has got. The card being shown is counted as remaining,
 * so the first card of three reads "Card 1 of 3" while the bar is still empty;
 * it fills as cards are rated and is full only once the session completes.
 *
 * The label has no live region: focus moves to the card face on every new
 * card, and that face is labelled by this text (`aria-labelledby` in `Card`),
 * so a screen reader reads the position with the question instead of
 * announcing it separately.
 */
function Progress({ progress }: { progress: SessionProgress }) {
  const total = progress.reviewed + progress.remaining;
  const position = progress.reviewed + 1;

  return (
    <div className="progress">
      <p id="session-progress" className="progress-label">
        Card {position} of {total}
      </p>
      <progress
        className="progress-bar"
        value={progress.reviewed}
        max={total}
        aria-label={`${progress.reviewed} of ${total} cards reviewed`}
      />
    </div>
  );
}

function Card({
  sessionId,
  card,
  onReviewed,
}: {
  sessionId: number;
  card: Flashcard;
  onReviewed: () => void;
}) {
  const [revealed, setRevealed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // Blocks a second submission immediately, before the re-render lands.
  const savingRef = useRef(false);
  const questionRef = useFocusOnMount<HTMLElement>(true);
  const answerRef = useRef<HTMLElement>(null);

  // The Reveal button disappears once pressed, so move focus to the answer
  // instead of letting it fall back to the page.
  useEffect(() => {
    if (revealed) answerRef.current?.focus();
  }, [revealed]);

  async function rate(rating: Rating) {
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);

    try {
      await reviewCard(sessionId, card, rating);
      onReviewed();
      return; // Stay "saving" until this card is replaced.
    } catch (err) {
      const error = toCommandError(err);
      if (error.kind === "stale") {
        // Already reviewed, or the session ended: show what comes next.
        onReviewed();
        return;
      }
      setSaveError(error.message);
    }

    savingRef.current = false;
    setSaving(false);
  }

  // Keyboard answering: Space or Enter reveals, then 1–4 rate. The listener
  // is on the document because focus sits on the card face, which isn't a
  // control, so the keys have to work wherever focus happens to be. The
  // parent keys this component by card and reps, so every card gets a fresh
  // listener and a stale one can never rate the card after it.
  //
  // `rate` is called rather than duplicated, so a key and a click take exactly
  // the same path, including the `savingRef` guard and the `expectedReps`
  // check that makes a double submission stale. It reads only refs and props,
  // so the copy captured here stays correct for this card.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      // A modified key press is a browser or OS shortcut, not an answer, and
      // a held-down key shouldn't answer twice.
      if (event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return;
      if (event.repeat || isTypingTarget(event.target)) return;

      if (!revealed) {
        if (event.key !== " " && event.key !== "Enter") return;
        // Space and Enter on the focused Reveal button already press it;
        // revealing again here would be pointless, and on any other control
        // it would fight the browser's own handling.
        if (isButton(event.target)) return;
        event.preventDefault();
        setRevealed(true);
        return;
      }

      // Digits don't activate a focused button on their own, so unlike Space
      // and Enter they stay live wherever focus is — otherwise tabbing to a
      // rating button would silently kill the number keys. `rate` ignores the
      // press while a rating is saving.
      const rating = RATINGS.find((option) => option.key === event.key);
      if (!rating) return;
      event.preventDefault();
      void rate(rating.value);
    }

    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [revealed]);

  return (
    <article className="review">
      {/* The card itself: the question, and the answer under it once shown. */}
      <div className="review-card">
        <section
          ref={questionRef}
          className="card-side"
          tabIndex={-1}
          aria-labelledby="session-progress card-front-label"
        >
          <h3 id="card-front-label" className="side-label">
            Question
          </h3>
          <p className="card-text">{card.front}</p>
        </section>

        {revealed && (
          <section
            ref={answerRef}
            className="card-side card-back"
            tabIndex={-1}
            aria-labelledby="card-back-label"
          >
            <h3 id="card-back-label" className="side-label">
              Answer
            </h3>
            <p className="card-text">{card.back}</p>
          </section>
        )}
      </div>

      {revealed ? (
        <div className="rating" role="group" aria-labelledby="rating-prompt" aria-busy={saving}>
          <p id="rating-prompt" className="rating-prompt">
            How well did you remember it?
          </p>
          <div className="rating-buttons">
            {RATINGS.map(({ value, label, key, tone }) => (
              // `aria-disabled` rather than `disabled`: a disabled button
              // drops keyboard focus, which would strand keyboard users.
              <button
                key={value}
                type="button"
                className={`button rating-button ${tone}`}
                aria-disabled={saving}
                aria-keyshortcuts={key}
                onClick={() => rate(value)}
              >
                <span className="rating-label">
                  <span className="dot" aria-hidden="true" />
                  {label}
                </span>
                {/* The shortcut is on `aria-keyshortcuts` already, so this
                    copy of it is decorative. */}
                <span className="kbd" aria-hidden="true">
                  {key}
                </span>
              </button>
            ))}
          </div>
          <p className="rating-status" role="status">
            {saving ? "Saving…" : ""}
          </p>
          {saveError && (
            <p className="message-error" role="alert">
              {saveError} Choose a rating to try again.
            </p>
          )}
          <p className="shortcut-hint" aria-hidden="true">
            Press <span className="kbd">1</span>–<span className="kbd">4</span> to rate
          </p>
        </div>
      ) : (
        <div className="reveal">
          <button
            type="button"
            className="button button-primary button-large"
            aria-keyshortcuts="Space Enter"
            onClick={() => setRevealed(true)}
          >
            Reveal answer
          </button>
          <p className="shortcut-hint" aria-hidden="true">
            or press <span className="kbd">Space</span> or <span className="kbd">Enter</span>
          </p>
        </div>
      )}
    </article>
  );
}
