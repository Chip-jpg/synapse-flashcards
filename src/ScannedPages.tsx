import { useEffect, useRef, useState } from "react";
import {
  recogniseSourcePages,
  stopRecognition,
  toCommandError,
  type PageProblem,
  type RecognisedText,
  type SourcePage,
  type SourceRecognition,
} from "./api";
import { formatDateTime, useFocusOnMount } from "./ui";

/** Why a page was skipped, said plainly. */
const PROBLEMS: Record<PageProblem, string> = {
  noPicture: "no picture to read",
  unreadablePicture: "its picture is stored in a form Synapse can't read, such as JPEG 2000",
  tooLarge: "its picture is too large to read",
  noSuchPage: "the PDF has no such page",
};

/** "1 page", "12 pages". */
function counted(count: number, thing: string): string {
  return count === 1 ? `1 ${thing}` : `${count} ${thing}s`;
}

/** "2", "2 and 7", "2, 5, and 7". */
function listed(numbers: number[]): string {
  if (numbers.length <= 2) return numbers.join(" and ");
  return `${numbers.slice(0, -1).join(", ")}, and ${numbers[numbers.length - 1]}`;
}

/** "page 3", "pages 3 and 7". */
function pagesNamed(numbers: number[]): string {
  return `${numbers.length === 1 ? "page" : "pages"} ${listed(numbers)}`;
}

/** The text a page gives a note: what extraction read, or else what was recognised. */
export function readable(page: SourcePage): string {
  return page.text.trim() === "" ? (page.recognised?.text ?? "") : page.text;
}

/** A page with no extracted text whose picture was read, finding some text. */
export function isRecognised(page: SourcePage): boolean {
  return page.text.trim() === "" && (page.recognised?.text.trim() ?? "") !== "";
}

/** Who read some text: Synapse's own engine, or a model in Ollama. */
export function readBy(recognised: RecognisedText): string {
  return recognised.engine === "ocrs"
    ? "Synapse's built-in reader"
    : `${recognised.model}, a vision model in Ollama`;
}

/**
 * What reading went like, in a few sentences: how many pages were read, on
 * which of them nothing was found (`pages`, as they are after reading), and
 * which were skipped and why.
 */
export function readingSummary(result: SourceRecognition, pages: SourcePage[]): string {
  const read = [...result.read].sort((a, b) => a - b);
  const empty = read.filter((number) => {
    const page = pages.find((each) => each.number === number);
    return page !== undefined && readable(page).trim() === "";
  });
  const parts: string[] = [];

  if (result.stopped) {
    parts.push(
      read.length === 0
        ? "Stopped before any page was read."
        : `Stopped after reading ${counted(read.length, "page")}. Those pages are kept.`,
    );
  } else if (read.length === 0) {
    parts.push("No page was read.");
  } else if (read.length === 1) {
    parts.push(
      empty.length === 0
        ? `Read page ${read[0]}.`
        : `Read page ${read[0]}, but no text was found on it.`,
    );
  } else {
    parts.push(`Read ${read.length} pages.`);
  }
  if (empty.length > 0 && (result.stopped || read.length > 1)) {
    parts.push(`No text was found on ${pagesNamed(empty)}.`);
  }

  const byProblem = new Map<PageProblem, number[]>();
  for (const { page, problem } of result.skipped) {
    byProblem.set(problem, [...(byProblem.get(problem) ?? []), page]);
  }
  for (const [problem, numbers] of byProblem) {
    parts.push(`Skipped ${pagesNamed(numbers.sort((a, b) => a - b))}: ${PROBLEMS[problem]}.`);
  }
  return parts.join(" ");
}

/** Where reading a source's pages has got to. */
export type Reading =
  | { status: "idle" }
  | { status: "reading"; done: number; total: number; stopping: boolean }
  | { status: "error"; message: string };

/**
 * Reads pages of a source with `recogniseSourcePages`, one run at a time,
 * following its progress. Leaving the screen (unmounting) stops a run.
 */
export function useSourceReading() {
  const [reading, setReading] = useState<Reading>({ status: "idle" });
  // Blocks a second run immediately, before the re-render lands.
  const runningRef = useRef(false);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (runningRef.current) stopRecognition().catch(() => {});
    };
  }, []);

  /**
   * Reads `pages` of source `sourceId`, in order. Resolves with how it went,
   * or null when it failed (`reading` says why), another run was going, or
   * the screen was left.
   */
  async function start(sourceId: number, pages: number[]): Promise<SourceRecognition | null> {
    if (runningRef.current) return null;
    runningRef.current = true;
    setReading({ status: "reading", done: 0, total: Math.max(pages.length, 1), stopping: false });

    let result: SourceRecognition | null = null;
    try {
      result = await recogniseSourcePages(sourceId, pages, (progress) => {
        if (!mountedRef.current) return;
        setReading((now) =>
          now.status === "reading"
            ? { ...now, done: progress.done, total: progress.total }
            : now,
        );
      });
      if (mountedRef.current) setReading({ status: "idle" });
    } catch (err) {
      if (mountedRef.current) {
        setReading({ status: "error", message: toCommandError(err).message });
      }
    }
    runningRef.current = false;
    return mountedRef.current ? result : null;
  }

  /** Stops the run; the pages read so far are kept. */
  function stop() {
    setReading((now) => (now.status === "reading" ? { ...now, stopping: true } : now));
    stopRecognition().catch(() => {});
  }

  return { reading, start, stop };
}

/**
 * The progress of a run: "Reading page 3 of 12…", with *Stop*. Focus goes to
 * the status, so a held Enter can't stop it.
 */
export function ReadingProgress({
  reading,
  single,
  onStop,
}: {
  reading: Reading & { status: "reading" };
  /** The page being read, when only one is. */
  single: number | null;
  onStop: () => void;
}) {
  const statusRef = useFocusOnMount<HTMLParagraphElement>(true);
  const at = Math.min(reading.done + 1, reading.total);

  return (
    <div className="confirm confirm-calm reading-progress">
      <p ref={statusRef} className="confirm-question" role="status" tabIndex={-1}>
        {reading.stopping
          ? "Stopping… The pages read so far are kept."
          : single !== null
            ? `Reading page ${single}…`
            : `Reading page ${at} of ${reading.total}…`}
      </p>
      {single === null && (
        <progress
          className="progress-bar"
          value={reading.done}
          max={reading.total}
          aria-label={`${reading.done} of ${reading.total} pages read`}
        />
      )}
      <div className="deck-actions">
        <button
          type="button"
          className="button button-small"
          aria-disabled={reading.stopping}
          onClick={() => {
            if (!reading.stopping) onStop();
          }}
        >
          Stop
        </button>
      </div>
    </div>
  );
}

/** A plain warning above recognised text: who read it, when, and that it may be wrong. */
export function RecognisedLabel({ recognised }: { recognised: RecognisedText }) {
  return (
    <p className="import-warning recognised-label">
      <strong>Recognised text, check for mistakes.</strong>{" "}
      {`This page has no text of its own, so ${readBy(recognised)} read it from the page's picture on ${formatDateTime(recognised.createdAt)}.`}
      {recognised.engine === "ocrs" && " It doesn't read accents: è comes out as e."}
    </p>
  );
}

/** What an offer to read scanned pages says: how many, and what to expect. */
export function scannedHint(count: number): string {
  const one = count === 1;
  return `${counted(count, "page")} of this PDF ${one ? "has" : "have"} no text Synapse could read: ${
    one ? "it may be a scan" : "they may be scans"
  }. Synapse can read the text in ${one ? "its picture" : "their pictures"}, on this computer. Recognised text can hold mistakes, and the built-in reader doesn't read accents (è comes out as e), so check it.`;
}
