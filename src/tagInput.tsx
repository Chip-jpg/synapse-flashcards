import { useEffect, useRef, useState } from "react";
import { getTags } from "./api";

/**
 * Splits typed text into tag names: a tag is one word, so spaces and commas
 * separate them. Empty pieces are dropped.
 */
export function splitTags(text: string): string[] {
  return text.split(/[\s,]+/u).filter((name) => name !== "");
}

/**
 * `names` with `more` added after them, each name kept once ignoring letter
 * case (the first spelling wins), as Rust keeps them. Rust still checks every
 * name; this only keeps the chips tidy.
 */
export function addTags(names: string[], more: string[]): string[] {
  const seen = new Set(names.map((name) => name.toLowerCase()));
  const result = [...names];
  for (const name of more) {
    const key = name.toLowerCase();
    if (!seen.has(key)) {
      seen.add(key);
      result.push(name);
    }
  }
  return result;
}

/**
 * Whether `tags` answer a filter for `wanted`: the tag itself, ignoring
 * letter case, or one below it (`bio::cells` for `bio`), as Search reads it.
 */
export function hasTag(tags: string[] | undefined, wanted: string): boolean {
  const key = wanted.toLowerCase();
  return (tags ?? []).some((tag) => {
    const name = tag.toLowerCase();
    return name === key || name.startsWith(`${key}::`);
  });
}

/**
 * Every tag on `items`, each once ignoring letter case (the first spelling
 * met), sorted by name ignoring case, with how many items have it.
 */
export function tagsIn(items: { tags?: string[] }[]): { name: string; count: number }[] {
  const found = new Map<string, { name: string; count: number }>();
  for (const item of items) {
    for (const tag of item.tags ?? []) {
      const key = tag.toLowerCase();
      const entry = found.get(key);
      if (entry) entry.count += 1;
      else found.set(key, { name: tag, count: 1 });
    }
  }
  return [...found.values()].sort(
    (a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()) || a.name.localeCompare(b.name),
  );
}

/** Whether two tag lists hold the same names, ignoring order and letter case. */
export function sameTags(a: string[], b: string[]): boolean {
  const key = (names: string[]) =>
    names
      .map((name) => name.toLowerCase())
      .sort()
      .join("\n");
  return key(a) === key(b);
}

/**
 * The names of the tags in use, to suggest while typing. Loaded once; if they
 * can't be read there are just no suggestions.
 */
export function useTagNames(): string[] {
  const [names, setNames] = useState<string[]>([]);
  useEffect(() => {
    let current = true;
    getTags().then(
      (tags) => {
        if (current) setNames(tags.map((tag) => tag.name));
      },
      () => {},
    );
    return () => {
      current = false;
    };
  }, []);
  return names;
}

/** At most this many tags in use are offered while typing. */
const SUGGESTIONS_SHOWN = 8;

/**
 * A form field for a card's or note's tags: each tag a removable chip, and a
 * box to type more in. A space, a comma or Enter turns what's typed into a
 * tag, and so does leaving the box for another part of the page (so pressing
 * Save keeps a tag still being typed). Backspace in the empty box takes the
 * last tag off. Tags already in use that contain what's being typed are
 * offered as buttons under the box (not a datalist: WebKitGTK applies a
 * picked suggestion after the key or blur that has already finished the
 * half-typed tag). Rust checks the names when the form is saved; a refused
 * one comes back as this field's `error`, focusing the box (`id`).
 */
export function TagField({
  id,
  label = "Tags",
  hint,
  tags,
  onChange,
  error,
  disabled = false,
}: {
  id: string;
  label?: string;
  hint?: string;
  tags: string[];
  onChange: (tags: string[]) => void;
  error?: string;
  disabled?: boolean;
}) {
  const [typed, setTyped] = useState("");
  const suggestions = useTagNames();
  const suggestionsRef = useRef<HTMLUListElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const hintId = hint === undefined ? undefined : `${id}-hint`;
  const errorId = `${id}-error`;
  const description = [error ? errorId : undefined, hintId].filter(Boolean).join(" ");
  const wanted = typed.trim().toLowerCase();
  const offered =
    wanted === ""
      ? []
      : suggestions
          .filter(
            (name) =>
              name.toLowerCase().includes(wanted) &&
              !tags.some((tag) => tag.toLowerCase() === name.toLowerCase()),
          )
          .slice(0, SUGGESTIONS_SHOWN);

  function commit(text: string) {
    const names = splitTags(text);
    if (names.length > 0) onChange(addTags(tags, names));
  }

  function type(text: string) {
    // Whatever comes before the last space or comma is finished.
    const last = /[\s,](?=[^\s,]*$)/u.exec(text);
    if (last) {
      commit(text.slice(0, last.index));
      setTyped(text.slice(last.index + 1));
    } else {
      setTyped(text);
    }
  }

  function pick(name: string) {
    onChange(addTags(tags, [name]));
    setTyped("");
    inputRef.current?.focus();
  }

  function remove(tag: string) {
    onChange(tags.filter((name) => name !== tag));
    inputRef.current?.focus();
  }

  return (
    <div className="field">
      <label htmlFor={id} className="field-label">
        {label}
      </label>
      {hint !== undefined && (
        <p id={hintId} className="field-hint">
          {hint}
        </p>
      )}
      <div className="tag-field">
        {tags.length > 0 && (
          <ul className="tag-list" aria-label={`${label} added`}>
            {tags.map((tag) => (
              <li key={tag} className="tag-chip">
                <span className="tag-name">{tag}</span>
                <button
                  type="button"
                  className="tag-remove"
                  aria-label={`Remove tag ${tag}`}
                  disabled={disabled}
                  onClick={() => remove(tag)}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
        <input
          ref={inputRef}
          id={id}
          type="text"
          className="field-input tag-input"
          value={typed}
          autoComplete="off"
          spellCheck={false}
          disabled={disabled}
          aria-invalid={error ? true : undefined}
          aria-describedby={description || undefined}
          onChange={(event) => type(event.target.value)}
          onBlur={(event) => {
            // Moving to a suggestion: picking it replaces what's typed.
            if (suggestionsRef.current?.contains(event.relatedTarget as Node | null)) return;
            commit(typed);
            setTyped("");
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && typed.trim() !== "") {
              // Enter adds the tag rather than saving the form.
              event.preventDefault();
              commit(typed);
              setTyped("");
            } else if (event.key === "Backspace" && typed === "" && tags.length > 0) {
              event.preventDefault();
              onChange(tags.slice(0, -1));
            }
          }}
        />
      </div>
      {offered.length > 0 && (
        <ul ref={suggestionsRef} className="tag-list tag-suggestions" aria-label="Tags in use">
          {offered.map((name) => (
            <li key={name}>
              <button
                type="button"
                className="tag-chip tag-suggestion"
                aria-label={`Add tag ${name}`}
                disabled={disabled}
                // Keeps focus in the box, so leaving it doesn't finish the
                // half-typed tag first.
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => pick(name)}
              >
                <span className="tag-name">{name}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {error && (
        <p id={errorId} className="field-error">
          {error}
        </p>
      )}
    </div>
  );
}

/** A card's or note's tags, as small chips to read. Nothing for none. */
export function TagChips({ tags, label = "Tags" }: { tags?: string[]; label?: string }) {
  if (!tags || tags.length === 0) return null;
  return (
    <ul className="tag-list" aria-label={label}>
      {tags.map((tag) => (
        <li key={tag} className="tag-chip">
          <span className="tag-name">{tag}</span>
        </li>
      ))}
    </ul>
  );
}
