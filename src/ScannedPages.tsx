import { useT, translate, type MessageKey } from "./i18n";
import { useEffect, useRef, useState } from "react";
import {
  stopRecognition,
  toCommandError,
  type ModelSourceRecognition,
  type PageProblem,
  type RecognisedText,
  type SourcePage,
  type SourceRecognition,
} from "./api";
import { BUILT_IN, readSourcePages, type Reader } from "./ReadWith";
import { formatDateTime, useFocusOnMount } from "./ui";

/** Why a page was skipped, said plainly. */
const PROBLEMS: Record<PageProblem, MessageKey> = {
  noPicture: "pages.problemNoPicture",
  unreadablePicture: "pages.problemUnreadablePicture",
  tooLarge: "pages.problemTooLarge",
  noSuchPage: "pages.problemNoSuchPage",
};

/** "2", "2 and 7", "2, 5, and 7". */
function listed(numbers: number[], t: ReturnType<typeof useT>["t"]): string {
  if (numbers.length < 2) return numbers.join("");
  return t(numbers.length === 2 ? "occlusion.list" : "notes.listMany", {
    first: numbers.slice(0, -1).join(", "), last: numbers[numbers.length - 1],
  });
}

/** "page 3", "pages 3 and 7". */
function pagesNamed(numbers: number[], t: ReturnType<typeof useT>["t"]): string {
  return t(numbers.length === 1 ? "pages.namedOne" : "pages.namedOther", { pages: listed(numbers, t) });
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
export function readBy(recognised: RecognisedText, t: ReturnType<typeof useT>["t"] = (key, params) => translate("en", key, params)): string {
  return recognised.engine === "ocrs"
    ? t("pages.builtIn")
    : t("pages.vision", { model: recognised.model });
}

/**
 * What reading went like, in a few sentences: how many pages were read, on
 * which of them nothing was found (`pages`, as they are after reading), and
 * which were skipped and why.
 */
export function readingSummary(
  result: SourceRecognition & Partial<Pick<ModelSourceRecognition, "agreement">>,
  pages: SourcePage[],
  t: ReturnType<typeof useT>["t"] = (key, params) => translate("en", key, params),
): string {
  const read = [...result.read].sort((a, b) => a - b);
  const empty = read.filter((number) => {
    const page = pages.find((each) => each.number === number);
    return page !== undefined && readable(page).trim() === "";
  });
  const parts: string[] = [];

  if (result.stopped) {
    parts.push(
      read.length === 0
        ? t("pages.stoppedBefore")
        : t(read.length === 1 ? "pages.stoppedOne" : "pages.stoppedOther", { count: read.length }),
    );
  } else if (read.length === 0) {
    parts.push(t("pages.noneRead"));
  } else if (read.length === 1) {
    parts.push(
      empty.length === 0
        ? t("pages.readSingle", { page: read[0] })
        : t("pages.readSingleEmpty", { page: read[0] }),
    );
  } else {
    parts.push(t("pages.readMany", { count: read.length }));
  }
  if (empty.length > 0 && (result.stopped || read.length > 1)) {
    parts.push(t("pages.noText", { pages: pagesNamed(empty, t) }));
  }

  const byProblem = new Map<PageProblem, number[]>();
  for (const { page, problem } of result.skipped) {
    byProblem.set(problem, [...(byProblem.get(problem) ?? []), page]);
  }
  for (const [problem, numbers] of byProblem) {
    parts.push(t("pages.skipped", { pages: pagesNamed(numbers.sort((a, b) => a - b), t), problem: t(PROBLEMS[problem]) }));
  }

  // A vision model can misread or make up text: where it and the built-in
  // reader found mostly different words, say so.
  const doubtful = (result.agreement ?? [])
    .filter((each) => each.percent < 50)
    .map((each) => each.page)
    .sort((a, b) => a - b);
  if (doubtful.length > 0) {
    parts.push(
      doubtful.length === 1 ? t("pages.doubtfulOne", { pages: pagesNamed(doubtful, t) }) : t("pages.doubtfulOther", { pages: pagesNamed(doubtful, t) }),
    );
  }
  return parts.join(" ");
}

/** Where reading a source's pages has got to. */
export type Reading =
  | { status: "idle" }
  | { status: "reading"; done: number; total: number; stopping: boolean }
  | { status: "error"; message: string };

/**
 * Reads pages of a source with the reader chosen (the built-in engine, or a
 * vision model in Ollama), one run at a time, following its progress.
 * Leaving the screen (unmounting) stops a run.
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
   * Reads `pages` of source `sourceId`, in order, with `reader`. Resolves
   * with how it went, or null when it failed (`reading` says why), another
   * run was going, or the screen was left.
   */
  async function start(
    sourceId: number,
    pages: number[],
    reader: Reader = BUILT_IN,
  ): Promise<ModelSourceRecognition | null> {
    if (runningRef.current) return null;
    runningRef.current = true;
    setReading({ status: "reading", done: 0, total: Math.max(pages.length, 1), stopping: false });

    let result: ModelSourceRecognition | null = null;
    try {
      result = await readSourcePages(reader, sourceId, pages, (progress) => {
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
  const { t } = useT();
  const statusRef = useFocusOnMount<HTMLParagraphElement>(true);
  const at = Math.min(reading.done + 1, reading.total);

  return (
    <div className="confirm confirm-calm reading-progress">
      <p ref={statusRef} className="confirm-question" role="status" tabIndex={-1}>
        {reading.stopping
          ? t("pages.stopping")
          : single !== null
            ? t("pages.readingSingle", { page: single })
            : t("pages.reading", { page: at, total: reading.total })}
      </p>
      {single === null && (
        <progress
          className="progress-bar"
          value={reading.done}
          max={reading.total}
          aria-label={t("pages.progress", { done: reading.done, total: reading.total })}
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
          {t("settings.stop")}
        </button>
      </div>
    </div>
  );
}

/** A plain warning above recognised text: who read it, when, and that it may be wrong. */
export function RecognisedLabel({ recognised }: { recognised: RecognisedText }) {
  const { t } = useT();
  return (
    <p className="import-warning recognised-label">
      <strong>{t("pages.recognisedWarning")}</strong>{" "}
      {t("pages.recognisedInfo", { reader: readBy(recognised, t), date: formatDateTime(recognised.createdAt) })}
      {recognised.engine === "ocrs" && t("pages.accents")}
    </p>
  );
}

/** What an offer to read scanned pages says: how many, and what to expect. */
export function scannedHint(count: number, t: ReturnType<typeof useT>["t"] = (key, params) => translate("en", key, params)): string {
  return t(count === 1 ? "pages.scannedHintOne" : "pages.scannedHintOther", { count });
}
