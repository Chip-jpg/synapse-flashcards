import { useState } from "react";
import { createDeck, type DeckDetail } from "./api";
import { FormActions, TextField, useFormSave } from "./forms";

/** Creating a normal deck. All validation happens in Rust; errors come back per field. */
export function CreateDeckForm({
  onCancel,
  onCreated,
}: {
  onCancel: () => void;
  onCreated: (deck: DeckDetail) => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => createDeck(name, description),
    onCreated,
    { name: "deck-name", description: "deck-description" },
  );

  return (
    <form className="card" aria-labelledby="create-deck-heading" noValidate onSubmit={submit}>
      <h2 id="create-deck-heading" className="form-title">
        Create a deck
      </h2>

      <TextField
        id="deck-name"
        label="Deck name"
        hint="Required. Up to 100 characters."
        value={name}
        error={fieldError("name")}
        required
        autoFocus
        onChange={(value) => {
          setName(value);
          edited("name");
        }}
      />
      <TextField
        id="deck-description"
        label="Description"
        hint="Optional. Up to 500 characters."
        value={description}
        error={fieldError("description")}
        onChange={(value) => {
          setDescription(value);
          edited("description");
        }}
      />

      <FormActions saveLabel="Save deck" saving={saving} error={formError} onCancel={onCancel} />
    </form>
  );
}
