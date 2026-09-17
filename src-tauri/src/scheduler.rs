//! FSRS scheduling: turns a card's current state, a rating, and the current
//! time into the card's next state. Pure logic with no database access, so
//! it's easy to test. The FSRS maths itself comes from the `fsrs` crate.

use chrono::{DateTime, TimeDelta, Utc};
use fsrs::{MemoryState, FSRS};

use crate::db::DbError;

/// The probability of remembering a card that FSRS aims for when it's due.
/// 0.9 is FSRS's standard recommendation.
pub const DESIRED_RETENTION: f32 = 0.9;

const MS_PER_DAY: f64 = 86_400_000.0;

/// The four answer buttons. The numbers match `review_logs.rating`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl Rating {
    /// Validates a rating number from the frontend. Anything but 1–4 is `None`.
    pub fn from_number(n: i64) -> Option<Rating> {
        match n {
            1 => Some(Rating::Again),
            2 => Some(Rating::Hard),
            3 => Some(Rating::Good),
            4 => Some(Rating::Easy),
            _ => None,
        }
    }

    pub fn number(self) -> i64 {
        self as i64
    }
}

/// Where a card is in its learning life cycle (stored as `fsrs_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardState {
    New,
    Learning,
    Review,
    Relearning,
}

impl CardState {
    pub fn as_str(self) -> &'static str {
        match self {
            CardState::New => "New",
            CardState::Learning => "Learning",
            CardState::Review => "Review",
            CardState::Relearning => "Relearning",
        }
    }

    pub fn parse(s: &str) -> Option<CardState> {
        match s {
            "New" => Some(CardState::New),
            "Learning" => Some(CardState::Learning),
            "Review" => Some(CardState::Review),
            "Relearning" => Some(CardState::Relearning),
            _ => None,
        }
    }
}

/// A card's scheduling state before a review, as loaded from the database.
#[derive(Debug, Clone)]
pub struct Schedule {
    pub state: CardState,
    /// `None` for a card that has never been reviewed.
    pub memory: Option<MemoryState>,
    pub last_review: Option<DateTime<Utc>>,
    pub reps: i64,
    pub lapses: i64,
}

/// Everything that changes because of one review.
#[derive(Debug, Clone)]
pub struct ReviewOutcome {
    pub state: CardState,
    pub memory: MemoryState,
    pub due: DateTime<Utc>,
    pub reps: i64,
    pub lapses: i64,
    /// Days from this review until the card is next due (fractional).
    pub scheduled_days: f64,
    /// Days since the previous review (0 for a card's first review).
    pub elapsed_days: f64,
}

/// Applies one review using FSRS with its default parameters.
pub fn review(
    current: &Schedule,
    rating: Rating,
    now: DateTime<Utc>,
) -> Result<ReviewOutcome, DbError> {
    let elapsed_days = match current.last_review {
        Some(last) => ((now - last).num_milliseconds().max(0) as f64) / MS_PER_DAY,
        None => 0.0,
    };

    // FSRS counts whole days since the last review. `as u32` rounds down
    // (and can't overflow: it saturates at u32::MAX).
    let fsrs = FSRS::default();
    let next = fsrs
        .next_states(current.memory, DESIRED_RETENTION, elapsed_days as u32)
        .map_err(|err| format!("FSRS failed to compute next states: {err:?}"))?;

    let chosen = match rating {
        Rating::Again => next.again,
        Rating::Hard => next.hard,
        Rating::Good => next.good,
        Rating::Easy => next.easy,
    };

    let memory = chosen.memory;
    if !(chosen.interval.is_finite()
        && chosen.interval > 0.0
        && memory.stability.is_finite()
        && memory.difficulty.is_finite())
    {
        return Err(format!("FSRS returned an unusable result: {chosen:?}").into());
    }

    let scheduled_days = f64::from(chosen.interval);
    let due = TimeDelta::try_milliseconds((scheduled_days * MS_PER_DAY).round() as i64)
        .and_then(|delta| now.checked_add_signed(delta))
        .ok_or("FSRS interval is out of range")?;

    // The `fsrs` crate computes memory and intervals only; the named states
    // are ours. Forgetting (Again) sends a card back to (re)learning,
    // any other rating means the card is in regular review.
    let state = match (rating, current.state) {
        (Rating::Again, CardState::Review | CardState::Relearning) => CardState::Relearning,
        (Rating::Again, _) => CardState::Learning,
        _ => CardState::Review,
    };
    // A lapse is forgetting a card that had graduated to Review.
    let lapsed = rating == Rating::Again && current.state == CardState::Review;

    Ok(ReviewOutcome {
        state,
        memory,
        due,
        reps: current.reps + 1,
        lapses: current.lapses + i64::from(lapsed),
        scheduled_days,
        elapsed_days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn new_card() -> Schedule {
        Schedule {
            state: CardState::New,
            memory: None,
            last_review: None,
            reps: 0,
            lapses: 0,
        }
    }

    #[test]
    fn rating_validation_accepts_only_one_to_four() {
        assert_eq!(Rating::from_number(1), Some(Rating::Again));
        assert_eq!(Rating::from_number(2), Some(Rating::Hard));
        assert_eq!(Rating::from_number(3), Some(Rating::Good));
        assert_eq!(Rating::from_number(4), Some(Rating::Easy));
        for invalid in [i64::MIN, -1, 0, 5, 300, i64::MAX] {
            assert_eq!(Rating::from_number(invalid), None);
        }
    }

    #[test]
    fn card_state_round_trips_through_text() {
        for state in [
            CardState::New,
            CardState::Learning,
            CardState::Review,
            CardState::Relearning,
        ] {
            assert_eq!(CardState::parse(state.as_str()), Some(state));
        }
        assert_eq!(CardState::parse("new"), None);
    }

    #[test]
    fn good_on_a_new_card_schedules_a_sensible_future_review() {
        let now = at("2026-09-15T12:00:00Z");
        let out = review(&new_card(), Rating::Good, now).unwrap();

        assert_eq!(out.state, CardState::Review);
        assert_eq!(out.reps, 1);
        assert_eq!(out.lapses, 0);
        assert_eq!(out.elapsed_days, 0.0);
        // FSRS defaults put a first "Good" a couple of days out.
        assert!(out.due > now + TimeDelta::days(1), "due {}", out.due);
        assert!(out.due < now + TimeDelta::days(10), "due {}", out.due);
    }

    #[test]
    fn easier_ratings_never_schedule_sooner() {
        let now = at("2026-09-15T12:00:00Z");
        let dues: Vec<_> = [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy]
            .into_iter()
            .map(|r| review(&new_card(), r, now).unwrap().due)
            .collect();
        assert!(dues.windows(2).all(|pair| pair[0] <= pair[1]), "{dues:?}");
    }

    #[test]
    fn again_on_a_review_card_is_a_lapse() {
        let first = at("2026-09-15T12:00:00Z");
        let learned = review(&new_card(), Rating::Good, first).unwrap();
        let current = Schedule {
            state: learned.state,
            memory: Some(learned.memory),
            last_review: Some(first),
            reps: learned.reps,
            lapses: learned.lapses,
        };

        let later = first + TimeDelta::days(3);
        let out = review(&current, Rating::Again, later).unwrap();

        assert_eq!(out.state, CardState::Relearning);
        assert_eq!(out.reps, 2);
        assert_eq!(out.lapses, 1);
        assert_eq!(out.elapsed_days, 3.0);
    }
}
