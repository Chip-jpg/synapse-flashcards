//! Personal FSRS parameters: fitting FSRS to the user's own review history,
//! and which parameters the scheduler uses.
//!
//! FSRS schedules with 21 parameters. Its defaults fit the average learner;
//! fitting them to one person's `review_logs` (the `fsrs` crate's
//! `compute_parameters`) can predict that person's memory better.
//!
//! - **Optimizing writes nothing.** It fits parameters, measures how well the
//!   parameters in use and the fitted ones predict the user's own reviews,
//!   and holds the result for the user to look at.
//! - **Applying is the user's choice**, and only offered when the fitted
//!   parameters predict the history better. Each apply (and each reset to
//!   the defaults) adds a row to `fsrs_parameters` (migration 0011); the
//!   newest row is the one in use.
//! - **Only future ratings change.** `study::record_review` reads the
//!   parameters in use for each new rating. No card's stored schedule and no
//!   `review_logs` row is ever rewritten here.
//! - **Off the UI thread, and bounded.** Fitting runs on a blocking thread,
//!   and stops when the user stops it, when another optimization starts, or
//!   after [`TIME_LIMIT`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use fsrs::{
    CombinedProgressState, ComputeParametersInput, FSRSError, FSRSItem, FSRSReview,
    DEFAULT_PARAMETERS, FSRS,
};
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{to_db_time, DbError};
use crate::retention;

/// The fewest usable reviews Synapse fits parameters to. Below this, a fit
/// mostly follows chance in a small history, and FSRS's defaults (fitted to
/// many learners) are the better guess. FSRS's own guidance is several
/// hundred reviews; 400 is the figure Anki long used.
pub const MINIMUM_REVIEWS: usize = 400;

/// How long fitting may run before Synapse stops it. A few thousand reviews
/// take well under a second; this only stops a pathological run.
pub const TIME_LIMIT: Duration = Duration::from_secs(120);

const MS_PER_DAY: i64 = 86_400_000;

/// Why optimizing, applying, or resetting didn't happen.
#[derive(Debug)]
pub enum OptimizerError {
    /// Fewer than [`MINIMUM_REVIEWS`] usable reviews.
    NotEnoughReviews,
    /// The parameters in use changed since this was shown, or there's no
    /// longer an optimization to apply.
    Stale,
    /// The fitted parameters don't predict the history better than the ones
    /// in use, so they aren't applied.
    NotBetter,
    /// Fitting ran past [`TIME_LIMIT`].
    TimedOut,
    /// Anything else (database or FSRS failure). Detail is for logs only.
    Internal(DbError),
}

impl From<sqlx::Error> for OptimizerError {
    fn from(err: sqlx::Error) -> Self {
        OptimizerError::Internal(err.into())
    }
}

impl From<DbError> for OptimizerError {
    fn from(err: DbError) -> Self {
        OptimizerError::Internal(err)
    }
}

impl From<FSRSError> for OptimizerError {
    fn from(err: FSRSError) -> Self {
        OptimizerError::Internal(format!("FSRS failed: {err:?}").into())
    }
}

/// The parameters the scheduler uses now: the newest `fsrs_parameters` row.
#[derive(Debug, Clone, PartialEq)]
pub struct InUse {
    /// The newest row's id, or 0 when nothing was ever applied. Changes with
    /// every apply and reset, so a stale screen can be told apart.
    pub version: i64,
    /// The user's own parameters, or `None` for FSRS's defaults.
    pub personal: Option<Vec<f32>>,
    pub reviews_used: Option<i64>,
    pub applied_at: Option<String>,
}

impl InUse {
    /// All 21 parameters, the defaults filled in when none were applied.
    pub fn parameters(&self) -> Vec<f32> {
        self.personal
            .clone()
            .unwrap_or_else(|| DEFAULT_PARAMETERS.to_vec())
    }
}

/// Loads the parameters in use.
pub async fn in_use(conn: &mut SqliteConnection) -> Result<InUse, DbError> {
    let row: Option<(i64, Option<String>, Option<i64>, String)> = sqlx::query_as(
        "SELECT id, parameters, reviews_used, applied_at
         FROM fsrs_parameters ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some((version, parameters, reviews_used, applied_at)) = row else {
        return Ok(InUse {
            version: 0,
            personal: None,
            reviews_used: None,
            applied_at: None,
        });
    };
    let personal = match parameters {
        None => None,
        Some(json) => {
            let parameters: Vec<f32> = serde_json::from_str(&json)
                .map_err(|err| format!("stored FSRS parameters aren't readable: {err}"))?;
            Some(parameters)
        }
    };
    Ok(InUse {
        version,
        personal,
        reviews_used,
        applied_at: Some(applied_at),
    })
}

/// The parameters to schedule the next rating with, as `scheduler::review`
/// takes them: the user's own, or an empty slice for FSRS's defaults.
pub async fn parameters_for_review(conn: &mut SqliteConnection) -> Result<Vec<f32>, DbError> {
    Ok(in_use(conn).await?.personal.unwrap_or_default())
}

/// One rating from `review_logs`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LoggedReview {
    pub card_id: i64,
    pub rating: i64,
    pub state_before: String,
    pub reviewed_at: String,
}

/// The review history in the form FSRS learns from. `items[n]` is one
/// rating with every earlier rating of the same card (`card_ids[n]`), oldest
/// rating last-reviewed first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    pub items: Vec<FSRSItem>,
    pub card_ids: Vec<i64>,
}

impl History {
    /// How many reviews FSRS can learn from: the ratings made at least a day
    /// after the card's previous rating.
    pub fn usable_reviews(&self) -> usize {
        self.items.len()
    }
}

/// Every rating ever recorded, of every card: deleted cards, archived decks,
/// and the sample deck included, since each one is real evidence of how the
/// user remembers.
pub async fn history(pool: &SqlitePool) -> Result<History, DbError> {
    let reviews = sqlx::query_as::<_, LoggedReview>(
        "SELECT card_id, rating, state_before, reviewed_at
         FROM review_logs ORDER BY card_id, reviewed_at, id",
    )
    .fetch_all(pool)
    .await?;
    training_history(&reviews)
}

/// Builds FSRS's training items from ratings sorted by card, then time.
///
/// - A card's history starts at its first rating as a New card, whose gap
///   FSRS requires to be 0. Ratings before it (which Synapse never records)
///   are skipped, and a later New rating starts the history afresh.
/// - Gaps are whole days, rounded down, as `scheduler::review` counts them.
/// - Each rating at least a day after the previous one is an item to learn
///   from, together with every earlier rating of the card. Same-day ratings
///   stay in the history but aren't items themselves, as in FSRS's own tools.
/// - Items are sorted by when their rating was made, because FSRS weighs
///   recent reviews more heavily.
pub fn training_history(reviews: &[LoggedReview]) -> Result<History, DbError> {
    let mut items: Vec<(DateTime<Utc>, i64, FSRSItem)> = Vec::new();
    let mut card: Option<i64> = None;
    let mut sequence: Vec<FSRSReview> = Vec::new();
    let mut previous: Option<DateTime<Utc>> = None;

    for review in reviews {
        let at = DateTime::parse_from_rfc3339(&review.reviewed_at)
            .map_err(|err| format!("review time {:?} isn't readable: {err}", review.reviewed_at))?
            .with_timezone(&Utc);
        let rating = u32::try_from(review.rating)
            .ok()
            .filter(|rating| (1..=4).contains(rating))
            .ok_or_else(|| format!("review rating {} isn't 1 to 4", review.rating))?;

        if card != Some(review.card_id) {
            card = Some(review.card_id);
            sequence.clear();
            previous = None;
        }
        if review.state_before == "New" {
            sequence.clear();
            previous = None;
        } else if previous.is_none() {
            // No first rating to count from.
            continue;
        }

        let delta_t = match previous {
            None => 0,
            Some(before) => {
                let days = (at - before).num_milliseconds().max(0) / MS_PER_DAY;
                u32::try_from(days).unwrap_or(u32::MAX)
            }
        };
        sequence.push(FSRSReview { rating, delta_t });
        previous = Some(at);
        if delta_t > 0 {
            items.push((
                at,
                review.card_id,
                FSRSItem {
                    reviews: sequence.clone(),
                },
            ));
        }
    }

    // Stable, so ratings made at the same moment keep the card order.
    items.sort_by_key(|(at, _, _)| *at);
    let (card_ids, items) = items
        .into_iter()
        .map(|(_, card_id, item)| (card_id, item))
        .unzip();
    Ok(History { items, card_ids })
}

/// How well a set of parameters predicts the user's own reviews. Lower is
/// better for both.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    /// FSRS's log loss, which decides whether fitted parameters are better.
    pub log_loss: f32,
    /// How far predicted recall is from actual recall, on average (0 to 1).
    pub error: f32,
}

/// A schedule the user can compare: the interval, in days, that each set of
/// parameters gives.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntervalChange {
    pub case: IntervalCase,
    pub current: f32,
    pub proposed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IntervalCase {
    /// A new card's first rating, for each of the four ratings.
    FirstAgain,
    FirstHard,
    FirstGood,
    FirstEasy,
    /// Good on a new card, then Good again when it's due.
    SecondGood,
}

/// Fitted parameters, compared with the ones in use. Nothing is saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// The version of the parameters in use these were compared with.
    pub version: i64,
    pub reviews_used: usize,
    pub current: Vec<f32>,
    pub proposed: Vec<f32>,
    pub current_fit: Fit,
    pub proposed_fit: Fit,
    /// Whether `proposed` predicts the history better than `current`.
    pub better: bool,
    pub intervals: Vec<IntervalChange>,
}

/// What an optimization did.
#[derive(Debug)]
pub enum Optimized {
    /// Fitted, and held under `token` for [`Optimizing::take`].
    Proposed { token: u64, proposal: Proposal },
    /// Stopped by the user, or by another optimization starting.
    Stopped,
}

/// The optimization running now, and the last result waiting to be applied.
/// Managed by Tauri, like `suggest::Suggesting`.
#[derive(Default)]
pub struct Optimizing {
    state: Mutex<Running>,
}

#[derive(Default)]
struct Running {
    /// Numbers each optimization (and each held result), so one that ends
    /// late can't clear or replace the next.
    started: u64,
    current: Option<(u64, Arc<Mutex<CombinedProgressState>>)>,
    held: Option<(u64, Proposal)>,
}

impl Optimizing {
    fn running(&self) -> std::sync::MutexGuard<'_, Running> {
        // Nothing in the lock can panic halfway, so a poisoned lock is safe to use.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts tracking a new optimization, stopping any still running and
    /// forgetting any result not yet applied.
    fn begin(&self) -> Run<'_> {
        let progress = CombinedProgressState::new_shared();
        let mut running = self.running();
        running.started += 1;
        let id = running.started;
        running.held = None;
        if let Some((_, previous)) = running.current.replace((id, progress.clone())) {
            abort(&previous);
        }
        Run {
            owner: self,
            id,
            progress,
        }
    }

    /// Stops the optimization running now, if there is one.
    pub fn stop(&self) {
        if let Some((_, progress)) = self.running().current.take() {
            abort(&progress);
        }
    }

    /// Takes the result held under `token`, if it's still the latest.
    pub fn take(&self, token: u64) -> Option<Proposal> {
        let mut running = self.running();
        match &running.held {
            Some((held, _)) if *held == token => running.held.take().map(|(_, p)| p),
            _ => None,
        }
    }

    /// Stops and forgets everything: the data it was fitted to is gone (a
    /// restored backup replaced it).
    pub fn forget(&self) {
        self.stop();
        self.running().held = None;
    }

    fn hold(&self, id: u64, proposal: Proposal) -> Option<u64> {
        let mut running = self.running();
        // A newer optimization started meanwhile: this result is stale.
        if running.started != id {
            return None;
        }
        running.held = Some((id, proposal));
        Some(id)
    }
}

fn abort(progress: &Mutex<CombinedProgressState>) {
    progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .want_abort = true;
}

fn aborted(progress: &Mutex<CombinedProgressState>) -> bool {
    progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .want_abort
}

/// One optimization being tracked. Dropping it stops tracking it.
struct Run<'a> {
    owner: &'a Optimizing,
    id: u64,
    progress: Arc<Mutex<CombinedProgressState>>,
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        let mut running = self.owner.running();
        if running
            .current
            .as_ref()
            .is_some_and(|(id, _)| *id == self.id)
        {
            running.current = None;
        }
    }
}

/// Fits FSRS's parameters to the whole review history and compares them
/// with the ones in use. Writes nothing; the result is held in `optimizing`
/// until applied. Refused below [`MINIMUM_REVIEWS`]; stopped after `limit`.
pub async fn optimize(
    pool: &SqlitePool,
    optimizing: &Optimizing,
    limit: Duration,
) -> Result<Optimized, OptimizerError> {
    // Tracked from the start, so a stop while the history loads counts too.
    let run = optimizing.begin();
    let deadline = Instant::now() + limit;

    let (current, retention) = {
        let mut conn = pool.acquire().await?;
        let current = in_use(&mut conn).await?;
        (
            current,
            retention::as_share(retention::global(&mut conn).await?),
        )
    };
    let history = history(pool).await?;
    if history.usable_reviews() < MINIMUM_REVIEWS {
        return Err(OptimizerError::NotEnoughReviews);
    }
    if aborted(&run.progress) {
        return Ok(Optimized::Stopped);
    }

    // The fitting itself checks `want_abort` after every batch, so this
    // watchdog is what bounds it in time.
    let done = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    {
        let (progress, done, timed_out) = (run.progress.clone(), done.clone(), timed_out.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                if Instant::now() >= deadline {
                    timed_out.store(true, Ordering::Release);
                    abort(&progress);
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }

    let progress = run.progress.clone();
    let version = current.version;
    let parameters = current.parameters();
    let work = tauri::async_runtime::spawn_blocking(move || {
        fit(history, parameters, retention, version, progress, deadline)
    })
    .await;
    done.store(true, Ordering::Release);

    // The watchdog may not have noticed yet when `fit` saw the deadline.
    let timed_out = timed_out.load(Ordering::Acquire) || Instant::now() >= deadline;
    let result = match work {
        Ok(result) => result,
        Err(err) => {
            let detail: DbError = format!("the optimization thread failed: {err}").into();
            return Err(detail.into());
        }
    };
    match result {
        Err(FSRSError::Interrupted) if timed_out => Err(OptimizerError::TimedOut),
        Err(FSRSError::Interrupted) => Ok(Optimized::Stopped),
        Err(err) => Err(err.into()),
        // Stopped after the fitting finished, while comparing.
        Ok(_) if aborted(&run.progress) && !timed_out => Ok(Optimized::Stopped),
        Ok(proposal) => match optimizing.hold(run.id, proposal.clone()) {
            Some(token) => Ok(Optimized::Proposed { token, proposal }),
            None => Ok(Optimized::Stopped),
        },
    }
}

/// The blocking part of [`optimize`]: fitting, then comparing.
fn fit(
    history: History,
    current: Vec<f32>,
    retention: f32,
    version: i64,
    progress: Arc<Mutex<CombinedProgressState>>,
    deadline: Instant,
) -> Result<Proposal, FSRSError> {
    if Instant::now() >= deadline {
        return Err(FSRSError::Interrupted);
    }
    let reviews_used = history.usable_reviews();
    let proposed = fsrs::compute_parameters(ComputeParametersInput {
        train_set: history.items.clone(),
        card_ids: Some(history.card_ids),
        progress: Some(progress.clone()),
        ..ComputeParametersInput::default()
    })?;
    if proposed.len() != DEFAULT_PARAMETERS.len() || proposed.iter().any(|w| !w.is_finite()) {
        return Err(FSRSError::InvalidParameters);
    }

    let current_model = FSRS::new(&current)?;
    let proposed_model = FSRS::new(&proposed)?;
    let keep_going = |_| !aborted(&progress) && Instant::now() < deadline;
    let current_fit = current_model.evaluate(history.items.clone(), keep_going)?;
    let proposed_fit = proposed_model.evaluate(history.items, keep_going)?;
    let current_fit = Fit {
        log_loss: current_fit.log_loss,
        error: current_fit.rmse_bins,
    };
    let proposed_fit = Fit {
        log_loss: proposed_fit.log_loss,
        error: proposed_fit.rmse_bins,
    };

    let before = intervals(&current_model, retention)?;
    let after = intervals(&proposed_model, retention)?;
    let intervals = before
        .into_iter()
        .zip(after)
        .map(|((case, current), (_, proposed))| IntervalChange {
            case,
            current,
            proposed,
        })
        .collect();

    Ok(Proposal {
        version,
        reviews_used,
        current,
        proposed,
        better: proposed_fit.log_loss < current_fit.log_loss,
        current_fit,
        proposed_fit,
        intervals,
    })
}

/// The intervals, in days, a set of parameters gives a new card, aiming for
/// the global desired retention.
fn intervals(model: &FSRS, retention: f32) -> Result<Vec<(IntervalCase, f32)>, FSRSError> {
    let first = model.next_states(None, retention, 0)?;
    // Reviewed again when due, counting whole days as the scheduler does.
    let waited = first.good.interval.max(1.0).floor() as u32;
    let second = model.next_states(Some(first.good.memory), retention, waited)?;
    Ok(vec![
        (IntervalCase::FirstAgain, first.again.interval),
        (IntervalCase::FirstHard, first.hard.interval),
        (IntervalCase::FirstGood, first.good.interval),
        (IntervalCase::FirstEasy, first.easy.interval),
        (IntervalCase::SecondGood, second.good.interval),
    ])
}

/// Makes `proposal` the parameters in use, for every rating from now on.
/// Refused as stale if the parameters in use changed since it was made, and
/// refused if it doesn't predict the history better.
pub async fn apply(
    pool: &SqlitePool,
    proposal: &Proposal,
    now: DateTime<Utc>,
) -> Result<(), OptimizerError> {
    if !proposal.better {
        return Err(OptimizerError::NotBetter);
    }
    // Checked as the scheduler will use them, before anything is written.
    FSRS::new(&proposal.proposed)?;
    let reviews_used = i64::try_from(proposal.reviews_used)
        .map_err(|_| -> DbError { "review count out of range".into() })?;
    let json = serde_json::to_string(&proposal.proposed)
        .map_err(|err| -> DbError { err.to_string().into() })?;

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if in_use(&mut tx).await?.version != proposal.version {
        return Err(OptimizerError::Stale);
    }
    sqlx::query(
        "INSERT INTO fsrs_parameters (parameters, reviews_used, applied_at) VALUES (?1, ?2, ?3)",
    )
    .bind(json)
    .bind(reviews_used)
    .bind(to_db_time(now))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Goes back to FSRS's defaults for every rating from now on. `expected` is
/// the version of the parameters the user saw; refused as stale if they
/// changed since, or if the defaults are already in use.
pub async fn reset(
    pool: &SqlitePool,
    expected: i64,
    now: DateTime<Utc>,
) -> Result<(), OptimizerError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let current = in_use(&mut tx).await?;
    if current.version != expected || current.personal.is_none() {
        return Err(OptimizerError::Stale);
    }
    sqlx::query("INSERT INTO fsrs_parameters (applied_at) VALUES (?1)")
        .bind(to_db_time(now))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// The scheduling settings screen: `{ version, personal, parameters,
/// defaults, reviewsUsed, appliedAt, usableReviews, minimumReviews }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub version: i64,
    /// Whether the user's own parameters are in use (else FSRS's defaults).
    pub personal: bool,
    /// The 21 parameters in use.
    pub parameters: Vec<f32>,
    pub defaults: Vec<f32>,
    /// How many reviews the parameters in use were fitted to.
    pub reviews_used: Option<i64>,
    /// When the parameters in use were applied (or reset to the defaults).
    pub applied_at: Option<String>,
    /// How many reviews an optimization could learn from now.
    pub usable_reviews: usize,
    pub minimum_reviews: usize,
}

pub async fn settings(pool: &SqlitePool) -> Result<Settings, DbError> {
    let current = in_use(&mut *pool.acquire().await?).await?;
    let usable_reviews = history(pool).await?.usable_reviews();
    Ok(Settings {
        version: current.version,
        personal: current.personal.is_some(),
        parameters: current.parameters(),
        defaults: DEFAULT_PARAMETERS.to_vec(),
        reviews_used: current.reviews_used,
        applied_at: current.applied_at,
        usable_reviews,
        minimum_reviews: MINIMUM_REVIEWS,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::authoring::{create_deck, create_flashcard};
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::scheduler::Rating;
    use crate::study::{self, StartedSession};
    use chrono::TimeDelta;

    const NOW: &str = "2026-10-19T09:00:00Z";

    fn logged(card_id: i64, rating: i64, state: &str, at: &str) -> LoggedReview {
        LoggedReview {
            card_id,
            rating,
            state_before: state.to_string(),
            reviewed_at: at.to_string(),
        }
    }

    fn review(rating: u32, delta_t: u32) -> FSRSReview {
        FSRSReview { rating, delta_t }
    }

    async fn pool() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    /// Adds a history FSRS can learn from: `cards` cards in a new deck, each
    /// rated Good when new and then reviewed at growing gaps, remembered
    /// almost every time (a learner who remembers far better than FSRS's
    /// defaults expect). Written straight into the tables, as the reviews
    /// of the past months would be. Returns the deck's id.
    pub async fn add_history(pool: &SqlitePool, cards: i64) -> i64 {
        let deck = create_deck(pool, "History", None, at(NOW))
            .await
            .unwrap()
            .id;
        let start = at("2026-01-05T08:00:00Z");
        let mut seed: u64 = 7;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            seed >> 33
        };
        for n in 0..cards {
            let card = create_flashcard(pool, deck, &format!("Q{n}"), "A", at(NOW))
                .await
                .unwrap()
                .cards
                .last()
                .unwrap()
                .id;
            let mut when = start + TimeDelta::hours(n * 7);
            let mut state = "New";
            for gap in [0, 5, 12, 30, 70] {
                when += TimeDelta::days(gap + (next() % 3) as i64);
                let remembered = next() % 100 < 93;
                let rating = match (state, remembered) {
                    ("New", _) => 3,
                    (_, true) => 3,
                    (_, false) => 1,
                };
                sqlx::query(
                    "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                              elapsed_days, stability_after, difficulty_after,
                                              reviewed_at)
                     VALUES (?1, ?2, ?3, 1, 1, 1, 5, ?4)",
                )
                .bind(card)
                .bind(rating)
                .bind(state)
                .bind(to_db_time(when))
                .execute(pool)
                .await
                .unwrap();
                state = if rating == 1 { "Relearning" } else { "Review" };
            }
        }
        deck
    }

    #[test]
    fn the_history_counts_whole_days_and_learns_only_from_later_days() {
        let reviews = [
            // Card 1: new, again the same day, then two and five days on.
            logged(1, 3, "New", "2026-10-01T09:00:00.000Z"),
            logged(1, 1, "Review", "2026-10-01T20:00:00.000Z"),
            logged(1, 3, "Relearning", "2026-10-03T19:59:00.000Z"),
            logged(1, 4, "Review", "2026-10-08T20:00:00.000Z"),
            // Card 2: a history that doesn't start new is skipped until a
            // new rating, which starts afresh.
            logged(2, 3, "Review", "2026-09-01T09:00:00.000Z"),
            logged(2, 2, "New", "2026-10-02T09:00:00.000Z"),
            logged(2, 3, "Review", "2026-10-06T09:00:00.000Z"),
        ];
        let history = training_history(&reviews).unwrap();

        // 19:59 on the 3rd is 23h59m short of two days after 20:00 on the 1st.
        let card_one = vec![review(3, 0), review(1, 0), review(3, 1)];
        let mut card_one_later = card_one.clone();
        card_one_later.push(review(4, 5));
        assert_eq!(
            history.items,
            vec![
                FSRSItem { reviews: card_one },
                FSRSItem {
                    reviews: vec![review(2, 0), review(3, 4)]
                },
                FSRSItem {
                    reviews: card_one_later
                },
            ]
        );
        // In the order the ratings were made.
        assert_eq!(history.card_ids, vec![1, 2, 1]);
        assert_eq!(history.usable_reviews(), 3);

        assert!(training_history(&[logged(1, 5, "New", "2026-10-01T09:00:00.000Z")]).is_err());
        assert!(training_history(&[logged(1, 3, "New", "yesterday")]).is_err());
    }

    #[test]
    fn a_new_database_uses_the_defaults_and_cant_be_optimized_yet() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let settings = settings(&pool).await.unwrap();
            assert_eq!(
                settings,
                Settings {
                    version: 0,
                    personal: false,
                    parameters: DEFAULT_PARAMETERS.to_vec(),
                    defaults: DEFAULT_PARAMETERS.to_vec(),
                    reviews_used: None,
                    applied_at: None,
                    usable_reviews: 0,
                    minimum_reviews: MINIMUM_REVIEWS,
                }
            );

            // A small history is refused, and nothing is written or held.
            add_history(&pool, 20).await;
            assert_eq!(settings_of(&pool).await.usable_reviews, 80);
            let optimizing = Optimizing::default();
            let refused = optimize(&pool, &optimizing, TIME_LIMIT).await;
            assert!(matches!(refused, Err(OptimizerError::NotEnoughReviews)));
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM fsrs_parameters").await,
                0
            );
            assert!(optimizing.running().held.is_none());
            assert!(optimizing.running().current.is_none());
        });
    }

    async fn settings_of(pool: &SqlitePool) -> Settings {
        settings(pool).await.unwrap()
    }

    async fn rows(pool: &SqlitePool, sql: &'static str) -> Vec<String> {
        sqlx::query_scalar(sql).fetch_all(pool).await.unwrap()
    }

    const CARD_ROWS: &str = "SELECT id || fsrs_state || quote(fsrs_stability) || \
         quote(fsrs_difficulty) || quote(due) || quote(last_review) || reps || lapses \
         FROM flashcards ORDER BY id";
    const LOG_ROWS: &str = "SELECT id || card_id || rating || state_before || scheduled_days || \
         elapsed_days || stability_after || difficulty_after || reviewed_at \
         FROM review_logs ORDER BY id";

    #[test]
    fn optimized_parameters_are_applied_for_future_ratings_only() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            add_history(&pool, 150).await;
            assert_eq!(settings_of(&pool).await.usable_reviews, 600);
            let cards_before = rows(&pool, CARD_ROWS).await;
            let logs_before = rows(&pool, LOG_ROWS).await;

            let optimizing = Optimizing::default();
            let Optimized::Proposed { token, proposal } =
                optimize(&pool, &optimizing, TIME_LIMIT).await.unwrap()
            else {
                panic!("expected parameters to be proposed");
            };
            assert_eq!(proposal.version, 0);
            assert_eq!(proposal.reviews_used, 600);
            assert_eq!(proposal.current, DEFAULT_PARAMETERS.to_vec());
            assert_eq!(proposal.proposed.len(), 21);
            assert_ne!(proposal.proposed, proposal.current);
            // This learner remembers far better than the defaults expect, so
            // the fit predicts them better and spaces reviews further apart.
            assert!(proposal.better, "{proposal:?}");
            assert!(proposal.proposed_fit.error < proposal.current_fit.error);
            let good = |p: &Proposal, case| {
                p.intervals
                    .iter()
                    .find(|i| i.case == case)
                    .copied()
                    .unwrap()
            };
            let second = good(&proposal, IntervalCase::SecondGood);
            assert!(second.proposed > second.current, "{second:?}");
            assert_eq!(proposal.intervals.len(), 5);

            // Optimizing wrote nothing.
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM fsrs_parameters").await,
                0
            );

            let held = optimizing.take(token).unwrap();
            assert_eq!(held, proposal);
            assert!(optimizing.take(token).is_none(), "held only once");
            apply(&pool, &held, at(NOW)).await.unwrap();

            let applied = settings_of(&pool).await;
            assert!(applied.personal);
            assert_eq!(applied.version, 1);
            assert_eq!(applied.parameters, proposal.proposed);
            assert_eq!(applied.reviews_used, Some(600));
            assert_eq!(
                applied.applied_at.as_deref(),
                Some("2026-10-19T09:00:00.000Z")
            );

            // No stored schedule or review changed.
            assert_eq!(rows(&pool, CARD_ROWS).await, cards_before);
            assert_eq!(rows(&pool, LOG_ROWS).await, logs_before);

            // The next rating is scheduled with the user's parameters.
            let deck = create_deck(&pool, "Fresh", None, at(NOW)).await.unwrap().id;
            let card = create_flashcard(&pool, deck, "New", "card", at(NOW))
                .await
                .unwrap()
                .cards[0]
                .id;
            let StartedSession::Started { session_id } =
                study::start_session(&pool, deck, at(NOW)).await.unwrap()
            else {
                panic!("expected a session");
            };
            study::record_review(&pool, session_id, card, 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            let due: String = sqlx::query_scalar("SELECT due FROM flashcards WHERE id = ?1")
                .bind(card)
                .fetch_one(&pool)
                .await
                .unwrap();
            let expected = crate::scheduler::review(
                &crate::scheduler::Schedule {
                    state: crate::scheduler::CardState::New,
                    memory: None,
                    last_review: None,
                    reps: 0,
                    lapses: 0,
                },
                Rating::Good,
                at(NOW),
                &proposal.proposed,
                retention::as_share(retention::DEFAULT_PERCENT),
            )
            .unwrap();
            assert_eq!(due, to_db_time(expected.due));
            let first_good = good(&proposal, IntervalCase::FirstGood);
            assert_eq!(expected.scheduled_days, f64::from(first_good.proposed));

            // Applying the same result again is stale: what's in use changed.
            assert!(matches!(
                apply(&pool, &held, at(NOW)).await,
                Err(OptimizerError::Stale)
            ));

            // Resetting goes back to the defaults, keeping the history.
            assert!(matches!(
                reset(&pool, 0, at(NOW)).await,
                Err(OptimizerError::Stale)
            ));
            reset(&pool, 1, at("2026-10-20T09:00:00Z")).await.unwrap();
            let reset_to = settings_of(&pool).await;
            assert!(!reset_to.personal);
            assert_eq!(reset_to.version, 2);
            assert_eq!(reset_to.parameters, DEFAULT_PARAMETERS.to_vec());
            assert_eq!(reset_to.reviews_used, None);
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM fsrs_parameters").await,
                2
            );
            // Already the defaults.
            assert!(matches!(
                reset(&pool, 2, at(NOW)).await,
                Err(OptimizerError::Stale)
            ));
            let mut tx = pool.acquire().await.unwrap();
            assert!(parameters_for_review(&mut tx).await.unwrap().is_empty());
        });
    }

    #[test]
    fn parameters_that_dont_fit_better_are_not_applied() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let proposal = Proposal {
                version: 0,
                reviews_used: 500,
                current: DEFAULT_PARAMETERS.to_vec(),
                proposed: DEFAULT_PARAMETERS.to_vec(),
                current_fit: Fit {
                    log_loss: 0.3,
                    error: 0.05,
                },
                proposed_fit: Fit {
                    log_loss: 0.3,
                    error: 0.05,
                },
                better: false,
                intervals: Vec::new(),
            };
            assert!(matches!(
                apply(&pool, &proposal, at(NOW)).await,
                Err(OptimizerError::NotBetter)
            ));
            let unusable = Proposal {
                better: true,
                proposed: vec![f32::NAN; 21],
                ..proposal
            };
            assert!(matches!(
                apply(&pool, &unusable, at(NOW)).await,
                Err(OptimizerError::Internal(_))
            ));
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM fsrs_parameters").await,
                0
            );
        });
    }

    #[test]
    fn a_run_past_its_time_limit_stops_and_holds_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            add_history(&pool, 110).await;
            let optimizing = Optimizing::default();
            let outcome = optimize(&pool, &optimizing, Duration::ZERO).await;
            assert!(
                matches!(outcome, Err(OptimizerError::TimedOut)),
                "{outcome:?}"
            );
            assert!(optimizing.running().held.is_none());
            assert!(optimizing.running().current.is_none());
        });
    }

    #[test]
    fn stopping_or_starting_again_aborts_the_run_and_forgets_its_result() {
        let optimizing = Optimizing::default();
        let first = optimizing.begin();
        let second = optimizing.begin();
        assert!(aborted(&first.progress), "a new run stops the old one");
        assert!(!aborted(&second.progress));
        // The first run, ending late, can't hold its result.
        assert_eq!(optimizing.hold(first.id, sample_proposal()), None);
        let token = optimizing.hold(second.id, sample_proposal()).unwrap();

        optimizing.stop();
        assert!(aborted(&second.progress));
        assert!(optimizing.running().current.is_none());
        // Stopping doesn't drop a finished result; another run, or a
        // restored backup, does.
        assert!(optimizing.take(token + 1).is_none());
        assert!(optimizing.take(token).is_some());
        let token = optimizing.hold(second.id, sample_proposal()).unwrap();
        optimizing.forget();
        assert!(optimizing.take(token).is_none());
        drop(first);
        drop(second);
        let third = optimizing.begin();
        let token = optimizing.hold(third.id, sample_proposal()).unwrap();
        drop(third);
        let _fourth = optimizing.begin();
        assert!(optimizing.take(token).is_none());
    }

    fn sample_proposal() -> Proposal {
        Proposal {
            version: 0,
            reviews_used: 1,
            current: Vec::new(),
            proposed: Vec::new(),
            current_fit: Fit {
                log_loss: 0.0,
                error: 0.0,
            },
            proposed_fit: Fit {
                log_loss: 0.0,
                error: 0.0,
            },
            better: false,
            intervals: Vec::new(),
        }
    }

    #[test]
    fn the_table_keeps_every_change_and_checks_what_it_stores() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            for sql in [
                // Not 21 numbers, not JSON, or a count without parameters.
                "INSERT INTO fsrs_parameters (parameters, reviews_used, applied_at)
                 VALUES ('[1, 2, 3]', 500, 'x')",
                "INSERT INTO fsrs_parameters (parameters, reviews_used, applied_at)
                 VALUES ('not json', 500, 'x')",
                "INSERT INTO fsrs_parameters (reviews_used, applied_at) VALUES (500, 'x')",
                "INSERT INTO fsrs_parameters (parameters, applied_at)
                 VALUES ('[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]', 'x')",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            sqlx::query("INSERT INTO fsrs_parameters (applied_at) VALUES ('x')")
                .execute(&pool)
                .await
                .unwrap();
            for sql in [
                "UPDATE fsrs_parameters SET applied_at = 'y'",
                "DELETE FROM fsrs_parameters",
            ] {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM fsrs_parameters").await,
                1
            );
        });
    }
}
