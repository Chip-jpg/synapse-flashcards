import { useT, useLang } from "./i18n";
import { useEffect, useMemo, useRef, useState, type ClipboardEvent } from "react";
import {
  choosePicture,
  createFlashcard,
  createNote,
  createNoteFromSource,
  deleteNote,
  getDeletedNote,
  getDeletedNotes,
  getNote,
  getNoteSource,
  getNotes,
  getSource,
  importSource,
  restoreNote,
  searchNotes,
  storePicture,
  toCommandError,
  updateNote,
  NOTE_PICTURES_MAX,
  type CommandError,
  type Deck,
  type DeckDetail,
  type DeletedNote,
  type DeletedNoteText,
  type Asset,
  type Note,
  type NoteSummary,
  type Recognition,
  type SourceImport,
} from "./api";
import { loadDeckChoices, type DeckChoices } from "./deckChoices";
import { MarkdownView } from "./NoteMarkdown";
import { hasFormatting, parseMarkdown } from "./markdown";
import { FormActions, SelectField, TextField, useFormSave } from "./forms";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  NoteIcon,
  PencilIcon,
  PictureIcon,
  PlusIcon,
  RestoreIcon,
  SparkleIcon,
  TrashIcon,
  UploadIcon,
} from "./icons";
import { Quiz } from "./Quiz";
import { Tutor } from "./Tutor";
import { SuggestCards } from "./SuggestCards";
import { SUGGESTION_NOTE_MAX_CHARS } from "./noteParts";
import {
  draftFromImport,
  looksScanned,
  ReadScannedImport,
  type SourceNoteDraft,
} from "./Sources";
import { formatDateTime, Message, useFocusOnMount } from "./ui";
import { useUnsavedChanges } from "./unsaved";
import { appendText, usePictureReading } from "./pictureText";
import {
  bytesOf,
  PictureField,
  PictureGallery,
  pictureFiles,
  samePictures,
  usePictureAdder,
} from "./pictures";
import { hasTag, sameTags, TagChips, TagField, tagsIn } from "./tagInput";

/** What takes focus when the library appears. */
type LibraryFocus =
  | { to: "screen" }
  | { to: "new" }
  | { to: "import" }
  | { to: "photo" }
  | { to: "note"; noteId: number }
  | { to: "deleted"; noteId: number }
  | { to: "notice" };

/** A file *Import file* just kept as a source. */
type KeptFile = SourceImport & { status: "imported" };

/** Where in the notes area the user is. */
type Place =
  | { screen: "library"; focus: LibraryFocus; notice: string | null }
  | { screen: "new"; imported?: SourceNoteDraft; photo?: PhotoDraft }
  | { screen: "scanned"; kept: KeptFile }
  | { screen: "note"; noteId: number; justCreated: boolean }
  | { screen: "deleted"; noteId: number };

/**
 * The notes area: the library of the user's typed notes, writing a new one or
 * importing one from a file, opening one to read or edit it, and writing a
 * card from it. Every screen change follows a button press, so each new
 * screen takes focus.
 */
export function NotesArea({
  onBack,
  onOpenDeck,
  onOpenSource,
  openNote,
  newFromSource,
}: {
  onBack: () => void;
  /** Leaves notes for the deck screen, e.g. to see a card just written. */
  onOpenDeck: (deckId: number) => void;
  /** Leaves notes for the source a note was written from. */
  onOpenSource: (sourceId: number) => void;
  /**
   * Opens straight at one note, e.g. from a search, and goes back there
   * instead of to the library.
   */
  openNote?: { noteId: number; backLabel: string; onBack: () => void };
  /**
   * Opens straight at a new note written from a source, and goes back there
   * if it's cancelled.
   */
  newFromSource?: { draft: SourceNoteDraft; onCancel: () => void };
}) {
  const { t } = useT();
  const [place, setPlace] = useState<Place>(
    newFromSource
      ? { screen: "new", imported: newFromSource.draft }
      : openNote
        ? { screen: "note", noteId: openNote.noteId, justCreated: false }
        : { screen: "library", focus: { to: "screen" }, notice: null },
  );

  if (place.screen === "new") {
    const imported = place.imported;
    const photo = place.photo;
    return (
      <NoteForm
        imported={imported}
        photo={photo}
        onCancel={() =>
          newFromSource && imported === newFromSource.draft
            ? newFromSource.onCancel()
            : setPlace({
                screen: "library",
                focus: { to: imported ? "import" : photo ? "photo" : "new" },
                notice: null,
              })
        }
        onSaved={(note) => setPlace({ screen: "note", noteId: note.id, justCreated: true })}
      />
    );
  }

  if (place.screen === "scanned") {
    return (
      <ReadScannedImport
        kept={place.kept}
        onRead={(imported) => setPlace({ screen: "new", imported })}
        onCancel={() => setPlace({ screen: "library", focus: { to: "import" }, notice: null })}
      />
    );
  }

  if (place.screen === "note") {
    const noteId = place.noteId;
    return (
      <NoteScreen
        key={noteId}
        noteId={noteId}
        justCreated={place.justCreated}
        backLabel={openNote?.noteId === noteId ? openNote.backLabel : t("notes.backNotes")}
        onBack={() =>
          openNote?.noteId === noteId
            ? openNote.onBack()
            : setPlace({ screen: "library", focus: { to: "note", noteId }, notice: null })
        }
        // The note is gone from the library, so the screen it was opened from
        // takes over and says what happened.
        onDeleted={() =>
          setPlace({
            screen: "library",
            focus: { to: "notice" },
            notice: t("notes.deletedNotice"),
          })
        }
        onOpenDeck={onOpenDeck}
        onOpenSource={onOpenSource}
      />
    );
  }

  if (place.screen === "deleted") {
    const noteId = place.noteId;
    return (
      <DeletedNoteScreen
        key={noteId}
        noteId={noteId}
        onBack={() => setPlace({ screen: "library", focus: { to: "deleted", noteId }, notice: null })}
        // The library takes over and says what happened, as when restoring
        // from the list.
        onRestored={(title) =>
          setPlace({
            screen: "library",
            focus: { to: "notice" },
            notice: t("notes.restored", { title }),
          })
        }
        onStale={() =>
          setPlace({
            screen: "library",
            focus: { to: "notice" },
            notice: t("notes.stale"),
          })
        }
      />
    );
  }

  return (
    <NoteLibrary
      focus={place.focus}
      notice={place.notice}
      onNew={() => setPlace({ screen: "new" })}
      onImported={(kept) =>
        setPlace(
          looksScanned(kept)
            ? { screen: "scanned", kept }
            : { screen: "new", imported: draftFromImport(kept) }
        )
      }
      onPhoto={(photo) => setPlace({ screen: "new", photo })}
      onOpen={(noteId) => setPlace({ screen: "note", noteId, justCreated: false })}
      onRead={(noteId) => setPlace({ screen: "deleted", noteId })}
      onBack={onBack}
    />
  );
}

type LibraryState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; notes: NoteSummary[]; deleted: DeletedNote[] };

/**
 * How the library appears next. `key` changes every time, so the list
 * remounts and its focus target takes effect (as after restoring a note).
 */
type LibraryView = { key: number; notice: string | null; focus: LibraryFocus };

/** Fetches the library and its deleted-notes history; never rejects. */
async function loadNotes(): Promise<LibraryState> {
  try {
    const [notes, deleted] = await Promise.all([getNotes(), getDeletedNotes()]);
    return { status: "ready", notes, deleted };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

function NoteLibrary({
  focus,
  notice,
  onNew,
  onImported,
  onPhoto,
  onOpen,
  onRead,
  onBack,
}: {
  focus: LibraryFocus;
  /** A confirmation to announce when the library appears (e.g. after deleting). */
  notice: string | null;
  onNew: () => void;
  /** A file was kept as a source and its text read, to become a new note. */
  onImported: (kept: KeptFile) => void;
  /** A photo was kept and its text read, to become a new note. */
  onPhoto: (photo: PhotoDraft) => void;
  onOpen: (noteId: number) => void;
  /** Opens a deleted note to read it. */
  onRead: (noteId: number) => void;
  onBack: () => void;
}) {
  const { t } = useT();
  const [state, setState] = useState<LibraryState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or after a restore).
  const [attempt, setAttempt] = useState(0);
  const [view, setView] = useState<LibraryView>({ key: 0, notice, focus });

  useEffect(() => {
    let current = true;
    loadNotes().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  /**
   * Reloads both lists, announcing `notice` and giving it focus if there is
   * one. Goes back to "loading" first: until the fresh lists arrive the old
   * ones are wrong, and showing a just-restored note under *Deleted notes*
   * with a live Restore button would invite a second, doomed restore.
   */
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
        {t("notes.loading")}
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <div className="actions">
          <button type="button" className="button" onClick={() => reload(null)}>
            {t("settings.retry")}
          </button>
          <button type="button" className="button" onClick={onBack}>
            {t("review.backDesk")}
          </button>
        </div>
      </Message>
    );
  }

  return (
    <NoteList
      key={view.key}
      notes={state.notes}
      deleted={state.deleted}
      notice={view.notice}
      focus={view.focus}
      onNew={onNew}
      onImported={onImported}
      onPhoto={onPhoto}
      onOpen={onOpen}
      onRead={onRead}
      onBack={onBack}
      onRestored={(title) => reload(t("notes.restored", { title }))}
      onStale={() =>
        // Restoring failed because the note isn't deleted after all. Say so,
        // rather than reloading silently and leaving the press unexplained.
        reload(t("notes.stale"))
      }
    />
  );
}

function NoteList({
  notes,
  deleted,
  notice,
  focus,
  onNew,
  onImported,
  onPhoto,
  onOpen,
  onRead,
  onBack,
  onRestored,
  onStale,
}: {
  notes: NoteSummary[];
  deleted: DeletedNote[];
  notice: string | null;
  focus: LibraryFocus;
  onNew: () => void;
  onImported: (kept: KeptFile) => void;
  onPhoto: (photo: PhotoDraft) => void;
  onOpen: (noteId: number) => void;
  onRead: (noteId: number) => void;
  onBack: () => void;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const { t } = useT();
  // A note that's gone from its list (it was deleted, or restored) falls back
  // to the screen, and so does a notice that isn't there.
  const focusNoteId =
    focus.to === "note" && notes.some((note) => note.id === focus.noteId) ? focus.noteId : null;
  const focusDeletedId =
    focus.to === "deleted" && deleted.some((note) => note.id === focus.noteId)
      ? focus.noteId
      : null;
  const focusNotice = focus.to === "notice" && notice !== null;
  const screenRef = useFocusOnMount<HTMLElement>(
    focus.to === "screen" ||
      (focus.to === "note" && focusNoteId === null) ||
      (focus.to === "deleted" && focusDeletedId === null) ||
      (focus.to === "notice" && !focusNotice)
  );
  const newRef = useFocusOnMount<HTMLButtonElement>(focus.to === "new");
  const importRef = useFocusOnMount<HTMLButtonElement>(focus.to === "import");
  const photoRef = useFocusOnMount<HTMLButtonElement>(focus.to === "photo");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focusNotice);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  // Blocks a second file window immediately, before the re-render lands.
  const importingRef = useRef(false);
  const [photoStep, setPhotoStep] = useState<"none" | "choosing" | "keeping" | "reading">(
    "none",
  );
  const [photoError, setPhotoError] = useState<string | null>(null);
  const photoReading = usePictureReading();
  const search = useNoteSearch();
  const searching = search.query.trim() !== "";
  // A filter on what's shown, not a form: nothing is saved.
  const [tag, setTag] = useState<string | null>(null);
  const noteTags = useMemo(() => tagsIn(notes), [notes]);
  // While a new answer is on its way, the last one stays up, so the list
  // doesn't flash empty between keystrokes.
  const matching = searching ? (search.found?.notes ?? notes) : notes;
  const shown = tag === null ? matching : matching.filter((note) => hasTag(note.tags, tag));

  /**
   * Chooses a file, keeps it as a source and reads it; its text opens as a
   * new note to check.
   */
  async function chooseFile() {
    if (importingRef.current) return;
    importingRef.current = true;
    setImporting(true);
    setImportError(null);

    try {
      const result = await importSource();
      if (result.status === "imported") {
        onImported(result);
        return; // This screen is replaced.
      }
    } catch (err) {
      setImportError(toCommandError(err).message);
    }

    importingRef.current = false;
    setImporting(false);
  }

  /**
   * Keeps a photo (chosen, or pasted) and reads its text; the text opens as a
   * new note with the photo attached. A photo that couldn't be read still
   * opens, with the note left for the user to write. `keep` resolves with
   * null when no photo was chosen.
   */
  async function notePhoto(step: "choosing" | "keeping", keep: () => Promise<Asset | null>) {
    if (importingRef.current) return;
    importingRef.current = true;
    setPhotoStep(step);
    setPhotoError(null);

    try {
      const kept = await keep();
      if (kept) {
        setPhotoStep("reading");
        const read = await photoReading.read(kept.id);
        if (read === null) return; // Left the library meanwhile.
        onPhoto({
          picture: kept,
          recognition: read.status === "read" ? read.recognition : null,
          problem: read.status === "error" ? read.message : null,
        });
        return; // This screen is replaced.
      }
    } catch (err) {
      setPhotoError(toCommandError(err).message);
    }

    importingRef.current = false;
    setPhotoStep("none");
  }

  function chooseNotePhoto() {
    void notePhoto("choosing", async () => {
      const chosen = await choosePicture();
      return chosen.status === "added" ? chosen.asset : null;
    });
  }

  /**
   * A picture pasted on the library, outside its search field, becomes a
   * note from a photo, as *Note from a photo* would. Only the first picture
   * is taken: a note from a photo starts from one.
   */
  function pasteNotePhoto(event: ClipboardEvent) {
    const target = event.target;
    if (
      target instanceof HTMLElement &&
      (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)
    )
      return;
    const [file] = pictureFiles(event.clipboardData);
    if (!file) return;
    event.preventDefault();
    void notePhoto("keeping", async () => storePicture(await bytesOf(file)));
  }

  return (
    <section
      ref={screenRef}
      className="notes"
      tabIndex={-1}
      aria-labelledby="notes-heading"
      onPaste={pasteNotePhoto}
    >
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        {t("review.backDesk")}
      </button>

      <div className="page-header">
        <div className="page-heading">
          <h2 id="notes-heading" className="page-title">
            {t("nav.notes")}
          </h2>
          <p className="page-intro">
            {t("notes.intro")}
          </p>
        </div>
        <div className="deck-actions page-actions">
          <button
            ref={importRef}
            type="button"
            className="button"
            aria-disabled={importing}
            aria-describedby="import-hint"
            onClick={chooseFile}
          >
            <UploadIcon />
            {t("notes.import")}
          </button>
          <button
            ref={photoRef}
            type="button"
            className="button"
            aria-disabled={importing || photoStep !== "none"}
            aria-describedby="import-hint"
            onClick={chooseNotePhoto}
          >
            <PictureIcon />
            {t("notes.fromPhoto")}
          </button>
          <button ref={newRef} type="button" className="button button-primary" onClick={onNew}>
            <PlusIcon />
            {t("notes.new")}
          </button>
        </div>
      </div>
      <p id="import-hint" className="field-hint">
        {t("notes.importHint")}
      </p>
      <div className="picture-text-row">
        <p className="form-status" role="status">
          {importing
            ? t("notes.choosingFile")
            : photoStep === "choosing"
              ? t("notes.choosingPhoto")
              : photoStep === "keeping"
                ? t("notes.keepingPhoto")
                : photoStep === "reading"
                  ? t("notes.readingPhoto")
                  : ""}
        </p>
        {photoStep === "reading" && (
          <button type="button" className="button button-small" onClick={photoReading.stop}>
            {t("settings.stop")}
          </button>
        )}
      </div>
      {importError && (
        <p className="message-error" role="alert">
          {importError}
        </p>
      )}
      {photoError && (
        <p className="message-error" role="alert">
          {photoError}
        </p>
      )}
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {notes.length > 0 && <NoteSearchField search={search} />}
      {noteTags.length > 0 && (
        <NoteTagFilter
          tags={noteTags}
          tag={tag}
          shown={shown.length}
          onChange={setTag}
        />
      )}

      {notes.length === 0 ? (
        <p className="empty" role="status">
          {deleted.length === 0
            ? t("notes.empty")
            : t("notes.emptyDeleted")}
        </p>
      ) : searching && search.found?.query === search.query.trim() && shown.length === 0 ? (
        <p className="empty">
          {tag === null
            ? t("notes.noQuery", { query: search.found.query })
            : t("notes.noTaggedQuery", { tag, query: search.found.query })}
        </p>
      ) : !searching && shown.length === 0 ? (
        <p className="empty">{t("notes.noTagged", { tag: tag ?? "" })}</p>
      ) : (
        <ul className="card-list">
          {shown.map((note) => (
            <li key={note.id}>
              <NoteItem
                note={note}
                focusOpen={note.id === focusNoteId}
                onOpen={() => onOpen(note.id)}
              />
            </li>
          ))}
        </ul>
      )}

      {deleted.length > 0 && (
        <DeletedNoteList
          notes={deleted}
          focusReadId={focusDeletedId}
          onRead={onRead}
          onRestored={onRestored}
          onStale={onStale}
        />
      )}
    </section>
  );
}

/**
 * Narrows the library to the notes with one tag (or a tag below it, as
 * `bio::cells` is below `bio`). Done here: the library already has every
 * note's tags. Combines with *Search notes*.
 */
function NoteTagFilter({
  tags,
  tag,
  shown,
  onChange,
}: {
  tags: { name: string; count: number }[];
  tag: string | null;
  /** How many notes are listed now. */
  shown: number;
  onChange: (tag: string | null) => void;
}) {
  const { t, plural } = useT();
  return (
    <div className="field note-tag-filter">
      <label htmlFor="note-tag-filter" className="field-label">
        {t("search.tag")}
      </label>
      <select
        id="note-tag-filter"
        className="field-input"
        value={tag ?? ""}
        onChange={(event) => onChange(event.target.value === "" ? null : event.target.value)}
      >
        <option value="">{t("notes.everyTag")}</option>
        {tags.map((entry) => (
          <option key={entry.name} value={entry.name}>
            {`${entry.name} (${entry.count})`}
          </option>
        ))}
      </select>
      <p className="form-status" role="status">
        {tag === null ? "" : plural(shown, "notes.shownOne", "notes.shownOther")}
      </p>
    </div>
  );
}

/** The last search Rust answered: the trimmed query and the notes it found. */
type FoundNotes = { query: string; notes: NoteSummary[] };

type NoteSearch = {
  /** What's in the field, as typed. */
  query: string;
  found: FoundNotes | null;
  error: string | null;
  setQuery: (query: string) => void;
};

/**
 * Searching the library. Rust does the matching, since the library list has
 * no bodies. Each change asks again; an answer to an older query is dropped,
 * so a slow reply can't overwrite a newer one. This is a filter, not a form:
 * nothing is saved, so it never counts as an unsaved change.
 */
function useNoteSearch(): NoteSearch {
  const [query, setQueryState] = useState("");
  const [found, setFound] = useState<FoundNotes | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Numbers each request, so only the latest one's answer is shown.
  const latest = useRef(0);

  useEffect(
    () => () => {
      latest.current += 1; // Unmounted: ignore any answer still on its way.
    },
    [],
  );

  function setQuery(next: string) {
    setQueryState(next);
    setError(null);
    const request = ++latest.current;
    const trimmed = next.trim();
    if (trimmed === "") {
      setFound(null);
      return;
    }
    searchNotes(trimmed).then(
      (notes) => {
        if (request === latest.current) setFound({ query: trimmed, notes });
      },
      (err) => {
        if (request === latest.current) setError(toCommandError(err).message);
      },
    );
  }

  return { query, found, error, setQuery };
}

function NoteSearchField({ search }: { search: NoteSearch }) {
  const { t, plural } = useT();
  const { query, found, error, setQuery } = search;
  const trimmed = query.trim();
  const inputRef = useRef<HTMLInputElement>(null);

  let status = "";
  if (trimmed !== "" && error === null) {
    if (found?.query !== trimmed) status = t("search.searching");
    else if (found.notes.length === 0) status = t("notes.noMatches");
    else
      status = plural(found.notes.length, "notes.matchesOne", "notes.matchesOther");
  }

  return (
    <div className="field note-search">
      <label htmlFor="note-search" className="field-label">
        {t("notes.search")}
      </label>
      <div className="note-search-row">
        <input
          ref={inputRef}
          id="note-search"
          type="search"
          className="field-input"
          value={query}
          autoComplete="off"
          spellCheck={false}
          aria-describedby="note-search-hint"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape" && query !== "") {
              event.preventDefault();
              setQuery("");
            }
          }}
        />
        {query !== "" && (
          <button
            type="button"
            className="button"
            onClick={() => {
              setQuery("");
              inputRef.current?.focus();
            }}
          >
            {t("search.clear")}
          </button>
        )}
      </div>
      <p id="note-search-hint" className="field-hint">
        {t("notes.searchHint")}
      </p>
      <p className="form-status" role="status">
        {status}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

/**
 * Deleted notes: history, and the only place they appear. One can be opened
 * to read, but not edited; Restore puts one back in the library exactly as it
 * was.
 */
function DeletedNoteList({
  notes,
  focusReadId,
  onRead,
  onRestored,
  onStale,
}: {
  notes: DeletedNote[];
  /** The note whose *Read note* takes focus (coming back from reading it). */
  focusReadId: number | null;
  onRead: (noteId: number) => void;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const { t } = useT();
  return (
    <section className="cards section-quiet" aria-labelledby="deleted-notes-heading">
      <div className="section-heading">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          <RestoreIcon />
        </span>
        <h3 id="deleted-notes-heading" className="section-title section-title-quiet">
          {t("notes.deleted")}
        </h3>
      </div>
      <p className="field-hint">
        {t("notes.deletedHint")}
      </p>
      <ul className="card-list">
        {notes.map((note) => (
          <li key={note.id}>
            <DeletedNoteItem
              note={note}
              focusRead={note.id === focusReadId}
              onRead={() => onRead(note.id)}
              onRestored={onRestored}
              onStale={onStale}
            />
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * Restoring one deleted note, from the list or from reading it. A second
 * press while restoring sends nothing; "stale" means it wasn't deleted after
 * all, and `onStale` shows what's really there.
 */
function useRestoreNote(
  note: DeletedNote,
  onRestored: (title: string) => void,
  onStale: () => void
) {
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Blocks a second restore immediately, before the re-render lands.
  const restoringRef = useRef(false);

  async function restore() {
    if (restoringRef.current) return;
    restoringRef.current = true;
    setRestoring(true);
    setError(null);

    try {
      await restoreNote(note.id);
      onRestored(note.title);
      return; // This screen is replaced.
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

  return { restoring, error, restore };
}

/** One deleted note, with its two actions: Read and Restore. */
function DeletedNoteItem({
  note,
  focusRead,
  onRead,
  onRestored,
  onStale,
}: {
  note: DeletedNote;
  focusRead: boolean;
  onRead: () => void;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const { t } = useT();
  const { restoring, error, restore } = useRestoreNote(note, onRestored, onStale);
  const readRef = useFocusOnMount<HTMLButtonElement>(focusRead);
  const titleId = `deleted-note-${note.id}-title`;

  return (
    <div className="card-item card-item-recovery">
      <div>
        {/* A heading, like an active note's title, so screen-reader users can
            reach deleted notes by heading navigation. */}
        <h4 id={titleId} className="note-item-title">
          {note.title}
        </h4>
        <p className="item-meta">
          {t("notes.deletedEdited", {
            deleted: formatDateTime(note.deletedAt),
            edited: formatDateTime(note.updatedAt),
          })}
        </p>
      </div>
      <div className="deck-actions item-actions">
        <button
          ref={readRef}
          type="button"
          className="button button-small"
          aria-describedby={titleId}
          aria-disabled={restoring}
          onClick={() => {
            if (!restoring) onRead();
          }}
        >
          {t("notes.read")}
        </button>
        <button
          type="button"
          className="button button-small"
          aria-describedby={titleId}
          aria-disabled={restoring}
          onClick={restore}
        >
          <RestoreIcon />
          {t("notes.restore")}
        </button>
      </div>
      <p className="form-status" role="status">
        {restoring ? t("bulk.restoring") : ""}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function NoteItem({
  note,
  focusOpen,
  onOpen,
}: {
  note: NoteSummary;
  focusOpen: boolean;
  onOpen: () => void;
}) {
  const { t } = useT();
  const openRef = useFocusOnMount<HTMLButtonElement>(focusOpen);
  const titleId = `note-${note.id}-title`;

  return (
    <article className="card-item note-item" aria-labelledby={titleId}>
      <div className="note-item-main">
        <span className="section-icon section-icon-quiet" aria-hidden="true">
          <NoteIcon />
        </span>
        <div>
          {/* A heading, so screen-reader users can move between notes by heading. */}
          <h3 id={titleId} className="note-item-title">
            {note.title}
          </h3>
          <p className="item-meta">
            {note.updatedAt === note.createdAt
              ? t("notes.created", { date: formatDateTime(note.createdAt) })
              : t("notes.edited", { date: formatDateTime(note.updatedAt) })}
          </p>
          <TagChips tags={note.tags} />
        </div>
      </div>
      <button
        ref={openRef}
        type="button"
        className="button button-small"
        aria-describedby={titleId}
        onClick={onOpen}
      >
        {t("search.openNote")}
        <ArrowRightIcon />
      </button>
    </article>
  );
}

type NoteState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; note: Note };

/** What takes focus when the note view appears. */
type NoteFocus = "screen" | "notice" | "edit" | "card" | "suggest" | "quiz" | "tutor";

/**
 * How the note view appears next. `key` changes every time, so the view
 * remounts and its focus target takes effect. `cardDeck` is the deck cards
 * were just added to, offered as a way on.
 */
type NoteViewState = {
  key: number;
  notice: string | null;
  focus: NoteFocus;
  cardDeck: { id: number; name: string } | null;
};

/** Fetches one note and describes the outcome; never rejects. */
async function loadNote(noteId: number): Promise<NoteState> {
  try {
    return { status: "ready", note: await getNote(noteId) };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * One note: reading it, editing it in place, writing a card from it, and
 * having a local model suggest cards from it or quiz the user on it.
 */
function NoteScreen({
  noteId,
  justCreated,
  backLabel,
  onBack,
  onDeleted,
  onOpenDeck,
  onOpenSource,
}: {
  noteId: number;
  justCreated: boolean;
  backLabel: string;
  onBack: () => void;
  /** The note was deleted, so this screen can't show it any more. */
  onDeleted: () => void;
  onOpenDeck: (deckId: number) => void;
  onOpenSource: (sourceId: number) => void;
}) {
  const { t, plural } = useT();
  const [state, setState] = useState<NoteState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  // The note itself, or a form in its place.
  const [mode, setMode] = useState<"view" | "edit" | "card" | "suggest" | "quiz" | "tutor">("view");
  const [view, setView] = useState<NoteViewState>({
    key: 0,
    notice: justCreated ? t("notes.saved") : null,
    focus: justCreated ? "notice" : "screen",
    cardDeck: null,
  });

  useEffect(() => {
    let current = true;
    loadNote(noteId).then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [noteId, attempt]);

  function showView(
    notice: string | null,
    focus: NoteFocus,
    cardDeck: NoteViewState["cardDeck"] = null
  ) {
    setMode("view");
    setView((previous) => ({ key: previous.key + 1, notice, focus, cardDeck }));
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        {t("notes.loadingNote")}
      </p>
    );
  }

  // Every note screen follows a button press, so errors take focus.
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
            {t("settings.retry")}
          </button>
          <button type="button" className="button" onClick={onBack}>
            {backLabel}
          </button>
        </div>
      </Message>
    );
  }

  if (mode === "edit") {
    return (
      <NoteForm
        note={state.note}
        onCancel={() => showView(null, "edit")}
        onSaved={(note) => {
          setState({ status: "ready", note });
          showView(t("notes.updated"), "notice");
        }}
      />
    );
  }

  if (mode === "card") {
    return (
      <CardFromNote
        note={state.note}
        onCancel={() => showView(null, "card")}
        onCreated={(deck) =>
          showView(t("notes.cardAdded", { name: deck.name }), "notice", {
            id: deck.id,
            name: deck.name,
          })
        }
      />
    );
  }

  if (mode === "suggest") {
    return (
      <SuggestCards
        note={state.note}
        onCancel={() => showView(null, "suggest")}
        onSaved={(deck, count) =>
          showView(
            plural(count, "notes.cardsAddedOne", "notes.cardsAddedOther", { name: deck.name }),
            "notice",
            { id: deck.id, name: deck.name }
          )
        }
      />
    );
  }

  if (mode === "quiz") {
    return (
      <Quiz
        note={state.note}
        onCancel={() => showView(null, "quiz")}
        onSaved={(deck, count) =>
          showView(
            plural(count, "notes.cardsAddedOne", "notes.cardsAddedOther", { name: deck.name }),
            "notice",
            { id: deck.id, name: deck.name }
          )
        }
      />
    );
  }

  if (mode === "tutor") {
    return (
      <Tutor
        note={state.note}
        onCancel={() => showView(null, "tutor")}
        onSaved={(saved) =>
          showView(t("notes.summarySaved", { title: saved.title }), "notice")
        }
      />
    );
  }

  return (
    <NoteView
      key={view.key}
      note={state.note}
      notice={view.notice}
      focus={view.focus}
      cardDeck={view.cardDeck}
      onEdit={() => setMode("edit")}
      onCreateCard={() => setMode("card")}
      onSuggest={() => setMode("suggest")}
      onQuiz={() => setMode("quiz")}
      onTutor={() => setMode("tutor")}
      onDeleted={onDeleted}
      onOpenDeck={onOpenDeck}
      onOpenSource={onOpenSource}
      backLabel={backLabel}
      onBack={onBack}
    />
  );
}

function NoteView({
  note,
  notice,
  focus,
  cardDeck,
  onEdit,
  onCreateCard,
  onSuggest,
  onQuiz,
  onTutor,
  onDeleted,
  onOpenDeck,
  onOpenSource,
  backLabel,
  onBack,
}: {
  note: Note;
  notice: string | null;
  focus: NoteFocus;
  cardDeck: NoteViewState["cardDeck"];
  onEdit: () => void;
  onCreateCard: () => void;
  onSuggest: () => void;
  onQuiz: () => void;
  onTutor: () => void;
  onDeleted: () => void;
  onOpenDeck: (deckId: number) => void;
  onOpenSource: (sourceId: number) => void;
  backLabel: string;
  onBack: () => void;
}) {
  const { t } = useT();
  const screenRef = useFocusOnMount<HTMLElement>(focus === "screen");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus === "notice");
  const editRef = useFocusOnMount<HTMLButtonElement>(focus === "edit");
  const cardRef = useFocusOnMount<HTMLButtonElement>(focus === "card");
  const suggestRef = useFocusOnMount<HTMLButtonElement>(focus === "suggest");
  const quizRef = useFocusOnMount<HTMLButtonElement>(focus === "quiz");
  const tutorRef = useFocusOnMount<HTMLButtonElement>(focus === "tutor");
  const edited = note.updatedAt !== note.createdAt;
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  // Blocks a second delete immediately, before the re-render lands.
  const deletingRef = useRef(false);
  const deleteRef = useRef<HTMLButtonElement>(null);
  const questionRef = useRef<HTMLParagraphElement>(null);
  // Whether the question was ever opened, so only closing it moves focus.
  const openedRef = useRef(false);

  // As on the deck screen: focus moves to the question when it opens, so it
  // is read out and a repeated Enter can't delete, and back to t("notes.delete")
  // if the note is kept.
  useEffect(() => {
    if (confirming) {
      openedRef.current = true;
      questionRef.current?.focus();
    } else if (openedRef.current) {
      deleteRef.current?.focus();
    }
  }, [confirming]);

  async function confirmDelete() {
    if (deletingRef.current) return;
    deletingRef.current = true;
    setDeleting(true);
    setDeleteError(null);

    try {
      await deleteNote(note.id);
      onDeleted();
      return; // This screen is replaced.
    } catch (err) {
      const failure = toCommandError(err);
      if (failure.kind === "stale") {
        // Already deleted: the note is gone from this screen either way.
        onDeleted();
        return;
      }
      setDeleteError(failure.message);
    }

    deletingRef.current = false;
    setDeleting(false);
  }

  return (
    <section
      ref={screenRef}
      className="deck-detail note-detail"
      tabIndex={-1}
      aria-labelledby="note-heading"
    >
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        {backLabel}
      </button>

      <article className="card">
        <div>
          <h2 id="note-heading" className="page-title">
            {note.title}
          </h2>
          <p className="item-meta">
            {t("notes.created", { date: formatDateTime(note.createdAt) })}
            {edited && t("notes.editedSuffix", { date: formatDateTime(note.updatedAt) })}
          </p>
          <TagChips tags={note.tags} />
          <NoteSourceLine noteId={note.id} onOpenSource={onOpenSource} />
        </div>

        {notice && (
          <p ref={noticeRef} id="note-notice" className="notice" role="status" tabIndex={-1}>
            {notice}
          </p>
        )}
        {cardDeck && (
          <div className="deck-actions">
            <button
              type="button"
              className="button button-small"
              aria-describedby="note-notice"
              onClick={() => onOpenDeck(cardDeck.id)}
            >
              {t("notes.openDeck", { name: cardDeck.name })}
              <ArrowRightIcon />
            </button>
          </div>
        )}

        {/* The note's actions sit above its text, so they stay in reach
            however long the note is. */}
        {confirming ? (
          <div className="confirm">
            <p ref={questionRef} className="confirm-question" tabIndex={-1}>
              {t("notes.deleteConfirm")}
            </p>
            <div className="deck-actions">
              <button
                type="button"
                className="button button-danger-soft button-small"
                aria-describedby="note-heading"
                aria-disabled={deleting}
                onClick={confirmDelete}
              >
                {t("notes.delete")}
              </button>
              <button
                type="button"
                className="button button-small"
                aria-disabled={deleting}
                onClick={() => {
                  if (!deleting) setConfirming(false);
                }}
              >
                {t("notes.keep")}
              </button>
            </div>
            <p className="form-status" role="status">
              {deleting ? t("bulk.deleting") : ""}
            </p>
          </div>
        ) : (
          <div className="deck-actions note-toolbar">
            <button
              ref={editRef}
              type="button"
              className="button"
              aria-describedby="note-heading"
              onClick={onEdit}
            >
              <PencilIcon />
              {t("notes.edit")}
            </button>
            <button
              ref={cardRef}
              type="button"
              className="button button-primary"
              aria-describedby="note-heading"
              onClick={onCreateCard}
            >
              <PlusIcon />
              {t("notes.createCard")}
            </button>
            <button
              ref={suggestRef}
              type="button"
              className="button button-tonal"
              aria-describedby="note-heading"
              onClick={onSuggest}
            >
              <SparkleIcon />
              {t("notes.suggest")}
            </button>
            <button
              ref={quizRef}
              type="button"
              className="button button-tonal"
              aria-describedby="note-heading"
              onClick={onQuiz}
            >
              <SparkleIcon />
              {t("notes.quiz")}
            </button>
            <button
              ref={tutorRef}
              type="button"
              className="button button-tonal"
              aria-describedby="note-heading"
              onClick={onTutor}
            >
              <SparkleIcon />
              {t("notes.summarise")}
            </button>
            <button
              ref={deleteRef}
              type="button"
              className="button button-quiet"
              aria-describedby="note-heading"
              onClick={() => setConfirming(true)}
            >
              <TrashIcon />
              {t("notes.delete")}
            </button>
          </div>
        )}

        {deleteError && (
          <p className="message-error" role="alert">
            {deleteError}
          </p>
        )}

        <NoteReading body={note.body} />
        <PictureGallery
          pictures={note.pictures}
          label={t("notes.picture")}
          listLabel={t("notes.pictureList")}
          readText
        />
      </article>
    </section>
  );
}

type DeletedNoteState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; note: DeletedNoteText };

/**
 * A deleted note, opened from *Deleted notes* to read before deciding whether
 * to restore it. Reading only: its one action is *Restore note*. Editing,
 * cards, suggestions and quizzes wait until it's back in the library, and
 * Rust refuses them for a deleted note anyway.
 */
function DeletedNoteScreen({
  noteId,
  onBack,
  onRestored,
  onStale,
}: {
  noteId: number;
  onBack: () => void;
  onRestored: (title: string) => void;
  /** It wasn't deleted after all (restored meanwhile). */
  onStale: () => void;
}) {
  const { t } = useT();
  const [state, setState] = useState<DeletedNoteState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    getDeletedNote(noteId).then(
      (note) => {
        if (current) setState({ status: "ready", note });
      },
      (err) => {
        if (current) setState({ status: "error", message: toCommandError(err).message });
      }
    );
    return () => {
      current = false;
    };
  }, [noteId, attempt]);

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        {t("notes.loadingNote")}
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
            {t("settings.retry")}
          </button>
          <button type="button" className="button" onClick={onBack}>
            {t("notes.backNotes")}
          </button>
        </div>
      </Message>
    );
  }

  return (
    <DeletedNoteView
      note={state.note}
      onBack={onBack}
      onRestored={onRestored}
      onStale={onStale}
    />
  );
}

function DeletedNoteView({
  note,
  onBack,
  onRestored,
  onStale,
}: {
  note: DeletedNoteText;
  onBack: () => void;
  onRestored: (title: string) => void;
  onStale: () => void;
}) {
  const { t } = useT();
  const screenRef = useFocusOnMount<HTMLElement>(true);
  const { restoring, error, restore } = useRestoreNote(note, onRestored, onStale);
  const edited = note.updatedAt !== note.createdAt;

  return (
    <section
      ref={screenRef}
      className="deck-detail note-detail"
      tabIndex={-1}
      aria-labelledby="note-heading"
    >
      <button type="button" className="button button-quiet back-button" onClick={onBack}>
        <ArrowLeftIcon />
        {t("notes.backNotes")}
      </button>

      <article className="card">
        <div>
          <h2 id="note-heading" className="page-title">
            {note.title}
          </h2>
          <p className="item-meta">
            {t("notes.deletedCreated", {
              deleted: formatDateTime(note.deletedAt),
              created: formatDateTime(note.createdAt),
            })}
            {edited && t("notes.lastEditedSuffix", { date: formatDateTime(note.updatedAt) })}
          </p>
          <TagChips tags={note.tags} />
        </div>

        <p id="deleted-note-hint" className="field-hint">
          {t("notes.deletedReadHint")}
        </p>
        <div className="deck-actions note-toolbar">
          <button
            type="button"
            className="button button-primary"
            aria-describedby="note-heading deleted-note-hint"
            aria-disabled={restoring}
            onClick={restore}
          >
            <RestoreIcon />
            {t("notes.restore")}
          </button>
        </div>
        <p className="form-status" role="status">
          {restoring ? t("bulk.restoring") : ""}
        </p>
        {error && (
          <p className="message-error" role="alert">
            {error}
          </p>
        )}

        <NoteReading body={note.body} />
        {/* Reading a picture's text saves nothing, so a deleted note offers it too. */}
        <PictureGallery
          pictures={note.pictures}
          label={t("notes.picture")}
          listLabel={t("notes.pictureList")}
          readText
        />
      </article>
    </section>
  );
}

/**
 * Which source a note was written from, with the way to it: nothing for a
 * typed note, or while Rust looks, and a word about a source since deleted.
 */
function NoteSourceLine({
  noteId,
  onOpenSource,
}: {
  noteId: number;
  onOpenSource: (sourceId: number) => void;
}) {
  const { t } = useT();
  const [found, setFound] = useState<
    { status: "active"; sourceId: number; title: string } | { status: "deleted" } | null
  >(null);

  useEffect(() => {
    let current = true;
    (async () => {
      const sourceId = await getNoteSource(noteId);
      if (sourceId === null) return null;
      try {
        const source = await getSource(sourceId);
        return { status: "active" as const, sourceId, title: source.title };
      } catch (err) {
        if (toCommandError(err).kind === "stale") return { status: "deleted" as const };
        throw err;
      }
    })().then(
      (next) => {
        if (current) setFound(next);
      },
      () => {}
    );
    return () => {
      current = false;
    };
  }, [noteId]);

  if (found === null) return null;
  if (found.status === "deleted") {
    return (
      <p className="item-meta note-source">
        {t("notes.sourceDeleted")}
      </p>
    );
  }
  return (
    <p className="item-meta note-source">
      {t("notes.writtenFrom", { title: found.title })}
      <button
        type="button"
        className="button-link"
        onClick={() => onOpenSource(found.sourceId)}
      >
        {t("notes.openSource")}
      </button>
    </p>
  );
}

/** Where the way of showing notes last chosen is remembered, in this computer's web storage. */
const NOTE_FORMAT_KEY = "synapse.noteFormat";

// Remembering the choice is only a convenience: without storage, notes are
// shown formatted.
function rememberedFormatted(): boolean {
  try {
    return localStorage.getItem(NOTE_FORMAT_KEY) !== "plain";
  } catch {
    return true;
  }
}

function rememberFormatted(formatted: boolean) {
  try {
    localStorage.setItem(NOTE_FORMAT_KEY, formatted ? "formatted" : "plain");
  } catch {
    // As above.
  }
}

/**
 * A note's text, formatted as Markdown or as the plain text it's stored as.
 * The choice is only offered when formatting would change something; a note
 * with no Markdown in it is shown as plain text, as it was typed.
 */
function NoteReading({ body }: { body: string }) {
  const { t } = useT();
  // Parsed once, both to decide whether to offer formatting and to show it.
  const blocks = useMemo(() => parseMarkdown(body), [body]);
  const formattable = useMemo(() => hasFormatting(blocks), [blocks]);
  const [formatted, setFormatted] = useState(rememberedFormatted);

  if (!formattable) {
    return (
      <div className="note-reading">
        <p className="note-body">{body}</p>
      </div>
    );
  }

  function show(next: boolean) {
    setFormatted(next);
    rememberFormatted(next);
  }

  return (
    <div className="note-reading">
      <div className="note-view-switch" role="group" aria-label={t("notes.show")}>
        <button
          type="button"
          className="button button-small"
          aria-pressed={formatted}
          onClick={() => show(true)}
        >
          {t("notes.formatted")}
        </button>
        <button
          type="button"
          className="button button-small"
          aria-pressed={!formatted}
          onClick={() => show(false)}
        >
          {t("notes.plain")}
        </button>
      </div>
      {formatted ? <MarkdownView blocks={blocks} /> : <p className="note-body">{body}</p>}
    </div>
  );
}

/**
 * Writing a new note, editing `note` when one is given, or checking the text
 * of an `imported` file before it's saved as a new note. All validation
 * happens in Rust.
 */
function NoteForm({
  note,
  imported,
  photo,
  onCancel,
  onSaved,
}: {
  note?: Note;
  imported?: SourceNoteDraft;
  /** A new note from a photo: its text as read, and the photo attached. */
  photo?: PhotoDraft;
  onCancel: () => void;
  onSaved: (note: Note) => void;
}) {
  const { t } = useT();
  const [title, setTitle] = useState(imported?.title ?? note?.title ?? "");
  const [body, setBody] = useState(
    imported?.body ?? note?.body ?? photo?.recognition?.text.trim() ?? ""
  );
  const savedPictures = note?.pictures ?? [];
  const [pictures, setPictures] = useState<Asset[]>(photo ? [photo.picture] : savedPictures);
  const savedTags = note?.tags ?? [];
  const [tags, setTags] = useState<string[]>(savedTags);
  // Compared with what's saved: imported text, or a photo and its text, isn't
  // saved anywhere yet, so it counts as unsaved from the start.
  useUnsavedChanges(
    title !== (note?.title ?? "") ||
      body !== (note?.body ?? "") ||
      !samePictures(pictures, savedPictures) ||
      !sameTags(tags, savedTags)
  );
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => {
      const ids = pictures.map((picture) => picture.id);
      // Tags are sent only when there are some to save: left out, an edit
      // keeps the note's tags and a new note has none.
      const newTags = tags.length > 0 ? tags : undefined;
      return note
        ? updateNote(note.id, title, body, ids, sameTags(tags, savedTags) ? undefined : tags)
        : imported
          ? createNoteFromSource(imported.sourceId, title, body, ids, newTags)
          : createNote(title, body, ids, newTags);
    },
    onSaved,
    { title: "note-title", body: "note-body", pictures: "note-pictures", tags: "note-tags" }
  );
  const changePictures = (next: Asset[]) => {
    setPictures(next);
    edited("pictures");
  };
  const adder = usePictureAdder({
    pictures,
    onChange: changePictures,
    max: NOTE_PICTURES_MAX,
  });

  return (
    <form
      className="card"
      aria-labelledby="note-form-heading"
      noValidate
      onSubmit={submit}
      onPaste={adder.onPaste}
    >
      <h2 id="note-form-heading" className="form-title">
        {note
          ? t("notes.edit")
          : imported
            ? t("notes.newFrom", { name: imported.fileName })
            : photo
              ? t("notes.newPhoto")
              : t("notes.new")}
      </h2>
      {imported && <ImportSummary imported={imported} length={[...body].length} />}
      {photo && <PhotoSummary photo={photo} />}

      <TextField
        id="note-title"
        label={t("exams.title")}
        hint={t("notes.titleHint")}
        value={title}
        error={fieldError("title")}
        required
        autoFocus={!imported && !photo}
        onChange={(value) => {
          setTitle(value);
          edited("title");
        }}
      />
      <TextField
        id="note-body"
        label={t("notes.body")}
        hint={t("notes.bodyHint")}
        value={body}
        error={fieldError("body")}
        required
        multiline
        rows={12}
        onChange={(value) => {
          setBody(value);
          edited("body");
        }}
      />
      <PictureField
        id="note-pictures"
        label={t("notes.pictures")}
        hint={t("notes.picturesHint")}
        pictures={pictures}
        onChange={changePictures}
        adder={adder}
        error={fieldError("pictures")}
        disabled={saving}
        addText={{
          label: t("notes.addText"),
          done: t("notes.addedText"),
          onAdd: (text) => {
            setBody((current) => appendText(current, text));
            edited("body");
          },
        }}
      />
      <TagField
        id="note-tags"
        hint={t("notes.tagsHint")}
        tags={tags}
        onChange={(next) => {
          setTags(next);
          edited("tags");
        }}
        error={fieldError("tags")}
        disabled={saving}
      />

      <FormActions
        saveLabel={note ? t("occlusion.saveChanges") : t("notes.save")}
        saving={saving}
        error={formError}
        onCancel={onCancel}
      />
    </form>
  );
}

/**
 * A new note from a photo: the kept picture, what was read in it (null if
 * reading stopped or failed), and why it failed.
 */
export type PhotoDraft = {
  picture: Asset;
  recognition: Recognition | null;
  problem: string | null;
};

/**
 * What came of reading a photo, said before the user checks the note. Focus
 * goes to it, so it's read out first.
 */
function PhotoSummary({ photo }: { photo: PhotoDraft }) {
  const { t } = useT();
  const summaryRef = useFocusOnMount<HTMLParagraphElement>(true);
  const { recognition } = photo;
  const found = recognition !== null && recognition.text.trim() !== "";
  return (
    <>
      <p ref={summaryRef} className="notice" role="status" tabIndex={-1}>
        {found
          ? t("notes.photoFound")
          : recognition !== null
            ? t("notes.photoEmpty")
            : photo.problem
              ? t("notes.photoProblem", { problem: photo.problem })
              : t("notes.photoStopped")}
      </p>
      {found && (
        <p className="field-hint">
          {t("notes.photoReader", {
            reader: recognition.engine === "ocrs"
              ? t("notes.readerBuiltIn")
              : t("notes.readerOllama", { model: String(recognition.model) }),
          })}
        </p>
      )}
    </>
  );
}

/** The most characters a note's text can hold: `NOTE_BODY_MAX_CHARS` in `notes.rs`. */
const NOTE_BODY_MAX_CHARS = 100_000;

/** Page numbers joined in the current language. */
function listed(numbers: number[], t: ReturnType<typeof useT>["t"]): string {
  if (numbers.length < 2) return numbers.join("");
  return t(numbers.length === 2 ? "occlusion.list" : "notes.listMany", {
    first: numbers.slice(0, -1).join(", "),
    last: numbers[numbers.length - 1],
  });
}

/**
 * What was read from an imported file, said before the user checks its text:
 * where it came from, any pages with no text, and whether the text (now
 * `length` characters, as edited) is too long for a note, or to suggest
 * cards from whole. Focus goes to it, so it's read out first.
 */
function ImportSummary({ imported, length }: { imported: SourceNoteDraft; length: number }) {
  const { t, plural } = useT();
  const { lang } = useLang();
  const summaryRef = useFocusOnMount<HTMLParagraphElement>(true);
  const parts =
    imported.pages === null
      ? ""
      : ` (${plural(
          imported.pages,
          imported.kind === "powerPoint" ? "notes.slideOne" : "notes.pageOne",
          imported.kind === "powerPoint" ? "notes.slideOther" : "notes.pageOther",
        )})`;
  const missing = imported.pagesWithoutText;
  const recognised = imported.recognisedPages;

  return (
    <>
      <p ref={summaryRef} className="notice" role="status" tabIndex={-1}>
        {imported.justKept
          ? t("notes.importKept", { name: imported.fileName, parts })
          : t("notes.importExisting", { name: imported.fileName, parts })}
      </p>
      {recognised.length > 0 && (
        <p className="import-warning">
          {plural(recognised.length, "notes.scannedWarningOne", "notes.scannedWarningOther", {
            pages: recognised.length > 12
              ? t("notes.scannedMany", { count: recognised.length })
              : plural(recognised.length, "notes.scannedOne", "notes.scannedOther", { pages: listed(recognised, t) }),
          })}
        </p>
      )}
      {missing.length > 0 && (
        <p className="import-warning">
          {t("notes.missingWarning", {
            pages: missing.length > 12
              ? plural(missing.length, "notes.pageOne", "notes.pageOther")
              : plural(missing.length, "notes.missingOne", "notes.missingOther", { pages: listed(missing, t) }),
          })}
        </p>
      )}
      {(imported.imagesLeftOut ?? 0) > 0 && (
        <p className="import-warning">
          {plural(imported.imagesLeftOut ?? 0, "notes.imagesLeftOutOne", "notes.imagesLeftOutOther")}
        </p>
      )}
      {length > NOTE_BODY_MAX_CHARS && (
        <p className="import-warning">
          {t("notes.tooLong", { length: length.toLocaleString(lang === "it" ? "it-IT" : "en-US") })}
        </p>
      )}
      {length > SUGGESTION_NOTE_MAX_CHARS && length <= NOTE_BODY_MAX_CHARS && (
        <p className="field-hint">
          {t("notes.suggestParts", { length: length.toLocaleString(lang === "it" ? "it-IT" : "en-US") })}
        </p>
      )}
    </>
  );
}

/**
 * Writing an ordinary card while reading a note. Nothing is generated: the
 * user writes the front and back and chooses the deck, and the card is saved
 * by the same Rust command as on the deck screen, with the same rules. The
 * card isn't linked to the note, and the note isn't changed.
 */
function CardFromNote({
  note,
  onCancel,
  onCreated,
}: {
  note: Note;
  onCancel: () => void;
  onCreated: (deck: DeckDetail) => void;
}) {
  const { t } = useT();
  const [choices, setChoices] = useState<DeckChoices>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let current = true;
    loadDeckChoices().then((next) => {
      if (current) setChoices(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  if (choices.status === "loading") {
    return (
      <p className="loading" role="status">
        {t("desk.loading")}
      </p>
    );
  }

  if (choices.status === "error") {
    return (
      <Message focus tone="alert" text={choices.message}>
        <div className="actions">
          <button
            type="button"
            className="button"
            onClick={() => {
              setChoices({ status: "loading" });
              setAttempt((n) => n + 1);
            }}
          >
            {t("settings.retry")}
          </button>
          <button type="button" className="button" onClick={onCancel}>
            {t("notes.backNote")}
          </button>
        </div>
      </Message>
    );
  }

  if (choices.decks.length === 0) {
    return (
      <Message
        focus
        tone="status"
        text={t("notes.noDeck")}
      >
        <button type="button" className="button" onClick={onCancel}>
          {t("notes.backNote")}
        </button>
      </Message>
    );
  }

  return (
    <CardFromNoteForm
      note={note}
      decks={choices.decks}
      onCancel={onCancel}
      onCreated={onCreated}
      onNoDecks={() => setChoices({ status: "ready", decks: [] })}
    />
  );
}

function CardFromNoteForm({
  note,
  decks: initialDecks,
  onCancel,
  onCreated,
  onNoDecks,
}: {
  note: Note;
  decks: Deck[];
  onCancel: () => void;
  onCreated: (deck: DeckDetail) => void;
  /** Reloading the choices found none left, so there's nothing to save into. */
  onNoDecks: () => void;
}) {
  const { t } = useT();
  const [decks, setDecks] = useState(initialDecks);
  // An explicit choice: no deck is picked until the user picks one.
  const [deckId, setDeckId] = useState("");
  const [front, setFront] = useState("");
  const [back, setBack] = useState("");
  useUnsavedChanges(deckId !== "" || front !== "" || back !== "");

  /** Reloads the deck choices in place, keeping what the user wrote. */
  function refreshDecks() {
    loadDeckChoices().then((next) => {
      if (next.status !== "ready") return;
      if (next.decks.length === 0) {
        onNoDecks();
        return;
      }
      setDecks(next.decks);
      // Checked against the choice when the list arrives, not when it was
      // asked for, so a deck picked in the meantime isn't cleared.
      setDeckId((current) =>
        next.decks.some((deck) => String(deck.id) === current) ? current : ""
      );
    });
  }

  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      if (deckId === "") {
        // Nothing to send yet; shown under the deck list like any field error.
        const unchosen: CommandError = {
          kind: "invalid",
          message: t("notes.chooseDeck"),
          field: "deck",
        };
        throw unchosen;
      }
      try {
        return await createFlashcard(Number(deckId), front, back, undefined, undefined, note.id);
      } catch (err) {
        const error = toCommandError(err);
        if (error.field || error.kind === "failed") throw err;
        // A refusal about neither side of the card is about the deck: it was
        // archived or is gone since the list loaded. Show it under the deck
        // list, and refresh the choices before the user picks again.
        refreshDecks();
        const refused: CommandError = { ...error, field: "deck" };
        throw refused;
      }
    },
    onCreated,
    { deck: "note-card-deck", front: "note-card-front", back: "note-card-back" }
  );

  return (
    <form className="card" aria-labelledby="note-card-heading" noValidate onSubmit={submit}>
      <h2 id="note-card-heading" className="form-title">
        {t("notes.cardFrom", { title: note.title })}
      </h2>

      {/* Focusable so keyboard users can scroll a long note. */}
      <section className="note-context" tabIndex={0} aria-labelledby="note-context-label">
        <p id="note-context-label" className="side-label">
          {t("notes.yourNote")}
        </p>
        <p className="note-body">{note.body}</p>
      </section>
      <p id="note-card-hint" className="field-hint">
        {t("notes.cardHint")}
      </p>

      <SelectField
        id="note-card-deck"
        label={t("search.deck")}
        hint={t("notes.deckHint")}
        describedBy="note-card-hint"
        value={deckId}
        options={decks.map((deck) => ({ value: String(deck.id), label: deck.name }))}
        placeholder={t("exams.chooseDeck")}
        error={fieldError("deck")}
        required
        autoFocus
        onChange={(value) => {
          setDeckId(value);
          edited("deck");
        }}
      />
      <TextField
        id="note-card-front"
        label={t("csv.front")}
        hint={t("deck.frontHint")}
        value={front}
        error={fieldError("front")}
        required
        multiline
        onChange={(value) => {
          setFront(value);
          edited("front");
        }}
      />
      <TextField
        id="note-card-back"
        label={t("csv.back")}
        hint={t("deck.backHint")}
        value={back}
        error={fieldError("back")}
        required
        multiline
        onChange={(value) => {
          setBack(value);
          edited("back");
        }}
      />

      <FormActions saveLabel={t("deck.saveCard")} saving={saving} error={formError} onCancel={onCancel} />
    </form>
  );
}
