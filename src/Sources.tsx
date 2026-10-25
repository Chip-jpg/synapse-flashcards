import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  deleteSource,
  getDeletedSources,
  getSource,
  getSourceNotes,
  getSearchedPage,
  getSourcePages,
  getSources,
  importSource,
  openSourceOriginal,
  renameSource,
  restoreSource,
  saveSourceCopy,
  toCommandError,
  type FileKind,
  type NoteSummary,
  type SnippetPiece,
  type Source,
  type SourceImport,
  type SourcePage,
  type SourceRecognition,
} from "./api";
import { FormActions, TextField, useFormSave } from "./forms";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  DownloadIcon,
  FileIcon,
  NoteIcon,
  PencilIcon,
  PlusIcon,
  RestoreIcon,
  TrashIcon,
  UploadIcon,
} from "./icons";
import {
  isRecognised,
  readable,
  readable as pageText,
  ReadingProgress,
  readingSummary,
  RecognisedLabel,
  scannedHint,
  useSourceReading,
  type Reading,
} from "./ScannedPages";
import { BUILT_IN, pagesToRead, ReadWith, useReader, type Reader, type ReaderState } from "./ReadWith";
import { KIND_NAMES } from "./SourcePages";
import {
  SourceResults,
  SourceSearchField,
  useSourceSearch,
  type SourceSearchState,
} from "./SourceSearch";
import { formatDateTime, Message, useFocusOnMount } from "./ui";
import { useUnsavedChanges } from "./unsaved";

/**
 * A new note to write from a source, checked before it's saved: the text
 * Synapse read (or the source's pages, joined), and what to say about it.
 * `justKept` is true when the file was kept a moment ago, by *Import file*.
 */
export type SourceNoteDraft = {
  sourceId: number;
  fileName: string;
  kind: FileKind;
  title: string;
  body: string;
  /** Pages of a PDF or slides of a presentation; null for other documents. */
  pages: number | null;
  pagesWithoutText: number[];
  /** Pages whose text was recognised from their picture, so may hold mistakes. */
  recognisedPages: number[];
  justKept: boolean;
};

/** The note a source just kept would give: the text `importFile` reads. */
export function draftFromImport(kept: SourceImport & { status: "imported" }): SourceNoteDraft {
  return {
    sourceId: kept.source.id,
    fileName: kept.source.fileName,
    kind: kept.text.kind,
    title: kept.text.title,
    body: kept.text.body,
    pages: kept.text.pages,
    pagesWithoutText: kept.text.pagesWithoutText,
    recognisedPages: [],
    justKept: true,
  };
}

/**
 * Whether a file just kept is a PDF with no text at all, as a scan is: its
 * pages are better read before a note is written from it.
 */
export function looksScanned(kept: SourceImport & { status: "imported" }): boolean {
  return (
    kept.text.kind === "pdf" &&
    kept.text.body.trim() === "" &&
    kept.text.pagesWithoutText.length > 0
  );
}

/**
 * A note from a source kept earlier: its pages, in order, as importing it
 * gave them (a PDF's pages and a presentation's slides already say where one
 * ends), under the source's title. A page without text gives the text
 * recognised on it, if it was read.
 */
export function draftFromPages(source: Source, pages: SourcePage[]): SourceNoteDraft {
  const paged = source.kind === "pdf" || source.kind === "powerPoint";
  return {
    sourceId: source.id,
    fileName: source.fileName,
    kind: source.kind,
    title: source.title,
    body: pages
      .map((page) => readable(page).trim())
      .filter((text) => text !== "")
      .join("\n\n"),
    pages: paged ? source.pageCount : null,
    pagesWithoutText:
      source.kind === "pdf"
        ? pages.filter((page) => readable(page).trim() === "").map((page) => page.number)
        : [],
    recognisedPages: pages.filter(isRecognised).map((page) => page.number),
    justKept: false,
  };
}

/** "1 page", "12 slides". */
function counted(count: number, thing: string): string {
  return count === 1 ? `1 ${thing}` : `${count} ${thing}s`;
}

/** "840 KB", "2.3 MB": a file's size, as file windows show it. */
export function formatSize(bytes: number): string {
  if (bytes < 1000) return `${bytes} bytes`;
  if (bytes < 1_000_000) return `${Math.round(bytes / 1000)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

/** "PDF · 12 pages · 2.3 MB": what a source is, in one line. */
function describe(source: Source): string {
  const parts: string[] = [KIND_NAMES[source.kind]];
  if (source.kind === "pdf") parts.push(counted(source.pageCount, "page"));
  if (source.kind === "powerPoint") parts.push(counted(source.pageCount, "slide"));
  parts.push(formatSize(source.asset.size));
  return parts.join(" · ");
}

/** What takes focus when the library appears. */
type LibraryFocus =
  | { to: "screen" }
  | { to: "add" }
  | { to: "source"; sourceId: number }
  | { to: "notice" };

/** Where in the sources area the user is. */
type Place =
  | { screen: "library"; focus: LibraryFocus; notice: string | null }
  | { screen: "source"; sourceId: number; notice: string | null; found?: FoundAt };

/** A page a search found words on, opened with those words marked. */
type FoundAt = { page: number; query: string };

/**
 * The Sources screen: the documents Synapse keeps, as they were imported.
 * Each can be opened to read its pages, opened in its own program, saved as
 * a copy, renamed, made into a note, deleted and restored. Every screen
 * change follows a button press, so each new screen takes focus.
 */
export function SourcesArea({
  openSource,
  onBack,
  onOpenNote,
  onMakeNote,
}: {
  /** Opens straight at one source, e.g. coming back from one of its notes. */
  openSource?: { sourceId: number; notice: string | null };
  onBack: () => void;
  /** Opens a note written from the source `sourceId`. */
  onOpenNote: (noteId: number, sourceId: number) => void;
  /** Writes a new note from a source, starting from `draft`. */
  onMakeNote: (draft: SourceNoteDraft) => void;
}) {
  const [place, setPlace] = useState<Place>(
    openSource
      ? { screen: "source", sourceId: openSource.sourceId, notice: openSource.notice }
      : { screen: "library", focus: { to: "screen" }, notice: null },
  );
  // Kept here, so a search is still there coming back from a source.
  const search = useSourceSearch();

  if (place.screen === "source") {
    const sourceId = place.sourceId;
    return (
      <SourceScreen
        key={sourceId}
        sourceId={sourceId}
        notice={place.notice}
        found={place.found ?? null}
        onBack={() => {
          // The source may have changed (renamed, read) while it was open.
          if (search.query.trim() !== "") search.refresh();
          setPlace({ screen: "library", focus: { to: "source", sourceId }, notice: null });
        }}
        onDeleted={(notice) => setPlace({ screen: "library", focus: { to: "notice" }, notice })}
        onOpenNote={(noteId) => onOpenNote(noteId, sourceId)}
        onMakeNote={onMakeNote}
      />
    );
  }

  return (
    <SourceLibrary
      focus={place.focus}
      notice={place.notice}
      onKept={(kept) =>
        setPlace({
          screen: "source",
          sourceId: kept.source.id,
          notice: looksScanned(kept)
            ? `Kept ${kept.source.fileName}, as it was. None of its pages has text Synapse could read, so it looks like a scan. Choose Read scanned pages to read the text in its pictures, then make a note from it.`
            : `Kept ${kept.source.fileName}. Its text is below, page by page, and the original file is kept as it was. Make a note from it to edit the text and study it.`,
        })
      }
      onOpen={(sourceId) => setPlace({ screen: "source", sourceId, notice: null })}
      search={search}
      onOpenFound={(sourceId, page) =>
        setPlace({
          screen: "source",
          sourceId,
          notice: null,
          found: page === null ? undefined : { page, query: search.query.trim() },
        })
      }
      onBack={onBack}
    />
  );
}

type LibraryState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; sources: Source[]; deleted: Source[] };

/** Fetches every source and every deleted one; never rejects. */
async function loadSources(): Promise<LibraryState> {
  try {
    const [sources, deleted] = await Promise.all([getSources(), getDeletedSources()]);
    return { status: "ready", sources, deleted };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

function SourceLibrary({
  focus: initialFocus,
  notice: initialNotice,
  onKept,
  onOpen,
  search,
  onOpenFound,
  onBack,
}: {
  focus: LibraryFocus;
  notice: string | null;
  onKept: (kept: SourceImport & { status: "imported" }) => void;
  onOpen: (sourceId: number) => void;
  search: SourceSearchState;
  onOpenFound: (sourceId: number, page: number | null) => void;
  onBack: () => void;
}) {
  const [state, setState] = useState<LibraryState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  // `key` changes with every reload, so the list remounts and its focus
  // target takes effect (as after restoring a source).
  const [view, setView] = useState({ key: 0, notice: initialNotice, focus: initialFocus });

  useEffect(() => {
    let current = true;
    loadSources().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  /** Reloads both lists, announcing `notice`, which then takes focus. */
  function reload(notice: string | null) {
    setState({ status: "loading" });
    setView((previous) => ({
      key: previous.key + 1,
      notice,
      focus: notice === null ? { to: "screen" } : { to: "notice" },
    }));
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading your sources…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <div className="actions">
          <button type="button" className="button" onClick={() => reload(null)}>
            Retry
          </button>
          <button type="button" className="button" onClick={onBack}>
            Back to Study Desk
          </button>
        </div>
      </Message>
    );
  }

  return (
    <SourceList
      key={view.key}
      sources={state.sources}
      deleted={state.deleted}
      notice={view.notice}
      focus={view.focus}
      onKept={onKept}
      onOpen={onOpen}
      search={search}
      onOpenFound={onOpenFound}
      onBack={onBack}
      onRestored={(title) => reload(`${title} restored. It's back in your sources.`)}
      onStale={() =>
        reload("That source wasn't deleted after all. Here are your sources as they are now.")
      }
    />
  );
}

function SourceList({
  sources,
  deleted,
  notice,
  focus,
  onKept,
  onOpen,
  search,
  onOpenFound,
  onBack,
  onRestored,
  onStale,
}: {
  sources: Source[];
  deleted: Source[];
  notice: string | null;
  focus: LibraryFocus;
  onKept: (kept: SourceImport & { status: "imported" }) => void;
  onOpen: (sourceId: number) => void;
  search: SourceSearchState;
  onOpenFound: (sourceId: number, page: number | null) => void;
  onBack: () => void;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const searching = search.query.trim() !== "";
  const answered = searching && search.found?.query === search.query.trim();
  const shown = answered ? (search.found?.found.sources.map((hit) => hit.source) ?? []) : sources;
  const focusSourceId =
    focus.to === "source" && shown.some((source) => source.id === focus.sourceId)
      ? focus.sourceId
      : null;
  const focusNotice = focus.to === "notice" && notice !== null;
  const screenRef = useFocusOnMount<HTMLElement>(
    focus.to === "screen" ||
      (focus.to === "source" && focusSourceId === null) ||
      (focus.to === "notice" && !focusNotice),
  );
  const addRef = useFocusOnMount<HTMLButtonElement>(focus.to === "add");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focusNotice);
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  // Blocks a second file window immediately, before the re-render lands.
  const addingRef = useRef(false);

  /** Chooses a document and keeps it; it then opens. */
  async function add() {
    if (addingRef.current) return;
    addingRef.current = true;
    setAdding(true);
    setAddError(null);

    try {
      const result = await importSource();
      if (result.status === "imported") {
        onKept(result);
        return; // This screen is replaced.
      }
    } catch (err) {
      setAddError(toCommandError(err).message);
    }

    addingRef.current = false;
    setAdding(false);
  }

  return (
    <section ref={screenRef} className="notes" tabIndex={-1} aria-labelledby="sources-heading">
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        Back to Study Desk
      </button>

      <div className="page-header">
        <div className="page-heading">
          <h2 id="sources-heading" className="page-title">
            Sources
          </h2>
          <p className="page-intro">
            The documents you imported, kept as they were: the original file and the text of each
            page. Notes you write from a source are yours to edit; the source itself never changes.
          </p>
        </div>
        <div className="deck-actions page-actions">
          <button
            ref={addRef}
            type="button"
            className="button button-primary"
            aria-disabled={adding}
            aria-describedby="add-source-hint"
            onClick={add}
          >
            <UploadIcon />
            Add source
          </button>
        </div>
      </div>
      <p id="add-source-hint" className="field-hint">
        Add a PDF, text, Markdown, Word, or PowerPoint file, up to 50 MB. Synapse keeps a copy of it
        and reads the text of each page. Importing a file under Notes keeps it here too.
      </p>
      <p className="form-status" role="status">
        {adding ? "Choose a file. Synapse then keeps it and reads its text…" : ""}
      </p>
      {addError && (
        <p className="message-error" role="alert">
          {addError}
        </p>
      )}
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {(sources.length > 0 || searching) && <SourceSearchField search={search} />}

      {searching ? (
        answered &&
        search.found && (
          <SourceResults
            found={search.found.found}
            describe={describe}
            focusSourceId={focusSourceId}
            onOpen={onOpenFound}
          />
        )
      ) : sources.length === 0 ? (
        <p className="empty" role="status">
          {deleted.length === 0
            ? "No sources yet. Choose Add source, or import a file under Notes, to keep one."
            : "No sources kept. Choose Add source to keep one, or restore a deleted source below."}
        </p>
      ) : (
        <ul className="card-list">
          {sources.map((source) => (
            <li key={source.id}>
              <SourceItem
                source={source}
                focusOpen={source.id === focusSourceId}
                onOpen={() => onOpen(source.id)}
              />
            </li>
          ))}
        </ul>
      )}

      {deleted.length > 0 && !searching && (
        <section className="cards section-quiet" aria-labelledby="deleted-sources-heading">
          <div className="section-heading">
            <span className="section-icon section-icon-quiet" aria-hidden="true">
              <RestoreIcon />
            </span>
            <h3 id="deleted-sources-heading" className="section-title section-title-quiet">
              Deleted sources
            </h3>
          </div>
          <p className="field-hint">
            Kept with their files and pages. A deleted source can't be opened, but restoring it
            brings it back exactly as it was. Notes written from a source are never affected.
          </p>
          <ul className="card-list">
            {deleted.map((source) => (
              <li key={source.id}>
                <DeletedSourceItem source={source} onRestored={onRestored} onStale={onStale} />
              </li>
            ))}
          </ul>
        </section>
      )}
    </section>
  );
}

function SourceItem({
  source,
  focusOpen,
  onOpen,
}: {
  source: Source;
  focusOpen: boolean;
  onOpen: () => void;
}) {
  const openRef = useFocusOnMount<HTMLButtonElement>(focusOpen);
  const titleId = `source-${source.id}-title`;

  return (
    <article className="card-item note-item" aria-labelledby={titleId}>
      <div className="note-item-main">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          <FileIcon />
        </span>
        <div>
          <h3 id={titleId} className="note-item-title">
            {source.title}
          </h3>
          <p className="item-meta">
            {`${describe(source)} · Added ${formatDateTime(source.createdAt)}`}
          </p>
        </div>
      </div>
      <button
        ref={openRef}
        type="button"
        className="button button-small"
        aria-describedby={titleId}
        onClick={onOpen}
      >
        Open source
        <ArrowRightIcon />
      </button>
    </article>
  );
}

/** One deleted source, with the one action it has: Restore. */
function DeletedSourceItem({
  source,
  onRestored,
  onStale,
}: {
  source: Source;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const restoringRef = useRef(false);
  const titleId = `deleted-source-${source.id}-title`;

  async function restore() {
    if (restoringRef.current) return;
    restoringRef.current = true;
    setRestoring(true);
    setError(null);

    try {
      await restoreSource(source.id);
      onRestored(source.title);
      return; // The list is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        onStale();
        return;
      }
      setError(failure.message);
    }

    restoringRef.current = false;
    setRestoring(false);
  }

  return (
    <div className="card-item card-item-recovery">
      <div>
        <h4 id={titleId} className="note-item-title">
          {source.title}
        </h4>
        <p className="item-meta">
          {`${describe(source)} · Deleted ${formatDateTime(source.deletedAt ?? source.updatedAt)}`}
        </p>
      </div>
      <div className="deck-actions item-actions">
        <button
          type="button"
          className="button button-small"
          aria-describedby={titleId}
          aria-disabled={restoring}
          onClick={restore}
        >
          <RestoreIcon />
          Restore source
        </button>
      </div>
      <p className="form-status" role="status">
        {restoring ? "Restoring…" : ""}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

type SourceState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | {
      status: "ready";
      source: Source;
      pages: SourcePage[];
      notes: NoteSummary[];
      /** Pages without text not read yet: what *Read scanned pages* reads. */
      toRead: number[];
    };

/**
 * Fetches a source, its pages, the notes written from it, and the pages left
 * to read; never rejects.
 */
async function loadSource(sourceId: number, reader: Reader = BUILT_IN): Promise<SourceState> {
  try {
    const [source, pages, notes, toRead] = await Promise.all([
      getSource(sourceId),
      getSourcePages(sourceId),
      getSourceNotes(sourceId),
      pagesToRead(reader, sourceId),
    ]);
    return { status: "ready", source, pages, notes, toRead };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/** What the source screen shows in place of its toolbar. */
type Mode = "view" | "rename" | "confirmDelete";

/**
 * One source: what it is, the notes written from it, and its pages, with its
 * original a press away.
 */
function SourceScreen({
  sourceId,
  notice: initialNotice,
  found,
  onBack,
  onDeleted,
  onOpenNote,
  onMakeNote,
}: {
  sourceId: number;
  notice: string | null;
  /** The page a search found words on, to open first with them marked. */
  found: FoundAt | null;
  onBack: () => void;
  /** The source was deleted (or already was), so this screen can't show it. */
  onDeleted: (notice: string) => void;
  onOpenNote: (noteId: number) => void;
  onMakeNote: (draft: SourceNoteDraft) => void;
}) {
  const [state, setState] = useState<SourceState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  const [mode, setMode] = useState<Mode>("view");
  const [notice, setNotice] = useState(initialNotice);
  const reader = useSourceReading();
  const readWith = useReader();
  const readerKey = readWith.reader.engine === "ollama" ? readWith.reader.model : "";
  // Which run is going or last finished: every page left, or one page from
  // the page viewer. Its progress and summary show where it was started.
  const [run, setRun] = useState<{ single: number | null; summary: string | null } | null>(null);
  // Pages a run here skipped (no picture, say): reading them again would
  // only skip them again, so they aren't offered again on this screen.
  const [skipped, setSkipped] = useState<number[]>([]);

  useEffect(() => {
    let current = true;
    loadSource(sourceId, readWith.reader).then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
    // The pages left to read depend on the reader, by its key.
  }, [sourceId, attempt, readerKey]);

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading the source…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <div className="actions">
          <button
            type="button"
            className="button"
            onClick={() => {
              setState({ status: "loading" });
              setAttempt((n) => n + 1);
            }}
          >
            Retry
          </button>
          <button type="button" className="button" onClick={onBack}>
            Back to sources
          </button>
        </div>
      </Message>
    );
  }

  const { source, pages, notes } = state;
  const toRead = state.toRead.filter((page) => !skipped.includes(page));

  /**
   * Reads `single`, or every page left, then shows the pages as they are now
   * and says how it went, where the run was started.
   */
  async function read(single: number | null) {
    setRun({ single, summary: null });
    const result = await reader.start(
      sourceId,
      single === null ? toRead : [single],
      readWith.reader,
    );
    if (result === null) {
      // A model that has left Ollama: the choice offered is out of date.
      if (readWith.reader.engine === "ollama") void readWith.reload();
      return;
    }
    setSkipped((now) => withSkipped(now, result));
    const next = await loadSource(sourceId, readWith.reader);
    setState(next);
    if (next.status === "ready") setRun({ single, summary: readingSummary(result, next.pages) });
  }

  const reading = reader.reading.status === "reading" ? reader.reading : null;
  const runHere = (single: boolean) => run !== null && (run.single !== null) === single;

  return (
    <section className="deck-detail note-detail source-detail" aria-labelledby="source-heading">
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        Back to sources
      </button>

      <article className="card">
        {mode === "rename" ? (
          <RenameSource
            source={source}
            onCancel={() => setMode("view")}
            onSaved={(renamed) => {
              setState({ ...state, source: renamed });
              setNotice("Source renamed.");
              setMode("view");
            }}
          />
        ) : (
          <SourceHeader
            key={notice ?? ""}
            source={source}
            notice={notice}
            confirming={mode === "confirmDelete"}
            onConfirm={(confirming) => setMode(confirming ? "confirmDelete" : "view")}
            onRename={() => setMode("rename")}
            onMakeNote={() => onMakeNote(draftFromPages(source, pages))}
            onDeleted={onDeleted}
          />
        )}

        {source.kind === "pdf" && (
          <ScannedPanel
            scanned={pages.filter((page) => page.text.trim() === "").length}
            readWith={readWith}
            toRead={toRead.length}
            progress={runHere(false) ? reading : null}
            summary={runHere(false) ? (run?.summary ?? null) : null}
            error={
              runHere(false) && reader.reading.status === "error" ? reader.reading.message : null
            }
            busy={reading !== null}
            onRead={() => read(null)}
            onStop={reader.stop}
          />
        )}

        <SourceNotes notes={notes} onOpenNote={onOpenNote} />
        <PageViewer
          source={source}
          pages={pages}
          found={found}
          reading={reading !== null}
          pageRun={
            runHere(true) && run?.single != null
              ? {
                  page: run.single,
                  summary: run.summary,
                  error: reader.reading.status === "error" ? reader.reading.message : null,
                  progress: reading,
                }
              : null
          }
          readWith={readWith}
          onRead={read}
          onStop={reader.stop}
        />
      </article>
    </section>
  );
}

function SourceHeader({
  source,
  notice,
  confirming,
  onConfirm,
  onRename,
  onMakeNote,
  onDeleted,
}: {
  source: Source;
  notice: string | null;
  confirming: boolean;
  onConfirm: (confirming: boolean) => void;
  onRename: () => void;
  onMakeNote: () => void;
  onDeleted: (notice: string) => void;
}) {
  const headingRef = useFocusOnMount<HTMLHeadingElement>(notice === null);
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(notice !== null);
  const [busy, setBusy] = useState<"open" | "save" | "delete" | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second press immediately, before the re-render lands.
  const busyRef = useRef(false);
  const deleteRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  const openedRef = useRef(false);

  // As for a note: the question takes focus when it opens, so a repeated
  // Enter can't delete, and "Delete source" gets it back if the source stays.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      deleteRef.current?.focus();
    }
  }, [confirming]);

  async function run(kind: "open" | "save" | "delete", action: () => Promise<string | null>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(kind);
    setStatus(null);
    setError(null);
    try {
      setStatus(await action());
    } catch (err) {
      const failure = toCommandError(err);
      if (kind === "delete" && failure.kind === "stale") {
        onDeleted("That source was already deleted. It's kept in Deleted sources.");
        return;
      }
      setError(failure.message);
    }
    busyRef.current = false;
    setBusy(null);
  }

  const openOriginal = () =>
    run("open", async () => {
      await openSourceOriginal(source.id);
      return `Opened a copy of ${source.fileName}. Changes you make to it there don't change the source.`;
    });

  const saveCopy = () =>
    run("save", async () => {
      const outcome = await saveSourceCopy(source.id);
      return outcome.status === "saved" ? `Saved a copy as ${outcome.fileName}.` : null;
    });

  const confirmDelete = () =>
    run("delete", async () => {
      await deleteSource(source.id);
      onDeleted(
        `${source.title} deleted. It's kept in Deleted sources, with its file, where you can restore it.`,
      );
      return null;
    });

  return (
    <>
      <div>
        <h2 ref={headingRef} id="source-heading" className="page-title" tabIndex={-1}>
          {source.title}
        </h2>
        <p className="item-meta">
          {`${source.fileName} · ${describe(source)} · Added ${formatDateTime(source.createdAt)}`}
        </p>
      </div>

      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {confirming ? (
        <div className="confirm">
          <p ref={questionRef} className="confirm-question" tabIndex={-1}>
            Delete this source? It will leave your sources and can't be opened. Its file and pages
            are kept under Deleted sources, where you can restore it. Notes written from it aren't
            changed, but their cards and quizzes stop showing which page they came from.
          </p>
          <div className="deck-actions">
            <button
              type="button"
              className="button button-danger-soft button-small"
              aria-describedby="source-heading"
              aria-disabled={busy !== null}
              onClick={confirmDelete}
            >
              Delete source
            </button>
            <button
              type="button"
              className="button button-small"
              aria-disabled={busy !== null}
              onClick={() => {
                if (busy === null) onConfirm(false);
              }}
            >
              Keep source
            </button>
          </div>
        </div>
      ) : (
        <div className="deck-actions note-toolbar">
          <button
            type="button"
            className="button button-primary"
            aria-describedby="source-heading"
            onClick={onMakeNote}
          >
            <PlusIcon />
            Make a note
          </button>
          <button
            type="button"
            className="button"
            aria-describedby="source-heading"
            aria-disabled={busy !== null}
            onClick={openOriginal}
          >
            <FileIcon />
            Open original
          </button>
          <button
            type="button"
            className="button"
            aria-describedby="source-heading"
            aria-disabled={busy !== null}
            onClick={saveCopy}
          >
            <DownloadIcon />
            Save a copy
          </button>
          <button
            type="button"
            className="button"
            aria-describedby="source-heading"
            onClick={onRename}
          >
            <PencilIcon />
            Rename
          </button>
          <button
            ref={deleteRef}
            type="button"
            className="button button-quiet"
            aria-describedby="source-heading"
            onClick={() => onConfirm(true)}
          >
            <TrashIcon />
            Delete source
          </button>
        </div>
      )}

      <p className="form-status" role="status">
        {busy === "open"
          ? "Opening the original…"
          : busy === "save"
            ? "Choose where to save the copy…"
            : busy === "delete"
              ? "Deleting…"
              : (status ?? "")}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </>
  );
}

/** Renaming a source in place. Only the title changes; Rust checks it. */
function RenameSource({
  source,
  onCancel,
  onSaved,
}: {
  source: Source;
  onCancel: () => void;
  onSaved: (source: Source) => void;
}) {
  const [title, setTitle] = useState(source.title);
  useUnsavedChanges(title !== source.title);
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => renameSource(source.id, title),
    onSaved,
    { title: "source-title" },
  );

  return (
    <form aria-labelledby="rename-source-heading" noValidate onSubmit={submit}>
      <h2 id="rename-source-heading" className="form-title">
        Rename source
      </h2>
      <TextField
        id="source-title"
        label="Title"
        hint="Required. Up to 200 characters. The file and its pages don't change."
        value={title}
        error={fieldError("title")}
        required
        autoFocus
        onChange={(value) => {
          setTitle(value);
          edited("title");
        }}
      />
      <FormActions saveLabel="Save title" saving={saving} error={formError} onCancel={onCancel} />
    </form>
  );
}

function SourceNotes({
  notes,
  onOpenNote,
}: {
  notes: NoteSummary[];
  onOpenNote: (noteId: number) => void;
}) {
  return (
    <section className="source-notes" aria-labelledby="source-notes-heading">
      <h3 id="source-notes-heading" className="section-title">
        Notes from this source
      </h3>
      {notes.length === 0 ? (
        <p className="field-hint">
          No notes written from it yet. Make a note to study it: cards and quizzes from that note
          then say which page they came from.
        </p>
      ) : (
        <ul className="card-list">
          {notes.map((note) => {
            const titleId = `source-note-${note.id}-title`;
            return (
              <li key={note.id}>
                <article className="card-item note-item" aria-labelledby={titleId}>
                  <div className="note-item-main">
                    <span className="section-icon section-icon-quiet" aria-hidden="true">
                      <NoteIcon />
                    </span>
                    <div>
                      <h4 id={titleId} className="note-item-title">
                        {note.title}
                      </h4>
                      <p className="item-meta">
                        {note.updatedAt === note.createdAt
                          ? `Created ${formatDateTime(note.createdAt)}`
                          : `Edited ${formatDateTime(note.updatedAt)}`}
                      </p>
                    </div>
                  </div>
                  <button
                    type="button"
                    className="button button-small"
                    aria-describedby={titleId}
                    onClick={() => onOpenNote(note.id)}
                  >
                    Open note
                    <ArrowRightIcon />
                  </button>
                </article>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/**
 * Reading a PDF's scanned pages: the offer to read every page left, the
 * progress of that run, and how it went. Nothing when there's nothing to say.
 */
function ScannedPanel({
  scanned,
  readWith,
  toRead,
  progress,
  summary,
  error,
  busy,
  onRead,
  onStop,
}: {
  /** How many pages have no text of their own. */
  scanned: number;
  readWith: ReaderState;
  /** How many pages are left to read, by the reader chosen. */
  toRead: number;
  progress: (Reading & { status: "reading" }) | null;
  summary: string | null;
  error: string | null;
  /** Whether any run is going, here or on one page. */
  busy: boolean;
  onRead: () => void;
  onStop: () => void;
}) {
  if (scanned === 0 && toRead === 0 && !progress && !summary && !error) return null;

  return (
    <section className="scanned-pages" aria-labelledby="scanned-pages-heading">
      <h3 id="scanned-pages-heading" className="section-title">
        Scanned pages
      </h3>
      {progress ? (
        <ReadingProgress reading={progress} single={null} onStop={onStop} />
      ) : (
        <>
          {summary && <ReadingSummary key={summary} text={summary} />}
          {error && (
            <p className="message-error" role="alert">
              {error}
            </p>
          )}
          {toRead === 0 && scanned > 0 && (
            <p className="field-hint">
              {`Every page without text has been read by the reader chosen. To read ${
                scanned === 1 ? "it" : "them"
              } again with another, such as a vision model in Ollama for accents and handwriting, choose it below.`}
            </p>
          )}
          {scanned > 0 && <ReadWith id="scanned-read-with" state={readWith} disabled={busy} />}
          {toRead > 0 && (
            <>
              <p className="field-hint">{scannedHint(toRead)}</p>
              <div className="deck-actions">
                <button
                  type="button"
                  className="button"
                  aria-disabled={busy}
                  onClick={() => {
                    if (!busy) onRead();
                  }}
                >
                  {`Read scanned pages (${toRead})`}
                </button>
              </div>
            </>
          )}
        </>
      )}
    </section>
  );
}

/** How a run went, announced and focused once it ends. */
function ReadingSummary({ text }: { text: string }) {
  const ref = useFocusOnMount<HTMLParagraphElement>(true);
  return (
    <p ref={ref} className="notice" role="status" tabIndex={-1}>
      {text}
    </p>
  );
}

/** A run reading one page, started from the page viewer. */
type PageRun = {
  page: number;
  summary: string | null;
  error: string | null;
  progress: (Reading & { status: "reading" }) | null;
};

/**
 * A source's pages, one at a time: the text extraction read from each, as it
 * was, with the pages before and after a press away. A PDF page without text
 * shows the text recognised on it, labelled as such, and can be read (again).
 */
function PageViewer({
  source,
  pages,
  found,
  reading,
  pageRun,
  readWith,
  onRead,
  onStop,
}: {
  source: Source;
  pages: SourcePage[];
  /** Opened from a search: start at its page, with its words marked on every page. */
  found: FoundAt | null;
  readWith: ReaderState;
  /** Whether a run is going, so another can't start. */
  reading: boolean;
  pageRun: PageRun | null;
  onRead: (page: number) => void;
  onStop: () => void;
}) {
  const [index, setIndex] = useState(() =>
    Math.max(0, pages.findIndex((each) => each.number === found?.page)),
  );
  const [marking, setMarking] = useState(found !== null);
  const textRef = useRef<HTMLDivElement>(null);
  const slides = source.kind === "powerPoint";
  const thing = slides ? "Slide" : "Page";
  const page = pages[index];
  const paged = pages.length > 1;
  const marks = useSearchedPage(
    source.id,
    page?.number ?? null,
    marking && found ? found.query : null,
    page ? pageText(page) : "",
  );

  function go(next: number) {
    if (next < 0 || next >= pages.length) return;
    setIndex(next);
    textRef.current?.scrollTo?.({ top: 0 });
  }

  if (!page) return null;

  const blank = page.text.trim() === "";
  const recognised = blank ? page.recognised : null;
  const readable = source.kind === "pdf" && blank;
  const thisRun = pageRun?.page === page.number ? pageRun : null;

  return (
    <section className="source-pages" aria-labelledby="source-page-heading">
      <div className="source-pages-bar">
        <h3 id="source-page-heading" className="section-title">
          {paged ? `${thing} ${page.number} of ${pages.length}` : "Text"}
        </h3>
        {paged && (
          <div className="deck-actions">
            <button
              type="button"
              className="button button-small"
              aria-disabled={index === 0}
              onClick={() => go(index - 1)}
            >
              <ArrowLeftIcon />
              {`Previous ${thing.toLowerCase()}`}
            </button>
            <span className="source-page-jump">
              <select
                className="field-input"
                aria-label={`Go to ${thing.toLowerCase()}`}
                value={index}
                onChange={(event) => go(Number(event.target.value))}
              >
                {pages.map((each, at) => (
                  <option key={each.number} value={at}>
                    {`${thing} ${each.number}`}
                  </option>
                ))}
              </select>
            </span>
            <button
              type="button"
              className="button button-small"
              aria-disabled={index === pages.length - 1}
              onClick={() => go(index + 1)}
            >
              {`Next ${thing.toLowerCase()}`}
              <ArrowRightIcon />
            </button>
          </div>
        )}
      </div>

      {marking && found && (
        <div className="source-search-marks">
          <p className="field-hint">{`Words from your search “${found.query}” are marked.`}</p>
          <button type="button" className="button button-small" onClick={() => setMarking(false)}>
            Hide marks
          </button>
        </div>
      )}

      {thisRun?.progress ? (
        <ReadingProgress reading={thisRun.progress} single={page.number} onStop={onStop} />
      ) : (
        <>
          {thisRun?.summary && <ReadingSummary key={thisRun.summary} text={thisRun.summary} />}
          {thisRun?.error && (
            <p className="message-error" role="alert">
              {thisRun.error}
            </p>
          )}
          {recognised && <RecognisedLabel recognised={recognised} />}
        </>
      )}

      {/* Focusable so keyboard users can scroll a long page. */}
      <div
        ref={textRef}
        className="source-page-scroll"
        tabIndex={0}
        aria-labelledby="source-page-heading"
      >
        {!blank ? (
          <p className="note-body source-page-text">
            {marks ? <MarkedText pieces={marks} /> : page.text}
          </p>
        ) : recognised && recognised.text.trim() !== "" ? (
          <p className="note-body source-page-text recognised-text">
            {marks ? <MarkedText pieces={marks} /> : recognised.text}
          </p>
        ) : recognised ? (
          <p className="empty">
            {`Synapse read this page's picture but found no text in it. Open the original to see it.`}
          </p>
        ) : (
          <p className="empty">
            {readable
              ? "Synapse found no text on this page. It may be a scanned image: read it to recognise its text, or open the original to see it."
              : `Synapse found no text on this ${thing.toLowerCase()}. It may be a scanned image; open the original to see it.`}
          </p>
        )}
      </div>

      {readable && !thisRun?.progress && (
        <div className="deck-actions read-page-actions">
          <ReadWith id="page-read-with" state={readWith} disabled={reading} brief />
          <button
            type="button"
            className="button button-small"
            aria-disabled={reading}
            onClick={() => {
              if (!reading) onRead(page.number);
            }}
          >
            {recognised ? "Read this page again" : "Read this page"}
          </button>
        </div>
      )}
    </section>
  );
}

/**
 * Page `number` of a source with the words of `query` marked, asked of Rust
 * (which matches them as the search did). Null while there's nothing to mark,
 * while it's being asked, or if asking failed: the page then shows unmarked.
 * `text` is the page's text now, so marks are asked again once it's read.
 */
function useSearchedPage(
  sourceId: number,
  number: number | null,
  query: string | null,
  text: string,
): SnippetPiece[] | null {
  const [marked, setMarked] = useState<{ key: string; pieces: SnippetPiece[] } | null>(null);
  const key = `${number}\u0000${query}\u0000${text}`;

  useEffect(() => {
    if (number === null || query === null) return;
    let current = true;
    getSearchedPage(sourceId, number, query).then(
      (pieces) => {
        if (current) setMarked({ key, pieces });
      },
      () => {}, // Unmarked text is still the page.
    );
    return () => {
      current = false;
    };
  }, [sourceId, key]);

  if (query === null || marked?.key !== key) return null;
  // Pieces that no longer add up to the page are from before a change.
  return marked.pieces.map((piece) => piece.text).join("") === text ? marked.pieces : null;
}

/** A page's text with the words found marked. Never read as HTML. */
function MarkedText({ pieces }: { pieces: SnippetPiece[] }) {
  return (
    <>
      {pieces.map((piece, index) =>
        piece.hit ? (
          <mark key={index} className="search-mark">
            {piece.text}
          </mark>
        ) : (
          piece.text
        ),
      )}
    </>
  );
}

/** What a scan's import says first; focus goes to it, so it's read out first. */
function ScanIntro({ children }: { children: ReactNode }) {
  const ref = useFocusOnMount<HTMLParagraphElement>(true);
  return (
    <p ref={ref} className="import-warning" tabIndex={-1}>
      {children}
    </p>
  );
}

/** The pages skipped so far, after `result`: those it skipped, less those it read. */
function withSkipped(skipped: number[], result: SourceRecognition): number[] {
  const now = new Set(skipped);
  for (const { page } of result.skipped) now.add(page);
  for (const page of result.read) now.delete(page);
  return [...now];
}

/** Where reading a just-imported scan has got to. */
type ScanStep =
  | { step: "checking" }
  | { step: "offer" }
  | { step: "after"; summary: string; draft: SourceNoteDraft; toRead: number[] }
  | { step: "error"; message: string };

/**
 * After *Import file* kept a PDF with no text at all: rather than open an
 * empty note, offers to read its scanned pages first. Once they're read the
 * note form opens with their text (`onRead`). The source is kept already,
 * whatever the user chooses here.
 */
export function ReadScannedImport({
  kept,
  onRead,
  onCancel,
}: {
  kept: SourceImport & { status: "imported" };
  /** Opens the new-note form with `draft`. */
  onRead: (draft: SourceNoteDraft) => void;
  onCancel: () => void;
}) {
  const reader = useSourceReading();
  const readWith = useReader();
  const [scan, setScan] = useState<ScanStep>({ step: "checking" });
  const pageCount = kept.text.pagesWithoutText.length;
  const left = scan.step === "after" ? scan.toRead : kept.text.pagesWithoutText;
  // As on the source screen: pages skipped once aren't offered again.
  const skippedRef = useRef<number[]>([]);
  const onReadRef = useRef(onRead);
  onReadRef.current = onRead;

  // Readings belong to the file, so the same scan imported again is read
  // already: its text goes straight to the note form.
  useEffect(() => {
    let current = true;
    loadSource(kept.source.id).then((next) => {
      if (!current) return;
      if (next.status === "ready" && next.toRead.length === 0) {
        const draft = { ...draftFromPages(next.source, next.pages), justKept: true };
        if (draft.body.trim() !== "") {
          onReadRef.current(draft);
          return;
        }
      }
      setScan({ step: "offer" });
    });
    return () => {
      current = false;
    };
  }, [kept.source.id]);

  async function read() {
    const result = await reader.start(kept.source.id, left, readWith.reader);
    if (result === null) {
      if (readWith.reader.engine === "ollama") void readWith.reload();
      return;
    }
    skippedRef.current = withSkipped(skippedRef.current, result);
    const next = await loadSource(kept.source.id, readWith.reader);
    if (next.status === "error") {
      setScan({ step: "error", message: next.message });
      return;
    }
    if (next.status !== "ready") return;
    const draft = { ...draftFromPages(next.source, next.pages), justKept: true };
    if (!result.stopped && draft.body.trim() !== "") {
      onRead(draft);
      return; // This screen is replaced.
    }
    setScan({
      step: "after",
      summary: readingSummary(result, next.pages),
      draft,
      toRead: next.toRead.filter((page) => !skippedRef.current.includes(page)),
    });
  }

  const reading = reader.reading.status === "reading" ? reader.reading : null;
  const found = scan.step === "after" && scan.draft.body.trim() !== "";

  if (scan.step === "checking") {
    return (
      <p className="loading" role="status">
        {`Opening ${kept.source.fileName}…`}
      </p>
    );
  }

  return (
    <section className="card" aria-labelledby="read-scan-heading">
      <h2 id="read-scan-heading" className="form-title">
        {`New note from ${kept.source.fileName}`}
      </h2>
      <ScanIntro>
        {`Synapse kept ${kept.source.fileName} under Sources, but none of its ${
          pageCount === 1 ? "page has" : `${pageCount} pages have`
        } text it could read: it looks like a scan. Synapse can read the text in the pages' pictures first, on this computer, so the note doesn't start empty. Recognised text can hold mistakes, and the built-in reader doesn't read accents (è comes out as e), so check it before you save.`}
      </ScanIntro>

      {reading ? (
        <ReadingProgress reading={reading} single={null} onStop={reader.stop} />
      ) : (
        <>
          {scan.step === "after" && <ReadingSummary key={scan.summary} text={scan.summary} />}
          {scan.step === "error" && (
            <p className="message-error" role="alert">
              {scan.message}
            </p>
          )}
          {reader.reading.status === "error" && (
            <p className="message-error" role="alert">
              {reader.reading.message}
            </p>
          )}
          {left.length > 0 && <ReadWith id="import-read-with" state={readWith} />}
          <div className="form-actions">
            {left.length > 0 && (
              <button type="button" className="button button-primary" onClick={read}>
                {scan.step === "after"
                  ? `Carry on reading (${left.length})`
                  : `Read scanned pages (${left.length})`}
              </button>
            )}
            <button
              type="button"
              className={left.length > 0 ? "button" : "button button-primary"}
              onClick={() =>
                onRead(found && scan.step === "after" ? scan.draft : draftFromImport(kept))
              }
            >
              {found ? "Write the note with what was read" : "Write the note myself"}
            </button>
            <button type="button" className="button" onClick={onCancel}>
              Cancel
            </button>
          </div>
        </>
      )}
    </section>
  );
}
