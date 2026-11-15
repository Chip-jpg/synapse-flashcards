import { useState } from "react";
import { useT } from "./i18n";
import { createDeck, type DeckDetail } from "./api";
import { FormActions, TextField, useFormSave } from "./forms";
import { useUnsavedChanges } from "./unsaved";

/** Creating a normal deck. All validation happens in Rust; errors come back per field. */
export function CreateDeckForm({
  onCancel,
  onCreated,
}: {
  onCancel: () => void;
  onCreated: (deck: DeckDetail) => void;
}) {
  const { t } = useT();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  useUnsavedChanges(name !== "" || description !== "");
  const { saving, submit, fieldError, formError, edited } = useFormSave(
    () => createDeck(name, description),
    onCreated,
    { name: "deck-name", description: "deck-description" },
  );

  return (
    <form className="card" aria-labelledby="create-deck-heading" noValidate onSubmit={submit}>
      <h2 id="create-deck-heading" className="form-title">
        {t("createDeck.title")}
      </h2>

      <TextField
        id="deck-name"
        label={t("createDeck.name")}
        hint={t("createDeck.nameHint")}
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
        label={t("createDeck.description")}
        hint={t("createDeck.descriptionHint")}
        value={description}
        error={fieldError("description")}
        onChange={(value) => {
          setDescription(value);
          edited("description");
        }}
      />

      <FormActions saveLabel={t("createDeck.save")} saving={saving} error={formError} onCancel={onCancel} />
    </form>
  );
}
