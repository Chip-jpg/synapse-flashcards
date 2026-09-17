import { useState } from "react";
import { Dashboard } from "./Dashboard";
import { DeckDetail } from "./DeckDetail";
import { NotesArea } from "./Notes";
import { ReviewSession } from "./ReviewSession";
import "./App.css";

type View =
  | { screen: "decks"; notice: string | null; fromNotes?: boolean }
  | { screen: "deck"; deckId: number; justCreated: boolean }
  | { screen: "session"; sessionId: number; deckName: string }
  | { screen: "notes" };

const DECKS: View = { screen: "decks", notice: null };

function App() {
  const [view, setView] = useState<View>(DECKS);
  // Every screen change after launch comes from a button that disappears, so
  // the new screen takes focus; on first launch focus is left alone.
  const [navigated, setNavigated] = useState(false);
  // Bumped when a restored backup replaces all data, so the dashboard mounts
  // afresh instead of keeping lists loaded from the previous database.
  const [restores, setRestores] = useState(0);

  function show(next: View) {
    setNavigated(true);
    setView(next);
  }

  function startReview(sessionId: number, deckName: string) {
    show({ screen: "session", sessionId, deckName });
  }

  return (
    <main className="app">
      <h1 className="app-title">Synapse</h1>

      <div className="stage">
        {view.screen === "decks" && (
          <Dashboard
            key={restores}
            focusOnLoad={navigated}
            notice={view.notice}
            fromNotes={view.fromNotes ?? false}
            onStart={startReview}
            onOpen={(deckId) => show({ screen: "deck", deckId, justCreated: false })}
            onCreated={(deck) => show({ screen: "deck", deckId: deck.id, justCreated: true })}
            onOpenNotes={() => show({ screen: "notes" })}
            onRestored={(fileName) => {
              setRestores((n) => n + 1);
              show({
                screen: "decks",
                notice: `Backup restored from ${fileName}. Everything shown now comes from that backup.`,
              });
            }}
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
                notice: "Deck archived. Its cards and review history are kept.",
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
            onBack={() => show({ screen: "decks", notice: null, fromNotes: true })}
            onOpenDeck={(deckId) => show({ screen: "deck", deckId, justCreated: false })}
          />
        )}
      </div>
    </main>
  );
}

export default App;
