//! The injected clock. Everything time-dependent in the core reads it from here, so tests
//! control time instead of sleeping.

use std::{
    sync::atomic::{AtomicI64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Milliseconds since the Unix epoch.
pub type UnixMillis = i64;

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> UnixMillis;
}

/// How often [`sleep_until`] looks at the clock, in real time.
const POLL: Duration = Duration::from_millis(100);

/// Completes once `clock` reads `deadline` or later. The injected clock can jump, so this
/// checks it on a short real-time tick instead of sleeping for the difference.
pub(crate) async fn sleep_until(clock: &dyn Clock, deadline: UnixMillis) {
    while clock.now() < deadline {
        tokio::time::sleep(POLL).await;
    }
}

/// The wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixMillis {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as UnixMillis)
    }
}

/// A clock that only moves when told to.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub fn new(start: UnixMillis) -> Self {
        Self(AtomicI64::new(start))
    }

    pub fn advance(&self, millis: i64) {
        self.0.fetch_add(millis, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> UnixMillis {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_moves_only_when_advanced() {
        let clock = ManualClock::new(1_000);
        assert_eq!(clock.now(), 1_000);
        assert_eq!(clock.now(), 1_000);
        clock.advance(250);
        assert_eq!(clock.now(), 1_250);
    }

    #[tokio::test]
    async fn sleep_until_waits_for_the_clock_not_for_real_time() {
        let clock = std::sync::Arc::new(ManualClock::new(0));
        let waiting = tokio::spawn({
            let clock = clock.clone();
            async move { sleep_until(&*clock, 1_000).await }
        });
        clock.advance(999);
        tokio::time::sleep(POLL * 3).await;
        assert!(!waiting.is_finished(), "woke before the deadline");
        clock.advance(1);
        tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
    }
}
