import { useEffect, useRef, useState } from "react";
import {
  deleteSource,
  getDeletedSources,
  getSource,
  getSourceNotes,
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
  type Source,
  type SourceImport,
  type SourcePage,
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
import { KIND_NAMES } from "./SourcePages";
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
    justKept: true,
  };
}

/**
 * A note from a source kept earlier: its pages, in order, as importing it
 * gave them (a PDF's pages and a presentation's slides already say where one
 * ends), under the source's title.
 */
export function draftFromPages(source: Source, pages: SourcePage[]): SourceNoteDraft {
  const paged = source.kind === "pdf" || source.kind === "powerPoint";
  return {
    sourceId: source.id,
    fileName: source.fileName,
    kind: source.kind,
    title: source.title,
    body: pages
      .map((page) => page.text.trim())
      .filter((text) => text !== "")
      .join("\n\n"),
    pages: paged ? source.pageCount : null,
    pagesWithoutText:
      source.kind === "pdf"
        ? pages.filter((page) => page.text.trim() === "").map((page) => page.number)
        : [],
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
type LibraryFocus = { to: "screen" } | { to: "add" } | { to: "source"; sourceId: number } | { to: "notice" };

/** Where in the sources area the user is. */
type Place =
  | { screen: "library"; focus: LibraryFocus; notice: string | null }
  | { screen: "source"; sourceId: number; notice: string | null };

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
      : { screen: "library", focus: { to: "screen" }, notice: null }
  );

  if (place.screen === "source") {
    const sourceId = place.sourceId;
    return (
      <SourceScreen
        key={sourceId}
        sourceId={sourceId}
        notice={place.notice}
        onBack={() =>
          setPlace({ screen: "library", focus: { to: "source", sourceId }, notice: null })
        }
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
          notice: `Kept ${kept.source.fileName}. Its text is below, page by page, and the original file is kept as it was. Make a note from it to edit the text and study it.`,
        })
      }
      onOpen={(sourceId) => setPlace({ screen: "source", sourceId, notice: null })}
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
  onBack,
}: {
  focus: LibraryFocus;
  notice: string | null;
  onKept: (kept: SourceImport & { status: "imported" }) => void;
  onOpen: (sourceId: number) => void;
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
  onBack: () => void;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const focusSourceId =
    focus.to === "source" && sources.some((source) => source.id === focus.sourceId)
      ? focus.sourceId
      : null;
  const focusNotice = focus.to === "notice" && notice !== null;
  const screenRef = useFocusOnMount<HTMLElement>(
    focus.to === "screen" ||
      (focus.to === "source" && focusSourceId === null) ||
      (focus.to === "notice" && !focusNotice)
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

      {sources.length === 0 ? (
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

      {deleted.length > 0 && (
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
  | { status: "ready"; source: Source; pages: SourcePage[]; notes: NoteSummary[] };

/** Fetches a source, its pages, and the notes written from it; never rejects. */
async function loadSource(sourceId: number): Promise<SourceState> {
  try {
    const [source, pages, notes] = await Promise.all([
      getSource(sourceId),
      getSourcePages(sourceId),
      getSourceNotes(sourceId),
    ]);
    return { status: "ready", source, pages, notes };
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
  onBack,
  onDeleted,
  onOpenNote,
  onMakeNote,
}: {
  sourceId: number;
  notice: string | null;
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

  useEffect(() => {
    let current = true;
    loadSource(sourceId).then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [sourceId, attempt]);

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

  return (
    <section
      className="deck-detail note-detail source-detail"
      aria-labelledby="source-heading"
    >
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

        <SourceNotes notes={notes} onOpenNote={onOpenNote} />
        <PageViewer source={source} pages={pages} />
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
        `${source.title} deleted. It's kept in Deleted sources, with its file, where you can restore it.`
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
    { title: "source-title" }
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
 * A source's pages, one at a time: the text extraction read from each, as it
 * was, with the pages before and after a press away.
 */
function PageViewer({ source, pages }: { source: Source; pages: SourcePage[] }) {
  const [index, setIndex] = useState(0);
  const textRef = useRef<HTMLDivElement>(null);
  const slides = source.kind === "powerPoint";
  const thing = slides ? "Slide" : "Page";
  const page = pages[index];
  const paged = pages.length > 1;

  function go(next: number) {
    if (next < 0 || next >= pages.length) return;
    setIndex(next);
    textRef.current?.scrollTo?.({ top: 0 });
  }

  if (!page) return null;

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
      {/* Focusable so keyboard users can scroll a long page. */}
      <div
        ref={textRef}
        className="source-page-scroll"
        tabIndex={0}
        aria-labelledby="source-page-heading"
      >
        {page.text.trim() === "" ? (
          <p className="empty">
            {`Synapse found no text on this ${thing.toLowerCase()}. It may be a scanned image; open the original to see it.`}
          </p>
        ) : (
          <p className="note-body source-page-text">{page.text}</p>
        )}
      </div>
    </section>
  );
}
