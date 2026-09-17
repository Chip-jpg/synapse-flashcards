import { useEffect, useRef, useState, type FormEvent } from "react";
import { toCommandError, type CommandError, type Field } from "./api";

/**
 * Saving a form. Repeat submits are ignored while a save is running, and a
 * failed save leaves everything the user typed in place.
 *
 * `fieldIds` maps this form's fields to their input ids. An error about one
 * of them is shown under that input, which then takes focus. Any other error
 * is shown for the whole form, and saving again retries.
 */
export function useFormSave<T>(
  save: () => Promise<T>,
  onSaved: (result: T) => void,
  fieldIds: Partial<Record<Field, string>>,
) {
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<CommandError | null>(null);
  // Blocks a second submit immediately, before the re-render lands.
  const savingRef = useRef(false);

  const errorFieldId = error?.field ? fieldIds[error.field] : undefined;

  // Focus after the error has rendered, so the field is announced together
  // with its message.
  useEffect(() => {
    if (errorFieldId) document.getElementById(errorFieldId)?.focus();
  }, [error, errorFieldId]);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    setError(null);

    try {
      const result = await save();
      onSaved(result);
      return; // The form is replaced, so stay "saving" until it unmounts.
    } catch (err) {
      setError(toCommandError(err));
    }

    savingRef.current = false;
    setSaving(false);
  }

  return {
    saving,
    submit,
    /** The message for `field`, if the last save failed because of it. */
    fieldError: (field: Field) => (error?.field === field ? error.message : undefined),
    /** A failure that isn't about one of this form's fields. */
    formError: error && !errorFieldId ? error.message : null,
    /** Call when `field` changes: its error no longer applies. */
    edited: (field: Field) => {
      if (error?.field === field) setError(null);
    },
  };
}

/**
 * A labelled single-line input, or a textarea when `multiline`. The hint and
 * any error are linked to the control, so screen readers read them on focus.
 */
export function TextField({
  id,
  label,
  hint,
  value,
  onChange,
  error,
  required = false,
  multiline = false,
  rows = 3,
  autoFocus = false,
}: {
  id: string;
  label: string;
  hint: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
  required?: boolean;
  multiline?: boolean;
  /** The textarea's visible height, when `multiline`. */
  rows?: number;
  autoFocus?: boolean;
}) {
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const shared = {
    id,
    className: "field-input",
    value,
    autoFocus,
    "aria-required": required || undefined,
    "aria-invalid": error ? true : undefined,
    "aria-describedby": error ? `${errorId} ${hintId}` : hintId,
  };

  return (
    <div className="field">
      <label htmlFor={id} className="field-label">
        {label}
      </label>
      <p id={hintId} className="field-hint">
        {hint}
      </p>
      {multiline ? (
        <textarea {...shared} rows={rows} onChange={(e) => onChange(e.target.value)} />
      ) : (
        <input {...shared} type="text" onChange={(e) => onChange(e.target.value)} />
      )}
      {error && (
        <p id={errorId} className="field-error">
          {error}
        </p>
      )}
    </div>
  );
}

/**
 * A labelled drop-down list with an empty first choice (`placeholder`). The
 * hint and any error are linked to it, as in `TextField`.
 */
export function SelectField({
  id,
  label,
  hint,
  describedBy,
  value,
  options,
  placeholder,
  onChange,
  error,
  required = false,
  autoFocus = false,
}: {
  id: string;
  label: string;
  hint: string;
  /** The id of other text to read after the hint, such as the form's instructions. */
  describedBy?: string;
  value: string;
  options: { value: string; label: string }[];
  placeholder: string;
  onChange: (value: string) => void;
  error?: string;
  required?: boolean;
  autoFocus?: boolean;
}) {
  const hintId = `${id}-hint`;
  const errorId = `${id}-error`;
  const description = describedBy ? `${hintId} ${describedBy}` : hintId;

  return (
    <div className="field">
      <label htmlFor={id} className="field-label">
        {label}
      </label>
      <p id={hintId} className="field-hint">
        {hint}
      </p>
      <select
        id={id}
        className="field-input"
        value={value}
        autoFocus={autoFocus}
        aria-required={required || undefined}
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? `${errorId} ${description}` : description}
        onChange={(e) => onChange(e.target.value)}
      >
        <option value="">{placeholder}</option>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {error && (
        <p id={errorId} className="field-error">
          {error}
        </p>
      )}
    </div>
  );
}

/** A form's Save and Cancel buttons, with its saving status and form-wide error. */
export function FormActions({
  saveLabel,
  saving,
  error,
  onCancel,
}: {
  saveLabel: string;
  saving: boolean;
  error: string | null;
  onCancel: () => void;
}) {
  return (
    <>
      <div className="form-actions">
        {/* `aria-disabled` rather than `disabled`, so keyboard focus stays put. */}
        <button type="submit" className="button button-primary" aria-disabled={saving}>
          {saveLabel}
        </button>
        <button
          type="button"
          className="button"
          aria-disabled={saving}
          onClick={() => {
            if (!saving) onCancel();
          }}
        >
          Cancel
        </button>
      </div>
      <p className="form-status" role="status">
        {saving ? "Saving…" : ""}
      </p>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </>
  );
}
