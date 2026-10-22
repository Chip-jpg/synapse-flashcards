import { useEffect, useState } from "react";
import { getTags, renameTag, toCommandError, type TagSummary } from "./api";
import { FormActions, TextField, useFormSave } from "./forms";
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
 * sidebar. A tag can be renamed, which renames it everywhere, or used to find
 * what has it on the Search screen. Tags are added and taken off where the
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
      onStale={() =>
        reload("That tag is no longer there. Here are your tags as they are now.")
      }
    />
  );
}

function TagList({
  tags,
  notice,
  onFind,
  onRenamed,
  onStale,
}: {
  tags: TagSummary[];
  notice: string | null;
  onFind: (tag: string) => void;
  onRenamed: (tags: TagSummary[], from: string, to: string) => void;
  onStale: () => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(notice === null);
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(notice !== null);
  // One tag is renamed at a time.
  const [renaming, setRenaming] = useState<number | null>(null);
  // The tag whose rename was cancelled: its Rename button takes focus back.
  const [cancelled, setCancelled] = useState<number | null>(null);

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
            included; a name another tag already has is refused, so two tags are never joined.
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
              {renaming === tag.id ? (
                <RenameTag
                  tag={tag}
                  onCancel={() => {
                    setCancelled(tag.id);
                    setRenaming(null);
                  }}
                  onRenamed={(list, name) => onRenamed(list, tag.name, name)}
                  onStale={onStale}
                />
              ) : (
                <TagItem
                  tag={tag}
                  busy={renaming !== null}
                  focusRename={cancelled === tag.id}
                  onRename={() => setRenaming(tag.id)}
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

/** "3 cards · 1 note", leaving out a kind no active card or note has. */
function usedBy(tag: TagSummary): string {
  const parts = [];
  if (tag.cards > 0) parts.push(tag.cards === 1 ? "1 card" : `${tag.cards} cards`);
  if (tag.notes > 0) parts.push(tag.notes === 1 ? "1 note" : `${tag.notes} notes`);
  return parts.join(" · ");
}

function TagItem({
  tag,
  busy,
  focusRename,
  onRename,
  onFind,
}: {
  tag: TagSummary;
  /** Another tag is being renamed. */
  busy: boolean;
  focusRename: boolean;
  onRename: () => void;
  onFind: () => void;
}) {
  const renameRef = useFocusOnMount<HTMLButtonElement>(focusRename);
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
