import { getDecks, toCommandError, type Deck } from "./api";

/** The deck choices once loaded, or why they couldn't be. */
export type LoadedDeckChoices =
  | { status: "error"; message: string }
  | { status: "ready"; decks: Deck[] };

export type DeckChoices = { status: "loading" } | LoadedDeckChoices;

/**
 * The decks a card can be added to: active normal decks. The dashboard's list
 * is already active-only; the sample deck is left out because Rust refuses
 * cards for it (and still does, whatever is sent). Never rejects.
 */
export async function loadDeckChoices(): Promise<LoadedDeckChoices> {
  try {
    const decks = await getDecks();
    return { status: "ready", decks: decks.filter((deck) => !deck.isSample) };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}
