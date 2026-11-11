//! The exam planner's core: an exam is a date the user gives a deck or a tag.
//! Synapse then plans daily work so everything in scope is reviewed before
//! the date, and reports readiness (migration 0021).
//!
//! Nothing here changes a card, a schedule or a review log. The plan is
//! computed afresh from the cards' stored FSRS state each time it is asked
//! for, with the parameters in use (`optimizer.rs`) and each deck's desired
//! retention (`retention.rs`).
//!
//! Dates are the user's own calendar dates: `now` and the time zone say what
//! "today" is, as for the Study Desk's count (`study::day_bounds`).
//!
//! Also here: the study calendar, reviews per local day and the streak.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{to_db_time, DbError};
use crate::optimizer;
use crate::retention;
use crate::study::start_of_day;

/// The longest exam title, in characters.
pub const TITLE_MAX_CHARS: usize = 100;

/// How many days the study calendar covers, today included.
pub const CALENDAR_DAYS: i64 = 365;

const MS_PER_DAY: f64 = 86_400_000.0;

/// FSRS-6's default forgetting-curve decay, used if the parameters in use
/// hold no 21st value.
const DEFAULT_DECAY: f32 = 0.1542;

/// Why an exam was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExamProblem {
    TitleBlank,
    TitleTooLong,
    /// Not a real calendar date written `YYYY-MM-DD`.
    DateInvalid,
    /// Neither or both of a deck and a tag were given.
    ScopeNeeded,
    /// The deck is missing or archived, or the tag doesn't exist.
    ScopeGone,
}

#[derive(Debug)]
pub enum ExamError {
    Invalid(ExamProblem),
    /// No such active exam (never existed, or already deleted).
    NotFound,
    Internal(DbError),
}

impl From<sqlx::Error> for ExamError {
    fn from(err: sqlx::Error) -> Self {
        ExamError::Internal(Box::new(err))
    }
}

impl From<DbError> for ExamError {
    fn from(err: DbError) -> Self {
        ExamError::Internal(err)
    }
}

/// What an exam covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Deck(i64),
    Tag(i64),
}

impl Scope {
    /// Exactly one of a deck and a tag, else [`ExamProblem::ScopeNeeded`].
    pub fn from_ids(deck_id: Option<i64>, tag_id: Option<i64>) -> Result<Scope, ExamError> {
        match (deck_id, tag_id) {
            (Some(deck), None) => Ok(Scope::Deck(deck)),
            (None, Some(tag)) => Ok(Scope::Tag(tag)),
            _ => Err(ExamError::Invalid(ExamProblem::ScopeNeeded)),
        }
    }
}

/// An exam as the exams list shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exam {
    pub id: i64,
    pub title: String,
    /// `YYYY-MM-DD`, on the user's calendar.
    pub exam_date: String,
    pub deck_id: Option<i64>,
    pub tag_id: Option<i64>,
    /// The deck's or tag's name.
    pub scope_name: String,
    /// Whether the exam's deck is archived (its plan then has no cards).
    pub scope_archived: bool,
    pub created_at: String,
}

#[derive(sqlx::FromRow)]
struct ExamRow {
    id: i64,
    title: String,
    exam_date: String,
    deck_id: Option<i64>,
    tag_id: Option<i64>,
    scope_name: Option<String>,
    scope_archived: i64,
    created_at: String,
}

impl From<ExamRow> for Exam {
    fn from(row: ExamRow) -> Exam {
        Exam {
            id: row.id,
            title: row.title,
            exam_date: row.exam_date,
            deck_id: row.deck_id,
            tag_id: row.tag_id,
            scope_name: row.scope_name.unwrap_or_default(),
            scope_archived: row.scope_archived != 0,
            created_at: row.created_at,
        }
    }
}

const EXAM_SELECT: &str = "SELECT e.id, e.title, e.exam_date, e.deck_id, e.tag_id,
        COALESCE(d.name, t.name) AS scope_name,
        CASE WHEN d.archived_at IS NOT NULL THEN 1 ELSE 0 END AS scope_archived,
        e.created_at
     FROM exams e
     LEFT JOIN decks d ON d.id = e.deck_id
     LEFT JOIN tags t ON t.id = e.tag_id";

fn checked_title(title: &str) -> Result<String, ExamError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ExamError::Invalid(ExamProblem::TitleBlank));
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(ExamError::Invalid(ExamProblem::TitleTooLong));
    }
    Ok(title.to_string())
}

/// A `YYYY-MM-DD` calendar date (exactly that shape), as a date.
fn checked_date(text: &str) -> Result<NaiveDate, ExamError> {
    let text = text.trim();
    if text.len() != 10 {
        return Err(ExamError::Invalid(ExamProblem::DateInvalid));
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| ExamError::Invalid(ExamProblem::DateInvalid))
}

async fn load_exam(conn: &mut SqliteConnection, id: i64) -> Result<Exam, ExamError> {
    let sql = format!("{EXAM_SELECT} WHERE e.id = ?1 AND e.deleted_at IS NULL");
    sqlx::query_as::<_, ExamRow>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .map(Exam::from)
        .ok_or(ExamError::NotFound)
}

/// Every active exam, the soonest date first.
pub async fn list_exams(pool: &SqlitePool) -> Result<Vec<Exam>, ExamError> {
    let sql = format!("{EXAM_SELECT} WHERE e.deleted_at IS NULL ORDER BY e.exam_date, e.id");
    let rows = sqlx::query_as::<_, ExamRow>(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(Exam::from).collect())
}

/// Adds an exam covering a deck or a tag. A past date is allowed (the exam
/// is then finished). Returns the new exam.
pub async fn create_exam(
    pool: &SqlitePool,
    title: &str,
    exam_date: &str,
    scope: Scope,
    now: DateTime<Utc>,
) -> Result<Exam, ExamError> {
    let title = checked_title(title)?;
    let date = checked_date(exam_date)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let exists: Option<i64> = match scope {
        Scope::Deck(id) => {
            sqlx::query_scalar("SELECT id FROM decks WHERE id = ?1 AND archived_at IS NULL")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
        }
        Scope::Tag(id) => {
            sqlx::query_scalar("SELECT id FROM tags WHERE id = ?1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
        }
    };
    if exists.is_none() {
        return Err(ExamError::Invalid(ExamProblem::ScopeGone));
    }
    let (deck_id, tag_id) = match scope {
        Scope::Deck(id) => (Some(id), None),
        Scope::Tag(id) => (None, Some(id)),
    };
    let id = sqlx::query(
        "INSERT INTO exams (title, exam_date, deck_id, tag_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&title)
    .bind(date.format("%Y-%m-%d").to_string())
    .bind(deck_id)
    .bind(tag_id)
    .bind(to_db_time(now))
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    let exam = load_exam(&mut tx, id).await?;
    tx.commit().await?;
    Ok(exam)
}

/// Changes an active exam's title and date (its scope never changes).
pub async fn update_exam(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    exam_date: &str,
) -> Result<Exam, ExamError> {
    let title = checked_title(title)?;
    let date = checked_date(exam_date)?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query(
        "UPDATE exams SET title = ?1, exam_date = ?2 WHERE id = ?3 AND deleted_at IS NULL",
    )
    .bind(&title)
    .bind(date.format("%Y-%m-%d").to_string())
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ExamError::NotFound);
    }
    let exam = load_exam(&mut tx, id).await?;
    tx.commit().await?;
    Ok(exam)
}

/// Soft-deletes an active exam. A missing or already deleted one is
/// [`ExamError::NotFound`].
pub async fn delete_exam(pool: &SqlitePool, id: i64, now: DateTime<Utc>) -> Result<(), ExamError> {
    let changed =
        sqlx::query("UPDATE exams SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL")
            .bind(to_db_time(now))
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected();
    if changed == 0 {
        return Err(ExamError::NotFound);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The plan

/// Where an exam stands, relative to the user's today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// The exam is on a later day.
    Upcoming,
    /// The exam is today.
    Today,
    /// The exam date has passed. Not an error.
    Finished,
}

/// The plan for one exam. Every count is about active cards in active decks.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExamPlan {
    pub exam_id: i64,
    pub title: String,
    pub exam_date: String,
    pub status: Status,
    /// Calendar days from today to the exam date: 1 is tomorrow, 0 today.
    /// 0 as well once finished (see `status`).
    pub days_left: i64,
    /// Active cards in scope.
    pub cards: i64,
    /// Cards never reviewed.
    pub new_cards: i64,
    /// Reviewed cards that fall due before the exam (already due included).
    pub due_before_exam: i64,
    /// Reviewed cards predicted below their deck's desired retention on exam
    /// day if not reviewed again. (New cards are counted in `new_cards`.)
    pub below_target: i64,
    /// Reviews to do each day, counting from today (see [`plan_for`]).
    pub daily_goal: i64,
    /// Mean predicted recall on the exam date, 0–100 (one decimal). Cards
    /// never reviewed count 0. 0 for an empty scope.
    pub readiness: f64,
}

/// One card's stored FSRS state, as the plan needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PlanRow {
    deck_id: i64,
    fsrs_stability: Option<f64>,
    last_review: Option<String>,
    due: Option<String>,
}

/// A card ready for the maths: `memory` is `None` for a card never reviewed,
/// else `(stability in days, last review, due)`.
#[derive(Debug, Clone, Copy)]
pub struct PlanCard {
    pub memory: Option<(f64, DateTime<Utc>, DateTime<Utc>)>,
    /// The card's deck's desired retention, 0.70–0.97.
    pub target: f32,
}

/// The chance of recalling a card `elapsed_days` after its last review, on
/// FSRS-6's forgetting curve `(1 + factor * t / S) ^ -decay`, where
/// `factor = 0.9 ^ (-1 / decay) - 1` makes recall exactly 90% when `t = S`.
/// `decay` is the 21st parameter in use.
pub fn recall(stability: f64, elapsed_days: f64, decay: f32) -> f64 {
    let decay = f64::from(decay);
    let factor = 0.9_f64.powf(-1.0 / decay) - 1.0;
    let recall = (1.0 + factor * elapsed_days.max(0.0) / stability).powf(-decay);
    if recall.is_finite() {
        recall.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The counts of a plan, before they are labelled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Counts {
    pub cards: i64,
    pub new_cards: i64,
    pub due_before_exam: i64,
    pub below_target: i64,
    /// Reviewed cards that are due before the exam or below target.
    pub needing_review: i64,
    pub readiness: f64,
}

/// Predicts every card at `at` (the exam day's start, or now if later),
/// assuming no further review, and counts.
pub fn tally(cards: &[PlanCard], at: DateTime<Utc>, decay: f32) -> Counts {
    let mut counts = Counts {
        cards: cards.len() as i64,
        new_cards: 0,
        due_before_exam: 0,
        below_target: 0,
        needing_review: 0,
        readiness: 0.0,
    };
    let mut total = 0.0;
    for card in cards {
        let Some((stability, last_review, due)) = card.memory else {
            counts.new_cards += 1;
            continue;
        };
        let elapsed = (at - last_review).num_milliseconds().max(0) as f64 / MS_PER_DAY;
        let predicted = recall(stability, elapsed, decay);
        total += predicted;
        let due_before = due <= at;
        let below = predicted < f64::from(card.target);
        counts.due_before_exam += i64::from(due_before);
        counts.below_target += i64::from(below);
        counts.needing_review += i64::from(due_before || below);
    }
    if !cards.is_empty() {
        let percent = total / cards.len() as f64 * 100.0;
        counts.readiness = (percent * 10.0).round() / 10.0;
        counts.readiness = counts.readiness.clamp(0.0, 100.0);
    }
    counts
}

/// The daily goal: the work still to do spread over the days left.
///
/// The work is each reviewed card that is due before the exam or predicted
/// below target (one review), plus each new card (two reviews, so it is seen
/// and then seen again, when there are two days or more; one otherwise).
/// Today counts as a day, so an exam tomorrow means all of it today, and an
/// exam today means all of it now. Rounded up, so the goal never falls short.
pub fn daily_goal(counts: &Counts, days_left: i64) -> i64 {
    let passes = if days_left >= 2 { 2 } else { 1 };
    let work = counts.needing_review + counts.new_cards * passes;
    let days = days_left.max(1);
    // Whole days only, so round up without signed `div_ceil`.
    let (work, days) = (work.max(0), days.max(1));
    work / days + i64::from(work % days != 0)
}

/// The plan for `exam` as of `now` on the user's clock (`zone`).
pub async fn plan_for<Tz: TimeZone>(
    pool: &SqlitePool,
    exam_id: i64,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Result<ExamPlan, ExamError> {
    let mut conn = pool.acquire().await?;
    let exam = load_exam(&mut conn, exam_id).await?;
    let date = checked_date(&exam.exam_date)?;

    let rows = sqlx::query_as::<_, PlanRow>(
        "SELECT f.deck_id, f.fsrs_stability, f.last_review, f.due
         FROM flashcards f JOIN decks d ON d.id = f.deck_id
         WHERE f.deleted_at IS NULL AND d.archived_at IS NULL
           AND ((?1 IS NOT NULL AND f.deck_id = ?1)
                OR (?2 IS NOT NULL AND f.id IN
                    (SELECT card_id FROM card_tags WHERE tag_id = ?2)))",
    )
    .bind(exam.deck_id)
    .bind(exam.tag_id)
    .fetch_all(&mut *conn)
    .await?;

    let parameters = optimizer::in_use(&mut conn).await?.parameters();
    let decay = parameters.get(20).copied().unwrap_or(DEFAULT_DECAY);
    let decay = if decay.is_finite() && decay > 0.0 {
        decay
    } else {
        DEFAULT_DECAY
    };

    let mut targets: HashMap<i64, f32> = HashMap::new();
    let mut cards = Vec::with_capacity(rows.len());
    for row in &rows {
        let target = match targets.get(&row.deck_id) {
            Some(target) => *target,
            None => {
                let target = retention::for_deck(&mut conn, row.deck_id).await?;
                targets.insert(row.deck_id, target);
                target
            }
        };
        let memory = match (row.fsrs_stability, &row.last_review, &row.due) {
            (Some(stability), Some(last), Some(due)) if stability > 0.0 => {
                Some((stability, parse_time(last)?, parse_time(due)?))
            }
            _ => None,
        };
        cards.push(PlanCard { memory, target });
    }
    drop(conn);

    let today = now.with_timezone(zone).date_naive();
    let days_left = (date - today).num_days();
    let status = match days_left {
        d if d < 0 => Status::Finished,
        0 => Status::Today,
        _ => Status::Upcoming,
    };
    // Recall is predicted at the start of the exam day, or now if that's
    // already begun.
    let at = start_of_day(date, zone).max(now);
    let counts = tally(&cards, at, decay);

    let finished = status == Status::Finished;
    Ok(ExamPlan {
        exam_id: exam.id,
        title: exam.title,
        exam_date: exam.exam_date,
        status,
        days_left: days_left.max(0),
        cards: counts.cards,
        new_cards: if finished { 0 } else { counts.new_cards },
        due_before_exam: if finished { 0 } else { counts.due_before_exam },
        below_target: if finished { 0 } else { counts.below_target },
        daily_goal: if finished {
            0
        } else {
            daily_goal(&counts, days_left)
        },
        readiness: counts.readiness,
    })
}

fn parse_time(text: &str) -> Result<DateTime<Utc>, DbError> {
    Ok(DateTime::parse_from_rfc3339(text)
        .map_err(|err| format!("card time {text:?} isn't readable: {err}"))?
        .with_timezone(&Utc))
}

// ---------------------------------------------------------------------------
// The study calendar

/// One local day of the study calendar.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CalendarDay {
    /// `YYYY-MM-DD` on the user's calendar.
    pub date: String,
    pub reviews: i64,
}

/// Reviews per local day for the last [`CALENDAR_DAYS`] days, and streaks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StudyCalendar {
    /// Oldest first, today last; days without reviews are there with 0.
    pub days: Vec<CalendarDay>,
    /// Consecutive local days with a review, ending today or, if today has
    /// none yet, yesterday (today without reviews yet doesn't break it).
    pub current_streak: i64,
    /// The longest such run in the whole review history.
    pub longest_streak: i64,
}

/// Builds the calendar from the times of every review.
pub fn calendar_from<Tz: TimeZone>(
    reviewed: &[DateTime<Utc>],
    now: DateTime<Utc>,
    zone: &Tz,
) -> StudyCalendar {
    let mut per_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for time in reviewed {
        *per_day
            .entry(time.with_timezone(zone).date_naive())
            .or_default() += 1;
    }
    let today = now.with_timezone(zone).date_naive();

    let days = (0..CALENDAR_DAYS)
        .rev()
        .filter_map(|back| today.checked_sub_signed(TimeDelta::days(back)))
        .map(|date| CalendarDay {
            date: date.format("%Y-%m-%d").to_string(),
            reviews: per_day.get(&date).copied().unwrap_or(0),
        })
        .collect();

    let mut current_streak = 0;
    let mut day = if per_day.contains_key(&today) {
        Some(today)
    } else {
        today.pred_opt()
    };
    while let Some(d) = day.filter(|d| per_day.contains_key(d)) {
        current_streak += 1;
        day = d.pred_opt();
    }

    let mut longest_streak = 0;
    let mut run = 0;
    let mut previous: Option<NaiveDate> = None;
    for date in per_day.keys() {
        run = if previous.and_then(|p| p.succ_opt()) == Some(*date) {
            run + 1
        } else {
            1
        };
        longest_streak = longest_streak.max(run);
        previous = Some(*date);
    }

    StudyCalendar {
        days,
        current_streak,
        longest_streak,
    }
}

/// The study calendar from `review_logs`. Reads only.
pub async fn study_calendar<Tz: TimeZone>(
    pool: &SqlitePool,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Result<StudyCalendar, ExamError> {
    let texts: Vec<String> = sqlx::query_scalar("SELECT reviewed_at FROM review_logs")
        .fetch_all(pool)
        .await?;
    let times = texts
        .iter()
        .map(|text| parse_time(text))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(calendar_from(&times, now, zone))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{archive_deck, create_deck, create_flashcard, delete_flashcard};
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use chrono::FixedOffset;

    /// Wednesday 11 November 2026, 10:00 UTC.
    const NOW: &str = "2026-11-11T10:00:00Z";

    async fn pool() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    async fn deck(pool: &SqlitePool, name: &str) -> i64 {
        create_deck(pool, name, None, at(NOW)).await.unwrap().id
    }

    async fn card(pool: &SqlitePool, deck: i64, front: &'static str) -> i64 {
        create_flashcard(pool, deck, front, "back", at(NOW))
            .await
            .unwrap();
        sqlx::query_scalar("SELECT id FROM flashcards WHERE deck_id = ?1 AND front = ?2")
            .bind(deck)
            .bind(front)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Gives a card a stored schedule: reviewed `ago` days before NOW with
    /// `stability`, due `due_in` days from NOW.
    async fn reviewed(pool: &SqlitePool, card: i64, stability: f64, ago: i64, due_in: i64) {
        let day = TimeDelta::days(1);
        sqlx::query(
            "UPDATE flashcards SET fsrs_state = 'Review', fsrs_stability = ?1,
                 fsrs_difficulty = 5.0, last_review = ?2, due = ?3, reps = 1
             WHERE id = ?4",
        )
        .bind(stability)
        .bind(to_db_time(at(NOW) - day * ago as i32))
        .bind(to_db_time(at(NOW) + day * due_in as i32))
        .bind(card)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn exam_on(pool: &SqlitePool, date: &str, scope: Scope) -> i64 {
        create_exam(pool, "Midterm", date, scope, at(NOW))
            .await
            .unwrap()
            .id
    }

    async fn plan(pool: &SqlitePool, exam: i64) -> ExamPlan {
        plan_for(pool, exam, at(NOW), &Utc).await.unwrap()
    }

    #[test]
    fn exam_tomorrow_is_all_of_it_today() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            card(&pool, bio, "new").await;
            let strong = card(&pool, bio, "strong").await;
            reviewed(&pool, strong, 100.0, 1, 99).await;
            let weak = card(&pool, bio, "weak").await;
            reviewed(&pool, weak, 1.0, 5, -4).await;

            let exam = exam_on(&pool, "2026-11-12", Scope::Deck(bio)).await;
            let plan = plan(&pool, exam).await;
            assert_eq!(plan.status, Status::Upcoming);
            assert_eq!(plan.days_left, 1);
            assert_eq!(plan.cards, 3);
            assert_eq!(plan.new_cards, 1);
            assert_eq!(plan.due_before_exam, 1);
            assert_eq!(plan.below_target, 1);
            // The weak card once, the new card once, in one day.
            assert_eq!(plan.daily_goal, 2);
            // Only the strong card has real recall: (0 + ~1 + low) / 3.
            assert!(plan.readiness > 50.0 && plan.readiness < 65.0);
        });
    }

    #[test]
    fn exam_in_ten_days_spreads_the_work_and_sees_new_cards_twice() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            for front in ["n1", "n2", "n3", "n4"] {
                card(&pool, bio, front).await;
            }
            for front in ["w1", "w2"] {
                let id = card(&pool, bio, front).await;
                reviewed(&pool, id, 1.0, 1, 1).await;
            }
            let exam = exam_on(&pool, "2026-11-21", Scope::Deck(bio)).await;
            let plan = plan(&pool, exam).await;
            assert_eq!(plan.days_left, 10);
            assert_eq!((plan.cards, plan.new_cards), (6, 4));
            assert_eq!(plan.below_target, 2);
            // 2 weak + 4 new * 2 = 10 reviews over 10 days.
            assert_eq!(plan.daily_goal, 1);
        });
    }

    #[test]
    fn a_past_exam_is_finished_not_an_error() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            card(&pool, bio, "new").await;
            let exam = exam_on(&pool, "2026-11-01", Scope::Deck(bio)).await;
            let plan = plan(&pool, exam).await;
            assert_eq!(plan.status, Status::Finished);
            assert_eq!((plan.days_left, plan.daily_goal, plan.new_cards), (0, 0, 0));
            assert_eq!(plan.cards, 1);
        });
    }

    #[test]
    fn an_exam_today_asks_for_everything_now() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            card(&pool, bio, "a").await;
            card(&pool, bio, "b").await;
            let exam = exam_on(&pool, "2026-11-11", Scope::Deck(bio)).await;
            let plan = plan(&pool, exam).await;
            assert_eq!(plan.status, Status::Today);
            assert_eq!((plan.days_left, plan.daily_goal), (0, 2));
        });
    }

    #[test]
    fn an_empty_scope_has_nothing_to_do() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            let exam = exam_on(&pool, "2026-11-20", Scope::Deck(bio)).await;
            let plan = plan(&pool, exam).await;
            assert_eq!((plan.cards, plan.daily_goal), (0, 0));
            assert_eq!(plan.readiness, 0.0);
        });
    }

    #[test]
    fn a_tag_exam_covers_tagged_cards_across_decks() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            let chem = deck(&pool, "Chemistry").await;
            let a = card(&pool, bio, "a").await;
            let b = card(&pool, chem, "b").await;
            card(&pool, chem, "untagged").await;
            sqlx::query("INSERT INTO tags (id, name, created_at) VALUES (1, 'unit1', ?1)")
                .bind(NOW)
                .execute(&pool)
                .await
                .unwrap();
            for id in [a, b] {
                sqlx::query("INSERT INTO card_tags (card_id, tag_id) VALUES (?1, 1)")
                    .bind(id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let exam = exam_on(&pool, "2026-11-15", Scope::Tag(1)).await;
            let plan = plan(&pool, exam).await;
            assert_eq!((plan.cards, plan.new_cards), (2, 2));
            let listed = list_exams(&pool).await.unwrap();
            assert_eq!(listed[0].scope_name, "unit1");
            assert_eq!(listed[0].tag_id, Some(1));
        });
    }

    #[test]
    fn deleted_cards_and_archived_decks_are_left_out() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            let gone = card(&pool, bio, "gone").await;
            card(&pool, bio, "kept").await;
            let exam = exam_on(&pool, "2026-11-15", Scope::Deck(bio)).await;
            assert_eq!(plan(&pool, exam).await.cards, 2);
            delete_flashcard(&pool, gone, at(NOW)).await.unwrap();
            assert_eq!(plan(&pool, exam).await.cards, 1);

            archive_deck(&pool, bio, at(NOW)).await.unwrap();
            let after = plan(&pool, exam).await;
            assert_eq!(after.cards, 0);
            assert!(list_exams(&pool).await.unwrap()[0].scope_archived);
        });
    }

    #[test]
    fn readiness_is_a_mean_of_predicted_recall_between_0_and_100() {
        let now = at(NOW);
        let strong = PlanCard {
            memory: Some((1000.0, now, now + TimeDelta::days(900))),
            target: 0.9,
        };
        let fresh = PlanCard {
            memory: None,
            target: 0.9,
        };
        let forgotten = PlanCard {
            memory: Some((0.5, now - TimeDelta::days(400), now - TimeDelta::days(399))),
            target: 0.9,
        };
        let all_new = tally(&[fresh, fresh], now, DEFAULT_DECAY);
        assert_eq!(all_new.readiness, 0.0);
        let strong_only = tally(&[strong], now, DEFAULT_DECAY);
        assert_eq!(strong_only.readiness, 100.0);
        let mixed = tally(&[strong, fresh, forgotten], now, DEFAULT_DECAY);
        assert!(mixed.readiness > 0.0 && mixed.readiness < 100.0 * 2.0 / 3.0);
        // Recall is 90% when the time since review equals the stability.
        assert!((recall(10.0, 10.0, DEFAULT_DECAY) - 0.9).abs() < 1e-9);
        assert!(recall(10.0, 0.0, DEFAULT_DECAY) > 0.9999);
        assert!(recall(10.0, 1e12, DEFAULT_DECAY) >= 0.0);
    }

    #[test]
    fn the_streak_follows_local_midnight_and_forgives_a_quiet_today() {
        // Two hours east of UTC: 21:30Z and 22:30Z on the 10th are 23:30 on
        // the 10th and 00:30 on the 11th locally, so two separate days.
        let zone = FixedOffset::east_opt(2 * 3600).unwrap();
        let reviews = [
            at("2026-11-09T09:00:00Z"),
            at("2026-11-10T21:30:00Z"),
            at("2026-11-10T22:30:00Z"),
            at("2026-11-10T22:45:00Z"),
        ];
        // Local "now" is the 11th, 01:00; the 11th already has reviews.
        let cal = calendar_from(&reviews, at("2026-11-10T23:00:00Z"), &zone);
        assert_eq!(cal.days.len() as i64, CALENDAR_DAYS);
        let last = cal.days.last().unwrap();
        assert_eq!((last.date.as_str(), last.reviews), ("2026-11-11", 2));
        assert_eq!(cal.days[cal.days.len() - 2].reviews, 1);
        assert_eq!(cal.days[cal.days.len() - 3].reviews, 1);
        assert_eq!((cal.current_streak, cal.longest_streak), (3, 3));

        // Later that day, with no review on the 12th yet: streak kept.
        let quiet = calendar_from(&reviews, at("2026-11-11T22:30:00Z"), &zone);
        assert_eq!(quiet.days.last().unwrap().date, "2026-11-12");
        assert_eq!(quiet.days.last().unwrap().reviews, 0);
        assert_eq!(quiet.current_streak, 3);

        // A whole day missed breaks the current streak, not the longest.
        let broken = calendar_from(&reviews, at("2026-11-13T10:00:00Z"), &zone);
        assert_eq!((broken.current_streak, broken.longest_streak), (0, 3));

        let none = calendar_from(&[], at(NOW), &Utc);
        assert_eq!((none.current_streak, none.longest_streak), (0, 0));
    }

    #[test]
    fn exams_are_validated_and_soft_deleted() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let bio = deck(&pool, "Biology").await;
            let make = |title: &str, date: &str, scope: Scope| {
                let (title, date) = (title.to_string(), date.to_string());
                let pool = pool.clone();
                async move { create_exam(&pool, &title, &date, scope, at(NOW)).await }
            };
            let problem = |result: Result<Exam, ExamError>| match result {
                Err(ExamError::Invalid(problem)) => problem,
                other => panic!("expected a refusal, got {other:?}"),
            };
            assert_eq!(
                problem(make("  ", "2026-12-01", Scope::Deck(bio)).await),
                ExamProblem::TitleBlank
            );
            assert_eq!(
                problem(make(&"x".repeat(101), "2026-12-01", Scope::Deck(bio)).await),
                ExamProblem::TitleTooLong
            );
            for bad in ["2026-02-30", "2026-1-5", "tomorrow", "", "2026-12-01T00:00"] {
                assert_eq!(
                    problem(make("Exam", bad, Scope::Deck(bio)).await),
                    ExamProblem::DateInvalid,
                    "{bad}"
                );
            }
            assert_eq!(
                problem(make("Exam", "2026-12-01", Scope::Deck(9999)).await),
                ExamProblem::ScopeGone
            );
            assert_eq!(
                problem(make("Exam", "2026-12-01", Scope::Tag(9999)).await),
                ExamProblem::ScopeGone
            );
            assert!(matches!(
                Scope::from_ids(None, None),
                Err(ExamError::Invalid(ExamProblem::ScopeNeeded))
            ));
            assert!(matches!(
                Scope::from_ids(Some(1), Some(1)),
                Err(ExamError::Invalid(ExamProblem::ScopeNeeded))
            ));
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM exams").await, 0);

            // A title is trimmed; a 100-character one is fine.
            let exam = make(" Final ", "2026-12-01", Scope::Deck(bio))
                .await
                .unwrap();
            assert_eq!(exam.title, "Final");
            make(&"x".repeat(100), "2026-12-02", Scope::Deck(bio))
                .await
                .unwrap();

            let updated = update_exam(&pool, exam.id, "Final exam", "2026-12-05")
                .await
                .unwrap();
            assert_eq!(
                (updated.title.as_str(), updated.exam_date.as_str()),
                ("Final exam", "2026-12-05")
            );
            assert_eq!(updated.deck_id, Some(bio));
            assert!(matches!(
                update_exam(&pool, exam.id, "", "2026-12-05").await,
                Err(ExamError::Invalid(ExamProblem::TitleBlank))
            ));

            // Listed by date; deleting hides it, and a second delete is stale.
            let listed = list_exams(&pool).await.unwrap();
            assert_eq!(listed.len(), 2);
            assert!(listed[0].exam_date < listed[1].exam_date);
            delete_exam(&pool, exam.id, at(NOW)).await.unwrap();
            assert_eq!(list_exams(&pool).await.unwrap().len(), 1);
            assert!(matches!(
                delete_exam(&pool, exam.id, at(NOW)).await,
                Err(ExamError::NotFound)
            ));
            assert!(matches!(
                update_exam(&pool, exam.id, "Again", "2026-12-06").await,
                Err(ExamError::NotFound)
            ));
            assert!(matches!(
                plan_for(&pool, exam.id, at(NOW), &Utc).await,
                Err(ExamError::NotFound)
            ));
            // The row stays, and the database refuses to remove or rewrite it.
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM exams").await, 2);
            assert!(sqlx::query("DELETE FROM exams")
                .execute(&pool)
                .await
                .is_err());
            assert!(sqlx::query("UPDATE exams SET title = 'x' WHERE id = 1")
                .execute(&pool)
                .await
                .is_err());
        });
    }

    #[test]
    fn the_database_refuses_bad_exam_rows() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let sample = sample_deck_id(&pool).await;
            let insert =
                |title: &'static str, date: &'static str, deck: Option<i64>, tag: Option<i64>| {
                    let pool = pool.clone();
                    async move {
                        sqlx::query(
                            "INSERT INTO exams (title, exam_date, deck_id, tag_id, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        )
                        .bind(title)
                        .bind(date)
                        .bind(deck)
                        .bind(tag)
                        .bind(NOW)
                        .execute(&pool)
                        .await
                    }
                };
            assert!(insert("ok", "2026-12-01", Some(sample), None).await.is_ok());
            assert!(insert("", "2026-12-01", Some(sample), None).await.is_err());
            assert!(insert("ok", "2026-13-01", Some(sample), None)
                .await
                .is_err());
            assert!(insert("ok", "2026-1-1", Some(sample), None).await.is_err());
            assert!(insert("ok", "2026-12-01", None, None).await.is_err());
            assert!(insert("ok", "2026-12-01", Some(sample), Some(1))
                .await
                .is_err());
        });
    }
}
