import { useEffect, useState } from "react";
import { useT, translate } from "./i18n";
import { COMMAND_ERROR_FALLBACK, getTags, mergeTags, renameTag, toCommandError, type MergedTags, type TagSummary } from "./api";
import { FormActions, SelectField, TextField, useFormSave } from "./forms";
import { ArrowRightIcon } from "./icons";
import { useUnsavedChanges } from "./unsaved";
import { Message, useFocusOnMount } from "./ui";

type TagsState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; tags: TagSummary[] };

/**
 * How the list appears next. `key` changes every time, so the list remounts
 * and its focus target takes effect: the notice after a rename, or the
 * renamed tag's Rename button when the rename was cancelled.
 */
type TagsView = { key: number; notice: string | null };

/** Fetches the tags and describes the outcome; never rejects. */
async function loadTags(): Promise<TagsState> {
  try {
    return { status: "ready", tags: await getTags() };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * Tags: every tag on an active card or note, with how many of each, from the
 * sidebar. A tag can be renamed, which renames it everywhere, merged into
 * another tag, or used to find what has it on the Search screen. Tags are added and taken off where the
 * cards and notes are written; one nothing uses any more just isn't listed.
 */
export function TagsScreen({ onFind }: { onFind: (tag: string) => void }) {
  const { t } = useT();
  const [state, setState] = useState<TagsState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or after a stale rename).
  const [attempt, setAttempt] = useState(0);
  const [view, setView] = useState<TagsView>({ key: 0, notice: null });

  useEffect(() => {
    let current = true;
    loadTags().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  function reload(notice: string | null) {
    setState({ status: "loading" });
    setView((previous) => ({ key: previous.key + 1, notice }));
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        {t("tags.loading")}
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <button type="button" className="button" onClick={() => reload(null)}>
          {t("stats.retry")}
        </button>
      </Message>
    );
  }

  return (
    <TagList
      key={view.key}
      tags={state.tags}
      notice={view.notice}
      onFind={onFind}
      onRenamed={(tags, from, to) => {
        setState({ status: "ready", tags });
        setView((previous) => ({
          key: previous.key + 1,
          notice: t("tags.renamed", { from, to }),
        }));
      }}
      onMerged={(merged, from, into) => {
        setState({ status: "ready", tags: merged.tags });
        setView((previous) => ({
          key: previous.key + 1,
          notice: mergedNotice(merged, from, into, t),
        }));
      }}
      onStale={() =>
        reload(t("tags.stale"))
      }
    />
  );
}

function TagList({
  tags,
  notice,
  onFind,
  onRenamed,
  onMerged,
  onStale,
}: {
  tags: TagSummary[];
  notice: string | null;
  onFind: (tag: string) => void;
  onRenamed: (tags: TagSummary[], from: string, to: string) => void;
  onMerged: (merged: MergedTags, from: string, into: string) => void;
  onStale: () => void;
}) {
  const { t } = useT();
  const screenRef = useFocusOnMount<HTMLElement>(notice === null);
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(notice !== null);
  // One tag is renamed or merged at a time.
  const [editing, setEditing] = useState<Editing | null>(null);
  // The button whose form was cancelled takes focus back.
  const [cancelled, setCancelled] = useState<Editing | null>(null);
  const cancel = () => {
    setCancelled(editing);
    setEditing(null);
  };

  return (
    <section ref={screenRef} className="tags-screen" tabIndex={-1} aria-labelledby="tags-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="tags-heading" className="page-title">
            {t("app.tags")}
          </h2>
          <p className="page-intro">
            {t("tags.intro")}
          </p>
        </div>
      </div>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}
      {tags.length === 0 ? (
        <p className="empty" role="status">
          {t("tags.empty")}
        </p>
      ) : (
        <ul className="card-list">
          {tags.map((tag) => (
            <li key={tag.id}>
              {editing?.id === tag.id && editing.kind === "rename" ? (
                <RenameTag
                  tag={tag}
                  onCancel={cancel}
                  onRenamed={(list, name) => onRenamed(list, tag.name, name)}
                  onStale={onStale}
                />
              ) : editing?.id === tag.id && editing.kind === "merge" ? (
                <MergeTag
                  tag={tag}
                  others={tags.filter((other) => other.id !== tag.id)}
                  onCancel={cancel}
                  onMerged={(merged, into) => onMerged(merged, tag.name, into)}
                  onStale={onStale}
                />
              ) : (
                <TagItem
                  tag={tag}
                  canMerge={tags.length > 1}
                  busy={editing !== null}
                  focus={cancelled?.id === tag.id ? cancelled.kind : null}
                  onRename={() => setEditing({ kind: "rename", id: tag.id })}
                  onMerge={() => setEditing({ kind: "merge", id: tag.id })}
                  onFind={() => onFind(tag.name)}
                />
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** Which tag's form is open, renaming it or merging it into another. */
type Editing = { kind: "rename" | "merge"; id: number };

/** Counts only the kinds that are present. */
function cardsAndNotes(cards: number, notes: number, t: ReturnType<typeof useT>["t"]): string {
  const parts = [];
  if (cards > 0) parts.push(t(cards === 1 ? "common.cardOne" : "common.cardOther", { count: cards }));
  if (notes > 0) parts.push(t(notes === 1 ? "tags.noteOne" : "tags.noteOther", { count: notes }));
  return parts.join(t("tags.and"));
}

/** What the screen says after a merge, including what kept the old tag. */
export function mergedNotice(
  merged: MergedTags, from: string, into: string,
  t: ReturnType<typeof useT>["t"] = (key, params) => translate("en", key, params),
): string {
  const moved = cardsAndNotes(merged.cards, merged.notes, t);
  let notice = t(merged.cards + merged.notes === 1 ? "tags.mergedOne" : "tags.mergedOther", { from, into, moved });
  const kept = [];
  if (merged.keptCards > 0) {
    kept.push(t(merged.keptCards === 1 ? "tags.keptCardOne" : "tags.keptCardOther", { cards: t(merged.keptCards === 1 ? "common.cardOne" : "common.cardOther", { count: merged.keptCards }) }));
  }
  if (merged.keptNotes > 0) kept.push(t(merged.keptNotes === 1 ? "tags.deletedNoteOne" : "tags.deletedNoteOther", { count: merged.keptNotes }));
  if (kept.length > 0) notice += t("tags.kept", { kept: kept.join(t("tags.and")), from });
  return notice;
}

/** Counts for a tag's metadata. */
function usedBy(tag: TagSummary, t: ReturnType<typeof useT>["t"]): string {
  const parts = [];
  if (tag.cards > 0) parts.push(t(tag.cards === 1 ? "common.cardOne" : "common.cardOther", { count: tag.cards }));
  if (tag.notes > 0) parts.push(t(tag.notes === 1 ? "tags.noteOne" : "tags.noteOther", { count: tag.notes }));
  return parts.join(" · ");
}

function TagItem({
  tag,
  canMerge,
  busy,
  focus,
  onRename,
  onMerge,
  onFind,
}: {
  tag: TagSummary;
  /** There is another tag to merge into. */
  canMerge: boolean;
  /** Another tag is being renamed or merged. */
  busy: boolean;
  /** The button that takes focus, after its form was cancelled. */
  focus: Editing["kind"] | null;
  onRename: () => void;
  onMerge: () => void;
  onFind: () => void;
}) {
  const { t } = useT();
  const renameRef = useFocusOnMount<HTMLButtonElement>(focus === "rename");
  const mergeRef = useFocusOnMount<HTMLButtonElement>(focus === "merge");
  const nameId = `tag-${tag.id}-name`;
  return (
    <article className="card-item tag-item" aria-labelledby={nameId}>
      <div>
        <h3 id={nameId} className="note-item-title">
          <span className="tag-chip">
            <span className="tag-name">{tag.name}</span>
          </span>
        </h3>
        <p className="item-meta">{usedBy(tag, t)}</p>
      </div>
      <div className="deck-actions">
        <button
          ref={renameRef}
          type="button"
          className="button button-small"
          aria-describedby={nameId}
          aria-disabled={busy}
          onClick={() => {
            if (!busy) onRename();
          }}
        >
          {t("sources.rename")}
        </button>
        {canMerge && (
          <button
            ref={mergeRef}
            type="button"
            className="button button-small"
            aria-describedby={nameId}
            aria-disabled={busy}
            onClick={() => {
              if (!busy) onMerge();
            }}
          >
            {t("tags.mergeInto")}
          </button>
        )}
        <button
          type="button"
          className="button button-small"
          aria-describedby={nameId}
          onClick={onFind}
        >
          {t("tags.find")}
          <ArrowRightIcon />
        </button>
      </div>
    </article>
  );
}

/**
 * Renaming one tag. Rust checks the name (one word, at most 60 characters)
 * and refuses one another tag has, ignoring capitals; changing only the
 * capitals of this tag's own name is fine.
 */
function RenameTag({
  tag,
  onCancel,
  onRenamed,
  onStale,
}: {
  tag: TagSummary;
  onCancel: () => void;
  onRenamed: (tags: TagSummary[], name: string) => void;
  onStale: () => void;
}) {
  const { t } = useT();
  const [name, setName] = useState(tag.name);
  const inputId = `tag-${tag.id}-rename`;
  useUnsavedChanges(name !== tag.name);
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      try {
        return { tags: await renameTag(tag.id, name), name: name.trim() };
      } catch (err) {
        if (toCommandError(err).kind === "stale") return null;
        throw err;
      }
    },
    (result) => (result === null ? onStale() : onRenamed(result.tags, result.name)),
    { tags: inputId }
  );

  return (
    <form
      className="card-item tag-rename"
      aria-label={t("tags.renameLabel", { name: tag.name })}
      noValidate
      onSubmit={submit}
    >
      <TextField
        id={inputId}
        label={t("tags.newName", { name: tag.name })}
        hint={t("tags.nameHint")}
        value={name}
        error={fieldError("tags")}
        required
        autoFocus
        onChange={(value) => {
          setName(value);
          edited("tags");
        }}
      />
      <FormActions saveLabel={t("tags.renameTag")} saving={saving} error={formError === COMMAND_ERROR_FALLBACK ? t("common.somethingWrong") : formError} onCancel={onCancel} />
    </form>
  );
}

/**
 * Merging one tag into another chosen tag. Rust moves every active card and
 * note at once, skipping those that already have the other tag, so none gains
 * a tag; deleted cards and notes, and cards in archived decks, keep this tag,
 * since nothing about them can change while they are put away.
 */
function MergeTag({
  tag,
  others,
  onCancel,
  onMerged,
  onStale,
}: {
  tag: TagSummary;
  /** The tags it can be merged into. */
  others: TagSummary[];
  onCancel: () => void;
  onMerged: (merged: MergedTags, into: string) => void;
  onStale: () => void;
}) {
  const { t } = useT();
  const [into, setInto] = useState("");
  const selectId = `tag-${tag.id}-merge`;
  const target = others.find((other) => String(other.id) === into);
  useUnsavedChanges(into !== "");
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      if (!target) {
        throw { kind: "invalid", message: t("tags.chooseTarget"), field: "tags" };
      }
      try {
        return { merged: await mergeTags(tag.id, target.id), into: target.name };
      } catch (err) {
        if (toCommandError(err).kind === "stale") return null;
        throw err;
      }
    },
    (result) => (result === null ? onStale() : onMerged(result.merged, result.into)),
    { tags: selectId },
  );

  return (
    <form
      className="card-item tag-rename"
      aria-label={t("tags.mergeLabel", { name: tag.name })}
      noValidate
      onSubmit={submit}
    >
      <SelectField
        id={selectId}
        label={t("tags.mergeTarget", { name: tag.name })}
        hint={t("tags.mergeHint", { name: tag.name })}
        value={into}
        placeholder={t("exams.chooseTag")}
        options={others.map((other) => ({ value: String(other.id), label: other.name }))}
        error={fieldError("tags")}
        required
        autoFocus
        onChange={(value) => {
          setInto(value);
          edited("tags");
        }}
      />
      {target && (
        <p className="field-hint" role="status">
          {t(tag.cards + tag.notes === 1 ? "tags.previewOne" : "tags.previewOther", { items: cardsAndNotes(tag.cards, tag.notes, t), into: target.name, from: tag.name })}
        </p>
      )}
      <FormActions saveLabel={t("tags.mergeTags")} saving={saving} error={formError === COMMAND_ERROR_FALLBACK ? t("common.somethingWrong") : formError} onCancel={onCancel} />
    </form>
  );
}
