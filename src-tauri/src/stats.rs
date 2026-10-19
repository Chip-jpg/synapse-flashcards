//! Statistics: what the review history and the cards' schedules say, read-only.
//!
//! Everything here is counted from stored rows; nothing is estimated or
//! scored. Days are calendar days on the computer's clock, worked out by
//! [`study::day_bounds`], so a day with a clock change is 23 or 25 hours long.
//!
//! Two kinds of numbers, kept apart on purpose:
//! - **History** (reviews per day, retention) counts every rating that
//!   happened, like the Study Desk's "today": ratings of cards deleted since,
//!   and of decks archived since, still happened.
//! - **Current state** (cards by state, the due forecast) counts the cards
//!   you'd study now: not deleted, in decks that aren't archived.
//!
//! Optionally everything is limited to one deck. Only an active deck can be
//! chosen: an archived one has nothing current to show.

use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};
use sqlx::SqlitePool;

use crate::db::to_db_time;
use crate::study::{self, StudyError};

/// How many days of review history are shown, today included.
pub const HISTORY_DAYS: usize = 30;
/// How many days the due forecast covers, today included.
pub const FORECAST_DAYS: usize = 14;

/// Everything on the Statistics screen:
/// `{ reviewsByDay, retention, cardStates, forecast }`.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Statistics {
    /// Ratings recorded on each of the last [`HISTORY_DAYS`] days, oldest
    /// first, ending with today.
    pub reviews_by_day: Vec<DayCount>,
    /// See [`Retention`].
    pub retention: Retention,
    /// See [`CardStates`].
    pub card_states: CardStates,
    /// Cards due on each of the next [`FORECAST_DAYS`] days, starting with
    /// today. Today's count also holds every card already overdue and every
    /// new card not yet studied, since those are due now.
    pub forecast: Vec<DayCount>,
}

/// One calendar day and a count: `{ day: "2026-10-19", count: 4 }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DayCount {
    /// The local date, as `YYYY-MM-DD`.
    pub day: String,
    pub count: i64,
}

/// True retention over the last [`HISTORY_DAYS`] days: `{ reviews, recalled }`.
///
/// Only reviews of cards that were in the Review state when rated count, since
/// those test memory after a real interval; first sightings and (re)learning
/// steps don't. `recalled` is how many of those weren't rated Again. The share
/// is left to the screen, which shows it with `reviews` so a small sample
/// reads as one; with no reviews there is no share to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Retention {
    pub reviews: i64,
    pub recalled: i64,
}

/// How many current cards are in each FSRS state:
/// `{ new, learning, review, relearning }`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, sqlx::FromRow)]
pub struct CardStates {
    pub new: i64,
    pub learning: i64,
    pub review: i64,
    pub relearning: i64,
}

/// The [`Statistics`] at `now` in `zone`, for every deck or only `deck`.
///
/// Refuses a deck that doesn't exist (`NotFound`) or is archived
/// (`DeckArchived`). Writes nothing.
pub async fn statistics<Tz: TimeZone>(
    pool: &SqlitePool,
    deck: Option<i64>,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Result<Statistics, StudyError> {
    let mut tx = pool.begin().await?;

    if let Some(deck) = deck {
        let archived: Option<Option<String>> =
            sqlx::query_scalar("SELECT archived_at FROM decks WHERE id = ?1")
                .bind(deck)
                .fetch_optional(&mut *tx)
                .await?;
        match archived {
            None => return Err(StudyError::NotFound),
            Some(Some(_)) => return Err(StudyError::DeckArchived),
            Some(None) => {}
        }
    }

    let history = past_days(now, zone, HISTORY_DAYS);
    let ahead = next_days(now, zone, FORECAST_DAYS);
    let since = history[0].start;
    let until = history[HISTORY_DAYS - 1].end;

    // `reviewed_at` and `due` are always written by `to_db_time`, so comparing
    // the text compares the times, and each parses back exactly.
    let reviews: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT r.reviewed_at, r.rating, r.state_before
         FROM review_logs r
         JOIN flashcards f ON f.id = r.card_id
         WHERE r.reviewed_at >= ?1 AND r.reviewed_at < ?2
           AND (?3 IS NULL OR f.deck_id = ?3)",
    )
    .bind(to_db_time(since))
    .bind(to_db_time(until))
    .bind(deck)
    .fetch_all(&mut *tx)
    .await?;

    let card_states = sqlx::query_as::<_, CardStates>(
        "SELECT COALESCE(SUM(f.fsrs_state = 'New'), 0) AS new,
                COALESCE(SUM(f.fsrs_state = 'Learning'), 0) AS learning,
                COALESCE(SUM(f.fsrs_state = 'Review'), 0) AS review,
                COALESCE(SUM(f.fsrs_state = 'Relearning'), 0) AS relearning
         FROM flashcards f
         JOIN decks d ON d.id = f.deck_id
         WHERE f.deleted_at IS NULL AND d.archived_at IS NULL
           AND (?1 IS NULL OR f.deck_id = ?1)",
    )
    .bind(deck)
    .fetch_one(&mut *tx)
    .await?;

    let dues: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT f.due
         FROM flashcards f
         JOIN decks d ON d.id = f.deck_id
         WHERE f.deleted_at IS NULL AND d.archived_at IS NULL
           AND (?1 IS NULL OR f.deck_id = ?1)
           AND (f.due IS NULL OR f.due < ?2)",
    )
    .bind(deck)
    .bind(to_db_time(ahead[FORECAST_DAYS - 1].end))
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;

    let mut per_day = vec![0; HISTORY_DAYS];
    let mut retention = Retention {
        reviews: 0,
        recalled: 0,
    };
    for (reviewed_at, rating, state_before) in &reviews {
        if let Some(index) = day_of(&history, parse(reviewed_at)?) {
            per_day[index] += 1;
        }
        if state_before == "Review" {
            retention.reviews += 1;
            if *rating != 1 {
                retention.recalled += 1;
            }
        }
    }

    let mut due_per_day = vec![0; FORECAST_DAYS];
    for due in &dues {
        // A new card without a due date is due now, as is an overdue one.
        let index = match due {
            None => Some(0),
            Some(due) => {
                let due = parse(due)?;
                if due < ahead[0].start {
                    Some(0)
                } else {
                    day_of(&ahead, due)
                }
            }
        };
        if let Some(index) = index {
            due_per_day[index] += 1;
        }
    }

    Ok(Statistics {
        reviews_by_day: counts(&history, per_day),
        retention,
        card_states,
        forecast: counts(&ahead, due_per_day),
    })
}

/// One local calendar day, as UTC instants: `start` inclusive, `end` exclusive.
#[derive(Debug, Clone, Copy)]
struct Day {
    date: NaiveDate,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

fn day_at<Tz: TimeZone>(instant: DateTime<Utc>, zone: &Tz) -> Day {
    let (start, end) = study::day_bounds(instant, zone);
    Day {
        date: instant.with_timezone(zone).date_naive(),
        start,
        end,
    }
}

/// The `count` days ending with `now`'s day, oldest first.
fn past_days<Tz: TimeZone>(now: DateTime<Utc>, zone: &Tz, count: usize) -> Vec<Day> {
    let mut days = vec![day_at(now, zone)];
    while days.len() < count {
        let previous = days[days.len() - 1].start - TimeDelta::milliseconds(1);
        days.push(day_at(previous, zone));
    }
    days.reverse();
    days
}

/// The `count` days starting with `now`'s day.
fn next_days<Tz: TimeZone>(now: DateTime<Utc>, zone: &Tz, count: usize) -> Vec<Day> {
    let mut days = vec![day_at(now, zone)];
    while days.len() < count {
        let next = days[days.len() - 1].end;
        days.push(day_at(next, zone));
    }
    days
}

/// Which of `days` (consecutive, oldest first) `instant` falls on, if any.
fn day_of(days: &[Day], instant: DateTime<Utc>) -> Option<usize> {
    let index = days.partition_point(|day| day.end <= instant);
    days.get(index)
        .filter(|day| day.start <= instant)
        .map(|_| index)
}

fn counts(days: &[Day], counts: Vec<i64>) -> Vec<DayCount> {
    days.iter()
        .zip(counts)
        .map(|(day, count)| DayCount {
            day: day.date.format("%Y-%m-%d").to_string(),
            count,
        })
        .collect()
}

fn parse(stored: &str) -> Result<DateTime<Utc>, StudyError> {
    DateTime::parse_from_rfc3339(stored)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|err| StudyError::Internal(format!("stored time {stored:?}: {err}").into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate_and_seed;
    use crate::db::test_support::*;
    use chrono::FixedOffset;

    const NOW: &str = "2026-10-19T12:00:00Z";

    async fn seeded() -> SqlitePool {
        let pool = memory_pool().await;
        migrate_and_seed(&pool).await.unwrap();
        pool
    }

    async fn add_deck(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar("INSERT INTO decks (name) VALUES (?1) RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn add_card(pool: &SqlitePool, deck: i64, state: &str, due: Option<&str>) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO flashcards (deck_id, front, back, fsrs_state, due)
             VALUES (?1, 'q', 'a', ?2, ?3) RETURNING id",
        )
        .bind(deck)
        .bind(state)
        .bind(due.map(|due| to_db_time(at(due))))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn log_review(pool: &SqlitePool, card: i64, rating: i64, before: &str, when: &str) {
        sqlx::query(
            "INSERT INTO review_logs (card_id, rating, state_before, scheduled_days,
                                      elapsed_days, stability_after, difficulty_after, reviewed_at)
             VALUES (?1, ?2, ?3, 1.0, 0.0, 2.0, 5.0, ?4)",
        )
        .bind(card)
        .bind(rating)
        .bind(before)
        .bind(to_db_time(at(when)))
        .execute(pool)
        .await
        .unwrap();
    }

    async fn delete_card(pool: &SqlitePool, card: i64) {
        sqlx::query("UPDATE flashcards SET deleted_at = ?1 WHERE id = ?2")
            .bind(to_db_time(at(NOW)))
            .bind(card)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn archive(pool: &SqlitePool, deck: i64) {
        sqlx::query("UPDATE decks SET archived_at = ?1 WHERE id = ?2")
            .bind(to_db_time(at(NOW)))
            .bind(deck)
            .execute(pool)
            .await
            .unwrap();
    }

    /// Removes the sample deck's cards, so each test counts only its own.
    async fn without_sample_cards(pool: &SqlitePool) {
        let sample = sample_deck_id(pool).await;
        sqlx::query("UPDATE flashcards SET deleted_at = ?1 WHERE deck_id = ?2")
            .bind(to_db_time(at(NOW)))
            .bind(sample)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn stats_at<Tz: TimeZone>(pool: &SqlitePool, deck: Option<i64>, zone: &Tz) -> Statistics {
        statistics(pool, deck, at(NOW), zone).await.unwrap()
    }

    fn count_on(days: &[DayCount], day: &str) -> i64 {
        days.iter().find(|d| d.day == day).unwrap().count
    }

    #[test]
    fn the_history_is_thirty_days_ending_today_and_the_forecast_fourteen_starting_today() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let stats = stats_at(&pool, None, &Utc).await;
            let days: Vec<&str> = stats
                .reviews_by_day
                .iter()
                .map(|d| d.day.as_str())
                .collect();
            assert_eq!(days.len(), 30);
            assert_eq!(days[0], "2026-09-20");
            assert_eq!(days[29], "2026-10-19");
            let ahead: Vec<&str> = stats.forecast.iter().map(|d| d.day.as_str()).collect();
            assert_eq!(ahead.len(), 14);
            assert_eq!(ahead[0], "2026-10-19");
            assert_eq!(ahead[13], "2026-11-01");
        });
    }

    #[test]
    fn reviews_are_counted_on_their_local_day_within_the_window() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = add_deck(&pool, "Other").await;
            let card = add_card(&pool, deck, "Review", Some("2026-11-30T00:00:00Z")).await;
            log_review(&pool, card, 3, "New", "2026-09-19T23:59:59.999Z").await; // before the window
            log_review(&pool, card, 3, "Learning", "2026-09-20T00:00:00Z").await; // first day
            log_review(&pool, card, 3, "Review", "2026-10-18T10:00:00Z").await;
            log_review(&pool, card, 1, "Review", "2026-10-18T23:30:00Z").await;
            log_review(&pool, card, 3, "Relearning", "2026-10-19T11:00:00Z").await;
            log_review(&pool, card, 3, "Review", "2026-10-20T00:00:00Z").await; // after now's day

            let stats = stats_at(&pool, None, &Utc).await;
            assert_eq!(count_on(&stats.reviews_by_day, "2026-09-20"), 1);
            assert_eq!(count_on(&stats.reviews_by_day, "2026-10-18"), 2);
            assert_eq!(count_on(&stats.reviews_by_day, "2026-10-19"), 1);
            assert_eq!(stats.reviews_by_day.iter().map(|d| d.count).sum::<i64>(), 4);

            // In Tokyo, 23:30 UTC on the 18th is the morning of the 19th, and
            // the window starts nine hours earlier, so the review just before
            // it in UTC falls on its first day.
            let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
            let stats = stats_at(&pool, None, &tokyo).await;
            assert_eq!(count_on(&stats.reviews_by_day, "2026-10-18"), 1);
            assert_eq!(count_on(&stats.reviews_by_day, "2026-10-19"), 2);
            assert_eq!(count_on(&stats.reviews_by_day, "2026-09-20"), 2);
        });
    }

    #[test]
    fn retention_counts_only_reviews_of_cards_in_review_state() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let deck = add_deck(&pool, "Other").await;
            let card = add_card(&pool, deck, "Review", None).await;
            assert_eq!(
                stats_at(&pool, None, &Utc).await.retention,
                Retention {
                    reviews: 0,
                    recalled: 0
                }
            );

            for (rating, before) in [
                (3, "Review"),
                (2, "Review"),
                (4, "Review"),
                (1, "Review"),
                (1, "New"),
                (1, "Learning"),
                (3, "Relearning"),
            ] {
                log_review(&pool, card, rating, before, "2026-10-10T09:00:00Z").await;
            }
            // Too old to count.
            log_review(&pool, card, 1, "Review", "2026-09-01T09:00:00Z").await;

            assert_eq!(
                stats_at(&pool, None, &Utc).await.retention,
                Retention {
                    reviews: 4,
                    recalled: 3
                }
            );
        });
    }

    #[test]
    fn history_keeps_deleted_and_archived_cards_but_current_counts_leave_them_out() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            without_sample_cards(&pool).await;
            let kept = add_deck(&pool, "Kept").await;
            let shelved = add_deck(&pool, "Shelved").await;
            let active = add_card(&pool, kept, "Review", Some("2026-10-21T08:00:00Z")).await;
            let deleted = add_card(&pool, kept, "Review", Some("2026-10-21T08:00:00Z")).await;
            let archived = add_card(&pool, shelved, "Learning", None).await;
            for card in [active, deleted, archived] {
                log_review(&pool, card, 3, "Review", "2026-10-19T08:00:00Z").await;
            }
            delete_card(&pool, deleted).await;
            archive(&pool, shelved).await;

            let stats = stats_at(&pool, None, &Utc).await;
            assert_eq!(count_on(&stats.reviews_by_day, "2026-10-19"), 3);
            assert_eq!(stats.retention.reviews, 3);
            assert_eq!(
                stats.card_states,
                CardStates {
                    review: 1,
                    ..CardStates::default()
                }
            );
            assert_eq!(stats.forecast.iter().map(|d| d.count).sum::<i64>(), 1);
            assert_eq!(count_on(&stats.forecast, "2026-10-21"), 1);
        });
    }

    #[test]
    fn cards_are_counted_by_state() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            without_sample_cards(&pool).await;
            let deck = add_deck(&pool, "Other").await;
            for state in [
                "New",
                "New",
                "Learning",
                "Review",
                "Review",
                "Review",
                "Relearning",
            ] {
                add_card(&pool, deck, state, None).await;
            }
            assert_eq!(
                stats_at(&pool, None, &Utc).await.card_states,
                CardStates {
                    new: 2,
                    learning: 1,
                    review: 3,
                    relearning: 1
                }
            );
        });
    }

    #[test]
    fn the_forecast_puts_overdue_and_new_cards_today_and_leaves_out_later_ones() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            without_sample_cards(&pool).await;
            let deck = add_deck(&pool, "Other").await;
            add_card(&pool, deck, "New", None).await; // new: due now
            add_card(&pool, deck, "Review", Some("2026-09-01T00:00:00Z")).await; // overdue
            add_card(&pool, deck, "Review", Some("2026-10-19T23:59:59.999Z")).await; // later today
            add_card(&pool, deck, "Review", Some("2026-10-20T00:00:00Z")).await; // tomorrow
            add_card(&pool, deck, "Review", Some("2026-11-01T23:59:59.999Z")).await; // last day
            add_card(&pool, deck, "Review", Some("2026-11-02T00:00:00Z")).await; // too far

            let stats = stats_at(&pool, None, &Utc).await;
            assert_eq!(count_on(&stats.forecast, "2026-10-19"), 3);
            assert_eq!(count_on(&stats.forecast, "2026-10-20"), 1);
            assert_eq!(count_on(&stats.forecast, "2026-11-01"), 1);
            assert_eq!(stats.forecast.iter().map(|d| d.count).sum::<i64>(), 5);
        });
    }

    #[test]
    fn a_deck_filter_limits_every_number_to_that_deck() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let biology = add_deck(&pool, "Biology").await;
            let history = add_deck(&pool, "History").await;
            let cell = add_card(&pool, biology, "Review", Some("2026-10-22T00:00:00Z")).await;
            let war = add_card(&pool, history, "Learning", None).await;
            log_review(&pool, cell, 1, "Review", "2026-10-19T08:00:00Z").await;
            log_review(&pool, war, 3, "Review", "2026-10-19T09:00:00Z").await;
            log_review(&pool, war, 3, "New", "2026-10-18T09:00:00Z").await;

            let stats = stats_at(&pool, Some(biology), &Utc).await;
            assert_eq!(stats.reviews_by_day.iter().map(|d| d.count).sum::<i64>(), 1);
            assert_eq!(
                stats.retention,
                Retention {
                    reviews: 1,
                    recalled: 0
                }
            );
            assert_eq!(
                stats.card_states,
                CardStates {
                    review: 1,
                    ..CardStates::default()
                }
            );
            assert_eq!(count_on(&stats.forecast, "2026-10-22"), 1);
            assert_eq!(stats.forecast.iter().map(|d| d.count).sum::<i64>(), 1);

            let stats = stats_at(&pool, Some(history), &Utc).await;
            assert_eq!(stats.reviews_by_day.iter().map(|d| d.count).sum::<i64>(), 2);
            assert_eq!(
                stats.retention,
                Retention {
                    reviews: 1,
                    recalled: 1
                }
            );
            assert_eq!(count_on(&stats.forecast, "2026-10-19"), 1);
        });
    }

    #[test]
    fn statistics_reach_the_frontend_in_camel_case() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let value = serde_json::to_value(stats_at(&pool, None, &Utc).await).unwrap();
            let keys: Vec<&str> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys.len(), 4);
            for key in ["reviewsByDay", "retention", "cardStates", "forecast"] {
                assert!(keys.contains(&key), "{key} missing");
            }
            assert_eq!(
                value["reviewsByDay"][29],
                serde_json::json!({ "day": "2026-10-19", "count": 0 })
            );
            assert_eq!(
                value["retention"],
                serde_json::json!({ "reviews": 0, "recalled": 0 })
            );
            assert!(value["cardStates"]["relearning"].is_i64());
        });
    }

    #[test]
    fn a_missing_or_archived_deck_is_refused() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            assert!(matches!(
                statistics(&pool, Some(999), at(NOW), &Utc).await,
                Err(StudyError::NotFound)
            ));
            let deck = add_deck(&pool, "Old").await;
            archive(&pool, deck).await;
            assert!(matches!(
                statistics(&pool, Some(deck), at(NOW), &Utc).await,
                Err(StudyError::DeckArchived)
            ));
        });
    }

    #[test]
    fn statistics_write_nothing() {
        tauri::async_runtime::block_on(async {
            let pool = seeded().await;
            let sample = sample_deck_id(&pool).await;
            let before = sqlx::query_scalar::<_, i64>("SELECT total_changes()")
                .fetch_one(&pool)
                .await
                .unwrap();
            stats_at(&pool, None, &Utc).await;
            stats_at(&pool, Some(sample), &Utc).await;
            let after = sqlx::query_scalar::<_, i64>("SELECT total_changes()")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(before, after);
        });
    }

    #[test]
    fn local_days_follow_each_other_without_gaps() {
        // Each day's bounds meet the next day's, and days are local dates.
        // (Days with a clock change are `study::day_bounds`'s, tested there.)
        let rome_like = FixedOffset::east_opt(3600).unwrap();
        let days = past_days(at("2026-10-19T12:00:00Z"), &rome_like, HISTORY_DAYS);
        for pair in days.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
            assert_eq!(pair[0].date.succ_opt(), Some(pair[1].date));
        }
        let ahead = next_days(at("2026-10-19T12:00:00Z"), &rome_like, FORECAST_DAYS);
        assert_eq!(ahead[0].start, at("2026-10-18T23:00:00Z"));
        assert_eq!(day_of(&ahead, at("2026-10-18T22:59:59Z")), None);
        assert_eq!(day_of(&ahead, at("2026-10-18T23:00:00Z")), Some(0));
        assert_eq!(day_of(&ahead, at("2026-10-19T23:00:00Z")), Some(1));
    }
}
