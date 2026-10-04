import { useState } from "react";
import { ArchivedDecks } from "./ArchivedDecks";
import { Backup } from "./Backup";
import { Dashboard } from "./Dashboard";
import { DeckDetail } from "./DeckDetail";
import { NotesArea } from "./Notes";
import { ReviewSession } from "./ReviewSession";
import { Shell, type Place } from "./Shell";
import "./App.css";

type View =
  | { screen: "decks"; notice: string | null; fromNotes?: boolean }
  | { screen: "deck"; deckId: number; justCreated: boolean }
  | { screen: "session"; sessionId: number; deckName: string }
  | { screen: "notes" }
  | { screen: "archived" }
  | { screen: "backup" };

const DECKS: View = { screen: "decks", notice: null };

/** The screen each sidebar place opens. */
const PLACE_VIEWS: Record<Place, View> = {
  desk: DECKS,
  notes: { screen: "notes" },
  archived: { screen: "archived" },
  backup: { screen: "backup" },
};

/** Which sidebar place a screen belongs to, and where it is within it. */
function locate(view: View): { place: Place; atPlace: boolean; trail: string[] } {
  switch (view.screen) {
    case "decks":
      return { place: "desk", atPlace: true, trail: ["Synapse", "Study Desk"] };
    case "deck":
      return { place: "desk", atPlace: false, trail: ["Study Desk", "Deck"] };
    case "session":
      return { place: "desk", atPlace: false, trail: ["Study Desk", "Review"] };
    case "notes":
      return { place: "notes", atPlace: true, trail: ["Synapse", "Notes"] };
    case "archived":
      return { place: "archived", atPlace: true, trail: ["Synapse", "Archived decks"] };
    case "backup":
      return { place: "backup", atPlace: true, trail: ["Synapse", "Backup & restore"] };
  }
}

function App() {
  const [view, setView] = useState<View>(DECKS);
  // Every screen change after launch comes from a button press, so the new
  // screen takes focus; on first launch focus is left alone.
  const [navigated, setNavigated] = useState(false);
  // Bumped when a restored backup replaces all data, so the dashboard mounts
  // afresh instead of keeping lists loaded from the previous database.
  const [restores, setRestores] = useState(0);
  // Bumped by every sidebar press, so the chosen place opens afresh at its own
  // screen, even when it is already showing something inside it.
  const [visits, setVisits] = useState(0);
  // While a backup is being checked, waits for confirmation, or is being
  // restored, the sidebar is inert: nothing may leave that screen mid-restore.
  const [locked, setLocked] = useState(false);

  function show(next: View) {
    setNavigated(true);
    setView(next);
  }

  function startReview(sessionId: number, deckName: string) {
    show({ screen: "session", sessionId, deckName });
  }

  const { place, atPlace, trail } = locate(view);

  return (
    <Shell
      place={place}
      atPlace={atPlace}
      trail={trail}
      locked={locked}
      onNavigate={(next) => {
        setVisits((n) => n + 1);
        show(PLACE_VIEWS[next]);
      }}
    >
      {view.screen === "decks" && (
        <Dashboard
          key={`${restores}:${visits}`}
          focusOnLoad={navigated}
          notice={view.notice}
          fromNotes={view.fromNotes ?? false}
          onStart={startReview}
          onOpen={(deckId) => show({ screen: "deck", deckId, justCreated: false })}
          onCreated={(deck) => show({ screen: "deck", deckId: deck.id, justCreated: true })}
          onOpenNotes={() => show({ screen: "notes" })}
        />
      )}
      {view.screen === "deck" && (
        <DeckDetail
          key={view.deckId}
          deckId={view.deckId}
          justCreated={view.justCreated}
          onBack={() => show(DECKS)}
          onStart={startReview}
          onArchived={() =>
            show({
              screen: "decks",
              notice:
                "Deck archived. Its cards and review history are kept under Archived decks, " +
                "where you can unarchive it.",
            })
          }
        />
      )}
      {view.screen === "session" && (
        <ReviewSession
          key={view.sessionId}
          sessionId={view.sessionId}
          deckName={view.deckName}
          onExit={() => show(DECKS)}
        />
      )}
      {view.screen === "notes" && (
        <NotesArea
          key={visits}
          onBack={() => show({ screen: "decks", notice: null, fromNotes: true })}
          onOpenDeck={(deckId) => show({ screen: "deck", deckId, justCreated: false })}
        />
      )}
      {view.screen === "archived" && <ArchivedDecks key={visits} />}
      {view.screen === "backup" && (
        <Backup
          key={visits}
          onLock={setLocked}
          onRestored={(fileName) => {
            setLocked(false);
            setRestores((n) => n + 1);
            show({
              screen: "decks",
              notice: `Backup restored from ${fileName}. Everything shown now comes from that backup.`,
            });
          }}
        />
      )}
    </Shell>
  );
}

export default App;
