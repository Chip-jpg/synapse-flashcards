import { useEffect, useState } from "react";
import { getTags, mergeTags, renameTag, toCommandError, type MergedTags, type TagSummary } from "./api";
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
        Loading your tags…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <button type="button" className="button" onClick={() => reload(null)}>
          Retry
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
          notice: `${from} is now ${to}, on every card and note that had it.`,
        }));
      }}
      onMerged={(merged, from, into) => {
        setState({ status: "ready", tags: merged.tags });
        setView((previous) => ({
          key: previous.key + 1,
          notice: mergedNotice(merged, from, into),
        }));
      }}
      onStale={() =>
        reload("That tag has changed or is no longer used. Here are your tags as they are now.")
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
            Tags
          </h2>
          <p className="page-intro">
            Every tag on your cards and notes, with how many have it. Add or take off tags where
            you write a card or note. Renaming a tag changes it everywhere, deleted cards and notes
            included; a name another tag already has is refused. To make two tags one, use Merge
            into on the tag you no longer want.
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
          No tags yet. Give a note tags when you write or edit it, and they're listed here.
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

/** "1 card", "3 notes". */
function count(n: number, one: string, many: string): string {
  return n === 1 ? `1 ${one}` : `${n} ${many}`;
}

/** "3 cards and 1 note", leaving out a kind there are none of ("" for none). */
function cardsAndNotes(cards: number, notes: number, cardWord = "card"): string {
  const parts = [];
  if (cards > 0) parts.push(count(cards, cardWord, `${cardWord}s`));
  if (notes > 0) parts.push(count(notes, "note", "notes"));
  return parts.join(" and ");
}

/** What the screen says after a merge, including what kept the old tag. */
export function mergedNotice(merged: MergedTags, from: string, into: string): string {
  const moved = cardsAndNotes(merged.cards, merged.notes);
  let notice = `${from} is merged into ${into}: ${moved} now ${
    merged.cards + merged.notes === 1 ? "has" : "have"
  } ${into} instead.`;
  const kept = [];
  if (merged.keptCards > 0) {
    kept.push(`${count(merged.keptCards, "card", "cards")} that are deleted or in archived decks`);
  }
  if (merged.keptNotes > 0) kept.push(count(merged.keptNotes, "deleted note", "deleted notes"));
  if (kept.length > 0) {
    notice += ` ${kept.join(" and ")} keep ${from}, since they can't be changed: if you restore them, they come back with ${from}.`;
  }
  return notice;
}

/** "3 cards · 1 note", leaving out a kind no active card or note has. */
function usedBy(tag: TagSummary): string {
  const parts = [];
  if (tag.cards > 0) parts.push(tag.cards === 1 ? "1 card" : `${tag.cards} cards`);
  if (tag.notes > 0) parts.push(tag.notes === 1 ? "1 note" : `${tag.notes} notes`);
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
        <p className="item-meta">{usedBy(tag)}</p>
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
          Rename
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
            Merge into…
          </button>
        )}
        <button
          type="button"
          className="button button-small"
          aria-describedby={nameId}
          onClick={onFind}
        >
          Find in Search
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
      aria-label={`Rename tag ${tag.name}`}
      noValidate
      onSubmit={submit}
    >
      <TextField
        id={inputId}
        label={`New name for ${tag.name}`}
        hint="One word, up to 60 characters: no spaces or commas. Use - or :: to join words."
        value={name}
        error={fieldError("tags")}
        required
        autoFocus
        onChange={(value) => {
          setName(value);
          edited("tags");
        }}
      />
      <FormActions saveLabel="Rename tag" saving={saving} error={formError} onCancel={onCancel} />
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
  const [into, setInto] = useState("");
  const selectId = `tag-${tag.id}-merge`;
  const target = others.find((other) => String(other.id) === into);
  useUnsavedChanges(into !== "");
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      if (!target) {
        throw { kind: "invalid", message: "Choose the tag to merge into.", field: "tags" };
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
      aria-label={`Merge tag ${tag.name}`}
      noValidate
      onSubmit={submit}
    >
      <SelectField
        id={selectId}
        label={`Merge ${tag.name} into`}
        hint={`Every card and note with ${tag.name} gets the tag you choose instead, and ${tag.name} is no longer listed. Tags below it, such as ${tag.name}::more, stay as they are. This can't be undone.`}
        value={into}
        placeholder="Choose a tag"
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
          {cardsAndNotes(tag.cards, tag.notes)} will have {target.name} instead of {tag.name}.
          Deleted cards and notes, and cards in archived decks, keep {tag.name}.
        </p>
      )}
      <FormActions saveLabel="Merge tags" saving={saving} error={formError} onCancel={onCancel} />
    </form>
  );
}
