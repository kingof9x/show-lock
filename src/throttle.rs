//! Delay after repeated wrong passwords, so a USB gadget that types fast
//! can't guess the password.

use std::time::{Duration, Instant};

/// Wrong attempts allowed before the first delay.
const FREE_ATTEMPTS: u32 = 4;
const FIRST_DELAY: Duration = Duration::from_secs(30);
const MAX_DELAY: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default)]
pub struct Throttle {
    failures: u32,
    blocked_until: Option<Instant>,
}

impl Throttle {
    /// Delay after the given number of wrong attempts: none for the first 4,
    /// then 30 s, doubling each time, up to 15 minutes.
    pub fn delay_after(failures: u32) -> Duration {
        if failures <= FREE_ATTEMPTS {
            return Duration::ZERO;
        }
        let doublings = (failures - FREE_ATTEMPTS - 1).min(10);
        (FIRST_DELAY * 2u32.pow(doublings)).min(MAX_DELAY)
    }

    /// How long until another attempt is allowed, if blocked.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.blocked_until
            .and_then(|t| t.checked_duration_since(now))
            .filter(|d| !d.is_zero())
    }

    pub fn record_failure(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        let delay = Self::delay_after(self.failures);
        self.blocked_until = (!delay.is_zero()).then(|| now + delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_grow_and_cap() {
        assert_eq!(Throttle::delay_after(0), Duration::ZERO);
        assert_eq!(Throttle::delay_after(4), Duration::ZERO);
        assert_eq!(Throttle::delay_after(5), Duration::from_secs(30));
        assert_eq!(Throttle::delay_after(6), Duration::from_secs(60));
        assert_eq!(Throttle::delay_after(7), Duration::from_secs(120));
        assert_eq!(Throttle::delay_after(100), MAX_DELAY);
        assert_eq!(Throttle::delay_after(u32::MAX), MAX_DELAY);
    }

    #[test]
    fn blocks_after_free_attempts() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        for _ in 0..4 {
            t.record_failure(t0);
            assert_eq!(t.remaining(t0), None);
        }
        t.record_failure(t0);
        assert_eq!(t.remaining(t0), Some(Duration::from_secs(30)));
        assert_eq!(
            t.remaining(t0 + Duration::from_secs(10)),
            Some(Duration::from_secs(20))
        );
        assert_eq!(t.remaining(t0 + Duration::from_secs(30)), None);
        assert_eq!(t.failures, 5);
    }
}
