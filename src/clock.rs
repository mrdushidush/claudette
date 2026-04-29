//! Time abstraction for the scheduler (AD-5 from docs/life_agent.md).
//!
//! Every time-sensitive code path takes `&dyn Clock` instead of calling
//! `Utc::now()` directly. Production code wires in [`SystemClock`]; tests
//! wire in [`MockClock`] and advance it deterministically, so firing-order
//! assertions never depend on a real sleep.
//!
//! Trait objects — not a generic parameter — because the scheduler's
//! `BinaryHeap` needs a single concrete type across both the production
//! path and the test path, and because the cost of a vtable call is lost
//! in the noise next to the JSON I/O and Telegram HTTP each firing does.

use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

/// Monotonic source of wall-clock `DateTime<Utc>`. `Send + Sync` so the
/// scheduler can keep an `Arc<dyn Clock>` across the producer thread and
/// the consumer.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Production clock — delegates straight to `Utc::now()`.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Test clock — holds a fixed instant behind a Mutex. Advance with
/// [`MockClock::advance`] / [`MockClock::set`] from tests; the scheduler
/// reads `now()` like any other clock.
#[derive(Debug)]
pub struct MockClock {
    instant: Mutex<DateTime<Utc>>,
}

impl MockClock {
    /// Construct at the given UTC datetime.
    #[must_use]
    pub fn new(at: DateTime<Utc>) -> Self {
        Self {
            instant: Mutex::new(at),
        }
    }

    /// Move the mock clock forward by `delta`. Negative deltas move it back
    /// — we allow that explicitly because some catch-up tests want to
    /// rewind between assertions.
    pub fn advance(&self, delta: Duration) {
        if let Ok(mut g) = self.instant.lock() {
            *g += delta;
        }
    }

    /// Jump the clock to an absolute instant.
    pub fn set(&self, at: DateTime<Utc>) {
        if let Ok(mut g) = self.instant.lock() {
            *g = at;
        }
    }
}

impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        self.instant
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |g| *g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn system_clock_returns_recent_time() {
        let c = SystemClock;
        let before = Utc::now();
        let t = c.now();
        let after = Utc::now();
        assert!(t >= before && t <= after);
    }

    #[test]
    fn mock_clock_starts_at_given_instant() {
        let at = Utc.with_ymd_and_hms(2026, 4, 21, 7, 0, 0).unwrap();
        let c = MockClock::new(at);
        assert_eq!(c.now(), at);
    }

    #[test]
    fn mock_clock_advance_moves_forward() {
        let at = Utc.with_ymd_and_hms(2026, 4, 21, 7, 0, 0).unwrap();
        let c = MockClock::new(at);
        c.advance(Duration::minutes(30));
        assert_eq!(c.now(), at + Duration::minutes(30));
    }

    #[test]
    fn mock_clock_advance_accepts_negative_delta() {
        let at = Utc.with_ymd_and_hms(2026, 4, 21, 7, 0, 0).unwrap();
        let c = MockClock::new(at);
        c.advance(-Duration::hours(1));
        assert_eq!(c.now(), at - Duration::hours(1));
    }

    #[test]
    fn mock_clock_set_jumps_to_instant() {
        let a = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        let c = MockClock::new(a);
        c.set(b);
        assert_eq!(c.now(), b);
    }

    #[test]
    fn clock_is_object_safe() {
        // Compile-time check: the scheduler wants Arc<dyn Clock>, so the
        // trait must be object-safe.
        fn takes_dyn(_c: &dyn Clock) {}
        let c = MockClock::new(Utc::now());
        takes_dyn(&c);
        takes_dyn(&SystemClock);
    }

    #[test]
    fn mock_clock_is_send_sync() {
        // Used by the scheduler across threads, so this matters.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MockClock>();
        assert_send_sync::<SystemClock>();
    }
}
