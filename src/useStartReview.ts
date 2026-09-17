import { useRef, useState } from "react";
import { startSession, toCommandError } from "./api";

/**
 * Starting (or resuming) a review of one deck, shared by the dashboard and the
 * deck screen. If nothing turns out to be due (e.g. time passed since the
 * counts loaded), `onNoneDue` is called so the caller can refresh them.
 */
export function useStartReview(
  deckId: number,
  deckName: string,
  onStart: (sessionId: number, deckName: string) => void,
  onNoneDue: () => void,
) {
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second start immediately, before the re-render lands.
  const startingRef = useRef(false);

  async function start() {
    if (startingRef.current) return;
    startingRef.current = true;
    setStarting(true);
    setError(null);

    try {
      const result = await startSession(deckId);
      if (result.status === "started") {
        onStart(result.sessionId, deckName);
      } else {
        // Nothing is due any more (e.g. time passed): refresh the counts.
        onNoneDue();
      }
      return; // The caller's view is replaced either way.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // The deck changed since these counts were shown (it was archived, so
        // it can't be reviewed). Reload rather than leaving a dead deck card.
        onNoneDue();
        return;
      }
      setError(failure.message);
    }

    startingRef.current = false;
    setStarting(false);
  }

  return { starting, error, start };
}
