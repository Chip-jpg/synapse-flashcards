import { useEffect, useId, useState } from "react";
import {
  createExam,
  deleteExam,
  getExamPlan,
  getTags,
  listExams,
  toCommandError,
  updateExam,
  type Exam,
  type ExamPlan,
  type TagSummary,
} from "./api";
import { loadDeckChoices, type DeckChoices } from "./deckChoices";
import { FormActions, SelectField, TextField, useFormSave } from "./forms";
import { PencilIcon, PlayIcon, PlusIcon, TrashIcon } from "./icons";
import type { ReviewedTag } from "./ReviewSession";
import { useUnsavedChanges } from "./unsaved";
import { Message, useFocusOnMount } from "./ui";
import { useStartReview } from "./useStartReview";

type PlannerState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; exams: Exam[]; plans: Record<number, ExamPlan | null> };

/**
 * Fetches the exams and each one's plan; never rejects. A plan that can't be
 * read leaves that exam without numbers rather than failing the screen.
 */
async function loadExams(): Promise<PlannerState> {
  try {
    const exams = await listExams();
    const plans: Record<number, ExamPlan | null> = {};
    await Promise.all(
      exams.map(async (exam) => {
        plans[exam.id] = await getExamPlan(exam.id).catch(() => null);
      }),
    );
    return { status: "ready", exams, plans };
  } catch (err) {
    return { status: "error", message: toCommandError(err).message };
  }
}

/** Today on this computer's calendar, `YYYY-MM-DD`. */
function localToday(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** A `YYYY-MM-DD` date as a medium local date, e.g. "Dec 1, 2026". */
function formatDate(day: string): string {
  const [year, month, date] = day.split("-").map(Number);
  return new Date(year, month - 1, date).toLocaleDateString(undefined, { dateStyle: "medium" });
}

function plural(count: number, unit: string): string {
  return `${count} ${count === 1 ? unit : `${unit}s`}`;
}

/** "in 3 days", "tomorrow", "today" or "2 days ago". */
export function daysLeftText(plan: Pick<ExamPlan, "status" | "daysLeft">): string {
  if (plan.status === "today") return "Today";
  if (plan.status === "finished") return "Finished";
  return plan.daysLeft === 1 ? "Tomorrow" : `In ${plan.daysLeft} days`;
}

/**
 * Exams: a date for a deck or a tag, and what to review each day to be ready.
 * Everything shown is counted by Rust; the plan is an estimate from the
 * schedules and the FSRS parameters in use, not a promise.
 */
export function ExamPlanner({
  onStart,
  onStartTag,
}: {
  onStart: (sessionId: number, deckName: string) => void;
  onStartTag: (tag: ReviewedTag) => void;
}) {
  const [state, setState] = useState<PlannerState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  const [form, setForm] = useState<{ exam: Exam | null } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    loadExams().then((next) => {
      if (current) setState(next);
    });
    return () => {
      current = false;
    };
  }, [attempt]);

  function reload(message: string | null) {
    setState({ status: "loading" });
    setForm(null);
    setNotice(message);
    setAttempt((n) => n + 1);
  }

  if (state.status === "loading") {
    return (
      <p className="loading" role="status">
        Loading your exams…
      </p>
    );
  }

  if (state.status === "error") {
    return (
      <Message focus tone="alert" text={state.message}>
        <button type="button" className="button" onClick={() => reload(null)}>
          Retry
        </button>
      </Message>
    );
  }

  if (form) {
    return (
      <ExamForm
        exam={form.exam}
        onCancel={() => setForm(null)}
        onSaved={(exam, created) =>
          reload(created ? `Exam “${exam.title}” added.` : `Exam “${exam.title}” saved.`)
        }
        onStale={() => reload("That exam is no longer there. Here are your exams as they are now.")}
      />
    );
  }

  return (
    <ExamList
      exams={state.exams}
      plans={state.plans}
      notice={notice}
      onNew={() => {
        setNotice(null);
        setForm({ exam: null });
      }}
      onEdit={(exam) => {
        setNotice(null);
        setForm({ exam });
      }}
      onDeleted={(exam) => reload(`Exam “${exam.title}” deleted.`)}
      onStale={() => reload("That exam is no longer there. Here are your exams as they are now.")}
      onNoneDue={(name) => reload(`Nothing in ${name} is due right now.`)}
      onStart={onStart}
      onStartTag={onStartTag}
    />
  );
}

function ExamList({
  exams,
  plans,
  notice,
  onNew,
  onEdit,
  onDeleted,
  onStale,
  onNoneDue,
  onStart,
  onStartTag,
}: {
  exams: Exam[];
  plans: Record<number, ExamPlan | null>;
  notice: string | null;
  onNew: () => void;
  onEdit: (exam: Exam) => void;
  onDeleted: (exam: Exam) => void;
  onStale: () => void;
  onNoneDue: (scopeName: string) => void;
  onStart: (sessionId: number, deckName: string) => void;
  onStartTag: (tag: ReviewedTag) => void;
}) {
  const screenRef = useFocusOnMount<HTMLElement>(notice === null);
  const noticeRef = useFocusOnMount<HTMLParagraphElement>(notice !== null);
  // The list comes soonest first; finished exams (a past date) sit apart.
  const today = localToday();
  const isFinished = (exam: Exam) => {
    const plan = plans[exam.id];
    return plan ? plan.status === "finished" : exam.examDate < today;
  };
  const upcoming = exams.filter((exam) => !isFinished(exam));
  const finished = exams.filter(isFinished);

  const item = (exam: Exam) => (
    <li key={exam.id}>
      <ExamItem
        exam={exam}
        plan={plans[exam.id] ?? null}
        onEdit={() => onEdit(exam)}
        onDeleted={() => onDeleted(exam)}
        onStale={onStale}
        onNoneDue={() => onNoneDue(exam.scopeName)}
        onStart={onStart}
        onStartTag={onStartTag}
      />
    </li>
  );

  return (
    <section ref={screenRef} className="exams" tabIndex={-1} aria-labelledby="exams-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 id="exams-heading" className="page-title">
            Exams
          </h2>
          <p className="page-intro">
            Give a deck or a tag an exam date and Synapse works out a daily goal: the reviews that
            are due before the exam, plus two reviews for each new card. Readiness estimates how
            much of the material you would remember on the day, from your schedules and the FSRS
            parameters in use; it is an estimate, not a grade.
          </p>
        </div>
        <button type="button" className="button" onClick={onNew}>
          <PlusIcon />
          Add exam
        </button>
      </div>
      {notice && (
        <p ref={noticeRef} className="notice" role="status" tabIndex={-1}>
          {notice}
        </p>
      )}

      {exams.length === 0 ? (
        <p className="empty" role="status">
          No exams yet. Choose Add exam to plan one.
        </p>
      ) : (
        <>
          {upcoming.length > 0 && (
            <section className="deck-section" aria-labelledby="exams-upcoming">
              <h3 id="exams-upcoming" className="section-title">
                Upcoming <span className="count">{upcoming.length}</span>
              </h3>
              <ul className="card-list">{upcoming.map(item)}</ul>
            </section>
          )}
          {finished.length > 0 && (
            <details className="exams-finished">
              <summary>
                Finished <span className="count">{finished.length}</span>
              </summary>
              <ul className="card-list">{finished.map(item)}</ul>
            </details>
          )}
        </>
      )}
    </section>
  );
}

function ExamItem({
  exam,
  plan,
  onEdit,
  onDeleted,
  onStale,
  onNoneDue,
  onStart,
  onStartTag,
}: {
  exam: Exam;
  plan: ExamPlan | null;
  onEdit: () => void;
  onDeleted: () => void;
  onStale: () => void;
  onNoneDue: () => void;
  onStart: (sessionId: number, deckName: string) => void;
  onStartTag: (tag: ReviewedTag) => void;
}) {
  const nameId = `exam-${exam.id}-name`;
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Only a deck exam starts a session; a tag exam reviews the tag.
  const deck = useStartReview(exam.deckId ?? 0, exam.scopeName, onStart, onNoneDue);
  const finished = plan?.status === "finished";
  const canStudy = !exam.scopeArchived && !finished;

  async function remove() {
    if (deleting) return;
    setDeleting(true);
    setError(null);
    try {
      await deleteExam(exam.id);
      onDeleted();
    } catch (err) {
      if (toCommandError(err).kind === "stale") {
        onStale();
        return;
      }
      setError(toCommandError(err).message);
      setDeleting(false);
    }
  }

  return (
    <article className="card-item exam-item" aria-labelledby={nameId}>
      <div className="exam-head">
        <div>
          <h4 id={nameId} className="note-item-title">
            {exam.title}
          </h4>
          <p className="item-meta">
            {formatDate(exam.examDate)} · {exam.deckId !== null ? "Deck" : "Tag"}: {exam.scopeName}
            {plan && ` · ${daysLeftText(plan)}`}
          </p>
        </div>
        <div className="deck-actions">
          {canStudy && (
            <button
              type="button"
              className="button button-tonal button-small"
              aria-describedby={nameId}
              aria-disabled={deck.starting}
              onClick={() =>
                exam.deckId !== null
                  ? deck.start()
                  : exam.tagId !== null && onStartTag({ id: exam.tagId, name: exam.scopeName })
              }
            >
              <PlayIcon />
              Study now
            </button>
          )}
          <button
            type="button"
            className="button button-small"
            aria-describedby={nameId}
            onClick={onEdit}
          >
            <PencilIcon />
            Edit
          </button>
          <button
            type="button"
            className="button button-small"
            aria-describedby={nameId}
            onClick={() => setConfirming(true)}
          >
            <TrashIcon />
            Delete
          </button>
        </div>
      </div>

      {exam.scopeArchived ? (
        <p className="field-hint">
          This exam's deck is archived, so there is nothing to plan. Unarchive it to study again.
        </p>
      ) : plan === null ? (
        <p className="field-hint">The plan for this exam couldn't be read.</p>
      ) : (
        <PlanFigures plan={plan} />
      )}

      {confirming && (
        <div className="confirm confirm-danger" role="group" aria-label={`Delete ${exam.title}?`}>
          <p className="confirm-question">
            Delete the exam “{exam.title}”? Your cards and decks are not touched.
          </p>
          <div className="form-actions">
            <button
              type="button"
              className="button button-danger-soft"
              aria-disabled={deleting}
              onClick={remove}
            >
              Delete exam
            </button>
            <button
              type="button"
              className="button"
              aria-disabled={deleting}
              onClick={() => setConfirming(false)}
            >
              Keep exam
            </button>
          </div>
        </div>
      )}
      {(error || deck.error) && (
        <p className="message-error" role="alert">
          {error ?? deck.error}
        </p>
      )}
    </article>
  );
}

/** Days left, today's goal, the counts and the readiness bar for one plan. */
function PlanFigures({ plan }: { plan: ExamPlan }) {
  const readiness = Math.max(0, Math.min(100, Math.round(plan.readiness)));
  return (
    <>
      <dl className="exam-figures">
        {plan.status !== "finished" && (
          <>
            <div>
              <dt>Days left</dt>
              <dd>{plan.daysLeft}</dd>
            </div>
            <div>
              <dt>Today's goal</dt>
              <dd>{plural(plan.dailyGoal, "review")}</dd>
            </div>
          </>
        )}
        <div>
          <dt>Cards</dt>
          <dd>{plan.cards}</dd>
        </div>
        <div>
          <dt>New</dt>
          <dd>{plan.newCards}</dd>
        </div>
        <div>
          <dt>Due before exam</dt>
          <dd>{plan.dueBeforeExam}</dd>
        </div>
        <div>
          <dt>Below target</dt>
          <dd>{plan.belowTarget}</dd>
        </div>
      </dl>
      <div className="exam-readiness">
        <p className="exam-readiness-label">
          Readiness <strong>{readiness}%</strong>
        </p>
        <div
          className="exam-bar"
          role="progressbar"
          aria-label="Readiness"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={readiness}
        >
          <div className="exam-bar-fill" style={{ width: `${readiness}%` }} />
        </div>
      </div>
    </>
  );
}

/**
 * Adding an exam (a title, a date, and a deck or tag) or changing one's title
 * and date; its deck or tag stays. Rust checks every field.
 */
function ExamForm({
  exam,
  onCancel,
  onSaved,
  onStale,
}: {
  exam: Exam | null;
  onCancel: () => void;
  onSaved: (exam: Exam, created: boolean) => void;
  onStale: () => void;
}) {
  const ids = useId();
  const titleId = `${ids}-title`;
  const dateId = `${ids}-date`;
  const scopeId = `${ids}-scope`;
  const [title, setTitle] = useState(exam?.title ?? "");
  const [date, setDate] = useState(exam?.examDate ?? "");
  const [kind, setKind] = useState<"deck" | "tag">("deck");
  const [scope, setScope] = useState("");
  const [decks, setDecks] = useState<DeckChoices>({ status: "loading" });
  const [tags, setTags] = useState<TagSummary[]>([]);
  const headingRef = useFocusOnMount<HTMLHeadingElement>(true);

  useEffect(() => {
    if (exam) return;
    let current = true;
    loadDeckChoices().then((next) => {
      if (current) setDecks(next);
    });
    getTags()
      .then((list) => {
        if (current) setTags(list);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [exam]);

  useUnsavedChanges(
    exam
      ? title !== exam.title || date !== exam.examDate
      : title !== "" || date !== "" || scope !== "",
  );

  const { saving, submit, fieldError, formError, edited } = useFormSave(
    async () => {
      try {
        if (exam) return { exam: await updateExam(exam.id, title, date), created: false };
        const chosen = Number(scope) || 0;
        const saved = await createExam(
          title,
          date,
          kind === "deck" ? { deckId: chosen } : { tagId: chosen },
        );
        return { exam: saved, created: true };
      } catch (err) {
        if (exam && toCommandError(err).kind === "stale") return null;
        throw err;
      }
    },
    (result) => (result === null ? onStale() : onSaved(result.exam, result.created)),
    { title: titleId, examDate: dateId, examScope: scopeId },
  );

  const deckOptions =
    decks.status === "ready"
      ? decks.decks.map((deck) => ({ value: String(deck.id), label: deck.name }))
      : [];
  const tagOptions = tags.map((tag) => ({ value: String(tag.id), label: tag.name }));
  const dateError = fieldError("examDate");

  return (
    <section className="exam-form" aria-labelledby="exam-form-heading">
      <div className="page-header">
        <div className="page-heading">
          <h2 ref={headingRef} id="exam-form-heading" className="page-title" tabIndex={-1}>
            {exam ? "Edit exam" : "Add exam"}
          </h2>
        </div>
      </div>
      <form className="card-item" noValidate onSubmit={submit}>
        <TextField
          id={titleId}
          label="Title"
          hint="What the exam is called, up to 100 characters."
          value={title}
          error={fieldError("title")}
          required
          onChange={(value) => {
            setTitle(value);
            edited("title");
          }}
        />

        <div className="field">
          <label htmlFor={dateId} className="field-label">
            Exam date
          </label>
          <input
            id={dateId}
            className="field-input"
            type="date"
            value={date}
            aria-required
            aria-invalid={dateError ? true : undefined}
            aria-describedby={dateError ? `${dateId}-error` : undefined}
            onChange={(e) => {
              setDate(e.target.value);
              edited("examDate");
            }}
          />
          {dateError && (
            <p id={`${dateId}-error`} className="field-error">
              {dateError}
            </p>
          )}
        </div>

        {exam ? (
          <p className="field-hint">
            {exam.deckId !== null ? "Deck" : "Tag"}: {exam.scopeName}. The deck or tag of an exam
            can't be changed; delete it and add a new one instead.
          </p>
        ) : (
          <>
            <fieldset className="field exam-kind">
              <legend className="field-label">What the exam covers</legend>
              <label className="check-field">
                <input
                  type="radio"
                  name={`${ids}-kind`}
                  checked={kind === "deck"}
                  onChange={() => {
                    setKind("deck");
                    setScope("");
                    edited("examScope");
                  }}
                />
                A deck
              </label>
              <label className="check-field">
                <input
                  type="radio"
                  name={`${ids}-kind`}
                  checked={kind === "tag"}
                  onChange={() => {
                    setKind("tag");
                    setScope("");
                    edited("examScope");
                  }}
                />
                A tag
              </label>
            </fieldset>
            <SelectField
              id={scopeId}
              label={kind === "deck" ? "Deck" : "Tag"}
              hint={
                kind === "deck"
                  ? "Every active card of this deck is planned."
                  : "Every active card with exactly this tag, in any deck."
              }
              value={scope}
              options={kind === "deck" ? deckOptions : tagOptions}
              placeholder={kind === "deck" ? "Choose a deck" : "Choose a tag"}
              error={fieldError("examScope")}
              required
              onChange={(value) => {
                setScope(value);
                edited("examScope");
              }}
            />
          </>
        )}

        <FormActions
          saveLabel={exam ? "Save exam" : "Add exam"}
          saving={saving}
          error={formError}
          onCancel={onCancel}
        />
      </form>
    </section>
  );
}
