//! The clock port. Nothing in the engine reads the wall clock directly.

use std::sync::Mutex;

use time::{Duration, OffsetDateTime};

pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

/// The wall clock, in UTC.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// A clock that stands still until a test moves it.
#[derive(Debug)]
pub struct TestClock {
    now: Mutex<OffsetDateTime>,
}

impl TestClock {
    pub fn new(start: OffsetDateTime) -> Self {
        TestClock {
            now: Mutex::new(start),
        }
    }

    pub fn advance(&self, by: Duration) {
        *self.now.lock().expect("test clock lock") += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        *self.now.lock().expect("test clock lock")
    }
}
