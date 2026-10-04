//! The injected clock. Everything time-dependent in the core reads it from here, so tests
//! control time instead of sleeping.

use std::{
    sync::atomic::{AtomicI64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Milliseconds since the Unix epoch.
pub type UnixMillis = i64;

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> UnixMillis;
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
}
