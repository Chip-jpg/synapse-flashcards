import { useEffect, useState } from "react";
import { useT } from "./i18n";
import {
  getDecks,
  getExamPlan,
  getTagsDue,
  listExams,
  getToday,
  toCommandError,
  type Deck,
  type DeckDetail,
  type ExamPlan,
  type TagDue,
  type TodaySummary,
} from "./api";
import { CreateDeckForm } from "./CreateDeckForm";
import {
  ArrowRightIcon,
  BookIcon,
  CalendarIcon,
  CheckIcon,
  PlayIcon,
  PlusIcon,
} from "./icons";
import type { ReviewedTag } from "./ReviewSession";
import { Message, useFocusOnMount } from "./ui";
import { useStartReview } from "./useStartReview";

type DecksState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; decks: Deck[]; today: TodaySummary | null; tags: TagDue[]; exam: ExamPlan | null };

/** Today on this computer's calendar, `YYYY-MM-DD`. */
function localToday(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** The plan of the soonest exam that isn't over, or null; never rejects. */
async function nextExam(): Promise<ExamPlan | null> {
  try {
    const today = localToday();
    // `list_exams` comes soonest first.
    const exam = (await listExams()).find((e) => e.examDate >= today && !e.scopeArchived);
    if (!exam) return null;
    const plan = await getExamPlan(exam.id);
    return plan.status === "finished" ? null : plan;
  } catch {
    return null;
  }
}

/**
 * Fetches the active decks, today's review count and the tags with cards due,
 * and describes the outcome; never rejects. Today's count and the tags are
 * side notes: if they can't be read, the desk shows without them rather than
 * failing.
 */
async function loadDecks(): Promise<DecksState> {
  try {
    const [decks, today, tags, exam] = await Promise.all([
      getDecks(),
      getToday().catch(() => null),
      getTagsDue().catch(() => []),
      nextExam(),
    ]);
    return { status: "ready", decks, today, tags, exam };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/**
 * The Study Desk: the deck due for review first, every active deck with its
 * due count, creating a deck, and the way into notes.
 */
export function Dashboard({
  focusOnLoad,
  notice: initialNotice,
  fromNotes,
  onStart,
  onStartTag = () => {},
  onOpen,
  onCreated,
  onOpenNotes,
  onOpenExams = () => {},
}: {
  focusOnLoad: boolean;
  /** A confirmation to announce when the dashboard appears (e.g. after archiving). */
  notice: string | null;
  /** The user just left the notes area, so focus returns to "Open notes". */
  fromNotes: boolean;
  onStart: (sessionId: number, deckName: string) => void;
  /** Review the due cards with a tag, from every deck. */
  onStartTag?: (tag: ReviewedTag) => void;
  onOpen: (deckId: number) => void;
  onCreated: (deck: DeckDetail) => void;
  onOpenNotes: () => void;
  /** Open the Exams screen. */
  onOpenExams?: () => void;
}) {
  const { t } = useT();
  const [state, setState] = useState<DecksState>({ status: "loading" });
  // Bumping this re-runs the load effect (Retry, or refreshing counts).
  const [attempt, setAttempt] = useState(0);
  const [creating, setCreating] = useState(false);
  // After cancelling the create form, focus returns to "Create deck".
  const [cancelledCreate, setCancelledCreate] = useState(false);
  // Shown until the list reloads or the create form opens.
  const [notice, setNotice] = useState(initialNotice);

  useEffect(() => {
    let current = true;
    loadDecks().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  /**
   * Reloads the deck list. Goes back to "loading" first: until the fresh list
   * arrives the old due counts are wrong.
   */
  function reload() {
    setState({ status: "loading" });
    setCancelledCreate(false);
    setNotice(null);
    setAttempt((n) => n + 1);
  }

  const moveFocus = focusOnLoad || attempt > 0;
  const focus = cancelledCreate
    ? "create"
    : notice
      ? "notice"
      : fromNotes && attempt === 0
        ? "notes"
        : moveFocus
          ? "list"
          : null;

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        {t("desk.loading")}
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus={moveFocus} tone="alert" text={state.message}>
        <button type="button" className="button" onClick={() => reload()}>
          {t("desk.retry")}
        </button>
      </Message>
    );
  }

  if (creating) {
    return (
      <CreateDeckForm
        onCancel={() => {
          setCancelledCreate(true);
          setCreating(false);
        }}
        onCreated={onCreated}
      />
    );
  }

  return (
    <StudyDesk
      decks={state.decks}
      today={state.today}
      tags={state.tags}
      exam={state.exam}
      notice={notice}
      focus={focus}
      onCreate={() => {
        setNotice(null);
        setCreating(true);
      }}
      onStart={onStart}
      onStartTag={onStartTag}
      onOpen={onOpen}
      onOpenNotes={onOpenNotes}
      onOpenExams={onOpenExams}
      onChanged={() => reload()}
    />
  );
}

function StudyDesk({
  decks,
  today,
  tags,
  exam,
  notice,
  focus,
  onCreate,
  onStart,
  onStartTag,
  onOpen,
  onOpenNotes,
  onOpenExams,
  onChanged,
}: {
  decks: Deck[];
  today: TodaySummary | null;
  tags: TagDue[];
  exam: ExamPlan | null;
  notice: string | null;
  /** What takes focus when the desk appears, if anything. */
  focus: "list" | "create" | "notice" | "notes" | null;
  onCreate: () => void;
  onStart: (sessionId: number, deckName: string) => void;
  onStartTag: (tag: ReviewedTag) => void;
  onOpen: (deckId: number) => void;
  onOpenNotes: () => void;
  onOpenExams: () => void;
  onChanged: () => void;
}) {
  const { t } = useT();
  const ref = useFocusOnMount<HTMLElement>(focus === "list");
  const createRef = useFocusOnMount<HTMLButtonElement>(focus === "create");
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(focus === "notice");
  // The first deck with cards due is the one place to begin, so it leads the
  // desk with the screen's only filled Start review button.
  const next = decks.find((deck) => deck.dueCount > 0);

  return (
    <section ref={ref} className="desk" tabIndex={-1} aria-labelledby="desk-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="desk-heading" className="page-title">
            {t("nav.desk")}
          </h2>
          <p className="page-intro">
            {t("desk.intro")}
          </p>
          {today && <TodayLine today={today} />}
        </div>
        <button ref={createRef} type="button" className="button" onClick={onCreate}>
          <PlusIcon />
          {t("desk.createDeck")}
        </button>
      </div>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {exam && <NextExam plan={exam} onOpen={onOpenExams} />}

      {decks.length === 0 ? (
        <p className="empty" role="status">
          {t("desk.empty")}
        </p>
      ) : (
        <>
          {next ? (
            <NextReview deck={next} onStart={onStart} onOpen={onOpen} onChanged={onChanged} />
          ) : (
            <NothingDue />
          )}

          <section className="deck-section" aria-labelledby="deck-list-heading">
            <h3 id="deck-list-heading" className="section-title">
              {t("desk.decks")} <span className="count">{decks.length}</span>
            </h3>
            <ul className="deck-list">
              {decks.map((deck) => (
                <li key={deck.id}>
                  <DeckRow deck={deck} onStart={onStart} onOpen={onOpen} onChanged={onChanged} />
                </li>
              ))}
            </ul>
          </section>

          <TagReviews tags={tags} onStart={onStartTag} />
        </>
      )}

      <NotesLink focus={focus === "notes"} onOpen={onOpenNotes} />
    </section>
  );
}

/**
 * How much reviewing happened today: ratings recorded, and the different cards
 * they were for, which differ once a card is rated more than once.
 */
function TodayLine({ today }: { today: TodaySummary }) {
  const { t, plural } = useT();
  const { reviews, cards } = today;
  return (
    <p className="today-line">
      {reviews === 0
        ? t("desk.reviewZero")
        : t("desk.today", { reviews, reviewWord: t(reviews === 1 ? "desk.reviewOne" : "desk.reviewOther"), cardsText: plural(cards, "common.cardOne", "common.cardOther") })}
    </p>
  );
}

/** The soonest exam that isn't over, with its goal for today and its readiness. */
function NextExam({ plan, onOpen }: { plan: ExamPlan; onOpen: () => void }) {
  const { t } = useT();
  const readiness = Math.round(plan.readiness);
  const days = plan.status === "today" ? t("desk.examToday") : plan.daysLeft === 1 ? t("desk.examTomorrow") : t("desk.examInDays", { count: plan.daysLeft });
  return (
    <section className="panel panel-row next-exam" aria-labelledby="next-exam-heading">
      <span className="section-icon section-icon-amber" aria-hidden="true">
        <CalendarIcon />
      </span>
      <div className="panel-row-text">
        <h3 id="next-exam-heading" className="section-title">
          {t("desk.nextExam", { name: plan.title })}
        </h3>
        <p className="field-hint">
          {t("desk.examGoal", { days, count: plan.dailyGoal, reviewWord: t(plan.dailyGoal === 1 ? "desk.reviewOne" : "desk.reviewOther"), readiness })}
        </p>
      </div>
      <button type="button" className="button button-small" onClick={onOpen}>
        {t("desk.openPlanner")}
        <ArrowRightIcon />
      </button>
    </section>
  );
}

/** The first deck with cards due, offered as the place to begin. */
function NextReview({
  deck,
  onStart,
  onOpen,
  onChanged,
}: {
  deck: Deck;
  onStart: (sessionId: number, deckName: string) => void;
  onOpen: (deckId: number) => void;
  onChanged: () => void;
}) {
  const { starting, error, start } = useStartReview(deck.id, deck.name, onStart, onChanged);
  const { t, plural } = useT();
  const due = deck.dueCount;

  return (
    <section className="focus-card" aria-labelledby="next-review-name">
      {/* The name comes first, so moving by heading reaches it before its
          details; the tag and count are drawn above it (see `.focus-meta`). */}
      <div>
        <h3 id="next-review-name" className="focus-title">
          {deck.name}
        </h3>
        {deck.description && <p className="focus-description">{deck.description}</p>}
      </div>
      <div className="focus-meta">
        <p className="tag tag-primary">{t("desk.ready")}</p>
        <p className="due-line">
          <span className="dot" aria-hidden="true" />
          {plural(due, "desk.cardDueOne", "desk.cardDueOther")}
        </p>
      </div>
      <div className="focus-actions">
        <button
          type="button"
          className="button button-primary button-large focus-start"
          aria-describedby="next-review-name"
          aria-disabled={starting}
          onClick={start}
        >
          <PlayIcon />
          {t("desk.startReview")}
        </button>
        {/* The sample deck can be reviewed but not opened for editing. */}
        {!deck.isSample && (
          <button
            type="button"
            className="button button-large"
            aria-describedby="next-review-name"
            onClick={() => onOpen(deck.id)}
          >
            {t("desk.openDeck")}
          </button>
        )}
      </div>
      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

/** In place of the next review when no deck has cards due. */
function NothingDue() {
  const { t } = useT();
  return (
    <section className="focus-card focus-card-calm" aria-labelledby="nothing-due-heading">
      <div>
        <h3 id="nothing-due-heading" className="focus-title">
          {t("desk.noCardsNow")}
        </h3>
        <p className="focus-description">
          {t("desk.nothingDueDescription")}
        </p>
      </div>
      <div className="focus-meta">
        <p className="tag tag-sage">
          <CheckIcon />
          {t("desk.allCaughtUp")}
        </p>
      </div>
    </section>
  );
}

/**
 * Reviewing by tag: each tag with cards due, from however many decks, and a
 * way to review just those. Shown only when some tag has cards due.
 */
function TagReviews({
  tags,
  onStart,
}: {
  tags: TagDue[];
  onStart: (tag: ReviewedTag) => void;
}) {
  const { t, plural } = useT();
  const due = tags.filter((tag) => tag.dueCount > 0);
  if (due.length === 0) return null;

  return (
    <section className="deck-section" aria-labelledby="tag-review-heading">
      <h3 id="tag-review-heading" className="section-title">
        {t("desk.reviewByTag")} <span className="count">{due.length}</span>
      </h3>
      <p className="field-hint">
        {t("desk.tagDescription")}
      </p>
      <ul className="deck-list">
        {due.map((tag) => {
          const nameId = `tag-review-${tag.id}`;
          return (
            <li key={tag.id}>
              <article className="deck-row" aria-labelledby={nameId}>
                <div className="deck-row-main">
                  <h4 id={nameId} className="deck-name">
                    {tag.name}
                  </h4>
                  <p className="due-line">
                    <span className="dot" aria-hidden="true" />
                    {t("desk.tagDue", { cards: plural(tag.dueCount, "common.cardOne", "common.cardOther"), decks: plural(tag.decks, "desk.deckOne", "desk.deckOther") })}
                  </p>
                </div>
                <div className="deck-actions">
                  <button
                    type="button"
                    className="button button-tonal button-small"
                    aria-describedby={nameId}
                    onClick={() => onStart({ id: tag.id, name: tag.name })}
                  >
                    <PlayIcon />
                    {t("desk.reviewTag")}
                  </button>
                </div>
              </article>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** The way into the notes area. */
function NotesLink({ focus, onOpen }: { focus: boolean; onOpen: () => void }) {
  const { t } = useT();
  const openRef = useFocusOnMount<HTMLButtonElement>(focus);

  return (
    <section className="panel panel-row" aria-labelledby="notes-link-heading">
      <span className="section-icon section-icon-amber" aria-hidden="true">
        <BookIcon />
      </span>
      <div className="panel-row-text">
        <h3 id="notes-link-heading" className="section-title">
          {t("desk.notes")}
        </h3>
        <p id="notes-link-hint" className="field-hint">
          {t("desk.notesDescription")}
        </p>
      </div>
      <button
        ref={openRef}
        type="button"
        className="button button-small"
        aria-describedby="notes-link-hint"
        onClick={onOpen}
      >
        {t("desk.openNotes")}
        <ArrowRightIcon />
      </button>
    </section>
  );
}

function DeckRow({
  deck,
  onStart,
  onOpen,
  onChanged,
}: {
  deck: Deck;
  onStart: (sessionId: number, deckName: string) => void;
  onOpen: (deckId: number) => void;
  onChanged: () => void;
}) {
  const { starting, error, start } = useStartReview(deck.id, deck.name, onStart, onChanged);
  const { t, plural } = useT();
  const nameId = `deck-${deck.id}-name`;
  const due = deck.dueCount;
  // The sample deck can be reviewed but not opened for editing.
  const canOpen = !deck.isSample;

  return (
    <article className="deck-row" aria-labelledby={nameId}>
      {/* The name comes first, so moving by heading reaches it before its due
          count; the count is drawn above it (see `.deck-row-main`). */}
      <div className="deck-row-main">
        <h4 id={nameId} className="deck-name">
          {deck.name}
        </h4>
        <p className={due === 0 ? "due-line due-line-idle" : "due-line"}>
          {due > 0 && <span className="dot" aria-hidden="true" />}
          {due === 0
            ? t("desk.noCardsNow")
            : plural(due, "desk.cardDueOne", "desk.cardDueOther")}
        </p>
        {deck.description && <p className="deck-description">{deck.description}</p>}
      </div>

      {(due > 0 || canOpen) && (
        <div className="deck-actions">
          {due > 0 && (
            <button
              type="button"
              className="button button-tonal button-small"
              aria-describedby={nameId}
              aria-disabled={starting}
              onClick={start}
            >
              <PlayIcon />
              {t("desk.startReview")}
            </button>
          )}
          {canOpen && (
            <button
              type="button"
              className="button button-small"
              aria-describedby={nameId}
              onClick={() => onOpen(deck.id)}
            >
              {t("desk.openDeck")}
            </button>
          )}
        </div>
      )}

      {error && (
        <p className="message-error" role="alert">
          {error}
        </p>
      )}
    </article>
  );
}
