import { useEffect, useRef, useState } from "react";
import { getDecks, type Deck } from "./api";
import { useT, type MessageKey } from "./i18n";
import type { Place } from "./Shell";

const places: { place: Place; label: MessageKey }[] = [
  { place: "desk", label: "nav.desk" },
  { place: "search", label: "nav.search" },
  { place: "tags", label: "nav.tags" },
  { place: "exams", label: "nav.exams" },
  { place: "notes", label: "nav.notes" },
  { place: "sources", label: "nav.sources" },
  { place: "stats", label: "nav.stats" },
  { place: "archived", label: "nav.archived" },
  { place: "backup", label: "nav.backup" },
  { place: "settings", label: "nav.settings" },
];

function normalize(value: string) {
  return value.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

/** Search places and decks, then run the selected command. */
export function CommandPalette({
  onClose,
  onPlace,
  onDeck,
}: {
  onClose: () => void;
  onPlace: (place: Place) => void;
  onDeck: (deck: Deck, review: boolean) => void;
}) {
  const { t } = useT();
  const dialog = useRef<HTMLDialogElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const [decks, setDecks] = useState<Deck[]>([]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const element = dialog.current!;
    element.showModal();
    input.current?.focus();
    let active = true;
    void getDecks()
      .then((result) => {
        if (active) setDecks(result);
      })
      .catch(() => {});
    return () => {
      active = false;
      element.close();
      previous?.focus();
    };
  }, []);

  const entries = [
    ...places.map(({ place, label }) => ({
      id: `place-${place}`,
      label: t(label),
      run: () => onPlace(place),
    })),
    ...decks.flatMap((deck) => [
      {
        id: `open-${deck.id}`,
        label: t("palette.openDeck", { name: deck.name }),
        run: () => onDeck(deck, false),
      },
      ...(deck.dueCount > 0
        ? [
            {
              id: `review-${deck.id}`,
              label: t("palette.reviewDeck", { name: deck.name }),
              run: () => onDeck(deck, true),
            },
          ]
        : []),
    ]),
  ];
  const words = normalize(query).trim().split(/\s+/).filter(Boolean);
  const matches = entries.filter((entry) =>
    words.every((word) => normalize(entry.label).includes(word)),
  );
  const index = Math.min(selected, matches.length - 1);

  return (
    <dialog
      ref={dialog}
      className="command-palette"
      aria-modal="true"
      aria-labelledby="palette-title"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <h2 id="palette-title">{t("palette.title")}</h2>
      <input
        ref={input}
        role="combobox"
        aria-label={t("palette.title")}
        aria-expanded="true"
        aria-controls="palette-options"
        aria-autocomplete="list"
        aria-activedescendant={
          index >= 0 ? `palette-${matches[index].id}` : undefined
        }
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
          setSelected(0);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onClose();
          } else if (
            event.key === "ArrowDown" ||
            event.key === "ArrowUp"
          ) {
            event.preventDefault();
            if (matches.length) {
              setSelected(
                (index +
                  (event.key === "ArrowDown" ? 1 : -1) +
                  matches.length) %
                  matches.length,
              );
            }
          } else if (event.key === "Enter" && index >= 0) {
            event.preventDefault();
            matches[index].run();
          }
        }}
      />
      <ul id="palette-options" role="listbox" aria-label={t("palette.title")}>
        {matches.map((entry, i) => (
          <li
            key={entry.id}
            id={`palette-${entry.id}`}
            role="option"
            aria-selected={i === index}
            onMouseMove={() => setSelected(i)}
            onClick={entry.run}
          >
            {entry.label}
          </li>
        ))}
      </ul>
      {matches.length === 0 && <p>{t("palette.noMatches")}</p>}
    </dialog>
  );
}
