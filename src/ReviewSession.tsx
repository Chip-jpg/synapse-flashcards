import { useEffect, useRef, useState } from "react";
import {
  getSessionCard,
  reviewCard,
  toCommandError,
  type Flashcard,
  type Rating,
} from "./api";
import { Message, useFocusOnMount } from "./ui";

type SessionState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "due"; card: Flashcard }
  | { status: "completed"; cardsReviewed: number };

const RATINGS: { value: Rating; label: string }[] = [
  { value: 1, label: "Again" },
  { value: 2, label: "Hard" },
  { value: 3, label: "Good" },
  { value: 4, label: "Easy" },
];

/** Fetches the session's next card and describes the outcome; never rejects. */
async function loadCard(sessionId: number): Promise<SessionState> {
  try {
    return await getSessionCard(sessionId);
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
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
      <h2 id="session-heading" className="section-title">
        Reviewing {deckName}
      </h2>

      {state.status === "loading" && (
        <p className="message" role="status">
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
              Back to decks
            </button>
          </div>
        </Message>
      )}

      {state.status === "due" && (
        <Card
          key={`${state.card.id}:${state.card.reps}`}
          sessionId={sessionId}
          card={state.card}
          onReviewed={reload}
        />
      )}

      {state.status === "completed" && (
        <Message
          focus
          tone="status"
          text={
            <>
              <strong className="message-title">Session complete.</strong>
              {` You reviewed ${state.cardsReviewed} ${state.cardsReviewed === 1 ? "card" : "cards"}. No more cards are due in this deck right now.`}
            </>
          }
        >
          <button type="button" className="button button-primary" onClick={onExit}>
            Back to decks
          </button>
        </Message>
      )}
    </section>
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

  return (
    <article className="card">
      <section
        ref={questionRef}
        className="card-side"
        tabIndex={-1}
        aria-labelledby="card-front-label"
      >
        <h3 id="card-front-label" className="side-label">
          Question
        </h3>
        <p className="card-text">{card.front}</p>
      </section>

      {revealed ? (
        <>
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

          <div
            className="rating"
            role="group"
            aria-labelledby="rating-prompt"
            aria-busy={saving}
          >
            <p id="rating-prompt" className="rating-prompt">
              How well did you remember it?
            </p>
            <div className="rating-buttons">
              {RATINGS.map(({ value, label }) => (
                // `aria-disabled` rather than `disabled`: a disabled button
                // drops keyboard focus, which would strand keyboard users.
                <button
                  key={value}
                  type="button"
                  className="button"
                  aria-disabled={saving}
                  onClick={() => rate(value)}
                >
                  {label}
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
          </div>
        </>
      ) : (
        <button
          type="button"
          className="button button-primary"
          onClick={() => setRevealed(true)}
        >
          Reveal answer
        </button>
      )}
    </article>
  );
}
