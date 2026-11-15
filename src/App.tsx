import { useEffect, useState } from "react";
import { closeWindow, holdWindowClose } from "./api";
import { ArchivedDecks } from "./ArchivedDecks";
import { Backup } from "./Backup";
import { Dashboard } from "./Dashboard";
import { useT, type MessageKey } from "./i18n";
import { DeckDetail } from "./DeckDetail";
import { ExamPlanner } from "./ExamPlanner";
import { NotesArea } from "./Notes";
import { ReviewSession, type ReviewedTag } from "./ReviewSession";
import { NEW_SEARCH, SearchScreen, type SearchCriteria } from "./Search";
import { Settings } from "./Settings";
import { Shell, type Place } from "./Shell";
import { TagsScreen } from "./Tags";
import { SourcesArea, type SourceNoteDraft } from "./Sources";
import { StatisticsScreen } from "./Statistics";
import { DiscardDialog, UnsavedChangesProvider, useUnsavedTracker } from "./unsaved";
import "./App.css";

/** A search left to open one of its results, and that result, to come back to. */
type FromSearch = { criteria: SearchCriteria; returnTo: string };

type View =
  | { screen: "decks"; notice: string | null; fromNotes?: boolean }
  | { screen: "deck"; deckId: number; justCreated: boolean; cardId?: number; search?: FromSearch }
  | { screen: "session"; sessionId: number; deckName: string }
  | { screen: "tagReview"; tag: ReviewedTag }
  | { screen: "search"; criteria: SearchCriteria; returnTo: string | null }
  | {
      screen: "notes";
      noteId?: number;
      search?: FromSearch;
      /** The source the note was opened from, to go back to. */
      fromSource?: number;
      /** A new note being written from a source, to go back to if cancelled. */
      draft?: SourceNoteDraft;
    }
  | { screen: "sources"; sourceId?: number; notice?: string | null }
  | { screen: "tags" }
  | { screen: "exams" }
  | { screen: "stats" }
  | { screen: "archived" }
  | { screen: "backup" }
  | { screen: "settings" };

const DECKS: View = { screen: "decks", notice: null };

/** The screen each sidebar place opens. */
const PLACE_VIEWS: Record<Place, View> = {
  desk: DECKS,
  search: { screen: "search", criteria: NEW_SEARCH, returnTo: null },
  tags: { screen: "tags" },
  exams: { screen: "exams" },
  notes: { screen: "notes" },
  sources: { screen: "sources" },
  stats: { screen: "stats" },
  archived: { screen: "archived" },
  backup: { screen: "backup" },
  settings: { screen: "settings" },
};

/** Which sidebar place a screen belongs to, and where it is within it. */
function locate(view: View, t: (key: MessageKey) => string): { place: Place; atPlace: boolean; trail: string[] } {
  switch (view.screen) {
    case "decks":
      return { place: "desk", atPlace: true, trail: ["Synapse", t("app.studyDesk")] };
    case "deck":
      return view.search
        ? { place: "search", atPlace: false, trail: [t("app.search"), t("app.deck")] }
        : { place: "desk", atPlace: false, trail: [t("app.studyDesk"), t("app.deck")] };
    case "session":
    case "tagReview":
      return { place: "desk", atPlace: false, trail: [t("app.studyDesk"), t("app.review")] };
    case "search":
      return { place: "search", atPlace: true, trail: ["Synapse", t("app.search")] };
    case "tags":
      return { place: "tags", atPlace: true, trail: ["Synapse", t("app.tags")] };
    case "notes":
      return view.search
        ? { place: "search", atPlace: false, trail: [t("app.search"), t("app.note")] }
        : view.fromSource !== undefined || view.draft
          ? { place: "sources", atPlace: false, trail: [t("app.sources"), t("app.note")] }
          : { place: "notes", atPlace: true, trail: ["Synapse", t("app.notes")] };
    case "sources":
      return view.sourceId === undefined
        ? { place: "sources", atPlace: true, trail: ["Synapse", t("app.sources")] }
        : { place: "sources", atPlace: false, trail: [t("app.sources"), t("app.source")] };
    case "exams":
      return { place: "exams", atPlace: true, trail: ["Synapse", t("app.exams")] };
    case "stats":
      return { place: "stats", atPlace: true, trail: ["Synapse", t("app.statistics")] };
    case "archived":
      return { place: "archived", atPlace: true, trail: ["Synapse", t("app.archived")] };
    case "backup":
      return { place: "backup", atPlace: true, trail: ["Synapse", t("app.backup")] };
    case "settings":
      return { place: "settings", atPlace: true, trail: ["Synapse", t("app.settings")] };
  }
}

function App() {
  const { t } = useT();
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
  // Whether an open form holds text that hasn't been saved.
  const { unsaved, report } = useUnsavedTracker();
  // Where the user was going when asked whether to discard unsaved changes: a
  // sidebar place, or "close" for closing the window. Null when not asking.
  const [leaving, setLeaving] = useState<Place | "close" | null>(null);

  // While something is unsaved, closing the window asks first. Otherwise
  // nothing is held, and the window closes as it always has.
  useEffect(() => {
    if (!unsaved) return;
    return holdWindowClose(() => setLeaving("close"));
  }, [unsaved]);

  function show(next: View) {
    setNavigated(true);
    setView(next);
  }

  function startReview(sessionId: number, deckName: string) {
    show({ screen: "session", sessionId, deckName });
  }

  function backToSearch(search: FromSearch) {
    show({ screen: "search", criteria: search.criteria, returnTo: search.returnTo });
  }

  function goTo(next: Place) {
    setVisits((n) => n + 1);
    show(PLACE_VIEWS[next]);
  }

  function discard() {
    setLeaving(null);
    if (leaving === "close") void closeWindow();
    else if (leaving !== null) goTo(leaving);
  }

  const { place, atPlace, trail } = locate(view, t);

  return (
    <UnsavedChangesProvider value={report}>
      <Shell
        place={place}
        atPlace={atPlace}
        trail={trail}
        locked={locked}
        onNavigate={(next) => (unsaved ? setLeaving(next) : goTo(next))}
      >
        {view.screen === "decks" && (
          <Dashboard
            key={`${restores}:${visits}`}
            focusOnLoad={navigated}
            notice={view.notice}
            fromNotes={view.fromNotes ?? false}
            onStart={startReview}
            onStartTag={(tag) => show({ screen: "tagReview", tag })}
            onOpen={(deckId) => show({ screen: "deck", deckId, justCreated: false })}
            onCreated={(deck) => show({ screen: "deck", deckId: deck.id, justCreated: true })}
            onOpenNotes={() => show({ screen: "notes" })}
            onOpenExams={() => goTo("exams")}
          />
        )}
        {view.screen === "deck" && (
          <DeckDetail
            key={view.deckId}
            deckId={view.deckId}
            justCreated={view.justCreated}
            focusCardId={view.cardId ?? null}
            backLabel={view.search ? t("app.backSearch") : undefined}
            onBack={() => (view.search ? backToSearch(view.search) : show(DECKS))}
            onStart={startReview}
            onArchived={() =>
              show({
                screen: "decks",
                notice:
                  t("app.archivedNotice"),
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
        {view.screen === "tagReview" && (
          <ReviewSession key={`tag:${view.tag.id}`} tag={view.tag} onExit={() => show(DECKS)} />
        )}
        {view.screen === "search" && (
          <SearchScreen
            key={`${restores}:${visits}`}
            initial={view.criteria}
            returnTo={view.returnTo}
            onOpenCard={(criteria, deckId, cardId, returnTo) =>
              show({
                screen: "deck",
                deckId,
                justCreated: false,
                cardId,
                search: { criteria, returnTo },
              })
            }
            onOpenNote={(criteria, noteId, returnTo) =>
              show({ screen: "notes", noteId, search: { criteria, returnTo } })
            }
          />
        )}
        {view.screen === "notes" && (
          <NotesArea
            key={visits}
            onBack={() => show({ screen: "decks", notice: null, fromNotes: true })}
            onOpenDeck={(deckId) => show({ screen: "deck", deckId, justCreated: false })}
            onOpenSource={(sourceId) => show({ screen: "sources", sourceId })}
            openNote={
              view.search && view.noteId !== undefined
                ? {
                    noteId: view.noteId,
                    backLabel: t("app.backSearch"),
                    onBack: () => backToSearch(view.search!),
                  }
                : view.fromSource !== undefined && view.noteId !== undefined
                  ? {
                      noteId: view.noteId,
                      backLabel: t("app.backSource"),
                      onBack: () => show({ screen: "sources", sourceId: view.fromSource }),
                    }
                  : undefined
            }
            newFromSource={
              view.draft
                ? {
                    draft: view.draft,
                    onCancel: () => show({ screen: "sources", sourceId: view.draft!.sourceId }),
                  }
                : undefined
            }
          />
        )}
        {view.screen === "sources" && (
          <SourcesArea
            key={`${restores}:${visits}:${view.sourceId ?? ""}`}
            openSource={
              view.sourceId === undefined
                ? undefined
                : { sourceId: view.sourceId, notice: view.notice ?? null }
            }
            onBack={() => show(DECKS)}
            onOpenNote={(noteId, sourceId) =>
              show({ screen: "notes", noteId, fromSource: sourceId })
            }
            onMakeNote={(draft) => show({ screen: "notes", draft })}
          />
        )}
        {view.screen === "tags" && (
          <TagsScreen
            key={`${restores}:${visits}`}
            onFind={(tag) =>
              show({ screen: "search", criteria: { ...NEW_SEARCH, tag }, returnTo: null })
            }
          />
        )}
        {view.screen === "exams" && (
          <ExamPlanner
            key={`${restores}:${visits}`}
            onStart={startReview}
            onStartTag={(tag) => show({ screen: "tagReview", tag })}
          />
        )}
        {view.screen === "stats" && <StatisticsScreen key={`${restores}:${visits}`} />}
        {view.screen === "archived" && <ArchivedDecks key={visits} />}
        {view.screen === "settings" && <Settings key={visits} />}
        {view.screen === "backup" && (
          <Backup
            key={visits}
            onLock={setLocked}
            onRestored={(fileName) => {
              setLocked(false);
              setRestores((n) => n + 1);
              show({
                screen: "decks",
                notice: t("app.backupNotice", { name: fileName }),
              });
            }}
          />
        )}
      </Shell>
      <DiscardDialog
        leaving={leaving === null ? null : leaving === "close" ? "close" : "navigate"}
        onDiscard={discard}
        onKeep={() => setLeaving(null)}
      />
    </UnsavedChangesProvider>
  );
}

export default App;
