//! Transfer speed: an exponential moving average over about five seconds.

use std::time::Duration;

use tokio::time::Instant;

const TIME_CONSTANT: Duration = Duration::from_secs(5);
/// Samples closer together than this are folded into the next one.
const MIN_SAMPLE: Duration = Duration::from_millis(50);
/// An ETA built on less than this much data would jump around too much to show.
const ETA_AFTER: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) struct RateTracker {
    started: Instant,
    last_at: Instant,
    last_bytes: u64,
    average: Option<f64>,
}

impl RateTracker {
    /// `bytes` is where the transfer starts counting (a resumed one starts above zero, and
    /// those bytes were not transferred now).
    pub(crate) fn new(now: Instant, bytes: u64) -> Self {
        Self {
            started: now,
            last_at: now,
            last_bytes: bytes,
            average: None,
        }
    }

    pub(crate) fn sample(&mut self, now: Instant, bytes: u64) {
        let elapsed = now.duration_since(self.last_at);
        if elapsed < MIN_SAMPLE {
            return;
        }
        let instant = bytes.saturating_sub(self.last_bytes) as f64 / elapsed.as_secs_f64();
        self.average = Some(match self.average {
            None => instant,
            Some(previous) => {
                let weight = 1.0 - (-elapsed.as_secs_f64() / TIME_CONSTANT.as_secs_f64()).exp();
                previous + weight * (instant - previous)
            }
        });
        self.last_at = now;
        self.last_bytes = bytes;
    }

    /// Bytes per second, once there is a sample.
    pub(crate) fn rate(&self) -> Option<f64> {
        self.average
    }

    /// Time left for `remaining` bytes: hidden until a second of data exists, and while the
    /// speed is (still) zero.
    pub(crate) fn eta(&self, now: Instant, remaining: u64) -> Option<Duration> {
        let rate = self.average.filter(|r| *r > 0.0)?;
        (now.duration_since(self.started) >= ETA_AFTER)
            .then(|| Duration::from_secs_f64(remaining as f64 / rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[tokio::test(start_paused = true)]
    async fn a_steady_transfer_converges_on_its_speed() {
        let start = Instant::now();
        let mut tracker = RateTracker::new(start, 0);
        for step in 1..=100u64 {
            tracker.sample(at(start, step * 100), step * 100_000); // 1 MB/s
        }
        let rate = tracker.rate().unwrap();
        assert!((rate - 1_000_000.0).abs() < 1.0, "{rate}");
    }

    #[tokio::test(start_paused = true)]
    async fn the_average_follows_a_change_in_speed_over_about_five_seconds() {
        let start = Instant::now();
        let mut tracker = RateTracker::new(start, 0);
        let mut bytes = 0;
        for step in 1..=100u64 {
            bytes += 100_000; // 1 MB/s for 10 s
            tracker.sample(at(start, step * 100), bytes);
        }
        for step in 101..=150u64 {
            bytes += 400_000; // then 4 MB/s for 5 s
            tracker.sample(at(start, step * 100), bytes);
        }
        let rate = tracker.rate().unwrap();
        // after one time constant (5 s) it has moved ~63 % of the way from 1 to 4 MB/s
        assert!((rate - 2_900_000.0).abs() < 150_000.0, "{rate}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_resumed_transfer_does_not_count_the_bytes_it_started_with_as_speed() {
        let start = Instant::now();
        let mut tracker = RateTracker::new(start, 4_000_000);
        tracker.sample(at(start, 1000), 4_500_000);
        assert!((tracker.rate().unwrap() - 500_000.0).abs() < 1.0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_eta_waits_for_a_second_of_data_and_for_a_speed() {
        let start = Instant::now();
        let mut tracker = RateTracker::new(start, 0);
        assert_eq!(tracker.eta(start, 1000), None, "no sample yet");
        tracker.sample(at(start, 500), 500_000);
        assert_eq!(
            tracker.eta(at(start, 500), 1_000_000),
            None,
            "less than a second of data"
        );
        tracker.sample(at(start, 1000), 1_000_000);
        assert_eq!(
            tracker.eta(at(start, 1000), 3_000_000),
            Some(Duration::from_secs(3))
        );

        let mut stalled = RateTracker::new(start, 0);
        stalled.sample(at(start, 1500), 0);
        assert_eq!(
            stalled.eta(at(start, 1500), 1000),
            None,
            "no ETA at zero speed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn samples_too_close_together_are_folded_into_the_next() {
        let start = Instant::now();
        let mut tracker = RateTracker::new(start, 0);
        tracker.sample(at(start, 10), 1_000_000);
        assert_eq!(tracker.rate(), None);
        tracker.sample(at(start, 100), 100_000);
        assert!((tracker.rate().unwrap() - 1_000_000.0).abs() < 1.0);
    }
}
