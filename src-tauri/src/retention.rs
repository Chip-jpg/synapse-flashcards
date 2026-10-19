//! Desired retention: the chance of remembering a card that FSRS aims for
//! when the card is due. Higher means shorter intervals and more reviews.
//!
//! - **One global value, and optional per-deck values.** Both are whole
//!   percentages from [`MIN_PERCENT`] to [`MAX_PERCENT`]; with nothing chosen,
//!   [`DEFAULT_PERCENT`]. The sample deck and archived decks follow the global
//!   value.
//! - **Every change is a new row** in `desired_retention` (migration 0014);
//!   the newest row for each scope is in use. Changes are refused as stale if
//!   anything changed since the screen was shown.
//! - **Only future ratings change.** `study::record_review` reads the value
//!   for the card's deck at each rating. No stored schedule and no
//!   `review_logs` row is rewritten.
//! - **Estimates before applying.** [`preview`] runs the `fsrs` crate's
//!   simulator over the user's own cards, with the parameters in use, and
//!   compares the daily reviews at the values in use with the chosen ones.
//!   Nothing is written.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use fsrs::{check_and_fill_parameters, Card, FSRSError, SimulatorConfig};
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::{to_db_time, DbError};
use crate::optimizer;

/// FSRS's standard recommendation, and Synapse's only value before 0014.
pub const DEFAULT_PERCENT: i64 = 90;
/// Below this, a quarter or more of due cards are forgotten; FSRS's own tools
/// stop recommending values here.
pub const MIN_PERCENT: i64 = 70;
/// Above this, the reviews needed grow very fast for little gain.
pub const MAX_PERCENT: i64 = 97;

/// How many days ahead an estimate looks. A new value only takes effect at
/// each card's next rating, so a short horizon would hide most of it.
pub const HORIZON_DAYS: usize = 90;
/// Simulated ratings are random; each estimate is the average of this many
/// runs, each with its own fixed seed so the same data gives the same result.
const RUNS: u64 = 4;
/// Below this many first ratings (or this many remembered reviews), the
/// shares of each rating come from FSRS's averages, not the user's own.
const OWN_RATINGS_MINIMUM: i64 = 50;

const MS_PER_DAY: f64 = 86_400_000.0;

/// Why a change (or its estimate) was refused.
#[derive(Debug)]
pub enum RetentionError {
    /// Not a whole percentage from 70 to 97 (or no value for the global one).
    OutOfRange,
    NotFound,
    /// The sample deck follows the global value.
    SampleDeck,
    /// An archived deck follows the global value.
    ArchivedDeck,
    /// The value chosen is already the one in use.
    Unchanged,
    /// Something changed since the settings were shown.
    Stale,
    /// Database or FSRS failure. Detail is for logs only.
    Internal(DbError),
}

impl From<sqlx::Error> for RetentionError {
    fn from(err: sqlx::Error) -> Self {
        RetentionError::Internal(err.into())
    }
}

impl From<DbError> for RetentionError {
    fn from(err: DbError) -> Self {
        RetentionError::Internal(err)
    }
}

impl From<FSRSError> for RetentionError {
    fn from(err: FSRSError) -> Self {
        RetentionError::Internal(format!("FSRS failed: {err:?}").into())
    }
}

/// A change the user asks for: the global value (`deck_id` `None`), or a
/// deck's own value, where `percent` `None` means "follow the global value".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub deck_id: Option<i64>,
    pub percent: Option<i64>,
}

/// What's stored: the newest row for each scope.
#[derive(Debug, Clone, Default, PartialEq)]
struct Stored {
    /// The newest row's id, or 0 with no rows. Changes with every change.
    version: i64,
    /// `None` until a global value is chosen (then [`DEFAULT_PERCENT`]).
    global: Option<i64>,
    /// Each deck's newest row: its own value, or `None` to follow the global.
    decks: HashMap<i64, Option<i64>>,
}

impl Stored {
    fn global(&self) -> i64 {
        self.global.unwrap_or(DEFAULT_PERCENT)
    }

    /// A deck's own value, if it has one in use.
    fn own(&self, deck_id: i64) -> Option<i64> {
        self.decks.get(&deck_id).copied().flatten()
    }

    /// What the stored values become with `change`.
    fn with(&self, change: Change) -> Stored {
        let mut next = self.clone();
        match change.deck_id {
            None => next.global = change.percent,
            Some(deck) => {
                next.decks.insert(deck, change.percent);
            }
        }
        next
    }
}

async fn stored(conn: &mut SqliteConnection) -> Result<Stored, DbError> {
    let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM desired_retention")
        .fetch_one(&mut *conn)
        .await?;
    let global: Option<i64> = sqlx::query_scalar(
        "SELECT percent FROM desired_retention WHERE deck_id IS NULL ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    let decks: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT r.deck_id, r.percent FROM desired_retention r
         WHERE r.id = (SELECT MAX(id) FROM desired_retention WHERE deck_id = r.deck_id)",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Stored {
        version,
        global,
        decks: decks.into_iter().collect(),
    })
}

/// The desired retention for the next rating of a card in `deck_id`, as
/// FSRS takes it (0.70 to 0.97): the deck's own value if it's an active,
/// ordinary deck with one, else the global value.
pub async fn for_deck(conn: &mut SqliteConnection, deck_id: i64) -> Result<f32, DbError> {
    let own: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT r.percent FROM desired_retention r JOIN decks d ON d.id = r.deck_id
         WHERE r.deck_id = ?1 AND d.is_sample = 0 AND d.archived_at IS NULL
         ORDER BY r.id DESC LIMIT 1",
    )
    .bind(deck_id)
    .fetch_optional(&mut *conn)
    .await?;
    let percent = match own.flatten() {
        Some(percent) => percent,
        None => global(conn).await?,
    };
    Ok(as_share(percent))
}

/// The global value in use, as a whole percentage.
pub async fn global(conn: &mut SqliteConnection) -> Result<i64, DbError> {
    let global: Option<i64> = sqlx::query_scalar(
        "SELECT percent FROM desired_retention WHERE deck_id IS NULL ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(global.unwrap_or(DEFAULT_PERCENT))
}

pub fn as_share(percent: i64) -> f32 {
    percent as f32 / 100.0
}

/// Checks `change` against what's stored. Writes nothing.
async fn check(
    conn: &mut SqliteConnection,
    stored: &Stored,
    change: Change,
) -> Result<(), RetentionError> {
    if let Some(percent) = change.percent {
        if !(MIN_PERCENT..=MAX_PERCENT).contains(&percent) {
            return Err(RetentionError::OutOfRange);
        }
    }
    match change.deck_id {
        None => {
            let percent = change.percent.ok_or(RetentionError::OutOfRange)?;
            if percent == stored.global() {
                return Err(RetentionError::Unchanged);
            }
        }
        Some(deck_id) => {
            let deck: Option<(bool, bool)> = sqlx::query_as(
                "SELECT is_sample, archived_at IS NOT NULL FROM decks WHERE id = ?1",
            )
            .bind(deck_id)
            .fetch_optional(&mut *conn)
            .await?;
            match deck {
                None => return Err(RetentionError::NotFound),
                Some((true, _)) => return Err(RetentionError::SampleDeck),
                Some((_, true)) => return Err(RetentionError::ArchivedDeck),
                Some(_) => {}
            }
            if change.percent == stored.own(deck_id) {
                return Err(RetentionError::Unchanged);
            }
        }
    }
    Ok(())
}

/// Makes `change`, for every rating from now on. `version` is the version
/// of the settings the user saw; refused as stale if they changed since.
pub async fn set(
    pool: &SqlitePool,
    version: i64,
    change: Change,
    now: DateTime<Utc>,
) -> Result<(), RetentionError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let stored = stored(&mut tx).await?;
    if stored.version != version {
        return Err(RetentionError::Stale);
    }
    check(&mut tx, &stored, change).await?;
    sqlx::query("INSERT INTO desired_retention (deck_id, percent, set_at) VALUES (?1, ?2, ?3)")
        .bind(change.deck_id)
        .bind(change.percent)
        .bind(to_db_time(now))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// A deck on the settings screen: `{ id, name, percent }`, `percent` being
/// its own value or `null` when it follows the global value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeckRetention {
    pub id: i64,
    pub name: String,
    pub percent: Option<i64>,
}

/// The settings screen: `{ version, global, default, minimum, maximum,
/// decks, archivedOwn }`, values in whole percentages. `decks` are the
/// active, ordinary decks by name; `archivedOwn` counts archived decks with
/// a value of their own, kept for when they're unarchived.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub version: i64,
    pub global: i64,
    pub default: i64,
    pub minimum: i64,
    pub maximum: i64,
    pub decks: Vec<DeckRetention>,
    pub archived_own: usize,
}

pub async fn settings(pool: &SqlitePool) -> Result<Settings, DbError> {
    let mut conn = pool.acquire().await?;
    let stored = stored(&mut conn).await?;
    let decks: Vec<(i64, String, bool)> = sqlx::query_as(
        "SELECT id, name, archived_at IS NOT NULL FROM decks
         WHERE is_sample = 0 ORDER BY name COLLATE NOCASE, id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let archived_own = decks
        .iter()
        .filter(|(id, _, archived)| *archived && stored.own(*id).is_some())
        .count();
    Ok(Settings {
        version: stored.version,
        global: stored.global(),
        default: DEFAULT_PERCENT,
        minimum: MIN_PERCENT,
        maximum: MAX_PERCENT,
        decks: decks
            .into_iter()
            .filter(|(_, _, archived)| !archived)
            .map(|(id, name, _)| DeckRetention {
                percent: stored.own(id),
                id,
                name,
            })
            .collect(),
        archived_own,
    })
}

/// One active card, as the estimate needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CardRow {
    pub deck_id: i64,
    pub is_sample: bool,
    pub fsrs_stability: Option<f64>,
    pub fsrs_difficulty: Option<f64>,
    pub last_review: Option<String>,
    pub due: Option<String>,
}

/// How often the user picks each rating, which the simulator samples from:
/// first ratings (Again to Easy) and remembered reviews (Hard to Easy).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatingShares {
    pub first: [f32; 4],
    pub review: [f32; 3],
    /// Whether these are the user's own (else FSRS's averages).
    pub own: bool,
}

impl Default for RatingShares {
    fn default() -> Self {
        let config = SimulatorConfig::default();
        RatingShares {
            first: config.first_rating_prob,
            review: config.review_rating_prob,
            own: false,
        }
    }
}

async fn rating_shares(conn: &mut SqliteConnection) -> Result<RatingShares, DbError> {
    let first: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT rating, COUNT(*) FROM review_logs WHERE state_before = 'New' GROUP BY rating",
    )
    .fetch_all(&mut *conn)
    .await?;
    let review: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT rating, COUNT(*) FROM review_logs
         WHERE state_before = 'Review' AND rating > 1 GROUP BY rating",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(shares_from_counts(&first, &review))
}

/// The user's own shares when there are enough ratings, else FSRS's
/// averages. One is added to every count, so no rating is impossible.
fn shares_from_counts(first: &[(i64, i64)], review: &[(i64, i64)]) -> RatingShares {
    let total = |counts: &[(i64, i64)]| counts.iter().map(|(_, n)| n).sum::<i64>();
    if total(first) < OWN_RATINGS_MINIMUM || total(review) < OWN_RATINGS_MINIMUM {
        return RatingShares::default();
    }
    let count = |counts: &[(i64, i64)], rating: i64| {
        counts
            .iter()
            .find(|(r, _)| *r == rating)
            .map_or(0, |(_, n)| *n) as f32
            + 1.0
    };
    RatingShares {
        first: [1, 2, 3, 4].map(|rating| count(first, rating)),
        review: [2, 3, 4].map(|rating| count(review, rating)),
        own: true,
    }
}

/// An estimated workload over the next [`HORIZON_DAYS`] days.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    /// Reviews a day, on average: every rating, new cards' first included.
    pub reviews_per_day: f32,
    /// How many of the cards the user would remember on the last day.
    pub remembered: f32,
}

/// The estimate for a change, at the values in use and with it: `{ version,
/// cards, days, current, proposed, ownRatings }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub version: i64,
    /// How many active cards were simulated.
    pub cards: usize,
    pub days: usize,
    pub current: Estimate,
    pub proposed: Estimate,
    /// Whether ratings were drawn from the user's own shares.
    pub own_ratings: bool,
}

/// Estimates the daily reviews at the values in use and with `change`.
/// Writes nothing. Refused like [`set`] would refuse the change.
pub async fn preview(
    pool: &SqlitePool,
    version: i64,
    change: Change,
    now: DateTime<Utc>,
) -> Result<Preview, RetentionError> {
    let mut conn = pool.acquire().await?;
    let stored = stored(&mut conn).await?;
    if stored.version != version {
        return Err(RetentionError::Stale);
    }
    check(&mut conn, &stored, change).await?;
    let parameters = optimizer::in_use(&mut conn).await?.parameters();
    let shares = rating_shares(&mut conn).await?;
    // Archived decks aren't reviewed, so they add nothing to the workload.
    let cards = sqlx::query_as::<_, CardRow>(
        "SELECT f.deck_id, d.is_sample, f.fsrs_stability, f.fsrs_difficulty,
                f.last_review, f.due
         FROM flashcards f JOIN decks d ON d.id = f.deck_id
         WHERE f.deleted_at IS NULL AND d.archived_at IS NULL",
    )
    .fetch_all(&mut *conn)
    .await?;
    drop(conn);

    let proposed = stored.with(change);
    let percent_of = |values: &Stored, card: &CardRow| match card.is_sample {
        true => values.global(),
        false => values.own(card.deck_id).unwrap_or(values.global()),
    };
    let current_percents: Vec<i64> = cards.iter().map(|c| percent_of(&stored, c)).collect();
    let proposed_percents: Vec<i64> = cards.iter().map(|c| percent_of(&proposed, c)).collect();

    let work = tauri::async_runtime::spawn_blocking(move || {
        let simulated = sim_cards(&cards, now)?;
        let current = estimate(&simulated, &current_percents, &parameters, shares)?;
        let proposed = estimate(&simulated, &proposed_percents, &parameters, shares)?;
        Ok::<_, RetentionError>((simulated.len(), current, proposed))
    })
    .await;
    let (cards, current, proposed) =
        work.map_err(|err| -> DbError { format!("the estimate thread failed: {err}").into() })??;
    Ok(Preview {
        version,
        cards,
        days: HORIZON_DAYS,
        current,
        proposed,
        own_ratings: shares.own,
    })
}

/// A card as the estimate sees it: `None` for one never reviewed, else its
/// stability, difficulty, and days since its last review and until it's due
/// (0 when already due), counted from `now`.
pub type SimCard = Option<(f32, f32, f32, f32)>;

pub fn sim_cards(cards: &[CardRow], now: DateTime<Utc>) -> Result<Vec<SimCard>, DbError> {
    let days_from_now = |text: &str| -> Result<f32, DbError> {
        let at = DateTime::parse_from_rfc3339(text)
            .map_err(|err| format!("card time {text:?} isn't readable: {err}"))?
            .with_timezone(&Utc);
        Ok(((at - now).num_milliseconds() as f64 / MS_PER_DAY) as f32)
    };
    cards
        .iter()
        .map(|card| {
            match (
                card.fsrs_stability,
                card.fsrs_difficulty,
                &card.last_review,
                &card.due,
            ) {
                (Some(stability), Some(difficulty), Some(last), Some(due)) => {
                    let since = -days_from_now(last)?;
                    let until = days_from_now(due)?.max(0.0);
                    Ok(Some((
                        stability as f32,
                        difficulty as f32,
                        since.max(0.0),
                        until,
                    )))
                }
                _ => Ok(None),
            }
        })
        .collect()
}

/// Simulates `cards`, each aiming for its percentage in `percents`, and
/// averages [`RUNS`] runs. Cards that aim for the same value are simulated
/// together; groups don't affect each other, since nothing limits the
/// reviews a day.
pub fn estimate(
    cards: &[SimCard],
    percents: &[i64],
    parameters: &[f32],
    shares: RatingShares,
) -> Result<Estimate, FSRSError> {
    let filled = Arc::new(check_and_fill_parameters(parameters)?);
    let mut groups: BTreeMap<i64, (Vec<Card>, usize)> = BTreeMap::new();
    for (index, (card, percent)) in cards.iter().zip(percents).enumerate() {
        let group = groups.entry(*percent).or_default();
        group.1 += 1;
        if let Some((stability, difficulty, since, until)) = *card {
            group.0.push(Card {
                id: index as i64 + 1,
                difficulty,
                stability,
                last_date: -since,
                due: until,
                interval: until + since,
                lapses: 0,
                desired_retention: as_share(*percent),
                parameters: filled.clone(),
            });
        }
    }

    let mut reviews = 0.0;
    let mut remembered = 0.0;
    for (percent, (reviewed, total)) in groups {
        let config = SimulatorConfig {
            deck_size: total,
            learn_span: HORIZON_DAYS,
            max_cost_perday: f32::INFINITY,
            first_rating_prob: shares.first,
            review_rating_prob: shares.review,
            ..SimulatorConfig::default()
        };
        for seed in 0..RUNS {
            let result = fsrs::simulate(
                &config,
                parameters,
                as_share(percent),
                Some(seed),
                Some(reviewed.clone()),
            )?;
            let done: usize = result.review_cnt_per_day.iter().sum::<usize>()
                + result.learn_cnt_per_day.iter().sum::<usize>();
            reviews += done as f32;
            remembered += result.memorized_cnt_per_day.last().copied().unwrap_or(0.0);
        }
    }
    let runs = RUNS as f32;
    Ok(Estimate {
        reviews_per_day: reviews / runs / HORIZON_DAYS as f32,
        remembered: remembered / runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::{archive_deck, create_deck, create_flashcard, unarchive_deck};
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use crate::scheduler::Rating;
    use crate::study::{self, StartedSession};

    const NOW: &str = "2026-10-19T09:00:00Z";

    async fn pool() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    fn global_to(percent: i64) -> Change {
        Change {
            deck_id: None,
            percent: Some(percent),
        }
    }

    fn deck_to(deck: i64, percent: Option<i64>) -> Change {
        Change {
            deck_id: Some(deck),
            percent,
        }
    }

    async fn deck_share(pool: &SqlitePool, deck: i64) -> f32 {
        for_deck(&mut pool.acquire().await.unwrap(), deck)
            .await
            .unwrap()
    }

    async fn rows(pool: &SqlitePool) -> i64 {
        count(pool, "SELECT COUNT(*) FROM desired_retention").await
    }

    #[test]
    fn a_new_database_aims_for_ninety_percent_everywhere() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let deck = create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            assert_eq!(
                settings(&pool).await.unwrap(),
                Settings {
                    version: 0,
                    global: 90,
                    default: 90,
                    minimum: 70,
                    maximum: 97,
                    // The sample deck isn't listed: it follows the global value.
                    decks: vec![DeckRetention {
                        id: deck,
                        name: "Biology".into(),
                        percent: None,
                    }],
                    archived_own: 0,
                }
            );
            assert_eq!(deck_share(&pool, deck).await, 0.9);
            assert_eq!(deck_share(&pool, sample_deck_id(&pool).await).await, 0.9);
        });
    }

    #[test]
    fn global_and_deck_values_are_appended_and_followed() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let biology = create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            let history = create_deck(&pool, "History", None, at(NOW))
                .await
                .unwrap()
                .id;
            let sample = sample_deck_id(&pool).await;

            set(&pool, 0, global_to(85), at(NOW)).await.unwrap();
            set(&pool, 1, deck_to(biology, Some(95)), at(NOW))
                .await
                .unwrap();
            assert_eq!(deck_share(&pool, biology).await, 0.95);
            assert_eq!(deck_share(&pool, history).await, 0.85);
            assert_eq!(deck_share(&pool, sample).await, 0.85);

            // The global value changes for every deck without its own.
            set(&pool, 2, global_to(80), at(NOW)).await.unwrap();
            assert_eq!(deck_share(&pool, biology).await, 0.95);
            assert_eq!(deck_share(&pool, history).await, 0.8);

            // A deck can go back to following it.
            set(&pool, 3, deck_to(biology, None), at(NOW))
                .await
                .unwrap();
            assert_eq!(deck_share(&pool, biology).await, 0.8);

            let shown = settings(&pool).await.unwrap();
            assert_eq!(shown.version, 4);
            assert_eq!(shown.global, 80);
            assert!(shown.decks.iter().all(|deck| deck.percent.is_none()));
            assert_eq!(rows(&pool).await, 4);
        });
    }

    #[test]
    fn refused_changes_write_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let deck = create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            let archived = create_deck(&pool, "Old", None, at(NOW)).await.unwrap().id;
            archive_deck(&pool, archived, at(NOW)).await.unwrap();
            let sample = sample_deck_id(&pool).await;

            let refused = [
                (global_to(69), "out of range"),
                (global_to(98), "out of range"),
                (
                    Change {
                        deck_id: None,
                        percent: None,
                    },
                    "out of range",
                ),
                (global_to(90), "unchanged"),
                (deck_to(deck, None), "unchanged"),
                (deck_to(deck, Some(101)), "out of range"),
                (deck_to(sample, Some(80)), "sample"),
                (deck_to(archived, Some(80)), "archived"),
                (deck_to(9_999, Some(80)), "missing"),
            ];
            for (change, why) in refused {
                let result = set(&pool, 0, change, at(NOW)).await;
                let expected = match why {
                    "out of range" => matches!(result, Err(RetentionError::OutOfRange)),
                    "unchanged" => matches!(result, Err(RetentionError::Unchanged)),
                    "sample" => matches!(result, Err(RetentionError::SampleDeck)),
                    "archived" => matches!(result, Err(RetentionError::ArchivedDeck)),
                    _ => matches!(result, Err(RetentionError::NotFound)),
                };
                assert!(
                    expected,
                    "{change:?} should be refused as {why}: {result:?}"
                );
                // The estimate refuses it the same way.
                assert!(preview(&pool, 0, change, at(NOW)).await.is_err());
            }

            // A screen showing an older version is stale, for both.
            set(&pool, 0, global_to(85), at(NOW)).await.unwrap();
            assert!(matches!(
                set(&pool, 0, global_to(80), at(NOW)).await,
                Err(RetentionError::Stale)
            ));
            assert!(matches!(
                preview(&pool, 0, global_to(80), at(NOW)).await,
                Err(RetentionError::Stale)
            ));
            assert_eq!(rows(&pool).await, 1);
        });
    }

    #[test]
    fn archived_decks_follow_the_global_value_until_unarchived() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let deck = create_deck(&pool, "Biology", None, at(NOW))
                .await
                .unwrap()
                .id;
            set(&pool, 0, deck_to(deck, Some(75)), at(NOW))
                .await
                .unwrap();
            archive_deck(&pool, deck, at(NOW)).await.unwrap();

            assert_eq!(deck_share(&pool, deck).await, 0.9);
            let shown = settings(&pool).await.unwrap();
            assert!(shown.decks.is_empty());
            assert_eq!(shown.archived_own, 1);

            unarchive_deck(&pool, deck).await.unwrap();
            assert_eq!(deck_share(&pool, deck).await, 0.75);
            assert_eq!(settings(&pool).await.unwrap().decks[0].percent, Some(75));
        });
    }

    #[test]
    fn the_database_refuses_bad_rows_and_never_changes_them() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let sample = sample_deck_id(&pool).await;
            sqlx::query("INSERT INTO desired_retention (percent, set_at) VALUES (85, 'x')")
                .execute(&pool)
                .await
                .unwrap();
            let refused = [
                "INSERT INTO desired_retention (percent, set_at) VALUES (NULL, 'x')",
                "INSERT INTO desired_retention (percent, set_at) VALUES (69, 'x')",
                "INSERT INTO desired_retention (percent, set_at) VALUES (98, 'x')",
                "INSERT INTO desired_retention (percent, set_at) VALUES (0.9, 'x')",
                "INSERT INTO desired_retention (percent) VALUES (80)",
                "INSERT INTO desired_retention (deck_id, percent, set_at) VALUES (9999, 80, 'x')",
                "UPDATE desired_retention SET percent = 80",
                "DELETE FROM desired_retention",
            ];
            for sql in refused {
                assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
            }
            assert!(sqlx::query(
                "INSERT INTO desired_retention (deck_id, percent, set_at) VALUES (?1, 80, 'x')"
            )
            .bind(sample)
            .execute(&pool)
            .await
            .is_err());
            assert_eq!(rows(&pool).await, 1);
        });
    }

    #[test]
    fn ratings_use_the_decks_value_and_stored_schedules_stay() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let low = create_deck(&pool, "Low", None, at(NOW)).await.unwrap().id;
            let high = create_deck(&pool, "High", None, at(NOW)).await.unwrap().id;
            let mut cards = Vec::new();
            for deck in [low, high] {
                let card = create_flashcard(&pool, deck, "Q", "A", at(NOW))
                    .await
                    .unwrap()
                    .cards[0]
                    .id;
                cards.push(card);
            }
            // A card reviewed before anything changed keeps its schedule.
            let StartedSession::Started { session_id } =
                study::start_session(&pool, low, at(NOW)).await.unwrap()
            else {
                panic!("expected a session");
            };
            let early = create_flashcard(&pool, low, "Early", "A", at(NOW))
                .await
                .unwrap()
                .cards
                .last()
                .unwrap()
                .id;
            study::record_review(&pool, session_id, early, 0, Rating::Good, at(NOW))
                .await
                .unwrap();
            let card_rows = "SELECT id, fsrs_stability, due, reps FROM flashcards ORDER BY id";
            let before: Vec<(i64, Option<f64>, Option<String>, i64)> =
                sqlx::query_as(card_rows).fetch_all(&pool).await.unwrap();

            set(&pool, 0, global_to(97), at(NOW)).await.unwrap();
            set(&pool, 1, deck_to(low, Some(70)), at(NOW))
                .await
                .unwrap();
            let after: Vec<(i64, Option<f64>, Option<String>, i64)> =
                sqlx::query_as(card_rows).fetch_all(&pool).await.unwrap();
            assert_eq!(before, after);

            let mut dues = Vec::new();
            for (deck, card) in [low, high].into_iter().zip(&cards) {
                let session_id = match study::start_session(&pool, deck, at(NOW)).await.unwrap() {
                    StartedSession::Started { session_id } => session_id,
                    StartedSession::NoneDue => panic!("expected a session"),
                };
                study::record_review(&pool, session_id, *card, 0, Rating::Good, at(NOW))
                    .await
                    .unwrap();
                let due: String = sqlx::query_scalar("SELECT due FROM flashcards WHERE id = ?1")
                    .bind(card)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                dues.push(due);
            }
            let expected = |retention: f32| {
                crate::scheduler::review(
                    &crate::scheduler::Schedule {
                        state: crate::scheduler::CardState::New,
                        memory: None,
                        last_review: None,
                        reps: 0,
                        lapses: 0,
                    },
                    Rating::Good,
                    at(NOW),
                    &[],
                    retention,
                )
                .unwrap()
                .due
            };
            assert_eq!(dues[0], to_db_time(expected(0.7)));
            assert_eq!(dues[1], to_db_time(expected(0.97)));
            // Aiming lower waits longer.
            assert!(dues[0] > dues[1]);
        });
    }

    #[test]
    fn the_estimate_compares_workloads_and_writes_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = pool().await;
            let deck = optimizer::tests::add_history(&pool, 120).await;
            // Bring the cards' stored state in line with their history, as
            // reviewing them would have.
            sqlx::query(
                "UPDATE flashcards SET fsrs_state = 'Review', fsrs_stability = 40,
                        fsrs_difficulty = 5, last_review = '2026-10-01T09:00:00.000Z',
                        due = '2026-11-10T09:00:00.000Z', reps = 5
                 WHERE deck_id = ?1",
            )
            .bind(deck)
            .execute(&pool)
            .await
            .unwrap();
            for n in 0..20 {
                create_flashcard(&pool, deck, &format!("New {n}"), "A", at(NOW))
                    .await
                    .unwrap();
            }

            let lower = preview(&pool, 0, global_to(80), at(NOW)).await.unwrap();
            // 120 reviewed, 20 new, and the sample card.
            assert_eq!(lower.cards, 141);
            assert_eq!(lower.days, HORIZON_DAYS);
            assert!(lower.own_ratings);
            assert!(
                lower.proposed.reviews_per_day < lower.current.reviews_per_day,
                "{lower:?}"
            );
            assert!(
                lower.proposed.remembered < lower.current.remembered,
                "{lower:?}"
            );

            let higher = preview(&pool, 0, deck_to(deck, Some(97)), at(NOW))
                .await
                .unwrap();
            assert!(
                higher.proposed.reviews_per_day > higher.current.reviews_per_day,
                "{higher:?}"
            );
            assert_eq!(higher.current, lower.current);
            // Same data, same estimate.
            assert_eq!(
                preview(&pool, 0, global_to(80), at(NOW)).await.unwrap(),
                lower
            );
            assert_eq!(rows(&pool).await, 0);
        });
    }

    #[test]
    fn an_estimate_with_no_cards_is_zero() {
        let estimate = estimate(&[], &[], &[], RatingShares::default()).unwrap();
        assert_eq!(
            estimate,
            Estimate {
                reviews_per_day: 0.0,
                remembered: 0.0
            }
        );
    }

    #[test]
    fn own_rating_shares_need_enough_ratings() {
        assert_eq!(
            shares_from_counts(&[(3, 49)], &[(3, 100)]),
            RatingShares::default()
        );
        let own = shares_from_counts(&[(1, 9), (3, 50)], &[(2, 4), (3, 60), (4, 10)]);
        assert!(own.own);
        assert_eq!(own.first, [10.0, 1.0, 51.0, 1.0]);
        assert_eq!(own.review, [5.0, 61.0, 11.0]);
    }

    #[test]
    fn cards_are_placed_relative_to_now() {
        let row = |stability, last: Option<&str>, due: Option<&str>| CardRow {
            deck_id: 1,
            is_sample: false,
            fsrs_stability: stability,
            fsrs_difficulty: stability.map(|_| 5.0),
            last_review: last.map(str::to_string),
            due: due.map(str::to_string),
        };
        let cards = sim_cards(
            &[
                row(None, None, None),
                row(
                    Some(10.0),
                    Some("2026-10-09T09:00:00.000Z"),
                    Some("2026-10-21T21:00:00.000Z"),
                ),
                // Overdue: due today.
                row(
                    Some(3.0),
                    Some("2026-10-01T09:00:00.000Z"),
                    Some("2026-10-04T09:00:00.000Z"),
                ),
            ],
            at(NOW),
        )
        .unwrap();
        assert_eq!(
            cards,
            vec![
                None,
                Some((10.0, 5.0, 10.0, 2.5)),
                Some((3.0, 5.0, 18.0, 0.0))
            ]
        );
    }
}
